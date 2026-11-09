//! Mermaid source → themed, resvg-safe SVG.
//!
//! merman owns parsing, layout and SVG emission. This module supplies the
//! site configuration (theme palette, dark mode, font family), a deterministic
//! diagram id, and merman's [`SvgPipeline::resvg_safe`] post-processing, then
//! applies three small passes to the final SVG: unclipping every generated
//! HTML-label viewport so real glyphs are not cut, flooring non-positive
//! generated mindmap edge widths so deep branches stay visible, and painting
//! mindmap branch edges with the contract line color (merman has no
//! independent variable for them).
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
    // SVG post-processing runs inside the same boundary.
    match catch_unwind(AssertUnwindSafe(|| {
        render_inner(source, theme).map(|svg| postprocess(svg, theme))
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
        Ok(RenderOutput::Svg(Some(output))) => {
            let svg = output.into_parts().0;
            Ok(apply_accents(svg, source, theme))
        }
        Ok(_) => Err(MermaidError::Render(
            "the diagram renderer produced no SVG".into(),
        )),
        Err(error) => Err(classify(error)),
    }
}

/// Apply the accent pass to merman's SVG. An empty palette returns the input
/// unchanged before scanning, so the neutral look stays byte-identical.
fn apply_accents(svg: String, source: &str, theme: &MermaidTheme) -> String {
    if theme.accents.is_empty() {
        svg
    } else {
        crate::accents::apply(&svg, source, theme).into_owned()
    }
}

/// Root accessibility marker merman puts on every SVG document.
const ROOT_ROLE: &str = r#"role="graphics-document document""#;

/// Opening tag of a label that merman's resvg-safe pipeline lowered from a
/// `<foreignObject>` into a nested, measured-width viewport.
const FALLBACK_VIEWPORT_PREFIX: &str =
    r#"<svg xmlns="http://www.w3.org/2000/svg" x="0" y="0" width=""#;

const FALLBACK_HIDDEN_STYLE: &str = r#"overflow:hidden"#;
const FALLBACK_HIDDEN_STYLE_SPACED: &str = r#"overflow: hidden"#;
const FALLBACK_VISIBLE_STYLE: &str = r#"overflow:visible"#;
const FALLBACK_VISIBLE_STYLE_SPACED: &str = r#"overflow: visible"#;

/// Post-process the final merman SVG: unclip every generated HTML-label
/// viewport, floor non-positive mindmap edge widths, then fix mindmap
/// branch-edge paint.
fn postprocess(svg: String, theme: &MermaidTheme) -> String {
    let svg = unclip_fallback_viewports(svg);
    let svg = clamp_mindmap_edge_widths(svg);
    paint_mindmap_edges(svg, theme)
}

/// merman's resvg-safe pipeline lowers every HTML label (`<foreignObject>`)
/// into a nested viewport of exactly the deterministic text profile's measured
/// width, with `overflow: hidden`. Real system-font glyphs can be a little
/// wider than that profile, so the first and last glyphs get clipped even when
/// the enclosing shape or edge-label box has room. Let every generated
/// viewport paint outside its tight bounds: font size, text positions and all
/// geometry stay untouched.
///
/// The final pipeline minifies the declaration (`overflow:hidden`); the
/// spaced spelling is accepted too so the fix does not depend on the
/// minifier's exact formatting.
fn unclip_fallback_viewports(svg: String) -> String {
    let mut out = String::with_capacity(svg.len() + 64);
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
        if let Some(unclipped) = unclip_style(tag) {
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

/// merman generates one `.edge-depth-{section}` stylesheet rule per section
/// with width `17 - 3 * (section + 1)`. From depth 5 that value is zero or
/// negative, and usvg drops a non-positive stroke width entirely, so deep
/// mindmap edges vanish. Clamp only those generated widths to a one-unit
/// floor: every already-positive width stays byte-for-byte identical, and
/// geometry, colors and every other stylesheet rule are untouched.
///
/// The rewrite is narrow on purpose: only `<style>` contents are inspected,
/// and only rules scoped to this document's generated root id
/// (`#<diagram-id> .edge-depth-{n}`) are candidates, so a label or an
/// unrelated stylesheet selector that happens to spell the same text is
/// never touched. The pass is gated on the rendered document being a
/// mindmap, so timeline/kanban output keeps its own generated widths.
fn clamp_mindmap_edge_widths(svg: String) -> String {
    if !is_mindmap(&svg) {
        return svg;
    }
    let Some(diagram_id) = root_tag(&svg).and_then(|tag| attribute(tag, "id")) else {
        return svg;
    };
    let selector = format!("#{diagram_id} .edge-depth-");

    const STYLE_OPEN: &str = "<style";
    const STYLE_CLOSE: &str = "</style>";

    let mut out = String::with_capacity(svg.len());
    let mut rest = svg.as_str();
    let mut changed = false;
    while let Some(start) = rest.find(STYLE_OPEN) {
        let (head, tail) = rest.split_at(start);
        out.push_str(head);
        let Some(open_end) = tail.find('>') else {
            out.push_str(tail);
            return out;
        };
        let (open_tag, after_open) = tail.split_at(open_end + 1);
        out.push_str(open_tag);
        let Some(close_rel) = after_open.find(STYLE_CLOSE) else {
            out.push_str(after_open);
            return out;
        };
        let (css, after_close) = after_open.split_at(close_rel);
        if let Some(clamped) = clamp_style_depth_widths(css, &selector) {
            out.push_str(&clamped);
            changed = true;
        } else {
            out.push_str(css);
        }
        out.push_str(STYLE_CLOSE);
        rest = &after_close[STYLE_CLOSE.len()..];
    }
    out.push_str(rest);

    if changed { out } else { svg }
}

/// Floors non-positive widths of already-isolated stylesheet text for one
/// exact selector. Returns `None` when every matching rule is already
/// positive, so the caller can keep the original bytes.
fn clamp_style_depth_widths(style: &str, selector: &str) -> Option<String> {
    const DECLARATION: &str = "{stroke-width:";

    let mut out = String::with_capacity(style.len());
    let mut rest = style;
    let mut changed = false;
    while let Some(start) = rest.find(selector) {
        let (head, tail) = rest.split_at(start);
        out.push_str(head);
        out.push_str(selector);
        let suffix = &tail[selector.len()..];
        let suffix_len = suffix
            .find(|ch: char| !(ch.is_ascii_digit() || ch == '-'))
            .unwrap_or(suffix.len());
        out.push_str(&suffix[..suffix_len]);
        let after_selector = &suffix[suffix_len..];
        let Some(declaration) = after_selector.strip_prefix(DECLARATION) else {
            rest = after_selector;
            continue;
        };
        let Some(value_len) = declaration.find([';', '}']) else {
            rest = after_selector;
            continue;
        };
        let value = &declaration[..value_len];
        if value.parse::<f64>().is_ok_and(|width| width <= 0.0) {
            out.push_str(DECLARATION);
            out.push('1');
            changed = true;
        } else {
            out.push_str(DECLARATION);
            out.push_str(value);
        }
        rest = &declaration[value_len..];
    }
    out.push_str(rest);

    if changed { Some(out) } else { None }
}

/// Mindmap branch edges have no independent theme variable: merman paints
/// `.section-edge-*` paths with `cScale{i}`, the same color as the node body
/// (mindmap/render.rs). The base theme's `cScaleInv{i}` only reaches section
/// `<line>` elements. Paint the branch paths with the contract `line` color
/// inline so they stay visible against the background; geometry is untouched.
fn paint_mindmap_edges(svg: String, theme: &MermaidTheme) -> String {
    if !is_mindmap(&svg) {
        return svg;
    }
    let stroke = theme.line.to_css();
    let mut out = String::with_capacity(svg.len() + 64);
    let mut rest = svg.as_str();
    while let Some(start) = rest.find("<path") {
        let (head, tail) = rest.split_at(start);
        out.push_str(head);
        let Some(tag_len) = tail.find('>') else {
            out.push_str(tail);
            return out;
        };
        let (tag, after) = tail.split_at(tag_len + 1);
        let is_section_edge = attribute(tag, "class").is_some_and(|classes| {
            classes
                .split_whitespace()
                .any(|class| class.starts_with("section-edge-"))
        });
        if is_section_edge {
            out.push_str(&set_inline_style(tag, "stroke", &stroke));
        } else {
            out.push_str(tag);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Adds or replaces `property` in an element's inline `style` attribute.
/// Inline styles beat regular generated stylesheet rules, but an `!important`
/// declaration is stronger. Used only where merman has no independent theme
/// variable and the generated CSS must be overridden.
fn set_inline_style(tag: &str, property: &str, value: &str) -> String {
    if let Some(style_start) = tag.find(r#"style=""#) {
        let style_value_start = style_start + r#"style=""#.len();
        let Some(style_len) = tag[style_value_start..].find('"') else {
            return tag.to_owned();
        };
        let style_end = style_value_start + style_len;
        let style = &tag[style_value_start..style_end];
        let mut rebuilt = String::with_capacity(style.len() + property.len() + value.len() + 4);
        for declaration in style.split(';') {
            let declaration = declaration.trim();
            if declaration.is_empty() {
                continue;
            }
            if declaration
                .split_once(':')
                .is_some_and(|(name, _)| name.trim() == property)
            {
                continue;
            }
            rebuilt.push_str(declaration);
            rebuilt.push(';');
        }
        rebuilt.push_str(property);
        rebuilt.push(':');
        rebuilt.push_str(value);
        rebuilt.push(';');

        let mut out = String::with_capacity(tag.len() + rebuilt.len());
        out.push_str(&tag[..style_value_start]);
        out.push_str(&rebuilt);
        out.push_str(&tag[style_end..]);
        out
    } else {
        let insert_at = if tag.ends_with("/>") {
            tag.len() - 2
        } else {
            tag.len() - 1
        };
        let mut out = String::with_capacity(tag.len() + property.len() + value.len() + 12);
        out.push_str(&tag[..insert_at]);
        out.push_str(&format!(" style=\"{property}:{value};\""));
        out.push_str(&tag[insert_at..]);
        out
    }
}

/// Minimal attribute lookup for one already-isolated start tag. Values never
/// contain a quote here: merman escapes them before emission.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = start + tag[start..].find('"')?;
    Some(&tag[start..end])
}

/// The document's root `<svg>` start tag, if there is one. Only the first
/// `<svg` is inspected, so a nested or quoted one cannot change the decision.
fn root_tag(svg: &str) -> Option<&str> {
    let start = svg.find("<svg")?;
    let tag_len = svg[start..].find('>')?;
    Some(&svg[start..start + tag_len])
}

/// Whether the rendered document is a mindmap. Checked on the root tag, never
/// on the user's source.
fn is_mindmap(svg: &str) -> bool {
    root_tag(svg).is_some_and(|tag| {
        tag.contains(ROOT_ROLE)
            && (tag.contains(r#"aria-roledescription="mindmap""#)
                || tag.contains(r#"class="mindmapDiagram""#))
    })
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

    // Mindmaps (and the shared cScale consumers) read their per-section
    // palette from these variables. merman's base theme derives cScale from
    // primary/secondary/tertiary and darkens it, but explicit theme variables
    // are replayed last, so pin every entry to the contract colors.
    let fills = [theme.primary, theme.secondary, theme.tertiary];
    let fill_css = fills.map(|fill| fill.to_css());
    let line_css = theme.line.to_css();
    let label_css = theme.primary_text.to_css();
    for index in 0..12 {
        let fill = &fill_css[index % fill_css.len()];
        variables.insert(format!("cScale{index}"), Value::String(fill.clone()));
        variables.insert(format!("cScalePeer{index}"), Value::String(fill.clone()));
        variables.insert(format!("cScaleInv{index}"), Value::String(line_css.clone()));
        variables.insert(
            format!("cScaleLabel{index}"),
            Value::String(label_css.clone()),
        );
    }
    variables.insert("git0".into(), Value::String(fill_css[0].clone()));
    variables.insert("gitBranchLabel0".into(), Value::String(label_css.clone()));
    variables.insert(
        "nodeBorder".into(),
        Value::String(theme.primary_border.to_css()),
    );
    variables.insert("mainBkg".into(), Value::String(fill_css[0].clone()));
    variables.insert("scaleLabelColor".into(), Value::String(label_css));
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
    note for Alpha \"a note\"
";

    const ER_CLIPPING_FIXTURE: &str = "\
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE_ITEM : contains
";

    const MINDMAP_CLIPPING_FIXTURE: &str = "\
mindmap
  root((plan))
    render
      compile
        package
    review
    release
";

    const OTHER_FAMILIES: [(&str, &str); 8] = [
        ("flowchart", "flowchart TD\n  A[Alpha] --> B[Beta]"),
        (
            "sequence",
            "sequenceDiagram\n  A->>B: hello\n  B-->>A: world",
        ),
        ("class", CLASS_CLIPPING_FIXTURE),
        ("state", "stateDiagram-v2\n  [*] --> Idle\n  Idle --> [*]"),
        ("er", ER_CLIPPING_FIXTURE),
        (
            "gantt",
            "gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section Build\n  Compile :a1, 2024-01-01, 3d",
        ),
        ("pie", "pie title Split\n  \"Build\" : 50\n  \"Test\" : 50"),
        (
            "gitGraph",
            "gitGraph\n  commit id: \"init\"\n  branch feature\n  checkout feature\n  commit id: \"work\"",
        ),
    ];

    /// Every generated resvg-safe fallback viewport's `overflow` value in
    /// document order.
    fn fallback_viewport_overflows(svg: &str) -> Vec<String> {
        let mut overflows = Vec::new();
        let mut rest = svg;
        while let Some(start) = rest.find(FALLBACK_VIEWPORT_PREFIX) {
            let tail = &rest[start..];
            let viewport_len = tail.find('>').expect("viewport end") + 1;
            let viewport = &tail[..viewport_len];
            let overflow = attribute(viewport, "style")
                .unwrap_or_default()
                .split(';')
                .find_map(|declaration| declaration.trim().strip_prefix("overflow:"))
                .unwrap_or_default()
                .trim()
                .to_owned();
            overflows.push(overflow);
            rest = &tail[viewport_len..];
        }
        overflows
    }

    /// The expected production SVG: apply only the hidden→visible viewport
    /// overflow rewrite to the genuine raw merman output. Viewports that are
    /// already visible (some family labels default that way) must stay visible
    /// byte-for-byte; this is the flowchart regression that a blanket re-clip
    /// helper missed.
    fn expected_unclip(raw: &str) -> String {
        let mut out = String::with_capacity(raw.len() + 64);
        let mut rest = raw;
        while let Some(start) = rest.find(FALLBACK_VIEWPORT_PREFIX) {
            let (head, tail) = rest.split_at(start);
            out.push_str(head);
            let tag_len = tail.find('>').expect("viewport end") + 1;
            let tag = &tail[..tag_len];
            out.push_str(&rewrite_hidden_overflow(tag));
            rest = &tail[tag_len..];
        }
        out.push_str(rest);
        out
    }

    fn rewrite_hidden_overflow(tag: &str) -> String {
        let Some(style_start) = tag.find(r#"style=""#) else {
            return tag.to_owned();
        };
        let style_value_start = style_start + r#"style=""#.len();
        let style_len = tag[style_value_start..].find('"').expect("viewport style");
        let style_end = style_value_start + style_len;
        let style = &tag[style_value_start..style_end];
        let rewritten = if style.contains("overflow:hidden") {
            style.replace("overflow:hidden", "overflow:visible")
        } else if style.contains("overflow: hidden") {
            style.replace("overflow: hidden", "overflow: visible")
        } else {
            return tag.to_owned();
        };
        let mut out = String::with_capacity(tag.len() + 1);
        out.push_str(&tag[..style_value_start]);
        out.push_str(&rewritten);
        out.push_str(&tag[style_end..]);
        out
    }

    fn overflow_value(style: &str) -> Option<&str> {
        style
            .split(';')
            .find_map(|declaration| declaration.trim().strip_prefix("overflow:"))
            .map(str::trim)
    }

    fn normalize_overflow(style: &str, value: &str) -> String {
        let compact = format!("overflow:{value}");
        if style.contains(&compact) {
            style.replacen(&compact, "overflow:<value>", 1)
        } else {
            style.replacen(&format!("overflow: {value}"), "overflow: <value>", 1)
        }
    }

    /// Proves the transformation touched no style byte except the overflow
    /// value, and that an already-visible viewport stayed visible.
    fn assert_only_viewport_overflow_changed(raw: &str, production: &str) {
        let raw_tags: Vec<&str> = viewport_tags(raw);
        let production_tags: Vec<&str> = viewport_tags(production);
        assert_eq!(
            raw_tags.len(),
            production_tags.len(),
            "viewport count changed: raw {} production {}",
            raw_tags.len(),
            production_tags.len()
        );
        for (index, (raw_tag, production_tag)) in
            raw_tags.iter().zip(production_tags.iter()).enumerate()
        {
            let raw_style = attribute(raw_tag, "style").expect("raw viewport style");
            let production_style =
                attribute(production_tag, "style").expect("production viewport style");
            let raw_overflow = overflow_value(raw_style).expect("raw overflow");
            let production_overflow =
                overflow_value(production_style).expect("production overflow");
            let expected = if raw_overflow == "hidden" {
                "visible"
            } else {
                raw_overflow
            };
            assert_eq!(
                production_overflow, expected,
                "viewport {index}: overflow {raw_overflow} -> {production_overflow}"
            );
            assert_eq!(
                normalize_overflow(raw_style, raw_overflow),
                normalize_overflow(production_style, production_overflow),
                "viewport {index}: style bytes outside the overflow value changed"
            );
        }
    }

    fn viewport_tags(svg: &str) -> Vec<&str> {
        let mut tags = Vec::new();
        let mut rest = svg;
        while let Some(start) = rest.find(FALLBACK_VIEWPORT_PREFIX) {
            let tail = &rest[start..];
            let tag_len = tail.find('>').expect("viewport end") + 1;
            tags.push(&tail[..tag_len]);
            rest = &tail[tag_len..];
        }
        tags
    }

    /// Removes the test-only mindmap section-edge `stroke` override from
    /// `<path>` tags so the result can be compared with the raw render.
    fn strip_edge_stroke(svg: &str) -> String {
        let mut out = String::with_capacity(svg.len());
        let mut rest = svg;
        while let Some(start) = rest.find("<path") {
            let (head, tail) = rest.split_at(start);
            out.push_str(head);
            let tag_len = tail.find('>').expect("path end") + 1;
            let tag = &tail[..tag_len];
            let is_edge = attribute(tag, "class").is_some_and(|classes| {
                classes
                    .split_whitespace()
                    .any(|class| class.starts_with("section-edge-"))
            });
            if is_edge {
                out.push_str(&remove_inline_property(tag, "stroke"));
            } else {
                out.push_str(tag);
            }
            rest = &tail[tag_len..];
        }
        out.push_str(rest);
        out
    }

    fn remove_inline_property(tag: &str, property: &str) -> String {
        let Some(style_start) = tag.find(r#"style=""#) else {
            return tag.to_owned();
        };
        let style_value_start = style_start + r#"style=""#.len();
        let style_len = tag[style_value_start..].find('"').expect("style value");
        let style_end = style_value_start + style_len;
        let style = &tag[style_value_start..style_end];
        let rebuilt = style
            .split(';')
            .map(str::trim)
            .filter(|declaration| !declaration.is_empty())
            .filter(|declaration| {
                !declaration
                    .split_once(':')
                    .is_some_and(|(name, _)| name.trim() == property)
            })
            .collect::<Vec<_>>()
            .join(";");
        if rebuilt.is_empty() {
            let mut start = style_start;
            if tag[..start].ends_with(' ') {
                start -= 1;
            }
            let mut out = String::with_capacity(tag.len());
            out.push_str(&tag[..start]);
            out.push_str(&tag[style_end + 1..]);
            out
        } else {
            let mut out = String::with_capacity(tag.len());
            out.push_str(&tag[..style_value_start]);
            out.push_str(&rebuilt);
            out.push(';');
            out.push_str(&tag[style_end..]);
            out
        }
    }

    fn css_rgb(css: &str) -> [u8; 3] {
        let hex = css
            .strip_prefix('#')
            .unwrap_or_else(|| panic!("not hex: {css}"));
        assert_eq!(hex.len(), 6, "not #rrggbb: {css}");
        [
            u8::from_str_radix(&hex[0..2], 16).expect("red"),
            u8::from_str_radix(&hex[2..4], 16).expect("green"),
            u8::from_str_radix(&hex[4..6], 16).expect("blue"),
        ]
    }

    fn relative_luminance(css: &str) -> f64 {
        let [r, g, b] = css_rgb(css);
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

    fn contrast_ratio(first: &str, second: &str) -> f64 {
        let first = relative_luminance(first);
        let second = relative_luminance(second);
        let (lighter, darker) = if first >= second {
            (first, second)
        } else {
            (second, first)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    fn assert_not_black(fill: &str) {
        let [r, g, b] = css_rgb(fill);
        assert!(
            r.max(g).max(b) >= 16,
            "mindmap body fill {fill} is effectively black"
        );
    }

    #[test]
    fn every_generated_fallback_viewport_is_unclipped() {
        let theme = MermaidTheme::dark();
        for (name, source) in OTHER_FAMILIES
            .into_iter()
            .chain([("mindmap", MINDMAP_CLIPPING_FIXTURE)])
        {
            let svg = render(source, &theme).unwrap_or_else(|error| panic!("{name}: {error}"));
            let overflows = fallback_viewport_overflows(&svg);
            assert!(
                overflows.iter().all(|overflow| overflow == "visible"),
                "{name}: generated viewport still clipped: {overflows:?}"
            );
            if matches!(name, "class" | "er" | "mindmap") {
                assert!(
                    !overflows.is_empty(),
                    "{name}: real fixture produced no generated fallback viewports"
                );
            }
        }

        let with_edge_and_note = render(CLASS_WITH_EDGE_AND_NOTE, &theme).unwrap();
        let overflows = fallback_viewport_overflows(&with_edge_and_note);
        assert!(!overflows.is_empty(), "class note/edge labels not reached");
        assert!(
            overflows.iter().all(|overflow| overflow == "visible"),
            "class note/edge labels still clipped: {overflows:?}"
        );
    }

    #[test]
    fn postprocess_changes_only_fallback_overflow_mindmap_widths_and_edge_stroke() {
        let theme = MermaidTheme::dark();
        for (name, source) in OTHER_FAMILIES {
            let raw = render_inner(source, &theme)
                .unwrap_or_else(|error| panic!("{name}: raw render failed: {error}"));
            let production = render(source, &theme)
                .unwrap_or_else(|error| panic!("{name}: render failed: {error}"));
            let expected = expected_unclip(&raw);
            assert!(
                production == expected,
                "{name}: post-process changed more than the hidden->visible viewport overflow (raw {} bytes, production {} bytes, expected {} bytes)",
                raw.len(),
                production.len(),
                expected.len()
            );
            assert_only_viewport_overflow_changed(&raw, &production);
        }

        let raw = render_inner(MINDMAP_CLIPPING_FIXTURE, &theme).expect("raw mindmap render");
        assert!(
            raw.contains(".edge-depth-5{stroke-width:-1;}"),
            "mindmap fixture never reached a generated non-positive edge width"
        );
        let production = render(MINDMAP_CLIPPING_FIXTURE, &theme).expect("mindmap render");
        assert!(
            production.contains(".edge-depth-5{stroke-width:1;}"),
            "production did not floor the generated edge width"
        );
        let raw_clamped = clamp_mindmap_edge_widths(raw.clone());
        let stripped = strip_edge_stroke(&production);
        let expected = expected_unclip(&raw_clamped);
        assert!(
            stripped == expected,
            "mindmap post-process changed more than viewport overflow, edge width floor and edge stroke (raw {} bytes, stripped {} bytes, expected {} bytes)",
            raw.len(),
            stripped.len(),
            expected.len()
        );
        assert_only_viewport_overflow_changed(&raw_clamped, &stripped);
    }

    #[test]
    fn expected_unclip_preserves_already_visible_viewports() {
        let raw = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20"><svg xmlns="http://www.w3.org/2000/svg" x="0" y="0" width="10" height="10" viewBox="0 0 10 10" preserveAspectRatio="none" style="overflow:visible;opacity:1;">a</svg><svg xmlns="http://www.w3.org/2000/svg" x="0" y="0" width="10" height="10" viewBox="0 0 10 10" preserveAspectRatio="none" style="overflow:hidden;opacity:1;">b</svg></svg>"##;
        let expected = expected_unclip(raw);
        assert_eq!(
            expected.matches("overflow:visible").count(),
            2,
            "already-visible viewport was not preserved: {expected}"
        );
        assert!(
            !expected.contains("overflow:hidden"),
            "hidden viewport was not unclipped: {expected}"
        );
        assert_only_viewport_overflow_changed(raw, &expected);
    }

    #[test]
    fn mindmap_neutral_variables_are_exact_and_readable() {
        for theme in [MermaidTheme::light(), MermaidTheme::dark()] {
            let variables = variables(&theme);
            let get = |key: &str| {
                variables
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("missing {key}"))
            };
            let fills = [
                theme.primary.to_css(),
                theme.secondary.to_css(),
                theme.tertiary.to_css(),
            ];
            let line = theme.line.to_css();
            let label = theme.primary_text.to_css();
            for index in 0..12 {
                let fill = &fills[index % fills.len()];
                assert_eq!(get(&format!("cScale{index}")), fill.as_str());
                assert_eq!(get(&format!("cScalePeer{index}")), fill.as_str());
                assert_eq!(get(&format!("cScaleInv{index}")), line.as_str());
                assert_eq!(get(&format!("cScaleLabel{index}")), label.as_str());
                assert_not_black(fill.as_str());
                assert!(
                    contrast_ratio(fill.as_str(), label.as_str()) >= 4.5,
                    "theme dark={}: cScale{index} {fill} vs label {label}",
                    theme.dark_mode
                );
            }
            assert_eq!(get("git0"), fills[0].as_str());
            assert_eq!(get("gitBranchLabel0"), label.as_str());
            assert_eq!(get("nodeBorder"), theme.primary_border.to_css().as_str());
            assert_eq!(get("mainBkg"), fills[0].as_str());
            assert_eq!(get("scaleLabelColor"), label.as_str());
        }
    }

    #[test]
    fn mindmap_section_edges_are_painted_with_the_line_color() {
        let theme = MermaidTheme::dark();
        let svg = render(MINDMAP_CLIPPING_FIXTURE, &theme).expect("mindmap render");
        let line = theme.line.to_css();
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
                assert_eq!(
                    style.matches("stroke:").count(),
                    1,
                    "edge has duplicate stroke declarations: {tag}"
                );
                assert!(
                    style.contains(&format!("stroke:{line};")),
                    "edge not painted with line {line}: {tag}"
                );
            }
            rest = &tail[tag_len..];
        }
        assert!(edges > 0, "mindmap fixture produced no section edges");
    }

    #[test]
    fn edge_width_clamp_floors_only_root_scoped_generated_style_rules() {
        let svg = r##"<svg id="d" role="graphics-document document" aria-roledescription="mindmap"><style>#d .edge-depth-4{stroke-width:2;}#d .edge-depth-5{stroke-width:0;}#d .edge-depth-6{stroke-width:-4;}#other .edge-depth-5{stroke-width:-9;}.label.edge-depth-5{stroke-width:-7;}</style><text>.edge-depth-5{stroke-width:-1;}</text></svg>"##;
        let expected = svg
            .replace(
                "#d .edge-depth-5{stroke-width:0;}",
                "#d .edge-depth-5{stroke-width:1;}",
            )
            .replace(
                "#d .edge-depth-6{stroke-width:-4;}",
                "#d .edge-depth-6{stroke-width:1;}",
            );
        let clamped = clamp_mindmap_edge_widths(svg.to_owned());
        assert_eq!(clamped, expected);
        assert!(
            clamped.contains(r#"<text>.edge-depth-5{stroke-width:-1;}</text>"#),
            "label text was rewritten"
        );
        assert!(
            clamped.contains("#other .edge-depth-5{stroke-width:-9;}")
                && clamped.contains(".label.edge-depth-5{stroke-width:-7;}"),
            "unrelated stylesheet rules were rewritten"
        );

        // A mindmap with only positive generated widths is untouched.
        let positive = r##"<svg id="d" role="graphics-document document" aria-roledescription="mindmap"><style>#d .edge-depth-4{stroke-width:2;}</style></svg>"##;
        assert_eq!(
            clamp_mindmap_edge_widths(positive.to_owned()),
            positive,
            "positive generated widths were rewritten"
        );
    }

    #[test]
    fn edge_width_clamp_leaves_non_mindmap_output_byte_identical() {
        for svg in [
            r##"<svg role="graphics-document document" aria-roledescription="timeline"><style>#d .edge-depth-5{stroke-width:-1;}</style></svg>"##,
            r##"<svg role="graphics-document document" aria-roledescription="kanban"><style>#d .edge-depth-6{stroke-width:-4;}</style></svg>"##,
        ] {
            assert_eq!(
                clamp_mindmap_edge_widths(svg.to_owned()),
                svg,
                "non-mindmap output was rewritten"
            );
        }
    }

    #[test]
    fn class_post_process_changes_bytes_and_pixels() {
        let theme = MermaidTheme::dark();
        let raw = render_inner(CLASS_CLIPPING_FIXTURE, &theme).expect("raw class render");
        let production = render(CLASS_CLIPPING_FIXTURE, &theme).expect("class render");
        assert!(
            raw != production,
            "post-process left the class SVG byte-identical"
        );
        assert!(raw.contains("overflow:hidden"));
        assert!(production.contains("overflow:visible"));

        let raw_raster = crate::rasterize(&raw, 2.0).expect("raw class raster");
        let production_raster = crate::rasterize(&production, 2.0).expect("class raster");
        assert!(
            raw_raster.bgra != production_raster.bgra,
            "post-process changed the SVG but not the pixels ({}x{})",
            production_raster.width,
            production_raster.height
        );
    }
}
