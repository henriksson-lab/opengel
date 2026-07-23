//! Rust converted code, derive from https://github.com/mattaq31/GelGenie, under Apache-2.0 license

#![allow(dead_code)]

// Generated from ONNX "/tmp/gelgenie_unet_1024.onnx" by burn-onnx
use burn::nn::conv::Conv2d;
use burn::nn::conv::Conv2dConfig;
use burn::nn::pool::MaxPool2d;
use burn::nn::pool::MaxPool2dConfig;
use burn::nn::PaddingConfig2d;
use burn::prelude::*;
use burn::tensor::Bytes;
use burn_store::BurnpackStore;
use burn_store::ModuleSnapshot;

#[derive(Module, Debug)]
pub struct Model<B: Backend> {
    conv2d1: Conv2d<B>,
    maxpool2d1: MaxPool2d,
    conv2d2: Conv2d<B>,
    conv2d3: Conv2d<B>,
    conv2d4: Conv2d<B>,
    conv2d5: Conv2d<B>,
    conv2d6: Conv2d<B>,
    conv2d7: Conv2d<B>,
    conv2d8: Conv2d<B>,
    conv2d9: Conv2d<B>,
    conv2d10: Conv2d<B>,
    conv2d11: Conv2d<B>,
    conv2d12: Conv2d<B>,
    conv2d13: Conv2d<B>,
    conv2d14: Conv2d<B>,
    conv2d15: Conv2d<B>,
    conv2d16: Conv2d<B>,
    conv2d17: Conv2d<B>,
    conv2d18: Conv2d<B>,
    conv2d19: Conv2d<B>,
    conv2d20: Conv2d<B>,
    resize1: burn::nn::interpolate::Interpolate2d,
    conv2d21: Conv2d<B>,
    conv2d22: Conv2d<B>,
    resize2: burn::nn::interpolate::Interpolate2d,
    conv2d23: Conv2d<B>,
    conv2d24: Conv2d<B>,
    resize3: burn::nn::interpolate::Interpolate2d,
    conv2d25: Conv2d<B>,
    conv2d26: Conv2d<B>,
    resize4: burn::nn::interpolate::Interpolate2d,
    conv2d27: Conv2d<B>,
    conv2d28: Conv2d<B>,
    resize5: burn::nn::interpolate::Interpolate2d,
    conv2d29: Conv2d<B>,
    conv2d30: Conv2d<B>,
    conv2d31: Conv2d<B>,
    phantom: core::marker::PhantomData<B>,
    #[module(skip)]
    device: B::Device,
}

extern crate std;

impl<B: Backend> Default for Model<B> {
    fn default() -> Self {
        Self::from_file(
            "/tmp/gelgenie_burn_probe/gelgenie_unet_1024.bpk",
            &Default::default(),
        )
    }
}

impl<B: Backend> Model<B> {
    /// Load model weights from a burnpack file.
    pub fn from_file<P: AsRef<std::path::Path>>(file: P, device: &B::Device) -> Self {
        let mut model = Self::new(device);
        let mut store = BurnpackStore::from_file(file);
        model
            .load_from(&mut store)
            .expect("Failed to load burnpack file");
        model
    }

    /// Load model weights from in-memory bytes.
    ///
    /// The bytes must be the contents of a `.bpk` file.
    pub fn from_bytes(bytes: Bytes, device: &B::Device) -> Self {
        let mut model = Self::new(device);
        let mut store = BurnpackStore::from_bytes(Some(bytes));
        model
            .load_from(&mut store)
            .expect("Failed to load burnpack bytes");
        model
    }
}

impl<B: Backend> Model<B> {
    #[allow(unused_variables)]
    pub fn new(device: &B::Device) -> Self {
        let conv2d1 = Conv2dConfig::new([1, 64], [7, 7])
            .with_stride([2, 2])
            .with_padding(PaddingConfig2d::Explicit(3, 3, 3, 3))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let maxpool2d1 = MaxPool2dConfig::new([3, 3])
            .with_strides([2, 2])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_ceil_mode(false)
            .init();
        let conv2d2 = Conv2dConfig::new([64, 64], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d3 = Conv2dConfig::new([64, 64], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d4 = Conv2dConfig::new([64, 64], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d5 = Conv2dConfig::new([64, 64], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d6 = Conv2dConfig::new([64, 128], [3, 3])
            .with_stride([2, 2])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d7 = Conv2dConfig::new([128, 128], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d8 = Conv2dConfig::new([64, 128], [1, 1])
            .with_stride([2, 2])
            .with_padding(PaddingConfig2d::Valid)
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d9 = Conv2dConfig::new([128, 128], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d10 = Conv2dConfig::new([128, 128], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d11 = Conv2dConfig::new([128, 256], [3, 3])
            .with_stride([2, 2])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d12 = Conv2dConfig::new([256, 256], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d13 = Conv2dConfig::new([128, 256], [1, 1])
            .with_stride([2, 2])
            .with_padding(PaddingConfig2d::Valid)
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d14 = Conv2dConfig::new([256, 256], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d15 = Conv2dConfig::new([256, 256], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d16 = Conv2dConfig::new([256, 512], [3, 3])
            .with_stride([2, 2])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d17 = Conv2dConfig::new([512, 512], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d18 = Conv2dConfig::new([256, 512], [1, 1])
            .with_stride([2, 2])
            .with_padding(PaddingConfig2d::Valid)
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d19 = Conv2dConfig::new([512, 512], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d20 = Conv2dConfig::new([512, 512], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let resize1 = burn::nn::interpolate::Interpolate2dConfig::new()
            .with_output_size(None)
            .with_scale_factor(Some([2.0, 2.0]))
            .with_mode(burn::nn::interpolate::InterpolateMode::Nearest)
            .with_align_corners(false)
            .init();
        let conv2d21 = Conv2dConfig::new([768, 256], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d22 = Conv2dConfig::new([256, 256], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let resize2 = burn::nn::interpolate::Interpolate2dConfig::new()
            .with_output_size(None)
            .with_scale_factor(Some([2.0, 2.0]))
            .with_mode(burn::nn::interpolate::InterpolateMode::Nearest)
            .with_align_corners(false)
            .init();
        let conv2d23 = Conv2dConfig::new([384, 128], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d24 = Conv2dConfig::new([128, 128], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let resize3 = burn::nn::interpolate::Interpolate2dConfig::new()
            .with_output_size(None)
            .with_scale_factor(Some([2.0, 2.0]))
            .with_mode(burn::nn::interpolate::InterpolateMode::Nearest)
            .with_align_corners(false)
            .init();
        let conv2d25 = Conv2dConfig::new([192, 64], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d26 = Conv2dConfig::new([64, 64], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let resize4 = burn::nn::interpolate::Interpolate2dConfig::new()
            .with_output_size(None)
            .with_scale_factor(Some([2.0, 2.0]))
            .with_mode(burn::nn::interpolate::InterpolateMode::Nearest)
            .with_align_corners(false)
            .init();
        let conv2d27 = Conv2dConfig::new([128, 32], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d28 = Conv2dConfig::new([32, 32], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let resize5 = burn::nn::interpolate::Interpolate2dConfig::new()
            .with_output_size(None)
            .with_scale_factor(Some([2.0, 2.0]))
            .with_mode(burn::nn::interpolate::InterpolateMode::Nearest)
            .with_align_corners(false)
            .init();
        let conv2d29 = Conv2dConfig::new([32, 16], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d30 = Conv2dConfig::new([16, 16], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        let conv2d31 = Conv2dConfig::new([16, 2], [3, 3])
            .with_stride([1, 1])
            .with_padding(PaddingConfig2d::Explicit(1, 1, 1, 1))
            .with_dilation([1, 1])
            .with_groups(1)
            .with_bias(true)
            .init(device);
        Self {
            conv2d1,
            maxpool2d1,
            conv2d2,
            conv2d3,
            conv2d4,
            conv2d5,
            conv2d6,
            conv2d7,
            conv2d8,
            conv2d9,
            conv2d10,
            conv2d11,
            conv2d12,
            conv2d13,
            conv2d14,
            conv2d15,
            conv2d16,
            conv2d17,
            conv2d18,
            conv2d19,
            conv2d20,
            resize1,
            conv2d21,
            conv2d22,
            resize2,
            conv2d23,
            conv2d24,
            resize3,
            conv2d25,
            conv2d26,
            resize4,
            conv2d27,
            conv2d28,
            resize5,
            conv2d29,
            conv2d30,
            conv2d31,
            phantom: core::marker::PhantomData,
            device: device.clone(),
        }
    }

    #[allow(clippy::let_and_return, clippy::approx_constant)]
    pub fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        let conv2d1_out1 = self.conv2d1.forward(input);
        let relu1_out1 = burn::tensor::activation::relu(conv2d1_out1);
        let maxpool2d1_out1 = self.maxpool2d1.forward(relu1_out1.clone());
        let conv2d2_out1 = self.conv2d2.forward(maxpool2d1_out1.clone());
        let relu2_out1 = burn::tensor::activation::relu(conv2d2_out1);
        let conv2d3_out1 = self.conv2d3.forward(relu2_out1);
        let add1_out1 = conv2d3_out1.add(maxpool2d1_out1);
        let relu3_out1 = burn::tensor::activation::relu(add1_out1);
        let conv2d4_out1 = self.conv2d4.forward(relu3_out1.clone());
        let relu4_out1 = burn::tensor::activation::relu(conv2d4_out1);
        let conv2d5_out1 = self.conv2d5.forward(relu4_out1);
        let add2_out1 = conv2d5_out1.add(relu3_out1);
        let relu5_out1 = burn::tensor::activation::relu(add2_out1);
        let conv2d6_out1 = self.conv2d6.forward(relu5_out1.clone());
        let relu6_out1 = burn::tensor::activation::relu(conv2d6_out1);
        let conv2d7_out1 = self.conv2d7.forward(relu6_out1);
        let conv2d8_out1 = self.conv2d8.forward(relu5_out1.clone());
        let add3_out1 = conv2d7_out1.add(conv2d8_out1);
        let relu7_out1 = burn::tensor::activation::relu(add3_out1);
        let conv2d9_out1 = self.conv2d9.forward(relu7_out1.clone());
        let relu8_out1 = burn::tensor::activation::relu(conv2d9_out1);
        let conv2d10_out1 = self.conv2d10.forward(relu8_out1);
        let add4_out1 = conv2d10_out1.add(relu7_out1);
        let relu9_out1 = burn::tensor::activation::relu(add4_out1);
        let conv2d11_out1 = self.conv2d11.forward(relu9_out1.clone());
        let relu10_out1 = burn::tensor::activation::relu(conv2d11_out1);
        let conv2d12_out1 = self.conv2d12.forward(relu10_out1);
        let conv2d13_out1 = self.conv2d13.forward(relu9_out1.clone());
        let add5_out1 = conv2d12_out1.add(conv2d13_out1);
        let relu11_out1 = burn::tensor::activation::relu(add5_out1);
        let conv2d14_out1 = self.conv2d14.forward(relu11_out1.clone());
        let relu12_out1 = burn::tensor::activation::relu(conv2d14_out1);
        let conv2d15_out1 = self.conv2d15.forward(relu12_out1);
        let add6_out1 = conv2d15_out1.add(relu11_out1);
        let relu13_out1 = burn::tensor::activation::relu(add6_out1);
        let conv2d16_out1 = self.conv2d16.forward(relu13_out1.clone());
        let relu14_out1 = burn::tensor::activation::relu(conv2d16_out1);
        let conv2d17_out1 = self.conv2d17.forward(relu14_out1);
        let conv2d18_out1 = self.conv2d18.forward(relu13_out1.clone());
        let add7_out1 = conv2d17_out1.add(conv2d18_out1);
        let relu15_out1 = burn::tensor::activation::relu(add7_out1);
        let conv2d19_out1 = self.conv2d19.forward(relu15_out1.clone());
        let relu16_out1 = burn::tensor::activation::relu(conv2d19_out1);
        let conv2d20_out1 = self.conv2d20.forward(relu16_out1);
        let add8_out1 = conv2d20_out1.add(relu15_out1);
        let relu17_out1 = burn::tensor::activation::relu(add8_out1);
        let resize1_out1 = self.resize1.forward(relu17_out1);
        let concat1_out1 = burn::tensor::Tensor::cat([resize1_out1, relu13_out1].into(), 1);
        let conv2d21_out1 = self.conv2d21.forward(concat1_out1);
        let relu18_out1 = burn::tensor::activation::relu(conv2d21_out1);
        let conv2d22_out1 = self.conv2d22.forward(relu18_out1);
        let relu19_out1 = burn::tensor::activation::relu(conv2d22_out1);
        let resize2_out1 = self.resize2.forward(relu19_out1);
        let concat2_out1 = burn::tensor::Tensor::cat([resize2_out1, relu9_out1].into(), 1);
        let conv2d23_out1 = self.conv2d23.forward(concat2_out1);
        let relu20_out1 = burn::tensor::activation::relu(conv2d23_out1);
        let conv2d24_out1 = self.conv2d24.forward(relu20_out1);
        let relu21_out1 = burn::tensor::activation::relu(conv2d24_out1);
        let resize3_out1 = self.resize3.forward(relu21_out1);
        let concat3_out1 = burn::tensor::Tensor::cat([resize3_out1, relu5_out1].into(), 1);
        let conv2d25_out1 = self.conv2d25.forward(concat3_out1);
        let relu22_out1 = burn::tensor::activation::relu(conv2d25_out1);
        let conv2d26_out1 = self.conv2d26.forward(relu22_out1);
        let relu23_out1 = burn::tensor::activation::relu(conv2d26_out1);
        let resize4_out1 = self.resize4.forward(relu23_out1);
        let concat4_out1 = burn::tensor::Tensor::cat([resize4_out1, relu1_out1].into(), 1);
        let conv2d27_out1 = self.conv2d27.forward(concat4_out1);
        let relu24_out1 = burn::tensor::activation::relu(conv2d27_out1);
        let conv2d28_out1 = self.conv2d28.forward(relu24_out1);
        let relu25_out1 = burn::tensor::activation::relu(conv2d28_out1);
        let resize5_out1 = self.resize5.forward(relu25_out1);
        let conv2d29_out1 = self.conv2d29.forward(resize5_out1);
        let relu26_out1 = burn::tensor::activation::relu(conv2d29_out1);
        let conv2d30_out1 = self.conv2d30.forward(relu26_out1);
        let relu27_out1 = burn::tensor::activation::relu(conv2d30_out1);
        let conv2d31_out1 = self.conv2d31.forward(relu27_out1);
        conv2d31_out1
    }
}
