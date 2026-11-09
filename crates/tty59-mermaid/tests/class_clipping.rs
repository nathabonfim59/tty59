//! Regression for the class-diagram label clipping seen in the native preview.
//!
//! merman measures class title/member rows with its deterministic text profile
//! and then the resvg-safe fallback paints each row inside a nested `<svg>`
//! viewport of exactly that measured width, `overflow: hidden`. Real system
//! fonts are slightly wider, so the first/last glyphs are cut off even though
//! the enclosing class box has room. The production fix unclips only the
//! generated class-node label viewports.
//!
//! These tests prove the fix at the pixel level against a real public render:
//! the production SVG must paint exactly like the same SVG with the node
//! viewports removed, while the pre-fix baseline (production re-clipped to
//! `overflow: hidden`) must be missing text ink. A node-bound containment test
//! keeps the unclipping from spilling outside the class boxes.

use tty59_mermaid::{MermaidTheme, Raster, rasterize, render_svg};

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
            classes,
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

/// Reconstructs the pre-fix production SVG: the same bytes with the generated
/// class-node viewports' `overflow: visible` restored to merman's default
/// `hidden`. The production hook only changes those declarations.
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

/// Independent pixel oracle: removes the class-node viewports entirely
/// (`<svg …>…</svg>` → `<g …>…</g>`), so there is no viewport clipping at all.
/// Font size, text coordinates and every other element stay byte-identical.
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

/// Text ink in the dark palette: `text` is #e6edf3 (sum 710), while edges and
/// borders are much dimmer (#8b949e, #30363d). Antialiased glyph fringes fall
/// below the threshold, which is what we want: the assertion is about glyph
/// bodies that the viewport clip removed.
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

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = start + tag[start..].find('"')?;
    Some(&tag[start..end])
}

fn translate(tag: &str) -> Option<(f64, f64)> {
    let inner = attribute(tag, "transform")?
        .strip_prefix("translate(")?
        .strip_suffix(')')?;
    let (x, y) = inner.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// Class node boxes in diagram coordinates, parsed from the node group's
/// translate and its first `basic label-container` path. The path is the class
/// box outline (`M left top L right top L right bottom L left bottom`), so it
/// gives the enclosing box the rows must stay inside.
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

#[test]
fn class_node_rows_are_unclipped_and_the_baseline_is_missing_ink() {
    let svg = render_svg(CLASS, &MermaidTheme::dark()).expect("class svg");
    let viewports = find_viewports(&svg);
    let node_viewports: Vec<_> = viewports
        .iter()
        .filter(|viewport| is_class_node_label(&viewport.classes))
        .collect();
    assert!(
        !node_viewports.is_empty(),
        "the public class fixture produced no class-node fallback viewports: {viewports:?}"
    );

    let baseline_svg = reclip_node_viewports(&svg);
    let oracle_svg = strip_node_viewports(&svg);
    let fixed = rasterize(&svg, 2.0).expect("fixed raster");
    let clipped = rasterize(&baseline_svg, 2.0).expect("baseline raster");
    let reference = rasterize(&oracle_svg, 2.0).expect("viewport-free raster");

    // The production output must paint exactly like the viewport-free oracle:
    // that is the real proof that these nested viewports were the clipper and
    // that overflow:visible removes only the clip.
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
        "overflow:visible output differs from a viewport-free reference: {} differing pixels ({}x{})",
        differing_pixels,
        fixed.width,
        fixed.height
    );

    // The pre-fix baseline (production, re-clipped to merman's default) must
    // really be missing text ink. With the production hook removed, `fixed`
    // equals `clipped` and this count is zero, so the regression fails.
    let missing = missing_text_pixels(&clipped, &fixed);
    assert!(
        missing > 100,
        "baseline is missing only {missing} text pixels; the production unclip hook may be disabled"
    );
    assert!(
        baseline_svg != svg,
        "fixture never reached the production unclip hook (SVG unchanged)"
    );
    assert!(
        node_viewports
            .iter()
            .all(|viewport| viewport.overflow == "visible"),
        "class node viewport still clipped after the pixel checks: {node_viewports:?}"
    );
}

#[test]
fn class_v2_header_reaches_the_same_unclip_path() {
    let source = CLASS.replacen("classDiagram", "classDiagram-v2", 1);
    let svg = render_svg(&source, &MermaidTheme::dark()).expect("classDiagram-v2 svg");
    let node_viewports: Vec<_> = find_viewports(&svg)
        .into_iter()
        .filter(|viewport| is_class_node_label(&viewport.classes))
        .collect();
    assert!(!node_viewports.is_empty(), "classDiagram-v2 rows not found");
    assert!(
        node_viewports
            .iter()
            .all(|viewport| viewport.overflow == "visible"),
        "classDiagram-v2 node rows still clipped: {node_viewports:?}"
    );
}

#[test]
fn unclipped_class_text_stays_inside_the_class_node_boxes() {
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
