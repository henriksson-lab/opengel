//! GelGenie ML segmentation via Burn.
//!
//! Rust converted code, derive from https://github.com/mattaq31/GelGenie, under Apache-2.0 license

use std::path::{Path, PathBuf};

use burn::backend::{wgpu::WgpuDevice, Flex, Wgpu};
use burn::prelude::*;
use burn::tensor::TensorData;
use ndarray::Array2;

use crate::core::GrayF32;
use crate::detect::blob_detector::BlobDetector;
use crate::detect::detector::{DetectParams, Detection, GelDetector};
use crate::detect::mask_segment::MaskSegmenter;
use crate::detect::models::gelgenie_unet_1024::Model;

const MODEL_SIZE: usize = 1024;
const MODEL_FILE: &str = "gelgenie_unet_1024.bpk";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GelGenieRuntime {
    Cpu,
    Wgpu,
}

impl GelGenieRuntime {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Wgpu,
            _ => Self::Cpu,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Wgpu => "WGPU",
        }
    }
}

pub struct GelGenieDetector {
    model_path: PathBuf,
    runtime: GelGenieRuntime,
    threshold: f32,
    min_area: u32,
}

impl GelGenieDetector {
    pub fn new(runtime: GelGenieRuntime) -> anyhow::Result<Self> {
        let model_path = resolve_model_path().ok_or_else(|| {
            anyhow::anyhow!("GelGenie model not found; set OPENGEL_GELGENIE_MODEL")
        })?;
        Ok(Self {
            model_path,
            runtime,
            threshold: 0.5,
            min_area: 8,
        })
    }

    pub fn model_available() -> bool {
        resolve_model_path().is_some()
    }

    fn segment_mask(&self, img: &GrayF32) -> GrayF32 {
        match self.runtime {
            GelGenieRuntime::Cpu => run_model::<Flex>(&self.model_path, img, &Default::default()),
            GelGenieRuntime::Wgpu => {
                let device = WgpuDevice::default();
                run_model::<Wgpu>(&self.model_path, img, &device)
            }
        }
    }
}

impl GelDetector for GelGenieDetector {
    fn name(&self) -> &str {
        "gelgenie"
    }

    fn detect(&self, img: &GrayF32, params: &DetectParams) -> Detection {
        let work = if params.signal_is_bright {
            img.clone()
        } else {
            img.inverted()
        };
        let mask = self.segment_mask(&work);
        let segmenter = MaskSegmenter::with_params(mask, self.threshold, self.min_area);
        BlobDetector::new(segmenter).detect(&work, params)
    }
}

fn run_model<B: Backend>(model_path: &Path, img: &GrayF32, device: &B::Device) -> GrayF32 {
    let input = preprocess::<B>(img, device);
    let model = Model::<B>::from_file(model_path, device);
    let output = model.forward(input);
    let logits = output.into_data().to_vec::<f32>().unwrap();
    logits_to_mask(&logits, img.width(), img.height())
}

fn preprocess<B: Backend>(img: &GrayF32, device: &B::Device) -> Tensor<B, 4> {
    let mut data = Vec::with_capacity(MODEL_SIZE * MODEL_SIZE);
    let sx = if MODEL_SIZE > 1 {
        (img.width().saturating_sub(1)) as f32 / (MODEL_SIZE - 1) as f32
    } else {
        0.0
    };
    let sy = if MODEL_SIZE > 1 {
        (img.height().saturating_sub(1)) as f32 / (MODEL_SIZE - 1) as f32
    } else {
        0.0
    };
    for y in 0..MODEL_SIZE {
        for x in 0..MODEL_SIZE {
            data.push(
                img.sample_bilinear(x as f32 * sx, y as f32 * sy)
                    .clamp(0.0, 1.0),
            );
        }
    }
    Tensor::<B, 4>::from_data(
        TensorData::new(data, [1, 1, MODEL_SIZE, MODEL_SIZE]),
        device,
    )
}

fn logits_to_mask(logits: &[f32], width: usize, height: usize) -> GrayF32 {
    debug_assert_eq!(logits.len(), 2 * MODEL_SIZE * MODEL_SIZE);
    let plane = MODEL_SIZE * MODEL_SIZE;
    let mut data = Array2::<f32>::zeros((height, width));
    if width == 0 || height == 0 {
        return GrayF32 { data };
    }
    let sx = if width > 1 {
        (MODEL_SIZE - 1) as f32 / (width - 1) as f32
    } else {
        0.0
    };
    let sy = if height > 1 {
        (MODEL_SIZE - 1) as f32 / (height - 1) as f32
    } else {
        0.0
    };
    for y in 0..height {
        let yy = ((y as f32 * sy).round() as usize).min(MODEL_SIZE - 1);
        for x in 0..width {
            let xx = ((x as f32 * sx).round() as usize).min(MODEL_SIZE - 1);
            let i = yy * MODEL_SIZE + xx;
            let bg = logits[i];
            let fg = logits[plane + i];
            data[[y, x]] = softmax_second(bg, fg);
        }
    }
    GrayF32 { data }
}

fn softmax_second(a: f32, b: f32) -> f32 {
    let m = a.max(b);
    let ea = (a - m).exp();
    let eb = (b - m).exp();
    eb / (ea + eb)
}

fn resolve_model_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OPENGEL_GELGENIE_MODEL").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("models")
            .join(MODEL_FILE),
        std::env::current_dir()
            .ok()
            .unwrap_or_default()
            .join("assets")
            .join("models")
            .join(MODEL_FILE),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("models").join(MODEL_FILE)))
            .unwrap_or_default(),
        std::env::current_exe()
            .ok()
            .and_then(|p| {
                p.parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("Resources").join("models").join(MODEL_FILE))
            })
            .unwrap_or_default(),
        PathBuf::from("/usr/share/opengel/models").join(MODEL_FILE),
    ];
    candidates.into_iter().find(|p| p.is_file())
}
