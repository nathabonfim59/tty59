//! Render a light+dark PNG gallery of every diagram family with per-shape
//! accent colors, plus the unsupported families demonstrating that they stay
//! neutral.
//!
//! ```text
//! cargo run --release -p tty59-mermaid --example accent_gallery -- <dir>
//! ```
//!
//! The output directory is created if needed. Each file is
//! `<type>-<light|dark>.png`, rendered through the public `render` path so the
//! images match the preview exactly (font resolution included).

use std::path::Path;
use std::process::ExitCode;

use resvg::tiny_skia;
use tty59_mermaid::{Accent, MermaidTheme, Rgba, render};

const DUMP_SCALE: f32 = 2.0;

const FLOWCHART: &str = "\
flowchart TD
    A[Source] --> B{Valid?}
    B -->|yes| C[Render diagram]
    B -->|no| D[Show syntax error]
    C --> E[Preview]
    D --> E
";

const SEQUENCE: &str = "\
sequenceDiagram
    participant C as Client
    participant S as Server
    participant D as Disk
    C->>S: request diagram
    activate S
    S->>D: read cache
    D-->>S: bytes
    S-->>C: response
    deactivate S
    Note over C,S: shared note
";

const CLASS: &str = "\
classDiagram
    class Shape {
      +area() double
    }
    class Circle {
      +radius double
    }
    class Square {
      +side double
    }
    Shape <|-- Circle
    Shape <|-- Square
";

const STATE: &str = "\
stateDiagram-v2
    [*] --> Idle
    Idle --> Running: start
    Running --> Paused: pause
    Paused --> Running: resume
    Running --> Idle: stop
    Running --> [*]
";

const ER: &str = "\
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE_ITEM : contains
    PRODUCT ||--o{ LINE_ITEM : appears
";

const MINDMAP: &str = "\
mindmap
  root((plan))
    build
      compile
      package
    test
      unit
      render
    ship
      release
";

const GANTT: &str = "\
gantt
    title Release plan
    dateFormat YYYY-MM-DD
    section Build
    Compile :a1, 2024-01-01, 3d
    Test :a2, after a1, 2d
    section Ship
    Package :b1, after a2, 1d
";

const PIE: &str = "\
pie title Work split
    \"Build\" : 50
    \"Test\" : 30
    \"Docs\" : 20
";

const GIT_GRAPH: &str = "\
gitGraph
    commit id: \"init\"
    branch feature
    checkout feature
    commit id: \"work\"
    checkout main
    merge feature
";

const XYCHART: &str = "\
xychart-beta
    title \"Sales\"
    x-axis [jan, feb, mar]
    y-axis \"Revenue\" 0 --> 100
    bar [10, 40, 70]
    line [5, 20, 50]
";

const SUPPORTED: [(&str, &str); 6] = [
    ("flowchart", FLOWCHART),
    ("sequence", SEQUENCE),
    ("class", CLASS),
    ("state", STATE),
    ("er", ER),
    ("mindmap", MINDMAP),
];

const UNSUPPORTED: [(&str, &str); 4] = [
    ("gantt", GANTT),
    ("pie", PIE),
    ("gitgraph", GIT_GRAPH),
    ("xychart", XYCHART),
];

fn rgb(value: u32) -> Rgba {
    Rgba::rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn light_accents() -> Vec<Accent> {
    vec![
        accent(0xfce7e7, 0xc0392b, 0x501a14),
        accent(0xe7f2fc, 0x1f6fb2, 0x0f3352),
        accent(0xe8f7ea, 0x2e7d32, 0x143d17),
        accent(0xfff6e0, 0xa86a00, 0x4d3100),
        accent(0xf3e8fd, 0x7b3fb5, 0x381b54),
        accent(0xe0f7f7, 0x0f7676, 0x053838),
    ]
}

fn dark_accents() -> Vec<Accent> {
    vec![
        accent(0x3a1d1d, 0xf28b82, 0xffd9d6),
        accent(0x162a3d, 0x6cb6ef, 0xd6e9fb),
        accent(0x17321a, 0x7dcf82, 0xdaf2dc),
        accent(0x3a2e12, 0xe8b84a, 0xfbeec8),
        accent(0x2b1c3a, 0xc79bef, 0xedddfb),
        accent(0x123333, 0x5ed1d1, 0xd3f5f5),
    ]
}

fn accent(fill: u32, border: u32, text: u32) -> Accent {
    Accent {
        fill: rgb(fill),
        border: rgb(border),
        text: rgb(text),
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(directory) = args.next() else {
        eprintln!("usage: accent_gallery <directory>");
        return ExitCode::from(2);
    };
    if args.next().is_some() {
        eprintln!("usage: accent_gallery <directory>");
        return ExitCode::from(2);
    }
    match dump(Path::new(&directory)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("accent_gallery: {message}");
            ExitCode::FAILURE
        }
    }
}

fn dump(directory: &Path) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;

    for (name, source) in SUPPORTED {
        for (mode, accents) in [("light", light_accents()), ("dark", dark_accents())] {
            let dark = mode == "dark";
            let mut theme = if dark {
                MermaidTheme::dark()
            } else {
                MermaidTheme::light()
            };
            theme.accents = accents;
            let raster = render(source, &theme, DUMP_SCALE)
                .map_err(|error| format!("{name} {mode}: {error}"))?;
            write_png(
                directory,
                &format!("{name}-{mode}.png"),
                &raster,
                theme.background,
            )?;
        }
    }

    // Unsupported families must look exactly like the neutral render. Compare
    // the pixels and report it; the PNGs let a reviewer confirm it visually.
    for (name, source) in UNSUPPORTED {
        for (mode, accents) in [("light", light_accents()), ("dark", dark_accents())] {
            let dark = mode == "dark";
            let neutral = if dark {
                MermaidTheme::dark()
            } else {
                MermaidTheme::light()
            };
            let mut accented = neutral.clone();
            accented.accents = accents;
            let plain = render(source, &neutral, DUMP_SCALE)
                .map_err(|error| format!("{name} {mode} neutral: {error}"))?;
            let colored = render(source, &accented, DUMP_SCALE)
                .map_err(|error| format!("{name} {mode} accented: {error}"))?;
            if plain.bgra != colored.bgra {
                return Err(format!(
                    "{name} {mode}: accents changed an unsupported family"
                ));
            }
            write_png(
                directory,
                &format!("{name}-{mode}.png"),
                &colored,
                neutral.background,
            )?;
        }
    }

    Ok(())
}

fn write_png(
    directory: &Path,
    file_name: &str,
    raster: &tty59_mermaid::Raster,
    background: Rgba,
) -> Result<(), String> {
    let pixmap = opaque_pixmap(raster, background)?;
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
    Ok(())
}

/// `Raster::bgra` is straight-alpha BGRA (what gpui's `RenderImage` wants),
/// while tiny-skia's `Pixmap` stores premultiplied RGBA. Composite over the
/// theme background so the PNG reproduces the host preview when opened
/// standalone, then premultiply for tiny-skia.
fn opaque_pixmap(
    raster: &tty59_mermaid::Raster,
    background: Rgba,
) -> Result<tiny_skia::Pixmap, String> {
    let mut pixmap = tiny_skia::Pixmap::new(raster.width, raster.height)
        .ok_or_else(|| format!("cannot allocate {}x{} pixmap", raster.width, raster.height))?;
    let background = [background.r, background.g, background.b];
    let target = pixmap.data_mut().as_chunks_mut::<4>().0;
    for (source, destination) in raster.bgra.as_chunks::<4>().0.iter().zip(target.iter_mut()) {
        let (b, g, r, a) = (source[0], source[1], source[2], source[3]);
        let alpha = u32::from(a);
        let blend = |channel: u8, behind: u8| -> u8 {
            ((u32::from(channel) * alpha + u32::from(behind) * (255 - alpha) + 127) / 255) as u8
        };
        destination[0] = blend(r, background[0]);
        destination[1] = blend(g, background[1]);
        destination[2] = blend(b, background[2]);
        destination[3] = 255;
    }
    Ok(pixmap)
}
