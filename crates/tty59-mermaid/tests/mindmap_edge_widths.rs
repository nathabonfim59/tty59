//! Deep mindmap branch edges must stay visible.
//!
//! merman's generated stylesheet gives `.edge-depth-{n}` the width
//! `14 - 3n`, so from depth 5 the width is non-positive; usvg then drops the
//! stroke entirely and a depth-6 mindmap loses its deepest edges. Production
//! floors only those generated widths at 1.
//!
//! This regression checks, for neutral light/dark and non-empty accents
//! light/dark:
//! * the effective normalized-tree stroke width of every generated section
//!   edge is positive (and positive widths are byte-preserved),
//! * every edge paints line-colored ink at its own most-interior on-curve
//!   point, so node fills and label text cannot satisfy the assertion. The
//!   pixels come from the public production SVG; only the sampling geometry
//!   comes from an independently floored reference, so disabling the hook
//!   fails on missing ink instead of on a helper precondition,
//! * the reconstructed pre-fix baseline (negative widths restored) paints no
//!   ink for the edges that were non-positive only, i.e. the regression
//!   genuinely fails without the floor.

use std::collections::BTreeMap;
use std::sync::Arc;

use resvg::usvg;
use tty59_mermaid::{Accent, MermaidTheme, Raster, Rgba, rasterize, render_svg};

/// Explicit root plus six descendant levels. merman counts indentation
/// columns, so one space per level yields edge depths 1 through 6. Depths 5
/// and 6 are the generated non-positive widths (-1 and -4).
const DEPTH6: &str = "\
mindmap
 root((plan))
  render
   compile
    package
     ship
      verify
       done
";

const EDGE_IDS: [&str; 6] = [
    "edge_0_1", "edge_1_2", "edge_2_3", "edge_3_4", "edge_4_5", "edge_5_6",
];

fn palette() -> Vec<Accent> {
    vec![
        Accent {
            fill: Rgba::rgb(0x10, 0x20, 0x30),
            border: Rgba::rgb(0x20, 0x30, 0x40),
            text: Rgba::rgb(0x30, 0x40, 0x50),
        },
        Accent {
            fill: Rgba::rgb(0x40, 0x50, 0x60),
            border: Rgba::rgb(0x50, 0x60, 0x70),
            text: Rgba::rgb(0x60, 0x70, 0x80),
        },
        Accent {
            fill: Rgba::rgb(0x70, 0x80, 0x90),
            border: Rgba::rgb(0x80, 0x90, 0xa0),
            text: Rgba::rgb(0x90, 0xa0, 0xb0),
        },
    ]
}

fn themes() -> [(&'static str, MermaidTheme); 4] {
    let mut accent_light = MermaidTheme::light();
    accent_light.accents = palette();
    let mut accent_dark = MermaidTheme::dark();
    accent_dark.accents = palette();
    [
        ("neutral light", MermaidTheme::light()),
        ("neutral dark", MermaidTheme::dark()),
        ("accent light", accent_light),
        ("accent dark", accent_dark),
    ]
}

/// merman's classic mindmap width for section `n`, then production's floor.
fn expected_effective_width(depth: u32) -> f32 {
    match 14 - 3 * i64::from(depth) {
        positive if positive > 0 => positive as f32,
        _ => 1.0,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RawEdge {
    id: String,
    depth: u32,
    d: String,
    data_points: String,
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = start + tag[start..].find('"')?;
    Some(&tag[start..end])
}

fn root_tag(svg: &str) -> &str {
    let start = svg.find("<svg").expect("svg root");
    let len = svg[start..].find('>').expect("svg root tag end");
    &svg[start..start + len]
}

fn root_diagram_id(svg: &str) -> &str {
    attribute(root_tag(svg), "id").expect("svg root id")
}

fn raw_edges(svg: &str) -> Vec<RawEdge> {
    let prefix = format!("{}-", root_diagram_id(svg));
    let mut edges = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find("<path") {
        let tail = &rest[start..];
        let tag_len = tail.find('>').expect("path tag end") + 1;
        let tag = &tail[..tag_len];
        if let Some(classes) = attribute(tag, "class")
            && classes
                .split_whitespace()
                .any(|class| class.starts_with("section-edge-"))
        {
            let depth = classes
                .split_whitespace()
                .find_map(|class| class.strip_prefix("edge-depth-"))
                .expect("edge carries an edge-depth-* class")
                .parse()
                .expect("edge depth is numeric");
            let full_id = attribute(tag, "id").expect("edge id");
            let id = full_id
                .strip_prefix(&prefix)
                .unwrap_or_else(|| {
                    panic!("edge id {full_id:?} does not start with the diagram prefix {prefix:?}")
                })
                .to_owned();
            edges.push(RawEdge {
                id,
                depth,
                d: attribute(tag, "d").expect("edge path data").to_owned(),
                data_points: attribute(tag, "data-points")
                    .expect("edge data-points")
                    .to_owned(),
            });
        }
        rest = &tail[tag_len..];
    }
    edges
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

fn walk_paths<'a>(group: &'a usvg::Group, visit: &mut impl FnMut(&'a usvg::Path)) {
    for child in group.children() {
        match child {
            usvg::Node::Path(path) => visit(path),
            usvg::Node::Group(group) => walk_paths(group, visit),
            _ => {}
        }
    }
}

/// Effective stroke width per edge id after the stylesheet cascade. `None`
/// means usvg resolved no stroke at all (what a non-positive width does).
fn effective_edge_widths(tree: &usvg::Tree) -> BTreeMap<String, Option<f32>> {
    let mut widths = BTreeMap::new();
    walk_paths(tree.root(), &mut |path| {
        if path.id().contains("-edge_") {
            widths.insert(
                path.id().to_owned(),
                path.stroke().map(|stroke| stroke.width().get()),
            );
        }
    });
    widths
}

fn find_edge_path<'a>(tree: &'a usvg::Tree, edge_id: &str) -> Option<&'a usvg::Path> {
    let mut found = None;
    walk_paths(tree.root(), &mut |path| {
        if path.id().ends_with(&format!("-{edge_id}")) {
            found = Some(path);
        }
    });
    found
}

fn width_for(widths: &BTreeMap<String, Option<f32>>, edge_id: &str) -> Option<Option<f32>> {
    let suffix = format!("-{edge_id}");
    let mut matches = widths.iter().filter(|(id, _)| id.ends_with(&suffix));
    let first = matches.next();
    assert!(
        matches.next().is_none(),
        "edge {edge_id} resolved to more than one path"
    );
    first.map(|(_, width)| *width)
}

/// The on-curve point farthest from both path endpoints: the interior of the
/// gap between parent and child. Returns the point in canvas coordinates and
/// its distance to the nearest path endpoint (which sits on a node edge).
fn interior_sample(path: &usvg::Path) -> (usvg::tiny_skia_path::Point, f32) {
    use usvg::tiny_skia_path::{PathSegment, Point};

    let mut previous = None;
    let mut start = None;
    let mut end = None;
    let mut candidates = Vec::new();
    for segment in path.data().segments() {
        let candidate = match segment {
            PathSegment::MoveTo(point) => {
                start = Some(point);
                None
            }
            PathSegment::LineTo(point) => previous.map(|from: Point| {
                Point::from_xy((from.x + point.x) * 0.5, (from.y + point.y) * 0.5)
            }),
            PathSegment::QuadTo(control, point) => previous.map(|from: Point| {
                Point::from_xy(
                    (from.x + 2.0 * control.x + point.x) * 0.25,
                    (from.y + 2.0 * control.y + point.y) * 0.25,
                )
            }),
            PathSegment::CubicTo(first, second, point) => previous.map(|from: Point| {
                Point::from_xy(
                    (from.x + 3.0 * first.x + 3.0 * second.x + point.x) * 0.125,
                    (from.y + 3.0 * first.y + 3.0 * second.y + point.y) * 0.125,
                )
            }),
            PathSegment::Close => None,
        };
        if let Some(point) = candidate {
            candidates.push(point);
        }
        match segment {
            PathSegment::MoveTo(point) | PathSegment::LineTo(point) => {
                previous = Some(point);
                end = Some(point);
            }
            PathSegment::QuadTo(_, point) | PathSegment::CubicTo(_, _, point) => {
                previous = Some(point);
                end = Some(point);
            }
            PathSegment::Close => {}
        }
    }

    let start = start.expect("edge path has a start point");
    let end = end.expect("edge path has an end point");
    let transform = path.abs_transform();
    let map = |mut point: Point| {
        transform.map_point(&mut point);
        point
    };
    let start = map(start);
    let end = map(end);
    let distance = |first: Point, second: Point| {
        ((first.x - second.x).powi(2) + (first.y - second.y).powi(2)).sqrt()
    };

    candidates
        .into_iter()
        .map(map)
        .map(|point| (point, distance(point, start).min(distance(point, end))))
        .max_by(|(_, first), (_, second)| first.total_cmp(second))
        .expect("edge path has interior curve segments")
}

fn near(actual: u8, expected: u8) -> bool {
    (i16::from(actual) - i16::from(expected)).abs() <= 3
}

fn window(raster: &Raster, sample: usvg::tiny_skia_path::Point) -> Vec<[u8; 4]> {
    let center_x = (sample.x * raster.scale).round() as i64;
    let center_y = (sample.y * raster.scale).round() as i64;
    let radius = (2.0 * raster.scale).ceil() as i64 + 1;
    let mut pixels = Vec::new();
    for y in (center_y - radius)..=(center_y + radius) {
        for x in (center_x - radius)..=(center_x + radius) {
            if x < 0 || y < 0 || x >= i64::from(raster.width) || y >= i64::from(raster.height) {
                continue;
            }
            let index = ((y as u32 * raster.width) + x as u32) as usize * 4;
            pixels.push(raster.bgra[index..index + 4].try_into().unwrap());
        }
    }
    pixels
}

fn has_line_ink(raster: &Raster, sample: usvg::tiny_skia_path::Point, line: Rgba) -> bool {
    window(raster, sample).iter().any(|pixel| {
        let [blue, green, red, alpha] = *pixel;
        alpha > 0 && near(red, line.r) && near(green, line.g) && near(blue, line.b)
    })
}

fn has_any_ink(raster: &Raster, sample: usvg::tiny_skia_path::Point) -> bool {
    window(raster, sample).iter().any(|pixel| pixel[3] > 0)
}

/// Rewrites every generated `.edge-depth-{n}{stroke-width:<int>;}` rule.
/// `replace` sees the rule's depth and its current width; `None` keeps the
/// current value. Used to build an independent floored reference tree and the
/// pre-fix negative-width baseline, so both exist even when the production
/// hook is disabled.
fn rewrite_edge_depth_widths(svg: &str, replace: impl Fn(i64, i64) -> Option<i64>) -> String {
    const RULE: &str = ".edge-depth-";
    const DECLARATION: &str = "{stroke-width:";

    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find(RULE) {
        let (head, tail) = rest.split_at(start);
        out.push_str(head);
        out.push_str(RULE);
        let selector = &tail[RULE.len()..];
        let suffix_len = selector
            .find(|ch: char| !(ch.is_ascii_digit() || ch == '-'))
            .unwrap_or(selector.len());
        let suffix = &selector[..suffix_len];
        let depth: i64 = suffix
            .parse()
            .unwrap_or_else(|_| panic!("edge-depth selector suffix {suffix:?} is not numeric"));
        out.push_str(&selector[..suffix_len]);
        let after_selector = &selector[suffix_len..];
        let Some(declaration) = after_selector.strip_prefix(DECLARATION) else {
            rest = after_selector;
            continue;
        };
        let Some(value_len) = declaration.find([';', '}']) else {
            rest = after_selector;
            continue;
        };
        let current: i64 = declaration[..value_len]
            .parse()
            .unwrap_or_else(|_| panic!("edge-depth width is not an integer"));
        out.push_str(DECLARATION);
        match replace(depth, current) {
            Some(width) => out.push_str(&width.to_string()),
            None => out.push_str(&declaration[..value_len]),
        }
        rest = &declaration[value_len..];
    }
    out.push_str(rest);
    out
}

/// Independent reference: the same document with every generated non-positive
/// edge width floored, used only to get a parsed tree/path geometry that is
/// valid even when production still emits negative widths.
fn floor_generated_widths(svg: &str) -> String {
    rewrite_edge_depth_widths(svg, |_, width| (width <= 0).then_some(1))
}

/// Pre-fix document: merman's original classic width (`14 - 3n`) restored for
/// every generated rule whose width was non-positive, positives untouched.
fn negative_width_baseline(reference: &str) -> String {
    rewrite_edge_depth_widths(reference, |depth, _| {
        let original = 14 - 3 * depth;
        (original <= 0).then_some(original)
    })
}

#[test]
fn depth6_edges_have_positive_effective_widths_in_every_mode() {
    for (name, theme) in themes() {
        let svg = render_svg(DEPTH6, &theme).unwrap_or_else(|error| panic!("{name}: {error}"));
        let edges = raw_edges(&svg);
        assert_eq!(
            edges
                .iter()
                .map(|edge| edge.id.as_str())
                .collect::<Vec<_>>()
                .as_slice(),
            EDGE_IDS.as_slice(),
            "{name}: fixture did not produce exactly the expected six edges"
        );
        assert!(
            svg.contains(".edge-depth-4{stroke-width:2;}"),
            "{name}: an already-positive generated width changed"
        );

        let tree = parse_tree(&svg);
        let widths = effective_edge_widths(&tree);
        for edge in &edges {
            let width = width_for(&widths, &edge.id)
                .unwrap_or_else(|| panic!("{name}: {} missing from the effective tree", edge.id))
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: {} has no effective stroke (non-positive width survived)",
                        edge.id
                    )
                });
            assert!(
                width > 0.0,
                "{name}: {} effective stroke width is {width}",
                edge.id
            );
            assert_eq!(
                width,
                expected_effective_width(edge.depth),
                "{name}: {} (depth {}) has the wrong effective width",
                edge.id,
                edge.depth
            );
        }
    }
}

#[test]
fn each_depth6_edge_paints_interior_line_ink_in_every_mode() {
    for (name, theme) in themes() {
        // The public production raster is the only source of truth for ink.
        let svg = render_svg(DEPTH6, &theme).unwrap_or_else(|error| panic!("{name}: {error}"));
        // Geometry comes from an independently floored reference and the
        // pre-fix baseline from merman's negative widths, so neither the path
        // lookup nor the baseline exists only when the production hook ran.
        let reference = floor_generated_widths(&svg);
        let baseline = negative_width_baseline(&reference);
        let edges = raw_edges(&svg);
        assert_eq!(
            edges.len(),
            EDGE_IDS.len(),
            "{name}: expected every depth6 edge, found {}",
            edges.len()
        );
        let tree = parse_tree(&reference);
        let fixed = rasterize(&svg, 2.0).unwrap_or_else(|error| panic!("{name} fixed: {error}"));
        let before =
            rasterize(&baseline, 2.0).unwrap_or_else(|error| panic!("{name} baseline: {error}"));
        assert_eq!(
            (fixed.width, fixed.height),
            (before.width, before.height),
            "{name}: width-only correction changed the raster size"
        );

        // Prove every edge's own gap carries line ink first. With the hook
        // disabled this is the failure this test must report.
        let mut samples = Vec::with_capacity(edges.len());
        for edge in &edges {
            let path = find_edge_path(&tree, &edge.id).unwrap_or_else(|| {
                panic!("{name}: {} edge path missing from reference tree", edge.id)
            });
            let (sample, gap) = interior_sample(path);
            assert!(
                gap > 5.0,
                "{name}: {} interior sample is only {gap} from a node boundary",
                edge.id
            );
            assert!(
                has_line_ink(&fixed, sample, theme.line),
                "{name}: {} paints no line-colored ink in its interior gap",
                edge.id
            );
            samples.push((edge, sample));
        }

        // Only after the ink proof: the hook actually rewrote the stylesheet.
        assert!(
            svg.contains(".edge-depth-5{stroke-width:1;}"),
            "{name}: production did not floor the generated edge width"
        );

        for (edge, sample) in samples {
            let original = 14 - 3 * i64::from(edge.depth);
            if original <= 0 {
                assert!(
                    !has_any_ink(&before, sample),
                    "{name}: {} still paints ink with its original width {original}",
                    edge.id
                );
            } else {
                assert!(
                    has_line_ink(&before, sample, theme.line),
                    "{name}: {} positive width {original} baseline lost its ink",
                    edge.id
                );
            }
        }
    }
}

/// The accent pass repaints shapes only; edge path geometry and the generated
/// positive widths must stay identical between the neutral and accent render
/// of the same color mode.
#[test]
fn accents_preserve_edge_geometry_and_positive_widths() {
    let themes = themes();
    for (neutral, accent) in [(&themes[0], &themes[2]), (&themes[1], &themes[3])] {
        let neutral_svg =
            render_svg(DEPTH6, &neutral.1).unwrap_or_else(|error| panic!("{}: {error}", neutral.0));
        let accent_svg =
            render_svg(DEPTH6, &accent.1).unwrap_or_else(|error| panic!("{}: {error}", accent.0));
        let geometry = |svg: &str| {
            raw_edges(svg)
                .into_iter()
                .map(|edge| (edge.id, edge.d, edge.data_points))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            geometry(&neutral_svg),
            geometry(&accent_svg),
            "{} vs {}: accents changed edge geometry",
            neutral.0,
            accent.0
        );
        for depth in [1_u32, 2, 3, 4] {
            let width = 14 - 3 * i64::from(depth);
            let rule = format!(".edge-depth-{depth}{{stroke-width:{width};}}");
            assert!(
                neutral_svg.contains(&rule) && accent_svg.contains(&rule),
                "positive width {width} for depth {depth} was not preserved in both modes"
            );
        }
    }
}
