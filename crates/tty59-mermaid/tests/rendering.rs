//! Public-contract tests for `tty59-mermaid`.
//!
//! Everything here goes through the frozen public API (`render_svg`,
//! `rasterize`, `render`, `MermaidTheme`, `MermaidError`, `MAX_PIXELS`); the
//! fixtures are neutral, self-contained Mermaid sources covering each diagram
//! family the crate enables. Direct raster edge cases (channel order, invalid
//! scales, premultiplication) are covered by the crate's unit tests.

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

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
