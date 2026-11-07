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
    // contract here is that this function never unwinds into the caller.
    match catch_unwind(AssertUnwindSafe(|| render_inner(source, theme))) {
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
}
