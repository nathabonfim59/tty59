//! Dark-theme review gallery for the class-clipping fix.
//!
//! Writes all nine enabled diagram families as SVG + PNG at 2× on the dark
//! theme, plus the class fixture in three forms:
//!
//! - `class-before.*`: production SVG with its generated class-node
//!   viewports re-clipped to merman's pre-fix `overflow: hidden` default;
//! - `class-after.*`: production SVG (the fix);
//! - `class-reference.*`: the same SVG with the class-node viewports removed
//!   entirely (no clipping possible), the pixel oracle.
//!
//! `class-row-widths.txt` diagnoses every class-node fallback row: the nested
//! viewport width merman measured, the real glyph ink width rendered by this
//! crate's font stack, and whether the tight viewport clipped it.
//!
//! ```text
//! cargo run -p tty59-mermaid --example diagram_gallery -- <directory>
//! ```

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use resvg::tiny_skia;
use tty59_mermaid::{MermaidTheme, Raster, Rgba, rasterize, render_svg};

const SCALE: f32 = 2.0;

const CLASS: &str = "\
classDiagram
    class Shape {
        +String name
        +area() float
    }
    class Circle {
        +float radius
        +area() float
    }
    class Square {
        +float side
        +area() float
    }
    Shape <|-- Circle
    Shape <|-- Square
";

const DIAGRAMS: [(&str, &str); 9] = [
    (
        "flowchart",
        "\
flowchart TD
    A[Source] --> B{Valid?}
    B -->|yes| C[Render diagram]
    B -->|no| D[Show syntax error]
    C --> E[Preview]
",
    ),
    (
        "sequence",
        "\
sequenceDiagram
    participant C as Client
    participant S as Server
    C->>S: request diagram
    S-->>C: response bytes
    Note over C,S: shared note
",
    ),
    ("class", CLASS),
    (
        "state",
        "\
stateDiagram-v2
    [*] --> Idle
    Idle --> Running: start
    Running --> Idle: stop
    Running --> [*]
",
    ),
    (
        "er",
        "\
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE_ITEM : contains
",
    ),
    (
        "gantt",
        "\
gantt
    title Release plan
    dateFormat YYYY-MM-DD
    axisFormat %m-%d
    section Build
    Compile :a1, 2024-01-01, 3d
    Test :a2, after a1, 2d
    section Ship
    Package :b1, after a2, 1d
",
    ),
    (
        "pie",
        "\
pie title Work split
    \"Build\" : 50
    \"Test\" : 30
    \"Docs\" : 20
",
    ),
    (
        "mindmap",
        "\
mindmap
  root((plan))
    build
      compile
      package
    test
      unit
      render
",
    ),
    (
        "gitgraph",
        "\
gitGraph
    commit id: \"init\"
    branch feature
    checkout feature
    commit id: \"work\"
    checkout main
    merge feature
",
    ),
];

const FALLBACK_VIEWPORT_PREFIX: &str =
    r#"<svg xmlns="http://www.w3.org/2000/svg" x="0" y="0" width=""#;

const CLASS_NODE_LABEL_CLASSES: [&str; 4] = [
    "label-group",
    "members-group",
    "methods-group",
    "annotation-group",
];

#[derive(Debug)]
struct Viewport {
    open_start: usize,
    open_end: usize,
    close_start: usize,
    close_end: usize,
    classes: String,
    overflow: String,
    width: String,
}

fn main() -> ExitCode {
    let Some(directory) = std::env::args().nth(1) else {
        eprintln!("usage: diagram_gallery <directory>");
        return ExitCode::from(2);
    };
    match dump(Path::new(&directory)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("diagram_gallery: {message}");
            ExitCode::FAILURE
        }
    }
}

fn dump(directory: &Path) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let theme = MermaidTheme::dark();

    for (name, source) in DIAGRAMS {
        let svg = render_svg(source, &theme).map_err(|error| format!("{name}: {error}"))?;
        write_svg(directory, &format!("{name}-dark.svg"), &svg)?;
        write_png(directory, &format!("{name}-dark.png"), &svg, &theme)?;
    }

    let after = render_svg(CLASS, &theme).map_err(|error| format!("class: {error}"))?;
    let before = reclip_node_viewports(&after);
    let reference = strip_node_viewports(&after);
    write_svg(directory, "class-before.svg", &before)?;
    write_svg(directory, "class-after.svg", &after)?;
    write_svg(directory, "class-reference.svg", &reference)?;
    write_png(directory, "class-before.png", &before, &theme)?;
    write_png(directory, "class-after.png", &after, &theme)?;
    write_png(directory, "class-reference.png", &reference, &theme)?;

    let diagnostics = row_diagnostics(&after)?;
    let path = directory.join("class-row-widths.txt");
    std::fs::write(&path, diagnostics)
        .map_err(|error| format!("write {}: {error}", path.display()))?;

    let before_pixels =
        rasterize(&before, SCALE).map_err(|error| format!("class-before: {error}"))?;
    let after_pixels = rasterize(&after, SCALE).map_err(|error| format!("class-after: {error}"))?;
    let reference_pixels =
        rasterize(&reference, SCALE).map_err(|error| format!("class-reference: {error}"))?;
    println!(
        "class: after==reference pixels: {}, before==after pixels: {}",
        after_pixels.bgra == reference_pixels.bgra,
        before_pixels.bgra == after_pixels.bgra
    );
    println!(
        "wrote gallery to {} (9 families dark @ {SCALE}x, class before/after/reference)",
        directory.display()
    );
    Ok(())
}

fn write_svg(directory: &Path, name: &str, svg: &str) -> Result<(), String> {
    let path = directory.join(name);
    std::fs::write(&path, svg).map_err(|error| format!("write {}: {error}", path.display()))
}

fn write_png(directory: &Path, name: &str, svg: &str, theme: &MermaidTheme) -> Result<(), String> {
    let raster = rasterize(svg, SCALE).map_err(|error| format!("{name}: {error}"))?;
    let pixmap = opaque_pixmap(&raster, theme.background)?;
    let path = directory.join(name);
    pixmap
        .save_png(&path)
        .map_err(|error| format!("write {}: {error}", path.display()))
}

/// Straight-alpha BGRA from the crate → premultiplied RGBA for tiny-skia,
/// composited over the theme background so the review PNG matches the preview.
fn opaque_pixmap(raster: &Raster, background: Rgba) -> Result<tiny_skia::Pixmap, String> {
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

fn find_viewports(svg: &str) -> Vec<Viewport> {
    let mut viewports = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = svg[cursor..].find(FALLBACK_VIEWPORT_PREFIX) {
        let open_start = cursor + relative;
        let open_end = open_start + svg[open_start..].find('>').expect("viewport end") + 1;
        let close_relative = svg[open_end..]
            .find("</svg>")
            .expect("generated viewport close");
        let close_start = open_end + close_relative;
        let close_end = close_start + "</svg>".len();
        let body = &svg[open_end..close_start];
        let classes = body
            .match_indices("data-merman-source-classes=\"")
            .map(|(start, _)| {
                let value_start = start + "data-merman-source-classes=\"".len();
                let value_len = body[value_start..].find('"').expect("source classes value");
                &body[value_start..value_start + value_len]
            })
            .collect::<Vec<_>>()
            .join(" ");
        let tag = &svg[open_start..open_end];
        viewports.push(Viewport {
            open_start,
            open_end,
            close_start,
            close_end,
            classes,
            overflow: style_attribute(tag)
                .split(';')
                .find_map(|declaration| declaration.trim().strip_prefix("overflow:"))
                .unwrap_or_default()
                .trim()
                .to_owned(),
            width: attribute(tag, "width").unwrap_or("?").to_owned(),
        });
        cursor = close_end;
    }
    viewports
}

fn style_attribute(tag: &str) -> &str {
    let Some(start) = tag.find(r#"style=""#) else {
        return "";
    };
    let start = start + r#"style=""#.len();
    let end = start + tag[start..].find('"').unwrap_or(0);
    &tag[start..end]
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = start + tag[start..].find('"')?;
    Some(&tag[start..end])
}

fn is_class_node_label(classes: &str) -> bool {
    classes
        .split_whitespace()
        .any(|class| CLASS_NODE_LABEL_CLASSES.contains(&class))
}

fn node_viewports(svg: &str) -> Vec<Viewport> {
    find_viewports(svg)
        .into_iter()
        .filter(|viewport| is_class_node_label(&viewport.classes))
        .collect()
}

/// Reconstructs the pre-fix production SVG from the current one: only the
/// generated class-node viewports' `overflow: visible` goes back to merman's
/// default `hidden`; every other byte stays identical.
fn reclip_node_viewports(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut cursor = 0;
    for viewport in node_viewports(svg) {
        out.push_str(&svg[cursor..viewport.open_start]);
        let tag = &svg[viewport.open_start..viewport.open_end];
        out.push_str(
            &tag.replace("overflow:visible", "overflow:hidden")
                .replace("overflow: visible", "overflow: hidden"),
        );
        cursor = viewport.open_end;
    }
    out.push_str(&svg[cursor..]);
    out
}

/// Removes the class-node viewports entirely (`<svg …>…</svg>` → `<g …>…</g>`)
/// so no viewport clipping is possible; the font size, coordinates and every
/// other element stay byte-identical.
fn strip_node_viewports(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut cursor = 0;
    for viewport in node_viewports(svg) {
        out.push_str(&svg[cursor..viewport.open_start]);
        let tag = &svg[viewport.open_start..viewport.open_end];
        out.push_str("<g");
        out.push_str(&tag["<svg".len()..]);
        out.push_str(&svg[viewport.open_end..viewport.close_start]);
        out.push_str("</g>");
        cursor = viewport.close_end;
    }
    out.push_str(&svg[cursor..]);
    out
}

/// Per-row diagnosis: merman's measured viewport width vs the real glyph ink
/// width from this crate's font stack, measured by rendering the fallback's
/// own `<text>` element on a roomy canvas at the same 2× scale.
fn row_diagnostics(svg: &str) -> Result<String, String> {
    let mut out = String::new();
    writeln!(out, "theme=dark scale={SCALE}").expect("write diagnostics");
    writeln!(
        out,
        "classes | viewport_width | ink_width | overflow | clipped | text"
    )
    .expect("write diagnostics");
    for viewport in node_viewports(svg) {
        let body = &svg[viewport.open_end..viewport.close_start];
        let mut texts = Vec::new();
        let mut cursor = 0;
        while let Some(relative) = body[cursor..].find("<text ") {
            let start = cursor + relative;
            let Some(open_len) = body[start..].find('>') else {
                break;
            };
            let open_end = start + open_len + 1;
            let Some(close_len) = body[open_end..].find("</text>") else {
                break;
            };
            let close_start = open_end + close_len;
            let tag = &body[start..open_end];
            let text = &body[open_end..close_start];
            texts.push((tag.to_owned(), text.to_owned()));
            cursor = close_start + "</text>".len();
        }
        for (tag, text) in texts {
            let ink_width = measure_ink_width(&tag, &text)?;
            let viewport_width: f64 = viewport.width.parse().unwrap_or(0.0);
            writeln!(
                out,
                "{} | {} | {:.2} | {} | {} | {}",
                viewport.classes,
                viewport.width,
                ink_width,
                viewport.overflow,
                if ink_width > viewport_width + 0.01 {
                    "yes"
                } else {
                    "no"
                },
                text
            )
            .expect("write diagnostics");
        }
    }
    Ok(out)
}

fn measure_ink_width(tag: &str, text: &str) -> Result<f64, String> {
    let style = style_attribute(tag);
    let measurement = format!(
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="2048" height="256">"#,
            r##"<rect width="2048" height="256" fill="#000000"/>"##,
            r#"<text x="1024" y="128" text-anchor="middle" dominant-baseline="central" style="{}">{}</text>"#,
            r#"</svg>"#
        ),
        style, text
    );
    let raster = rasterize(&measurement, SCALE).map_err(|error| format!("measure: {error}"))?;
    let mut min_x = u32::MAX;
    let mut max_x = 0u32;
    let mut found = false;
    for (index, pixel) in raster.bgra.as_chunks::<4>().0.iter().enumerate() {
        let (r, g, b) = (pixel[2], pixel[1], pixel[0]);
        if u16::from(r) + u16::from(g) + u16::from(b) > 30 {
            let x = index as u32 % raster.width;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            found = true;
        }
    }
    if !found {
        return Ok(0.0);
    }
    Ok(f64::from(max_x - min_x + 1) / f64::from(SCALE))
}
