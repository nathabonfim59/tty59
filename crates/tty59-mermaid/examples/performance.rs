//! Render-performance sanity check plus an optional PNG dump for visual review.
//!
//! Benchmark (release):
//!
//! ```text
//! cargo run --release -p tty59-mermaid --example performance
//! ```
//!
//! Dump the light-theme review images (flowchart + sequence, long lowercase
//! labels) into a directory supplied by the caller:
//!
//! ```text
//! cargo run --release -p tty59-mermaid --example performance -- --dump <dir>
//! ```
//!
//! The benchmark warms up first (font database load, pipeline caches) and then
//! reports the median wall time of the full `render` path over several samples.
//! It deliberately has no pass/fail threshold; the number is for the milestone
//! report, not a test.

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use resvg::tiny_skia;
use tty59_mermaid::{MermaidTheme, render};

const WARMUP_ROUNDS: usize = 3;
const SAMPLES: usize = 9;
const NODE_COUNT: usize = 30;
const DUMP_SCALE: f32 = 2.0;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [] => benchmark(),
        [dir] => dump(Path::new(dir)),
        [flag, dir] if flag == "--dump" => dump(Path::new(dir)),
        _ => {
            eprintln!("usage: performance [--dump] <directory>");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("performance: {message}");
            ExitCode::FAILURE
        }
    }
}

/// A 30-node flowchart: wide enough to exercise layout and raster work, small
/// enough to run in a few milliseconds in release mode.
fn benchmark_flowchart() -> String {
    let mut source = String::from("flowchart TD\n");
    for i in 0..NODE_COUNT {
        writeln!(source, "    N{i}[step {i}]").expect("write node");
    }
    for i in 0..NODE_COUNT - 1 {
        writeln!(source, "    N{i} --> N{}", i + 1).expect("write edge");
    }
    source
}

fn benchmark() -> Result<(), String> {
    let source = benchmark_flowchart();
    let theme = MermaidTheme::light();

    for round in 0..WARMUP_ROUNDS {
        render(&source, &theme, 1.0)
            .map_err(|error| format!("warmup render {round} failed: {error}"))?;
    }

    let mut samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let started = Instant::now();
        let raster = render(&source, &theme, 1.0)
            .map_err(|error| format!("sample {sample} failed: {error}"))?;
        std::hint::black_box(&raster);
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }

    samples.sort_by(f64::total_cmp);
    let median = samples[SAMPLES / 2];
    let min = samples[0];
    let max = samples[SAMPLES - 1];
    println!(
        "tty59-mermaid {NODE_COUNT}-node flowchart: warmup {WARMUP_ROUNDS}, samples {SAMPLES}, \
         median {median:.2} ms (min {min:.2} ms, max {max:.2} ms)"
    );

    Ok(())
}

/// Long, lowercase labels make font-family width mismatches between Mermaid's
/// text measurement and resvg's shaping easy to spot in the dump images.
fn review_flowchart() -> String {
    "\
flowchart TD
    Start([markdown preview receives a fenced diagram block]) --> Parse{language is mermaid and the fence body is not empty}
    Parse -->|valid| Layout[run merman layout with the selected theme font family]
    Layout --> Render[render svg and rasterize it to background bgra pixels]
    Layout -->|invalid| Error[show a short inline syntax message and the original code block]
    Render --> Preview[scale the image to fit the preview pane without upscaling]
    Error --> Preview
"
    .to_string()
}

fn review_sequence() -> String {
    "\
sequenceDiagram
    participant C as Client
    participant S as Server
    C->>S: please render this long lowercase sequence message without clipping the label text
    S-->>C: rendered diagram bytes are ready for the preview once the background task finishes
    Note over C,S: long labels make font width mismatches easy to spot
"
    .to_string()
}

fn dump(directory: &Path) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;

    let theme = MermaidTheme::light();
    for (file_name, source) in [
        ("flowchart.png", review_flowchart()),
        ("sequence.png", review_sequence()),
    ] {
        let raster =
            render(&source, &theme, DUMP_SCALE).map_err(|error| format!("{file_name}: {error}"))?;
        let pixmap = bgra_to_premultiplied_rgba(&raster)?;
        let path = directory.join(file_name);
        pixmap
            .save_png(&path)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        println!(
            "wrote {} ({}x{} at {:.1}x)",
            path.display(),
            raster.width,
            raster.height,
            raster.scale
        );
    }

    Ok(())
}

/// `Raster::bgra` is straight-alpha BGRA (what gpui's `RenderImage` wants),
/// while tiny-skia's `Pixmap` stores premultiplied RGBA. Convert channel order
/// and premultiply so the review PNG goes through the exact same public
/// `render` path (including the crate's font resolution) as the preview.
fn bgra_to_premultiplied_rgba(raster: &tty59_mermaid::Raster) -> Result<tiny_skia::Pixmap, String> {
    let mut pixmap = tiny_skia::Pixmap::new(raster.width, raster.height)
        .ok_or_else(|| format!("cannot allocate {}x{} pixmap", raster.width, raster.height))?;

    let pixels = raster.bgra.as_chunks::<4>().0;
    let target = pixmap.data_mut().as_chunks_mut::<4>().0;
    debug_assert_eq!(pixels.len(), target.len());
    for (source, destination) in pixels.iter().zip(target.iter_mut()) {
        let (b, g, r, a) = (source[0], source[1], source[2], source[3]);
        let alpha = u16::from(a);
        let premultiply = |channel: u8| ((u16::from(channel) * alpha + 127) / 255) as u8;
        destination[0] = premultiply(r);
        destination[1] = premultiply(g);
        destination[2] = premultiply(b);
        destination[3] = a;
    }

    Ok(pixmap)
}
