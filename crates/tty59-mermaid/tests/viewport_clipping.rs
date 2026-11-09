//! Regression for generated resvg-safe label viewports clipping real text.
//!
//! merman measures every HTML label with its deterministic text profile and
//! then the resvg-safe fallback paints it inside a nested `<svg>` viewport of
//! exactly that measured width, `overflow: hidden`. Real system fonts are
//! slightly wider, so the first/last glyphs get cut off even when the
//! enclosing shape or edge-label box has room. Production now unclips every
//! generated viewport.
//!
//! The pixel proof renders a real public fixture, re-clips the production
//! viewports to reconstruct the pre-fix baseline, and removes the viewports
//! entirely as an independent no-clip oracle. The same file also pins the
//! mindmap neutral paint: actual emitted body fills, actual fallback label
//! fills, contrast, and exact palette pixels in the public raster.

use std::collections::BTreeSet;
use std::sync::Arc;

use resvg::usvg;
use tty59_mermaid::{MermaidTheme, Raster, Rgba, rasterize, render_svg};

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

const ER: &str = "\
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE_ITEM : contains
";

const MINDMAP: &str = "\
mindmap
  root((plan))
    render
      compile
        package
    review
    release
";

const FALLBACK_VIEWPORT_PREFIX: &str =
    r#"<svg xmlns="http://www.w3.org/2000/svg" x="0" y="0" width=""#;

#[derive(Debug)]
struct Viewport {
    open_start: usize,
    open_end: usize,
    close_start: usize,
    close_end: usize,
    overflow: String,
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
        let overflow = style_attribute(&svg[open_start..open_end])
            .split(';')
            .find_map(|declaration| declaration.trim().strip_prefix("overflow:"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        viewports.push(Viewport {
            open_start,
            open_end,
            close_start,
            close_end,
            overflow,
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

/// Reconstructs the pre-fix production SVG: only generated viewport
/// `overflow: visible` declarations become merman's default `hidden`.
fn reclip_viewports(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut cursor = 0;
    for viewport in find_viewports(svg) {
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

/// Independent pixel oracle: removes every generated viewport
/// (`<svg …>…</svg>` → `<g …>…</g>`), so no viewport clipping is possible.
/// Font size, coordinates and every other element stay byte-identical.
fn strip_viewports(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut cursor = 0;
    for viewport in find_viewports(svg) {
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

/// Text ink in the dark palette: `text`/`primary_text` is #e6edf3 (sum 710),
/// while body fills and edges are much dimmer. Antialiased fringes fall below
/// the threshold, which is what we want: the assertion is about glyph bodies
/// that the viewport clip removed.
fn is_bright_text_pixel(pixel: &[u8; 4]) -> bool {
    let [b, g, r, a] = *pixel;
    a > 200 && u16::from(r) + u16::from(g) + u16::from(b) > 600
}

fn bright_text_pixels(raster: &Raster) -> Vec<(u32, u32)> {
    raster
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, pixel)| is_bright_text_pixel(pixel))
        .map(|(index, _)| {
            let index = index as u32;
            (index % raster.width, index / raster.width)
        })
        .collect()
}

fn missing_text_pixels(clipped: &Raster, unclipped: &Raster) -> usize {
    assert_eq!(
        (clipped.width, clipped.height),
        (unclipped.width, unclipped.height)
    );
    clipped
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .zip(unclipped.bgra.as_chunks::<4>().0)
        .filter(|(clipped, unclipped)| {
            is_bright_text_pixel(unclipped) && !is_bright_text_pixel(clipped)
        })
        .count()
}

/// A real public fixture must render the same pixels as the viewport-free
/// oracle, while the reconstructed pre-fix baseline must be missing text ink.
fn assert_clipping_fixed(name: &str, source: &str, min_missing: usize) {
    let theme = MermaidTheme::dark();
    let svg = render_svg(source, &theme).unwrap_or_else(|error| panic!("{name}: {error}"));
    let viewports = find_viewports(&svg);
    assert!(
        !viewports.is_empty(),
        "{name}: fixture produced no generated fallback viewports"
    );

    let baseline_svg = reclip_viewports(&svg);
    let oracle_svg = strip_viewports(&svg);
    let fixed = rasterize(&svg, 2.0).unwrap_or_else(|error| panic!("{name} fixed: {error}"));
    let clipped =
        rasterize(&baseline_svg, 2.0).unwrap_or_else(|error| panic!("{name} baseline: {error}"));
    let reference =
        rasterize(&oracle_svg, 2.0).unwrap_or_else(|error| panic!("{name} oracle: {error}"));

    assert_eq!(
        (fixed.width, fixed.height),
        (reference.width, reference.height)
    );
    let differing_pixels = fixed
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .zip(reference.bgra.as_chunks::<4>().0)
        .filter(|(fixed, reference)| fixed != reference)
        .count();
    assert!(
        fixed.bgra == reference.bgra,
        "{name}: overflow:visible output differs from the viewport-free reference by {differing_pixels} pixels ({}x{})",
        fixed.width,
        fixed.height
    );

    let missing = missing_text_pixels(&clipped, &fixed);
    assert!(
        missing > min_missing,
        "{name}: pre-fix baseline is missing only {missing} text pixels; the unclip hook may be disabled"
    );
    assert!(
        baseline_svg != svg,
        "{name}: fixture never reached the production unclip hook (SVG unchanged)"
    );
    assert!(
        viewports
            .iter()
            .all(|viewport| viewport.overflow == "visible"),
        "{name}: generated viewport still clipped after the pixel checks: {viewports:?}"
    );
}

#[test]
fn class_rows_are_unclipped() {
    assert_clipping_fixed("class", CLASS, 100);
}

#[test]
fn er_edge_labels_are_unclipped() {
    assert_clipping_fixed("er", ER, 0);
}

#[test]
fn mindmap_labels_are_unclipped() {
    assert_clipping_fixed("mindmap", MINDMAP, 0);
}

fn rgb_of(color: Rgba) -> [u8; 3] {
    [color.r, color.g, color.b]
}

fn relative_luminance(rgb: [u8; 3]) -> f64 {
    let [r, g, b] = rgb;
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast_ratio(first: [u8; 3], second: [u8; 3]) -> f64 {
    let first = relative_luminance(first);
    let second = relative_luminance(second);
    let (lighter, darker) = if first >= second {
        (first, second)
    } else {
        (second, first)
    };
    (lighter + 0.05) / (darker + 0.05)
}

fn assert_not_black(fill: [u8; 3]) {
    let [r, g, b] = fill;
    assert!(
        r.max(g).max(b) >= 16,
        "mindmap body fill {fill:?} is effectively black"
    );
}

#[derive(Debug, Default)]
struct NodePaint {
    body_fills: Vec<[u8; 3]>,
    text_fills: Vec<[u8; 3]>,
    texts: Vec<String>,
}

fn parse_tree(svg: &str) -> usvg::Tree {
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_system_fonts();
    let options = usvg::Options {
        fontdb: Arc::new(fontdb),
        ..usvg::Options::default()
    };
    usvg::Tree::from_str(svg, &options).expect("parse mindmap svg")
}

fn node_paints(group: &usvg::Group, out: &mut Vec<NodePaint>) {
    for child in group.children() {
        let usvg::Node::Group(child) = child else {
            continue;
        };
        if child.id().contains("-node_") {
            let mut paint = NodePaint::default();
            collect_node_paint(child, &mut paint);
            out.push(paint);
        } else {
            node_paints(child, out);
        }
    }
}

/// Body paint comes from direct shape children of the node group; label text
/// paint comes from every descendant text span. This reads the effective
/// normalized tree, after the stylesheet cascade and neo/root overrides.
fn collect_node_paint(group: &usvg::Group, paint: &mut NodePaint) {
    for child in group.children() {
        match child {
            usvg::Node::Path(path) => {
                let bounds = path.bounding_box();
                if bounds.width() > 0.0
                    && bounds.height() > 0.0
                    && let Some(fill) = path.fill()
                    && fill.opacity().get() > 0.5
                    && let usvg::Paint::Color(color) = fill.paint()
                {
                    paint.body_fills.push([color.red, color.green, color.blue]);
                }
            }
            usvg::Node::Group(child) => collect_text_paint(child, paint),
            usvg::Node::Text(text) => collect_text(text, paint),
            _ => {}
        }
    }
}

fn collect_text_paint(group: &usvg::Group, paint: &mut NodePaint) {
    for child in group.children() {
        match child {
            usvg::Node::Group(child) => collect_text_paint(child, paint),
            usvg::Node::Text(text) => collect_text(text, paint),
            _ => {}
        }
    }
}

fn collect_text(text: &usvg::Text, paint: &mut NodePaint) {
    for chunk in text.chunks() {
        paint.texts.push(chunk.text().to_owned());
        for span in chunk.spans() {
            if let Some(fill) = span.fill()
                && let usvg::Paint::Color(color) = fill.paint()
            {
                paint.text_fills.push([color.red, color.green, color.blue]);
            }
        }
    }
}

fn count_exact_color(raster: &Raster, color: Rgba) -> usize {
    let target = [color.b, color.g, color.r, 255];
    raster
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| **pixel == target)
        .count()
}

#[test]
fn zero_area_node_line_paths_are_not_recorded_as_body_fill() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><g id="x-node_0"><path d="M0 10 L20 10" fill="#000000"/><path d="M2 2 L18 2 L18 18 L2 18 Z" fill="#112233"/></g></svg>"##;
    let mut nodes = Vec::new();
    node_paints(parse_tree(svg).root(), &mut nodes);
    assert_eq!(nodes.len(), 1, "node group not found: {nodes:?}");
    assert_eq!(
        nodes[0].body_fills,
        vec![[0x11, 0x22, 0x33]],
        "zero-height node line was recorded as body fill"
    );
}

#[test]
fn mindmap_actual_paint_is_neutral_readable_and_exact() {
    for theme in [MermaidTheme::dark(), MermaidTheme::light()] {
        let svg = render_svg(MINDMAP, &theme)
            .unwrap_or_else(|error| panic!("mindmap dark={}: {error}", theme.dark_mode));
        let raster = rasterize(&svg, 2.0).expect("mindmap raster");
        let label = rgb_of(theme.primary_text);
        let palette = [
            rgb_of(theme.primary),
            rgb_of(theme.secondary),
            rgb_of(theme.tertiary),
        ];
        let expected_palette: BTreeSet<[u8; 3]> = palette.into_iter().collect();
        let line_color = theme.line.to_css();

        let mut nodes = Vec::new();
        node_paints(parse_tree(&svg).root(), &mut nodes);
        assert!(
            nodes.len() >= 6,
            "dark={}: expected at least 6 mindmap node groups, got {}",
            theme.dark_mode,
            nodes.len()
        );

        let mut seen_texts = BTreeSet::new();
        let mut seen_fills = BTreeSet::new();
        let mut root_fill = None;
        for node in &nodes {
            let body = node.body_fills.first().copied().unwrap_or_else(|| {
                panic!(
                    "dark={}: node with no effective body fill: {:?}",
                    theme.dark_mode, node
                )
            });
            for fill in &node.body_fills {
                assert_not_black(*fill);
                assert!(
                    expected_palette.contains(fill),
                    "dark={}: node body fill {fill:?} is not a contract neutral color",
                    theme.dark_mode
                );
            }
            assert!(
                !node.text_fills.is_empty(),
                "dark={}: node with no effective label paint: {:?}",
                theme.dark_mode,
                node
            );
            for fill in &node.text_fills {
                assert_eq!(
                    *fill, label,
                    "dark={}: node label paint {fill:?} is not primary_text",
                    theme.dark_mode
                );
            }
            assert!(
                contrast_ratio(body, label) >= 4.5,
                "dark={}: label {label:?} on body {body:?} has insufficient contrast",
                theme.dark_mode
            );
            seen_fills.insert(body);
            seen_texts.extend(node.texts.iter().cloned());
            if node.texts.iter().any(|text| text == "plan") {
                root_fill = Some(body);
            }
        }

        assert_eq!(
            root_fill,
            Some(rgb_of(theme.primary)),
            "dark={}: root body is not the contract primary",
            theme.dark_mode
        );
        assert_eq!(
            seen_fills, expected_palette,
            "dark={}: effective node bodies do not use the full neutral palette",
            theme.dark_mode
        );
        for expected in ["plan", "render", "compile", "package", "review", "release"] {
            assert!(
                seen_texts.contains(expected),
                "dark={}: expected mindmap label {expected:?} missing from effective paint",
                theme.dark_mode
            );
        }

        for fill in palette {
            let count = count_exact_color(&raster, Rgba::rgb(fill[0], fill[1], fill[2]));
            assert!(
                count > 0,
                "dark={}: neutral fill {fill:?} missing from the public raster",
                theme.dark_mode
            );
        }
        let line_raster = count_exact_color(&raster, theme.line);
        assert!(
            line_raster > 0,
            "dark={}: exact line-colored pixels missing from the public raster",
            theme.dark_mode
        );

        let mut edges = 0usize;
        let mut rest = svg.as_str();
        while let Some(start) = rest.find("<path") {
            let tail = &rest[start..];
            let tag_len = tail.find('>').expect("path end") + 1;
            let tag = &tail[..tag_len];
            let is_edge = attribute(tag, "class").is_some_and(|classes| {
                classes
                    .split_whitespace()
                    .any(|class| class.starts_with("section-edge-"))
            });
            if is_edge {
                edges += 1;
                let style = attribute(tag, "style").expect("edge style");
                assert!(
                    style.contains(&format!("stroke:{line_color};")),
                    "dark={}: mindmap edge not line-colored: {tag}",
                    theme.dark_mode
                );
            }
            rest = &tail[tag_len..];
        }
        assert!(
            edges > 0,
            "dark={}: mindmap fixture produced no section edges",
            theme.dark_mode
        );
    }
}

#[test]
fn unclipped_ink_stays_inside_the_class_boxes() {
    #[derive(Clone, Copy, Debug)]
    struct Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    fn translate(tag: &str) -> Option<(f64, f64)> {
        let inner = attribute(tag, "transform")?
            .strip_prefix("translate(")?
            .strip_suffix(')')?;
        let (x, y) = inner.split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    }

    fn class_node_boxes(svg: &str) -> Vec<Rect> {
        let mut boxes = Vec::new();
        let mut cursor = 0;
        while let Some(relative) = svg[cursor..].find(r#"<g class="node "#) {
            let start = cursor + relative;
            let Some(tag_len) = svg[start..].find('>') else {
                break;
            };
            let tag_end = start + tag_len + 1;
            let tag = &svg[start..tag_end];
            let (tx, ty) = translate(tag).expect("class node translate");
            let Some(container_relative) =
                svg[tag_end..].find(r#"<g class="basic label-container outer-path">"#)
            else {
                break;
            };
            let container = tag_end + container_relative;
            let container_end = container + svg[container..].find('>').expect("container end") + 1;
            let Some(path_relative) = svg[container_end..].find("<path d=\"") else {
                break;
            };
            let path_start = container_end + path_relative + "<path d=\"".len();
            let path_end = path_start + svg[path_start..].find('"').expect("path value");
            let path = &svg[path_start..path_end];
            let numbers: Vec<f64> = path
                .split(|character: char| {
                    character == 'M' || character == 'L' || character.is_whitespace()
                })
                .filter(|part| !part.is_empty())
                .map(|part| part.parse().expect("class box coordinate"))
                .collect();
            assert_eq!(numbers.len(), 8, "unexpected class box path: {path}");
            boxes.push(Rect {
                x: tx + numbers[0],
                y: ty + numbers[1],
                width: numbers[2] - numbers[0],
                height: numbers[5] - numbers[1],
            });
            cursor = path_end;
        }
        boxes
    }

    fn view_box(svg: &str) -> (f64, f64, f64, f64) {
        let start = svg.find("<svg").expect("root svg");
        let end = start + svg[start..].find('>').expect("root end");
        let value = attribute(&svg[start..end], "viewBox").expect("root viewBox");
        let numbers: Vec<f64> = value
            .split(|character: char| character.is_whitespace() || character == ',')
            .filter(|part| !part.is_empty())
            .map(|part| part.parse().expect("viewBox number"))
            .collect();
        assert_eq!(numbers.len(), 4, "unexpected viewBox: {value}");
        (numbers[0], numbers[1], numbers[2], numbers[3])
    }

    let svg = render_svg(CLASS, &MermaidTheme::dark()).expect("class svg");
    let raster = rasterize(&svg, 2.0).expect("class raster");
    let boxes = class_node_boxes(&svg);
    assert_eq!(boxes.len(), 3, "parsed class boxes: {boxes:?}");
    let ink = bright_text_pixels(&raster);
    assert!(
        !ink.is_empty(),
        "dark class render produced no bright text ink to check"
    );

    let (view_x, view_y, view_width, view_height) = view_box(&svg);
    let scale_x = f64::from(raster.width) / view_width;
    let scale_y = f64::from(raster.height) / view_height;
    let tolerance = 2.0;
    let mut outside = Vec::new();
    for (pixel_x, pixel_y) in ink {
        let x = view_x + (f64::from(pixel_x) + 0.5) / scale_x;
        let y = view_y + (f64::from(pixel_y) + 0.5) / scale_y;
        let inside = boxes.iter().any(|class_box| {
            x >= class_box.x - tolerance
                && x <= class_box.x + class_box.width + tolerance
                && y >= class_box.y - tolerance
                && y <= class_box.y + class_box.height + tolerance
        });
        if !inside {
            outside.push((x, y));
            if outside.len() == 8 {
                break;
            }
        }
    }
    assert!(
        outside.is_empty(),
        "unclipped text ink escaped the class boxes at {outside:?}"
    );
}
