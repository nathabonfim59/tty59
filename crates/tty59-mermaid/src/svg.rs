//! Mermaid source → themed, resvg-safe SVG.
//!
//! merman owns parsing, layout and SVG emission. This module only supplies the
//! site configuration (theme palette, dark mode, font family), a deterministic
//! diagram id, and merman's [`SvgPipeline::resvg_safe`] post-processing so the
//! result can be handed to `usvg`/`resvg` unchanged.
//!
//! Written from scratch against merman's public API. Never copy from Zed's
//! GPL-licensed `mermaid_render` crate.

use std::panic::{AssertUnwindSafe, catch_unwind};

use merman::svg::{SvgPipeline, SvgRenderOptions};
use merman::{
    Engine, MermaidConfig, OperationControl, RenderError, RenderOutput, RenderRequest, Renderer,
    SvgRequest,
};
use serde_json::{Map, Value};

use crate::{MermaidError, MermaidTheme};

pub(crate) fn render(source: &str, theme: &MermaidTheme) -> Result<String, MermaidError> {
    if source.trim().is_empty() {
        return Err(MermaidError::Syntax("diagram source is empty".into()));
    }

    // merman is not documented as panic-free for every hostile input, and the
    // contract here is that this function never unwinds into the caller. The
    // class clipping post-process runs inside the same boundary.
    match catch_unwind(AssertUnwindSafe(|| {
        render_inner(source, theme).map(unclip_class_fallback_viewports)
    })) {
        Ok(result) => result,
        Err(_) => Err(MermaidError::Render("the diagram renderer panicked".into())),
    }
}

fn render_inner(source: &str, theme: &MermaidTheme) -> Result<String, MermaidError> {
    let request = SvgRequest {
        options: SvgRenderOptions {
            diagram_id: Some(diagram_id(source)),
            ..SvgRenderOptions::default()
        },
        // Converts HTML labels and strips things resvg cannot draw.
        pipeline: Some(SvgPipeline::resvg_safe()),
        ..SvgRequest::default()
    };

    let renderer = Renderer::new().with_engine(Engine::new().with_site_config(site_config(theme)));

    match renderer.render(RenderRequest::svg(source, OperationControl::new(), request)) {
        Ok(RenderOutput::Svg(Some(output))) => Ok(output.into_parts().0),
        Ok(_) => Err(MermaidError::Render(
            "the diagram renderer produced no SVG".into(),
        )),
        Err(error) => Err(classify(error)),
    }
}

/// Root marker merman puts on class-diagram SVGs (`aria_roledescription` is
/// the diagram type). Checked on the rendered document's root tag, never on
/// the user's source, so text that merely contains these words cannot opt in.
/// merman's class detector currently reports `classDiagram`; the `-v2`
/// spelling of the Mermaid header and the render-family name are accepted
/// too, so a harmless upstream role rename cannot silently disable the fix.
const CLASS_ROOT_ROLE: &str = r#"role="graphics-document document""#;
const CLASS_ROOT_ROLEDESCRIPTIONS: [&str; 3] = [
    r#"aria-roledescription="classDiagram""#,
    r#"aria-roledescription="classDiagram-v2""#,
    r#"aria-roledescription="class""#,
];

/// Classes merman's resvg-safe pipeline copies from the source label groups
/// onto the generated fallback text elements. Only class *node* titles,
/// members, methods and annotations carry these; edge labels (`edgeLabel`)
/// and notes are deliberately left alone.
const CLASS_NODE_LABEL_CLASSES: [&str; 4] = [
    "label-group",
    "members-group",
    "methods-group",
    "annotation-group",
];

/// Opening tag of a label that merman's resvg-safe pipeline lowered from a
/// `<foreignObject>` into a nested, measured-width viewport.
const FALLBACK_VIEWPORT_PREFIX: &str =
    r#"<svg xmlns="http://www.w3.org/2000/svg" x="0" y="0" width=""#;

const FALLBACK_HIDDEN_STYLE: &str = r#"overflow:hidden"#;
const FALLBACK_HIDDEN_STYLE_SPACED: &str = r#"overflow: hidden"#;
const FALLBACK_VISIBLE_STYLE: &str = r#"overflow:visible"#;
const FALLBACK_VISIBLE_STYLE_SPACED: &str = r#"overflow: visible"#;

/// Class diagrams measure each title/member row with merman's deterministic
/// text profile, then the resvg-safe fallback paints that row inside a nested
/// `<svg>` viewport of exactly the measured width, with `overflow: hidden`.
/// Real system-font glyphs are often a little wider than the profile, so the
/// first and last glyphs get clipped even though the enclosing class box has
/// room to spare. Let only those generated class-node label viewports paint
/// outside their tight bounds: font size, text positions and class geometry
/// stay untouched, edge labels and notes keep their clipping, and every other
/// diagram family is returned byte-identical.
///
/// The final pipeline minifies the declaration (`overflow:hidden`); the
/// spaced spelling is accepted too so the fix does not depend on the
/// minifier's exact formatting.
fn unclip_class_fallback_viewports(svg: String) -> String {
    if !is_class_diagram(&svg) {
        return svg;
    }

    let mut out = String::with_capacity(svg.len() + 16);
    let mut rest = svg.as_str();
    let mut changed = false;
    while let Some(start) = rest.find(FALLBACK_VIEWPORT_PREFIX) {
        let (head, tail) = rest.split_at(start);
        out.push_str(head);
        let Some(tag_len) = tail.find('>') else {
            out.push_str(tail);
            return out;
        };
        let (tag, after) = tail.split_at(tag_len + 1);
        let Some(close) = after.find("</svg>") else {
            out.push_str(tag);
            out.push_str(after);
            return out;
        };
        let body = &after[..close];
        if fallback_body_is_class_node_label(body)
            && let Some(unclipped) = unclip_style(tag)
        {
            out.push_str(&unclipped);
            changed = true;
        } else {
            out.push_str(tag);
        }
        rest = after;
    }
    out.push_str(rest);

    if changed { out } else { svg }
}

/// Rewrites a generated viewport start tag's `overflow: hidden` declaration
/// to `visible`, preserving every other byte (font size, geometry and the
/// pipeline's minified style formatting).
fn unclip_style(tag: &str) -> Option<String> {
    let style_start = tag.find(r#"style=""#)? + r#"style=""#.len();
    let style_end = style_start + tag[style_start..].find('"')?;
    let style = &tag[style_start..style_end];
    let unclipped = if style.contains(FALLBACK_HIDDEN_STYLE) {
        style.replace(FALLBACK_HIDDEN_STYLE, FALLBACK_VISIBLE_STYLE)
    } else if style.contains(FALLBACK_HIDDEN_STYLE_SPACED) {
        style.replace(FALLBACK_HIDDEN_STYLE_SPACED, FALLBACK_VISIBLE_STYLE_SPACED)
    } else {
        return None;
    };
    let mut out = String::with_capacity(tag.len() + 1);
    out.push_str(&tag[..style_start]);
    out.push_str(&unclipped);
    out.push_str(&tag[style_end..]);
    Some(out)
}

/// Whether a generated fallback viewport's body carries a class-node label
/// group in merman's own `data-merman-source-classes` list. The list is copied
/// onto the generated fallback `<text>` elements from the source label groups,
/// so this needs no guess about the class diagram's emitted class names.
fn fallback_body_is_class_node_label(body: &str) -> bool {
    let mut rest = body;
    while let Some(start) = rest.find("data-merman-source-classes=\"") {
        let value_start = start + "data-merman-source-classes=\"".len();
        let Some(value_len) = rest[value_start..].find('"') else {
            return false;
        };
        let classes = &rest[value_start..value_start + value_len];
        if classes
            .split_whitespace()
            .any(|class| CLASS_NODE_LABEL_CLASSES.contains(&class))
        {
            return true;
        }
        rest = &rest[value_start + value_len..];
    }
    false
}

/// Minimal attribute lookup for one already-isolated start tag, used by the
/// fallback-shape unit tests. Values never contain a quote here: merman
/// escapes them before emission.
#[cfg(test)]
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = start + tag[start..].find('"')?;
    Some(&tag[start..end])
}

/// Whether the document's root `<svg>` tag carries the class-diagram
/// accessibility marker. Only the first `<svg` is inspected, so a nested or
/// quoted one cannot change the decision.
fn is_class_diagram(svg: &str) -> bool {
    let Some(start) = svg.find("<svg") else {
        return false;
    };
    let Some(tag_len) = svg[start..].find('>') else {
        return false;
    };
    let root_tag = &svg[start..start + tag_len];
    root_tag.contains(CLASS_ROOT_ROLE)
        && CLASS_ROOT_ROLEDESCRIPTIONS
            .iter()
            .any(|marker| root_tag.contains(marker))
}

/// Map merman's errors onto the crate's two public categories: anything the
/// user can fix by editing the source is `Syntax`, everything else is
/// `Render`.
fn classify(error: RenderError) -> MermaidError {
    match error {
        RenderError::NoDiagram => MermaidError::Syntax("no mermaid diagram was detected".into()),
        RenderError::Parse(diagnostic) => {
            MermaidError::Syntax(short_message(&diagnostic.to_string()))
        }
        other => MermaidError::Render(short_message(&other.to_string())),
    }
}

/// Collapse a diagnostic to one bounded, human-sized line.
///
/// The copy is bounded while it is built: a hostile diagnostic is never
/// duplicated in full just to truncate it afterwards. Truncation is counted in
/// `char`s, so it never splits a multibyte sequence.
fn short_message(message: &str) -> String {
    const LIMIT: usize = 240;
    let mut out = String::with_capacity(message.len().min(LIMIT * 4));
    let mut used = 0usize;
    let mut pending_space = false;

    for word in message.split_whitespace() {
        if pending_space {
            if used == LIMIT {
                out.push('…');
                return out;
            }
            out.push(' ');
            used += 1;
        }
        for ch in word.chars() {
            if used == LIMIT {
                out.push('…');
                return out;
            }
            out.push(ch);
            used += 1;
        }
        pending_space = true;
    }
    out
}

/// A stable id derived from the source. merman emits it in marker/gradient ids,
/// so the same source produces byte-identical SVG across calls and processes.
fn diagram_id(source: &str) -> String {
    // FNV-1a: tiny, dependency-free and deterministic across processes and
    // Rust versions (unlike `RandomState`).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("tty59-mermaid-{hash:016x}")
}

/// Mermaid site configuration: the neutral `base` theme with every contract
/// color pinned, so no upstream palette leaks between light and dark.
fn site_config(theme: &MermaidTheme) -> MermaidConfig {
    let mut root = Map::new();
    root.insert("theme".into(), Value::String("base".into()));
    root.insert(
        "fontFamily".into(),
        Value::String(theme.font_family.clone()),
    );
    root.insert("themeVariables".into(), Value::Object(variables(theme)));
    MermaidConfig::from_value(Value::Object(root))
}

fn variables(theme: &MermaidTheme) -> Map<String, Value> {
    let colors = [
        ("background", theme.background),
        ("primaryColor", theme.primary),
        ("primaryTextColor", theme.primary_text),
        ("primaryBorderColor", theme.primary_border),
        ("secondaryColor", theme.secondary),
        ("secondaryTextColor", theme.text),
        ("secondaryBorderColor", theme.primary_border),
        ("tertiaryColor", theme.tertiary),
        ("tertiaryTextColor", theme.text),
        ("tertiaryBorderColor", theme.primary_border),
        ("textColor", theme.text),
        ("titleColor", theme.text),
        ("nodeTextColor", theme.primary_text),
        ("lineColor", theme.line),
        ("defaultLinkColor", theme.line),
        ("arrowheadColor", theme.line),
        ("edgeLabelBackground", theme.edge_label_background),
        ("clusterBkg", theme.cluster_background),
        ("clusterBorder", theme.cluster_border),
        ("noteBkgColor", theme.note_background),
        ("noteBorderColor", theme.note_border),
        ("noteTextColor", theme.text),
        ("actorBkg", theme.actor_background),
        ("actorBorder", theme.actor_border),
        ("actorLineColor", theme.actor_border),
        ("actorTextColor", theme.text),
        ("signalColor", theme.text),
        ("signalTextColor", theme.text),
        ("labelBoxBkgColor", theme.actor_background),
        ("labelBoxBorderColor", theme.actor_border),
        ("labelTextColor", theme.text),
        ("loopTextColor", theme.text),
        ("activationBkgColor", theme.secondary),
        ("activationBorderColor", theme.primary_border),
        ("sectionBkgColor", theme.secondary),
        ("sectionBkgColor2", theme.primary),
        ("taskBkgColor", theme.primary),
        ("taskBorderColor", theme.primary_border),
        ("taskTextColor", theme.primary_text),
        ("taskTextOutsideColor", theme.text),
        ("activeTaskBkgColor", theme.secondary),
        ("activeTaskBorderColor", theme.primary_border),
        ("errorBkgColor", theme.error),
        ("errorTextColor", theme.background),
        ("critBkgColor", theme.error),
        ("critBorderColor", theme.error),
        ("pieStrokeColor", theme.primary_border),
        ("pieOuterStrokeColor", theme.primary_border),
        ("pieTitleTextColor", theme.text),
        ("pieSectionTextColor", theme.primary_text),
    ];

    let mut variables = Map::new();
    variables.insert("darkMode".into(), Value::Bool(theme.dark_mode));
    variables.insert(
        "fontFamily".into(),
        Value::String(theme.font_family.clone()),
    );
    for (key, value) in colors {
        variables.insert(key.into(), Value::String(value.to_css()));
    }
    variables
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    #[test]
    fn diagram_id_is_stable_and_varies_with_source() {
        let first = diagram_id("flowchart TD\nA --> B");
        assert_eq!(first, diagram_id("flowchart TD\nA --> B"));
        assert_ne!(first, diagram_id("flowchart TD\nA --> C"));
        assert!(first.starts_with("tty59-mermaid-"));
    }

    #[test]
    fn short_message_collapses_and_bounds() {
        assert_eq!(
            short_message("  line one\n\tline   two "),
            "line one line two"
        );
        let long = "x".repeat(500);
        let truncated = short_message(&long);
        assert_eq!(truncated.chars().count(), 241);
        assert!(truncated.ends_with('…'));

        // A long multibyte diagnostic truncates on a char boundary and never
        // panics or splits a scalar value.
        let multibyte = "é".repeat(1_000);
        let truncated = short_message(&multibyte);
        assert_eq!(truncated.chars().count(), 241);
        assert!(truncated.is_char_boundary(truncated.len()));
        assert_eq!(truncated.chars().filter(|ch| *ch == '…').count(), 1);
        assert_eq!(truncated.chars().filter(|ch| *ch == 'é').count(), 240);

        // Exactly at the limit: no ellipsis. One past: ellipsis.
        let exact = "a".repeat(240);
        assert_eq!(short_message(&exact), exact);
        let over = "a".repeat(241);
        assert_eq!(short_message(&over).chars().count(), 241);
        assert!(short_message(&over).ends_with('…'));

        // Multi-word input hits the limit between words.
        let words = "word ".repeat(100);
        let bounded = short_message(&words);
        assert!(bounded.chars().count() <= 241);
        assert!(bounded.ends_with('…'));
    }

    const FLOWCHART: &str = "flowchart TD\n  A[Alpha] --> B[Beta]";

    #[test]
    fn renders_flowchart_as_resvg_safe_svg() {
        let svg = render(FLOWCHART, &MermaidTheme::light()).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("viewBox="));
        assert!(svg.contains("<text"));
        assert!(svg.contains("Alpha"));
        assert!(svg.contains("tty59-mermaid-"));
        // HTML labels must be converted for resvg; nothing may remain in a
        // foreignObject.
        assert!(!svg.contains("foreignObject"));
    }

    #[test]
    fn output_is_deterministic_for_the_same_source() {
        let first = render(FLOWCHART, &MermaidTheme::light()).unwrap();
        let second = render(FLOWCHART, &MermaidTheme::light()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn light_and_dark_emit_their_palettes() {
        let light = render(FLOWCHART, &MermaidTheme::light()).unwrap();
        let dark = render(FLOWCHART, &MermaidTheme::dark()).unwrap();
        assert_ne!(light, dark);
        assert!(light.contains(&MermaidTheme::light().primary.to_css()));
        assert!(dark.contains(&MermaidTheme::dark().primary.to_css()));
        assert!(light.contains("Arial"));
    }

    #[test]
    fn custom_palette_reaches_the_svg() {
        let theme = MermaidTheme {
            primary: Rgba::rgb(0x11, 0x22, 0x33),
            primary_border: Rgba::rgb(0x44, 0x55, 0x66),
            line: Rgba::rgb(0x77, 0x88, 0x99),
            text: Rgba::rgb(0xaa, 0xbb, 0xcc),
            ..MermaidTheme::light()
        };
        let svg = render(FLOWCHART, &theme).unwrap();
        assert!(svg.contains("#112233"));
        assert!(svg.contains("#445566"));
        assert!(svg.contains("#778899"));
        assert!(svg.contains("#aabbcc"));
    }

    #[test]
    fn leading_whitespace_is_accepted() {
        for source in [
            "   flowchart TD\n  A[Alpha] --> B[Beta]",
            "\n\n   flowchart TD\n  A[Alpha] --> B[Beta]",
            "\tflowchart TD\n  A[Alpha] --> B[Beta]",
        ] {
            let svg = render(source, &MermaidTheme::light()).unwrap();
            assert!(svg.contains("Alpha"), "{source:?}");
        }
    }

    #[test]
    fn empty_and_whitespace_are_syntax_errors() {
        for source in ["", " ", "\n", "\t\r\n  "] {
            match render(source, &MermaidTheme::light()) {
                Err(MermaidError::Syntax(message)) => assert!(!message.is_empty()),
                other => panic!("{source:?} => {other:?}"),
            }
        }
    }

    #[test]
    fn invalid_sources_are_syntax_errors() {
        for source in [
            "not a diagram",
            "flowchart TD\n  A[",
            "sequenceDiagram\n  Actor->>: hi",
        ] {
            match render(source, &MermaidTheme::light()) {
                Err(MermaidError::Syntax(message)) => assert!(!message.is_empty()),
                other => panic!("{source:?} => {other:?}"),
            }
        }
    }

    #[test]
    fn garbage_inputs_never_panic() {
        let garbage = [
            "%%{init: [}%%",
            "\u{0}\u{1}\u{2}",
            "flowchart",
            "graph",
            "sequenceDiagram\n  participant",
            "pie\n  \"\" : nope",
            "<script>alert(1)</script>",
            "classDiagram\n  class {",
        ];
        for source in garbage {
            match render(source, &MermaidTheme::light()) {
                Ok(svg) => assert!(svg.starts_with("<svg")),
                Err(MermaidError::Syntax(_) | MermaidError::Render(_)) => {}
            }
        }
    }

    #[test]
    fn site_config_pins_contract_colors() {
        let theme = MermaidTheme {
            primary: Rgba::rgb(0x11, 0x22, 0x33),
            primary_border: Rgba::rgb(0x44, 0x55, 0x66),
            line: Rgba::rgb(0x77, 0x88, 0x99),
            ..MermaidTheme::light()
        };
        let config = site_config(&theme);
        let value = config.as_value();
        assert_eq!(value.get("theme").and_then(|v| v.as_str()), Some("base"));
        assert_eq!(
            value
                .get("themeVariables")
                .and_then(|v| v.get("primaryColor"))
                .and_then(|v| v.as_str()),
            Some("#112233")
        );
        assert_eq!(
            value
                .get("themeVariables")
                .and_then(|v| v.get("lineColor"))
                .and_then(|v| v.as_str()),
            Some("#778899")
        );
    }

    const CLASS_CLIPPING_FIXTURE: &str = "\
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

    const CLASS_WITH_EDGE_AND_NOTE: &str = "\
classDiagram
    class Alpha {
        +int value
    }
    class Beta
    Alpha --> Beta : relates
    note for Alpha \"kept clipped\"
";

    const OTHER_FAMILIES: [(&str, &str); 8] = [
        ("flowchart", "flowchart TD\n  A[Alpha] --> B[Beta]"),
        (
            "sequence",
            "sequenceDiagram\n  A->>B: hello\n  B-->>A: world",
        ),
        ("state", "stateDiagram-v2\n  [*] --> Idle\n  Idle --> [*]"),
        ("er", "erDiagram\n  CUSTOMER ||--o{ ORDER : places"),
        (
            "gantt",
            "gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section Build\n  Compile :a1, 2024-01-01, 3d",
        ),
        ("pie", "pie title Split\n  \"Build\" : 50\n  \"Test\" : 50"),
        ("mindmap", "mindmap\n  root((plan))\n    build\n    test"),
        (
            "gitGraph",
            "gitGraph\n  commit id: \"init\"\n  branch feature\n  checkout feature\n  commit id: \"work\"",
        ),
    ];

    /// Every generated resvg-safe fallback viewport in `svg` as
    /// `(source classes, overflow value)` pairs, in document order. The source
    /// classes are copied onto the fallback `<text>` elements inside the
    /// generated viewport.
    fn fallback_viewports(svg: &str) -> Vec<(String, String)> {
        let mut viewports = Vec::new();
        let mut rest = svg;
        while let Some(start) = rest.find(FALLBACK_VIEWPORT_PREFIX) {
            let tail = &rest[start..];
            let viewport_len = tail.find('>').expect("viewport end") + 1;
            let viewport = &tail[..viewport_len];
            let after = &tail[viewport_len..];
            let close = after.find("</svg>").expect("viewport close");
            let body = &after[..close];
            let classes = body
                .match_indices("data-merman-source-classes=\"")
                .map(|(start, _)| {
                    let value_start = start + "data-merman-source-classes=\"".len();
                    let value_len = body[value_start..].find('"').expect("source classes value");
                    &body[value_start..value_start + value_len]
                })
                .collect::<Vec<_>>()
                .join(" ");
            let overflow = attribute(viewport, "style")
                .unwrap_or_default()
                .split(';')
                .find_map(|declaration| declaration.trim().strip_prefix("overflow:"))
                .unwrap_or_default()
                .trim()
                .to_owned();
            viewports.push((classes, overflow));
            rest = &after[close + "</svg>".len()..];
        }
        viewports
    }

    fn is_class_node_label(classes: &str) -> bool {
        classes
            .split_whitespace()
            .any(|class| CLASS_NODE_LABEL_CLASSES.contains(&class))
    }

    #[test]
    fn class_rows_are_unclipped_and_edge_and_note_labels_are_not() {
        let theme = MermaidTheme::dark();
        let svg = render(CLASS_CLIPPING_FIXTURE, &theme).unwrap();
        let root = &svg[..svg.find('>').expect("root end")];
        assert!(
            is_class_diagram(&svg),
            "unrecognized class root role: {root}"
        );

        let viewports = fallback_viewports(&svg);
        let node_labels: Vec<_> = viewports
            .iter()
            .filter(|(classes, _)| is_class_node_label(classes))
            .collect();
        assert!(
            !node_labels.is_empty(),
            "real class fixture produced no node-label fallbacks: {viewports:?}"
        );
        assert!(
            node_labels
                .iter()
                .all(|(_, overflow)| overflow == "visible"),
            "class node-label viewport still clipped: {node_labels:?}"
        );

        let with_edge_and_note = render(CLASS_WITH_EDGE_AND_NOTE, &theme).unwrap();
        let other_viewports: Vec<_> = fallback_viewports(&with_edge_and_note)
            .into_iter()
            .filter(|(classes, _)| !is_class_node_label(classes))
            .collect();
        assert!(
            !other_viewports.is_empty(),
            "fixture did not reach the edge-label/note fallback path"
        );
        assert!(
            other_viewports
                .iter()
                .all(|(_, overflow)| overflow == "hidden"),
            "non-node class labels were unclipped: {other_viewports:?}"
        );
    }

    #[test]
    fn non_class_families_are_byte_and_pixel_identical_to_the_raw_render() {
        let theme = MermaidTheme::dark();
        for (name, source) in OTHER_FAMILIES {
            let raw = render_inner(source, &theme)
                .unwrap_or_else(|error| panic!("{name}: raw render failed: {error}"));
            let production = render(source, &theme)
                .unwrap_or_else(|error| panic!("{name}: render failed: {error}"));
            assert!(
                production == raw,
                "{name}: class clipping post-process changed the SVG"
            );
            let raw_raster = crate::rasterize(&raw, 1.0).expect("raw raster");
            let production_raster = crate::rasterize(&production, 1.0).expect("production raster");
            assert!(
                production_raster.bgra == raw_raster.bgra,
                "{name}: class clipping post-process changed pixels ({}x{})",
                production_raster.width,
                production_raster.height
            );
        }
    }

    #[test]
    fn class_post_process_changes_bytes_and_pixels_only_for_node_rows() {
        let theme = MermaidTheme::dark();
        let raw = render_inner(CLASS_CLIPPING_FIXTURE, &theme).expect("raw class render");
        let production = render(CLASS_CLIPPING_FIXTURE, &theme).expect("class render");
        assert!(
            raw != production,
            "class post-process left the SVG byte-identical"
        );
        assert!(raw.contains("overflow:hidden"));
        assert!(production.contains("overflow:visible"));

        let raw_raster = crate::rasterize(&raw, 2.0).expect("raw class raster");
        let production_raster = crate::rasterize(&production, 2.0).expect("class raster");
        assert!(
            raw_raster.bgra != production_raster.bgra,
            "class post-process changed the SVG but not the pixels ({}x{})",
            production_raster.width,
            production_raster.height
        );
    }
}
