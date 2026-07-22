//! `gel` — headless CLI for OpenGel: inspect projects, run the analysis
//! pipeline, list ladders, evaluate detectors against ground truth, and make a
//! synthetic demo gel.

mod demo;
mod masks;
mod work;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use opengel::core::model::GelType;
use opengel::core::{ladders, GelDocument};
use opengel::detect::detector::DetectParams;

#[derive(Parser)]
#[command(name = "gel", about = "OpenGel command-line tools", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print project metadata and analysis summary for a .gel.zip.
    Info { path: PathBuf },
    /// List built-in ladder templates.
    Ladders {
        /// Filter by gel type: dna | rna | protein.
        #[arg(long)]
        gel_type: Option<String>,
    },
    /// Detect lanes/bands, identify a ladder, and size bands.
    Analyze {
        path: PathBuf,
        /// Override the project's gel type.
        #[arg(long)]
        gel_type: Option<String>,
        /// Write the analyzed project here (defaults to overwriting input).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Minimum semi-log R² to accept a ladder identification.
        #[arg(long, default_value_t = 0.9)]
        min_r2: f64,
        /// Use an external band-segmentation **mask** image (e.g. GelGenie's
        /// output) instead of the classical detector. Foreground = bands.
        #[arg(long)]
        mask: Option<PathBuf>,
    },
    /// Evaluate the classical detector over a directory of `*.gt.json` +
    /// referenced images.
    Eval { dir: PathBuf },
    /// Write a synthetic demo gel to a .gel.zip for testing.
    MakeDemo { out: PathBuf },
    /// Write a synthetic evaluation dataset (image + ground truth) to a dir.
    MakeDataset { dir: PathBuf },
    /// Convert GelGenie-style segmentation masks into `*.gt.json` ground truth
    /// (written next to each image) so real annotated gels can be `eval`'d.
    ImportMasks { dir: PathBuf },
    /// Render a batch of simulated gels (with effects + ground truth) in
    /// parallel to a dataset directory.
    Simulate {
        dir: PathBuf,
        /// Number of gels to render.
        #[arg(long, default_value_t = 20)]
        count: usize,
        /// Base random seed.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Disable rotation (upright gels, for pure detection benchmarking).
        #[arg(long)]
        upright: bool,
        /// Warp model: `nurbs` (default) or `smilewobble` (fast analytic).
        #[arg(long, default_value = "nurbs")]
        warp_mode: String,
        /// Run the evaluation harness after generating.
        #[arg(long)]
        eval: bool,
    },
}

fn parse_gel_type(s: &str) -> Result<GelType> {
    match s.to_lowercase().as_str() {
        "dna" => Ok(GelType::Dna),
        "rna" => Ok(GelType::Rna),
        "protein" => Ok(GelType::Protein),
        other => anyhow::bail!("unknown gel type `{other}` (expected dna|rna|protein)"),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Info { path } => cmd_info(&path),
        Command::Ladders { gel_type } => cmd_ladders(gel_type.as_deref()),
        Command::Analyze {
            path,
            gel_type,
            out,
            min_r2,
            mask,
        } => cmd_analyze(&path, gel_type.as_deref(), out.as_deref(), min_r2, mask.as_deref()),
        Command::Eval { dir } => cmd_eval(&dir),
        Command::MakeDemo { out } => demo::write_demo(&out),
        Command::MakeDataset { dir } => demo::write_dataset(&dir),
        Command::ImportMasks { dir } => {
            let n = masks::import_dir(&dir)?;
            println!("wrote {n} ground-truth files from masks in {}", dir.display());
            Ok(())
        }
        Command::Simulate {
            dir,
            count,
            seed,
            upright,
            warp_mode,
            eval,
        } => cmd_simulate(&dir, count, seed, upright, &warp_mode, eval),
    }
}

fn parse_warp_mode(s: &str) -> Result<opengel::sim::WarpMode> {
    match s.to_lowercase().replace(['-', '_'], "").as_str() {
        "nurbs" => Ok(opengel::sim::WarpMode::Nurbs),
        "smilewobble" | "smile" | "analytic" => Ok(opengel::sim::WarpMode::SmileWobble),
        other => anyhow::bail!("unknown warp mode `{other}` (expected nurbs|smilewobble)"),
    }
}

fn cmd_info(path: &std::path::Path) -> Result<()> {
    let doc = GelDocument::load(path).with_context(|| format!("loading {}", path.display()))?;
    let p = &doc.project;
    println!("format {} v{}  gel_type {:?}", p.format, p.version, p.gel_type);
    println!("images: {}", p.images.len());
    for img in &p.images {
        println!(
            "  {} {}x{}  {:.3}s{}",
            img.filename,
            img.width,
            img.height,
            img.meta.exposure_seconds,
            img.meta
                .bracket_group
                .map(|g| format!("  bracket {g}"))
                .unwrap_or_default()
        );
    }
    let a = &p.analysis;
    println!(
        "analysis: {} lanes, {} bands, {} blobs, {} ladder assignment(s)",
        a.lanes.len(),
        a.bands.len(),
        a.blobs.len(),
        a.ladder_assignments.len()
    );
    for la in &a.ladder_assignments {
        println!("  lane {} = {}", la.lane_id, la.template_name);
    }
    Ok(())
}

fn cmd_ladders(gel_type: Option<&str>) -> Result<()> {
    let filter = gel_type.map(parse_gel_type).transpose()?;
    for t in ladders::all() {
        if let Some(gt) = filter {
            if t.gel_type != gt {
                continue;
            }
        }
        // Mark reference (extra-thick) bands with * and ng where known.
        let sizes: Vec<String> = t
            .bands
            .iter()
            .map(|b| {
                let star = if b.reference { "*" } else { "" };
                match b.mass_ng {
                    Some(ng) => format!("{}{star}({ng}ng)", b.size),
                    None => format!("{}{star}", b.size),
                }
            })
            .collect();
        let cat = t.catalog.as_deref().unwrap_or("-");
        let vendor = t.vendor.as_deref().unwrap_or("");
        println!(
            "{:?}  {} [{vendor} {cat}]  ({} bands, * = reference: {})",
            t.gel_type,
            t.name,
            t.bands.len(),
            sizes.join(", ")
        );
    }
    Ok(())
}

/// Load a `.gel.zip`, or wrap a loose image (jpg/png/…) into a new project.
fn load_or_import(path: &std::path::Path, gel_type: GelType) -> Result<GelDocument> {
    let is_zip = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);
    if is_zip {
        GelDocument::load(path).with_context(|| format!("loading {}", path.display()))
    } else {
        let img = image::open(path).with_context(|| format!("opening image {}", path.display()))?;
        let meta = opengel::core::CaptureMeta {
            camera_name: Some("imported".into()),
            ..Default::default()
        };
        Ok(GelDocument::from_frames(gel_type, vec![img], vec![meta]))
    }
}

fn cmd_analyze(
    path: &std::path::Path,
    gel_type: Option<&str>,
    out: Option<&std::path::Path>,
    min_r2: f64,
    mask: Option<&std::path::Path>,
) -> Result<()> {
    let gt_override = gel_type.map(parse_gel_type).transpose()?;
    let mut doc = load_or_import(path, gt_override.unwrap_or(GelType::Dna))?;
    if let Some(g) = gt_override {
        doc.project.gel_type = g;
    }
    let gt = doc.project.gel_type;
    let img = work::working_image(&doc).context("no images to analyze")?;

    let params = DetectParams::default();
    let analysis = if let Some(mask_path) = mask {
        // Mask-driven detection (e.g. a GelGenie segmentation mask): the mask's
        // connected components become bands, clustered into lanes, then run
        // through the same ladder-ID + sizing pipeline as any detector.
        use opengel::detect::detector::GelDetector;
        let mask_img = image::open(mask_path)
            .with_context(|| format!("opening mask {}", mask_path.display()))?;
        let mask_gray = opengel::core::GrayF32::from_dynamic(&mask_img);
        let segmenter = opengel::detect::MaskSegmenter::new(mask_gray);
        let det = opengel::detect::cellpose::CellposeDetector::new(segmenter).detect(&img, &params);
        opengel::detect::analyze_detection(det, gt, &[], min_r2)
    } else {
        opengel::detect::analyze(&img, gt, &params, &[], min_r2)
    };
    println!(
        "detected {} lanes, {} bands",
        analysis.lanes.len(),
        analysis.bands.len()
    );
    for la in &analysis.ladder_assignments {
        println!("  identified ladder: lane {} = {}", la.lane_id, la.template_name);
    }
    doc.project.analysis = analysis;

    let out_path = out.unwrap_or(path);
    doc.save(out_path)
        .with_context(|| format!("writing {}", out_path.display()))?;
    println!("wrote {}", out_path.display());
    Ok(())
}

fn cmd_simulate(
    dir: &std::path::Path,
    count: usize,
    seed: u64,
    upright: bool,
    warp_mode: &str,
    eval: bool,
) -> Result<()> {
    let mode = parse_warp_mode(warp_mode)?;
    let gels = opengel::sim::simulate_random_batch_with(count, seed, upright, mode);
    let n = opengel::sim::write_dataset(dir, &gels)?;
    println!(
        "rendered {n} simulated gels{} ({:?} warp) to {}",
        if upright { " (upright)" } else { " (rotated ≤50°)" },
        mode,
        dir.display()
    );
    if eval {
        println!("---");
        cmd_eval(dir)?;
    }
    Ok(())
}

fn cmd_eval(dir: &std::path::Path) -> Result<()> {
    use opengel::detect::detector::GelDetector;
    use opengel::detect::eval::{aggregate, evaluate, GroundTruth};
    use opengel::detect::ClassicalDetector;

    let detector = ClassicalDetector::new();
    let params = DetectParams::default();
    let mut metrics = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let gt: GroundTruth = serde_json::from_str(&text)
            .with_context(|| format!("parsing ground truth {}", path.display()))?;
        let img_path = dir.join(&gt.image);
        let dynimg = image::open(&img_path)
            .with_context(|| format!("opening image {}", img_path.display()))?;
        let img = opengel::core::GrayF32::from_dynamic(&dynimg);
        let m = evaluate(&detector, &img, &params, &gt, 6.0);
        println!(
            "{:<28} lanes {}/{}  band P/R {:.2}/{:.2}  MAE {:.2}px",
            gt.image,
            m.lane_count_pred,
            m.lane_count_true,
            m.band_precision(),
            m.band_recall(),
            m.band_pos_mae
        );
        metrics.push(m);
    }

    if metrics.is_empty() {
        println!("no *.json ground truth files found in {}", dir.display());
        return Ok(());
    }
    let agg = aggregate(&metrics);
    println!("---");
    println!(
        "[{}] {} images  lane IoU {:.3}  band F1 {:.3} (P {:.3} R {:.3})  MAE {:.2}px",
        detector.name(),
        agg.images,
        agg.mean_lane_iou,
        agg.mean_band_f1,
        agg.mean_band_precision,
        agg.mean_band_recall,
        agg.mean_band_pos_mae
    );
    Ok(())
}
