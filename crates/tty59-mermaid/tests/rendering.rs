//! Public-contract tests for `tty59-mermaid`.
//!
//! Everything here goes through the frozen public API (`render_svg`,
//! `rasterize`, `render`, `MermaidTheme`, `MermaidError`, `MAX_PIXELS`); the
//! fixtures are neutral, self-contained Mermaid sources covering each diagram
//! family the crate enables. Direct raster edge cases (channel order, invalid
//! scales, premultiplication) are covered by the crate's unit tests.

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use resvg::tiny_skia;
use tty59_mermaid::{MAX_PIXELS, MermaidError, MermaidTheme, Rgba, rasterize, render, render_svg};

const FLOWCHART: &str = "\
flowchart TD
    A[Source] --> B{Valid?}
    B -->|yes| C[Render diagram]
    B -->|no| D[Show syntax error]
    C --> E[Preview]
";

const SEQUENCE: &str = "\
sequenceDiagram
    participant C as Client
    participant S as Server
    C->>S: request diagram
    S-->>C: response bytes
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
    Shape <|-- Circle
";

const STATE: &str = "\
stateDiagram-v2
    [*] --> Idle
    Idle --> Running: start
    Running --> Idle: stop
    Running --> [*]
";

const ER: &str = "\
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE_ITEM : contains
";

const GANTT: &str = "\
gantt
    title Release plan
    dateFormat YYYY-MM-DD
    axisFormat %m-%d
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

const MINDMAP: &str = "\
mindmap
  root((plan))
    build
      compile
      package
    test
      unit
      render
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

const DIAGRAMS: [(&str, &str); 9] = [
    ("flowchart", FLOWCHART),
    ("sequence", SEQUENCE),
    ("class", CLASS),
    ("state", STATE),
    ("er", ER),
    ("gantt", GANTT),
    ("pie", PIE),
    ("mindmap", MINDMAP),
    ("gitGraph", GIT_GRAPH),
];

fn has_visible_pixel(raster: &tty59_mermaid::Raster) -> bool {
    // Straight-alpha BGRA: alpha is the fourth byte of each pixel.
    raster.bgra.as_chunks::<4>().0.iter().any(|px| px[3] != 0)
}

fn assert_raster_shape(raster: &tty59_mermaid::Raster, what: &str) {
    assert!(
        raster.width > 0 && raster.height > 0,
        "{what}: empty raster {}x{}",
        raster.width,
        raster.height
    );
    assert_eq!(
        raster.bgra.len(),
        raster.width as usize * raster.height as usize * 4,
        "{what}: BGRA length does not match {}x{}",
        raster.width,
        raster.height
    );
    assert!(
        has_visible_pixel(raster),
        "{what}: raster is fully transparent"
    );
}

/// Every enabled diagram family must produce SVG and rasters at scale 1.
#[test]
fn every_diagram_type_renders_svg_and_visible_pixels() {
    let theme = MermaidTheme::light();
    for (name, source) in DIAGRAMS {
        let svg = match render_svg(source, &theme) {
            Ok(svg) => svg,
            Err(error) => panic!("{name}: render_svg failed: {error}"),
        };
        assert!(svg.contains("<svg"), "{name}: output is not an SVG element");

        let raster = match rasterize(&svg, 1.0) {
            Ok(raster) => raster,
            Err(error) => panic!("{name}: rasterize failed: {error}"),
        };
        assert_raster_shape(&raster, name);
    }

    // The composition helper must reach the same pipeline.
    let raster = render(FLOWCHART, &theme, 1.0).expect("render(flowchart)");
    assert_raster_shape(&raster, "render(flowchart)");
}

#[test]
fn light_and_dark_themes_change_svg_and_pixels() {
    let light_svg = render_svg(FLOWCHART, &MermaidTheme::light()).expect("light svg");
    let dark_svg = render_svg(FLOWCHART, &MermaidTheme::dark()).expect("dark svg");
    assert_ne!(light_svg, dark_svg, "light and dark SVGs are identical");

    let light = render(FLOWCHART, &MermaidTheme::light(), 1.0).expect("light raster");
    let dark = render(FLOWCHART, &MermaidTheme::dark(), 1.0).expect("dark raster");
    assert_ne!(light.bgra, dark.bgra, "light and dark pixels are identical");
}

#[test]
fn custom_theme_colors_appear_in_svg() {
    let mut theme = MermaidTheme::light();
    theme.primary = Rgba::rgb(0xf0, 0xa0, 0xc0);
    theme.primary_text = Rgba::rgb(0x10, 0x20, 0x30);
    theme.line = Rgba::rgb(0x12, 0x34, 0x56);

    let svg = render_svg(FLOWCHART, &theme)
        .expect("custom theme svg")
        .to_lowercase();
    for (name, color) in [
        ("primary", theme.primary),
        ("primary_text", theme.primary_text),
        ("line", theme.line),
    ] {
        let css = color.to_css();
        assert!(
            svg.contains(&css),
            "{name} color {css} missing from the SVG"
        );
    }

    let custom = render(FLOWCHART, &theme, 1.0).expect("custom raster");
    let default = render(FLOWCHART, &MermaidTheme::light(), 1.0).expect("default raster");
    assert_ne!(
        custom.bgra, default.bgra,
        "custom theme did not change the raster"
    );
}

#[test]
fn output_is_deterministic_for_the_same_input() {
    let theme = MermaidTheme::light();

    let first_svg = render_svg(SEQUENCE, &theme).expect("first svg");
    let second_svg = render_svg(SEQUENCE, &theme).expect("second svg");
    assert_eq!(first_svg, second_svg, "repeated SVG renders differ");

    let first = render(SEQUENCE, &theme, 1.0).expect("first raster");
    let second = render(SEQUENCE, &theme, 1.0).expect("second raster");
    assert_eq!(first, second, "repeated raster renders differ");
}

#[test]
fn empty_whitespace_and_non_mermaid_sources_are_syntax_errors() {
    let theme = MermaidTheme::light();
    let bad_sources = [
        "",
        " ",
        "\n\t\r\n   ",
        "this is not a mermaid diagram",
        "   \n  hello world\n",
        // Malformed diagram bodies, not just unknown diagram types.
        "flowchart TD\n    A --> B\n    -->\n",
        "sequenceDiagram\n    A->>B: hello\n    deactivate B\n",
    ];

    for source in bad_sources {
        match render_svg(source, &theme) {
            Err(MermaidError::Syntax(message)) => {
                assert!(
                    !message.trim().is_empty(),
                    "empty syntax message for {source:?}"
                );
            }
            other => panic!("expected Syntax for {source:?}, got {other:?}"),
        }

        // `render` must surface the same class of error, not a raster failure.
        match render(source, &theme, 1.0) {
            Err(MermaidError::Syntax(_)) => {}
            other => panic!("expected Syntax from render({source:?}), got {other:?}"),
        }
    }
}

#[test]
fn garbage_sources_never_panic() {
    let theme = MermaidTheme::light();
    let long_garbage = "x".repeat(4096);
    let garbage = [
        "mermaid",
        "%%{init: {\"theme\": \"base\"}}%%",
        "flowchart TD\n  A --> B\n  C -->",
        "sequenceDiagram\n  ->",
        "<script>alert(1)</script>",
        "\u{0}\u{1}\u{2}\u{3}\u{fffd}",
        "flowchart TD\n  A[unterminated",
        long_garbage.as_str(),
    ];

    for source in garbage {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _ = render(source, &theme, 1.0);
        }));
        assert!(outcome.is_ok(), "render panicked on {source:?}");
    }
}

#[test]
fn leading_whitespace_and_tilde_fence_bodies_render() {
    let theme = MermaidTheme::light();

    // A `~~~mermaid` fence body as the preview layer extracts it: the lines
    // between the fences, no fence markers (the frozen API takes the body).
    let fenced = "~~~mermaid\nflowchart TD\n    A[source] --> B[render]\n~~~\n";
    let body = fenced
        .lines()
        .skip(1)
        .take_while(|line| *line != "~~~")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!body.contains("~~~"), "fence marker leaked into the body");

    let indented = format!("\n\n    {body}\n   ");
    for (label, source) in [
        ("tilde fence body", body.as_str()),
        ("leading whitespace", indented.as_str()),
    ] {
        let svg = render_svg(source, &theme).unwrap_or_else(|error| panic!("{label}: {error}"));
        assert!(svg.contains("<svg"), "{label}: output is not an SVG");

        let raster =
            rasterize(&svg, 1.0).unwrap_or_else(|error| panic!("{label}: rasterize: {error}"));
        assert_raster_shape(&raster, label);
    }
}

#[test]
fn huge_source_raster_stays_within_pixel_cap() {
    const NODES: usize = 128;
    // Large enough that the requested device-pixel area exceeds MAX_PIXELS,
    // while the SVG itself stays inside merman's own postprocess resource caps.
    const REQUESTED_SCALE: f32 = 16.0;
    let mut source = String::from("flowchart LR\n");
    for i in 0..NODES {
        writeln!(source, "    N{i}[step number {i}]").expect("write node");
    }
    for i in 0..NODES - 1 {
        writeln!(source, "    N{i} --> N{}", i + 1).expect("write edge");
    }

    let theme = MermaidTheme::light();
    let svg = render_svg(&source, &theme).expect("huge source svg");

    // A wide chain at the requested scale would blow past the cap; rasterize
    // must downscale instead of refusing or allocating the full area.
    let raster = rasterize(&svg, REQUESTED_SCALE).expect("huge source raster");
    let pixels = u64::from(raster.width) * u64::from(raster.height);
    assert!(
        pixels <= MAX_PIXELS,
        "raster {}x{} = {pixels} pixels exceeds MAX_PIXELS {MAX_PIXELS}",
        raster.width,
        raster.height
    );
    assert!(
        raster.scale < REQUESTED_SCALE,
        "requested scale {REQUESTED_SCALE} was not capped (returned {}), so the cap branch did not run",
        raster.scale
    );
    assert_eq!(
        raster.bgra.len(),
        raster.width as usize * raster.height as usize * 4,
        "BGRA length does not match {}x{}",
        raster.width,
        raster.height
    );
    assert!(
        has_visible_pixel(&raster),
        "capped huge raster is fully transparent"
    );
}

/// Standard-alphabet base64 with padding — just enough to embed the sentinel
/// PNG into a `data:` URL without adding a dependency.
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (chunks, rest) = data.as_chunks::<3>();
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in chunks {
        let word = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        for shift in [18, 12, 6, 0] {
            out.push(TABLE[(word >> shift & 0x3f) as usize] as char);
        }
    }
    match rest {
        [a] => {
            let word = u32::from(*a) << 16;
            out.push(TABLE[(word >> 18 & 0x3f) as usize] as char);
            out.push(TABLE[(word >> 12 & 0x3f) as usize] as char);
            out.push_str("==");
        }
        [a, b] => {
            let word = (u32::from(*a) << 16) | (u32::from(*b) << 8);
            out.push(TABLE[(word >> 18 & 0x3f) as usize] as char);
            out.push(TABLE[(word >> 12 & 0x3f) as usize] as char);
            out.push(TABLE[(word >> 6 & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// A 16×16 vivid-magenta PNG as a `data:image/png;base64,…` URL. Magenta is
/// absent from both neutral theme palettes, so any of its pixels in a raster
/// can only come from the embedded image being decoded and drawn.
fn sentinel_png_data_url() -> String {
    let mut pixmap = tiny_skia::Pixmap::new(16, 16).expect("16x16 pixmap");
    pixmap.fill(tiny_skia::Color::from_rgba8(0xff, 0x00, 0xff, 0xff));
    let png = pixmap.encode_png().expect("encode sentinel png");
    format!("data:image/png;base64,{}", base64(&png))
}

const SENTINEL_BGRA: [u8; 4] = [0xff, 0x00, 0xff, 0xff];

/// Post-fix assurance, end to end (untrusted markdown → pixels): an
/// embedded raster is the one image href merman's resvg-safe pipeline lets
/// through to the SVG. This crate's `resvg` build paints no raster hrefs
/// anyway (`raster-images` is off), so this cannot fail on the old
/// resolver; the failing-before proof is the parsed-tree unit test
/// `mermaid_embedded_image_href_parses_to_no_image_node`. What this pins
/// down: the href really reaches the SVG, the full render stays
/// sentinel-free, and the diagram's own shapes and labels still render.
#[test]
fn mermaid_embedded_image_href_is_not_drawn() {
    let source = format!(
        "flowchart TD\n    Shot@{{ img: \"{}\", label: \"blocked\" }}\n    Shot --> Check[preview]\n",
        sentinel_png_data_url()
    );
    let theme = MermaidTheme::light();

    let svg = render_svg(&source, &theme)
        .unwrap_or_else(|error| panic!("render_svg rejected the image-square source: {error}"));
    // This only proves anything while the href actually reaches the SVG.
    assert!(
        svg.contains(r#"<image href="data:image/png"#),
        "no image href in the SVG; the blocked-image path was not reached"
    );

    let raster = render(&source, &theme, 2.0).expect("render image-square diagram");
    assert!(
        has_visible_pixel(&raster),
        "diagram chrome (shapes/labels) must still render"
    );
    let sentinel_pixels = raster
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| **px == SENTINEL_BGRA)
        .count();
    assert_eq!(
        sentinel_pixels, 0,
        "embedded data-URL raster was decoded and drawn"
    );
}

/// Layered defense, documented end-to-end (NOT a before/after proof: merman
/// stops these itself, so this passes on the old rasterizer too). A
/// file-path image href in untrusted markdown never reaches the pixels:
/// today merman's resvg-safe sanitizer silently strips the href of a real,
/// existing PNG reference (its terminal validation errors on whatever
/// survives sanitization with a resvg-safe contract / non-local resource
/// message); should a future merman ever pass one through, the
/// rasterizer's pinned resolver still refuses to read the disk. The
/// sentinel PNG is really written to disk so a Syntax error or a
/// missing-file skip can never mask the outcome.
#[test]
fn mermaid_file_image_href_never_reaches_pixels() {
    let path = std::env::temp_dir().join(format!(
        "tty59-mermaid-e2e-image-{}.png",
        std::process::id()
    ));
    let mut pixmap = tiny_skia::Pixmap::new(16, 16).expect("16x16 pixmap");
    pixmap.fill(tiny_skia::Color::from_rgba8(0xff, 0x00, 0xff, 0xff));
    let png = pixmap.encode_png().expect("encode sentinel png");
    std::fs::write(&path, png).expect("write sentinel png");
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(path.clone());
    let href = path.display().to_string();

    let source = format!(
        "flowchart TD\n    Shot@{{ img: \"{href}\", label: \"blocked\" }}\n    Shot --> Check[preview]\n"
    );
    let theme = MermaidTheme::light();

    match render_svg(&source, &theme) {
        // A Syntax error would mean the fixture stopped exercising the href
        // path (the directive no longer parses), not that images are safe.
        Err(MermaidError::Syntax(message)) => {
            panic!("source no longer parses; fixture regressed: {message}");
        }
        // merman refusing what survives its sanitizer is the refusing
        // outcome; accept its current and historic wordings.
        Err(MermaidError::Render(message)) => {
            assert!(
                message.contains("resvg-safe contract") || message.contains("non-local resource"),
                "unexpected Render error: {message}"
            );
        }
        Ok(svg) => {
            // Current merman: the sanitizer strips the file href but keeps
            // the (now href-less) <image> element. A future merman may pass
            // the href through instead; both must stay sentinel-free.
            if svg.contains(&href) {
                assert!(
                    svg.contains(&format!(r#"<image href="{href}""#)),
                    "path present in the SVG but not as the image href"
                );
            } else {
                assert!(
                    svg.contains("<image"),
                    "image element vanished; fixture no longer reaches the image path"
                );
            }
            let raster = render(&source, &theme, 1.0).expect("render image-square diagram");
            assert!(
                has_visible_pixel(&raster),
                "diagram chrome (shapes/labels) must still render"
            );
            assert!(
                !raster.bgra.as_chunks::<4>().0.contains(&SENTINEL_BGRA),
                "file image leaked into pixels"
            );
        }
    }
}
