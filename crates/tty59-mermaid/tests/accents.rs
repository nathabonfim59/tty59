//! Per-shape accent tests.
//!
//! The accent post-processor lives in `src/accents.rs`; it is included here
//! by path for direct/unit coverage while the public-path tests go through
//! the real `render_svg`/`render` hook. Everything goes through merman's real
//! SVG output, never a hand-written approximation.

#[allow(unused_imports)]
pub use tty59_mermaid::*;

#[path = "../src/accents.rs"]
mod accents;

use std::panic::{AssertUnwindSafe, catch_unwind};

const FLOWCHART: &str = "\
flowchart TD
    A[Source] --> B{Valid?}
    B -->|yes| C[Render diagram]
    B -->|no| D[Show syntax error]
    C --> E[Preview]
";

const FLOWCHART_SUBGRAPH: &str = "\
flowchart TD
    A[Start] --> B[Inside]
    subgraph Group
        B --> C[Also inside]
    end
    C --> D[After]
";

const SEQUENCE: &str = "\
sequenceDiagram
    participant C as Client
    participant S as Server
    C->>S: request
    activate S
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
    Running --> Idle: stop
    Running --> [*]
";

const ER: &str = "\
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE_ITEM : contains
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

const GANTT: &str = "\
gantt
    title Release plan
    dateFormat YYYY-MM-DD
    section Build
    Compile :a1, 2024-01-01, 3d
    Test :a2, after a1, 2d
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

const FLOWCHART_INLINE: &str = "flowchart TD; A[inline] --> B; B --> C; C --> D";

const FLOWCHART_FRONTMATTER: &str = "\
---
title: demo
---
%%{init: {\"theme\": \"base\"}}%%
flowchart TD
    A[first] --> B[second]
    B --> C[third]
";

const SEQUENCE_ALIAS: &str = "\
sequenceDiagram
    participant A as Z
    participant B as Other
    participant Z as Last
    A->>B: hello
    B->>Z: relay
";

const SEQUENCE_ALIAS_EDITED: &str = "\
sequenceDiagram
    participant A as Renamed label
    participant B as Other
    participant Z as Last
    A->>B: hello
    B->>Z: relay
";

const SEQUENCE_NESTED: &str = "\
sequenceDiagram
    participant C as Client
    participant S as Server
    C->>S: one
    activate S
    S->>S: inner
    activate S
    S-->>S: inner reply
    deactivate S
    deactivate S
";

const SEQUENCE_WIDE_ACTIVATION: &str = "\
%%{init: {\"sequence\": {\"activationWidth\": 400}}}%%
sequenceDiagram
    participant C as Client
    participant S as Server
    C->>S: one
    activate S
    S->>S: inner
    activate S
    S-->>S: inner reply
    deactivate S
    deactivate S
";

const SEQUENCE_INTERLEAVED: &str = "\
sequenceDiagram
    participant C as Client
    participant S as Server
    C->>S: c start
    activate C
    S->>S: s start
    activate S
    C->>S: c end
    deactivate C
    S-->>C: s end
    deactivate S
";

const SEQUENCE_UNMATCHED_OPEN: &str = "\
sequenceDiagram
    participant C as Client
    participant S as Server
    C->>S: one
    activate C
    S->>S: two
    activate S
    deactivate S
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
    ("gitGraph", GIT_GRAPH),
    ("xychart", XYCHART),
];

fn rgb(value: u32) -> Rgba {
    Rgba::rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn accents() -> Vec<Accent> {
    vec![
        Accent {
            fill: rgb(0x10_20_30),
            border: rgb(0x20_30_40),
            text: rgb(0x30_40_50),
        },
        Accent {
            fill: rgb(0x40_50_60),
            border: rgb(0x50_60_70),
            text: rgb(0x60_70_80),
        },
        Accent {
            fill: rgb(0x70_80_90),
            border: rgb(0x80_90_a0),
            text: rgb(0x90_a0_b0),
        },
    ]
}

fn theme(dark: bool) -> MermaidTheme {
    let mut theme = if dark {
        MermaidTheme::dark()
    } else {
        MermaidTheme::light()
    };
    theme.accents = accents();
    theme
}

fn neutral(dark: bool) -> MermaidTheme {
    if dark {
        MermaidTheme::dark()
    } else {
        MermaidTheme::light()
    }
}

/// Run merman, then the accent pass exactly as the planned hook does.
fn accented(source: &str, theme: &MermaidTheme) -> String {
    let svg = render_svg(source, &neutral(theme.dark_mode)).expect("neutral svg");
    accents::apply(&svg, source, theme).into_owned()
}

fn css(color: Rgba) -> String {
    color.to_css()
}

fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

fn root_svg_id(svg: &str) -> String {
    let start = svg.find("<svg").expect("svg root");
    let id = svg[start..].find("id=\"").expect("root id") + start + 4;
    let end = svg[id..].find('"').expect("id quote") + id;
    svg[id..end].to_string()
}

fn tag_name(tag: &str) -> &str {
    let inner = tag.trim_start_matches('<').trim_start_matches('/');
    let end = inner
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(inner.len());
    &inner[..end]
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn tag_at(svg: &str, at: usize) -> Option<(usize, usize)> {
    let start = at + svg[at..].find('<')?;
    if svg[start..].starts_with("<!--") {
        let end = svg[start..].find("-->")? + start + 3;
        return Some((start, end));
    }
    let mut index = start + 1;
    let mut quote = 0u8;
    let bytes = svg.as_bytes();
    while index < bytes.len() {
        let byte = bytes[index];
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = byte;
        } else if byte == b'>' {
            return Some((start, index + 1));
        }
        index += 1;
    }
    None
}

fn is_g_start_tag(tag: &str) -> bool {
    let Some(rest) = tag.strip_prefix("<g") else {
        return false;
    };
    rest.chars()
        .next()
        .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
}

fn is_self_closing_tag(tag: &str) -> bool {
    tag.trim_end()
        .trim_end_matches('>')
        .trim_end()
        .ends_with('/')
}

/// Start tag and byte range of the `<g>` whose id belongs to `suffix` (a
/// source id) or whose `data-id` equals `data_id`. Nesting counts only real
/// (non-self-closing) `<g>` elements; merman emits many `<g .../>`
/// placeholders, and they must not extend a shape's range.
fn group_bounds(svg: &str, suffix: &str, data_id: Option<&str>) -> (usize, usize, usize) {
    let root = root_svg_id(svg);
    let mut from = 0;
    while let Some(offset) = svg[from..].find("<g") {
        let start = from + offset;
        let after = svg.as_bytes().get(start + 2).copied().unwrap_or(b' ');
        if !(after.is_ascii_whitespace() || after == b'>' || after == b'/') {
            from = start + 2;
            continue;
        }
        let Some((_, header_end)) = tag_at(svg, start) else {
            break;
        };
        let header = &svg[start..header_end];
        let matched = if let Some(wanted) = data_id {
            attr(header, "data-id") == Some(wanted)
        } else if let Some(id) = attr(header, "id") {
            let expected = format!("{root}-{suffix}");
            id == expected
                || id.starts_with(&format!("{expected}-"))
                || (id.starts_with(&root) && id.contains(&format!("-{suffix}-")))
        } else {
            false
        };
        if matched {
            let mut depth = 0usize;
            let mut cursor = start;
            while let Some((tag_start, tag_end)) = tag_at(svg, cursor) {
                let tag = &svg[tag_start..tag_end];
                if is_g_start_tag(tag) {
                    if !is_self_closing_tag(tag) {
                        depth += 1;
                    }
                } else if tag.starts_with("</g") {
                    depth = depth
                        .checked_sub(1)
                        .expect("unbalanced closing group in SVG");
                    if depth == 0 {
                        return (start, header_end, tag_end);
                    }
                }
                cursor = tag_end;
            }
            panic!("unterminated group for {suffix} / {data_id:?}");
        }
        from = header_end;
    }
    panic!("no group for {suffix} / {data_id:?}");
}

fn style_declaration(tag: &str, property: &str) -> Option<String> {
    let style_start = tag.find("style=\"")? + 7;
    let style_end = tag[style_start..].find('"')? + style_start;
    let style = &tag[style_start..style_end];
    let mut found = None;
    for declaration in style.split(';') {
        let Some((key, value)) = declaration.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(property) {
            // CSS: the last declaration wins, and `!important` is part of the
            // declaration rather than its value.
            found = Some(
                value
                    .trim()
                    .trim_end_matches("!important")
                    .trim()
                    .to_string(),
            );
        }
    }
    found
}

/// Effective paint: the winning inline style declaration, else the
/// presentation attribute. This is what usvg/resvg actually draws.
fn effective_paint(tag: &str, property: &str) -> Option<String> {
    style_declaration(tag, property).or_else(|| attr(tag, property).map(str::to_string))
}

fn shape_tags(svg: &str, start: usize, end: usize) -> Vec<&str> {
    let mut tags = Vec::new();
    let mut from = start;
    while from < end {
        let Some((tag_start, tag_end)) = tag_at(svg, from) else {
            break;
        };
        if tag_start >= end {
            break;
        }
        tags.push(&svg[tag_start..tag_end.min(end)]);
        from = tag_end;
    }
    tags
}

/// First paint declaration inside a shape group: its body fill.
fn group_fill(svg: &str, suffix: &str, data_id: Option<&str>) -> String {
    let (_, header_end, end) = group_bounds(svg, suffix, data_id);
    for tag in shape_tags(svg, header_end, end) {
        if tag.starts_with("</") {
            continue;
        }
        if matches!(
            tag_name(tag).to_ascii_lowercase().as_str(),
            "rect" | "circle" | "ellipse" | "polygon" | "polyline" | "path"
        ) && let Some(fill) = effective_paint(tag, "fill")
            && !fill.eq_ignore_ascii_case("none")
        {
            return fill;
        }
    }
    panic!("no shape fill for {suffix} / {data_id:?}");
}

fn element_bounds_by_id(svg: &str, id: &str) -> Option<(usize, usize)> {
    let needle = format!("id=\"{id}\"");
    let hit = svg.find(&needle)?;
    let start = svg[..hit].rfind('<')?;
    let mut depth = 0usize;
    let mut cursor = start;
    while let Some((tag_start, tag_end)) = tag_at(svg, cursor) {
        let tag = &svg[tag_start..tag_end];
        if is_g_start_tag(tag) {
            if !is_self_closing_tag(tag) {
                depth += 1;
            }
        } else if tag.starts_with("</g") {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some((start, tag_end));
            }
        }
        cursor = tag_end;
    }
    None
}

fn text_paints_in(svg: &str, start: usize, end: usize) -> Vec<String> {
    shape_tags(svg, start, end)
        .into_iter()
        .filter(|tag| tag_name(tag).eq_ignore_ascii_case("text"))
        .filter_map(|tag| effective_paint(tag, "fill"))
        .collect()
}

fn activation_rects(svg: &str) -> Vec<&str> {
    let mut tags = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = svg[cursor..].find("class=\"activation") {
        let hit = cursor + offset;
        let start = svg[..hit].rfind('<').expect("activation tag start");
        let end = svg[hit..].find('>').expect("activation tag end") + hit;
        tags.push(&svg[start..=end]);
        cursor = end;
    }
    tags
}

fn hrefs_in(svg: &str, start: usize, end: usize) -> Vec<String> {
    shape_tags(svg, start, end)
        .into_iter()
        .filter(|tag| tag_name(tag).eq_ignore_ascii_case("use"))
        .filter_map(|tag| {
            attr(tag, "href")
                .or_else(|| attr(tag, "xlink:href"))
                .map(|href| href.trim_start_matches('#').to_string())
        })
        .collect()
}

fn accent_for_fill<'a>(fill: &str, palette: &'a [Accent]) -> &'a Accent {
    palette
        .iter()
        .find(|accent| css(accent.fill) == fill)
        .unwrap_or_else(|| panic!("shape fill {fill} is not a palette accent"))
}

/// For one shape: prove its outline is accent.border and every one of its
/// labels resolves to that shape's exact accent.text through its own `<use>`
/// href → `<defs>` definition (or native text when no fallback is used).
/// Returns true when the shape actually owns text.
fn assert_shape_text(svg: &str, suffix: &str, data_id: Option<&str>, palette: &[Accent]) -> bool {
    let (_, header_end, end) = group_bounds(svg, suffix, data_id);
    let fill = group_fill(svg, suffix, data_id);
    let accent = accent_for_fill(&fill, palette);
    let text = css(accent.text);
    assert!(
        svg[header_end..end].contains(&format!("stroke:{}!important", css(accent.border))),
        "{}: outline is not accent.border",
        data_id.unwrap_or(suffix)
    );

    let mut textful = false;
    for paint in text_paints_in(svg, header_end, end) {
        assert_eq!(
            paint,
            text,
            "{}: native label used the wrong accent.text",
            data_id.unwrap_or(suffix)
        );
        textful = true;
    }

    for def_id in hrefs_in(svg, header_end, end) {
        let Some((def_start, def_end)) = element_bounds_by_id(svg, &def_id) else {
            panic!(
                "{}: linked definition {def_id} not found",
                data_id.unwrap_or(suffix)
            );
        };
        let paints = text_paints_in(svg, def_start, def_end);
        assert!(
            !paints.is_empty(),
            "{}: fallback definition {def_id} has no text",
            data_id.unwrap_or(suffix)
        );
        for paint in &paints {
            assert_eq!(
                paint,
                &text,
                "{}: linked fallback text must use the pair's exact accent.text",
                data_id.unwrap_or(suffix)
            );
        }
        textful = true;
    }
    textful
}

#[test]
fn self_closing_group_placeholders_do_not_extend_shape_ranges() {
    let svg = r##"<svg id="d"><g id="d-A"><g class="annotation-group text"/><rect fill="#111"/></g><g id="d-B"><text>B</text></g></svg>"##;
    let (_, _, end) = group_bounds(svg, "A", None);
    assert!(end < svg.find("id=\"d-B\"").expect("B group"));
    assert!(!svg[..end].contains("d-B"));

    let defs = r##"<svg id="d"><defs><g id="def-1"><g class="empty"/><text fill="#123">A</text></g><g id="def-2"><text fill="#456">B</text></g></defs></svg>"##;
    let (start, end) = element_bounds_by_id(defs, "def-1").expect("definition bounds");
    assert!(!defs[start..end].contains("def-2"));
}

#[test]
fn empty_palette_returns_the_input_byte_for_byte() {
    let source = FLOWCHART;
    let svg = render_svg(source, &neutral(false)).expect("neutral svg");
    let result = accents::apply(&svg, source, &neutral(false));
    assert!(matches!(&result, std::borrow::Cow::Borrowed(_)));
    assert_eq!(result.as_ref(), svg);
    for accent in accents() {
        assert!(!svg.contains(&css(accent.fill)));
    }
}

#[test]
fn supported_families_color_actual_shape_fill_stroke_and_text() {
    for dark in [false, true] {
        let theme = theme(dark);
        for (name, source) in SUPPORTED {
            let svg = accented(source, &theme);
            let palette = accents();

            let mut distinct = 0;
            for accent in &palette {
                if svg.contains(&format!("fill:{}", css(accent.fill))) {
                    distinct += 1;
                }
            }
            assert!(
                distinct >= 2,
                "{name} ({dark}): fewer than two distinctly colored shapes"
            );
        }
    }
}

#[test]
fn empty_theme_and_accented_theme_differ_for_every_supported_family() {
    for (name, source) in SUPPORTED {
        let plain = render_svg(source, &neutral(false)).expect("neutral");
        let colored = accented(source, &theme(false));
        assert_ne!(plain, colored, "{name}: accents changed nothing");
    }
}

#[test]
fn unsupported_families_are_byte_identical() {
    for (name, source) in UNSUPPORTED {
        let plain = render_svg(source, &neutral(false)).expect("neutral");
        for dark in [false, true] {
            let result = accents::apply(&plain, source, &theme(dark));
            assert!(
                matches!(&result, std::borrow::Cow::Borrowed(_)),
                "{name}: unsupported family was rewritten"
            );
            assert_eq!(result.as_ref(), plain, "{name}: unsupported SVG changed");
        }
    }
}

#[test]
fn output_is_deterministic() {
    for (name, source) in SUPPORTED {
        let first = accented(source, &theme(false));
        let second = accented(source, &theme(false));
        assert_eq!(first, second, "{name}: accent output is not deterministic");
    }
}

#[test]
fn palette_wraps_and_single_accent_colors_every_shape() {
    let mut wrapped = neutral(false);
    wrapped.accents = vec![accents()[0], accents()[1]];
    let svg = accented(FLOWCHART, &wrapped);
    assert_eq!(
        group_fill(&svg, "A", None),
        group_fill(&svg, "E", None),
        "first and fifth nodes must share the wrapped palette entry"
    );

    let mut single = neutral(false);
    single.accents = vec![accents()[0]];
    let svg = accented(FLOWCHART, &single);
    for id in ["A", "B", "C", "D", "E"] {
        assert_eq!(
            group_fill(&svg, id, None),
            css(accents()[0].fill),
            "single accent did not reach {id}"
        );
    }
}

#[test]
fn accents_are_assigned_by_source_first_appearance_not_document_order() {
    // Source order is Z, Y, X; document/layout order is deliberately the
    // reverse in the SVG walk by listing X and Y before Z in the edge list.
    let source = "\
flowchart TD
    Z[zulu] --> Y[yankee]
    X[xray] --> Z
";
    let theme = theme(false);
    let svg = accented(source, &theme);
    let palette = accents();
    assert_eq!(group_fill(&svg, "Z", None), css(palette[0].fill));
    assert_eq!(group_fill(&svg, "Y", None), css(palette[1].fill));
    assert_eq!(group_fill(&svg, "X", None), css(palette[2].fill));
}

#[test]
fn label_edits_do_not_recolor_other_shapes() {
    let short = "flowchart TD\n  A[one] --> B[two]\n";
    let long = "flowchart TD\n  A[a considerably longer label] --> B[another long label]\n";
    let short_svg = accented(short, &theme(false));
    let long_svg = accented(long, &theme(false));
    let palette = accents();

    for (index, id) in ["A", "B"].iter().enumerate() {
        assert_eq!(group_fill(&short_svg, id, None), css(palette[index].fill));
        assert_eq!(
            group_fill(&long_svg, id, None),
            css(palette[index].fill),
            "label edit recolored {id}"
        );
    }
}

#[test]
fn sequence_participants_share_copies_and_activations_follow_identity() {
    let svg = accented(SEQUENCE, &theme(false));
    let palette = accents();

    let client = group_fill(&svg, "C", Some("C"));
    let server = group_fill(&svg, "S", Some("S"));
    assert_eq!(client, css(palette[0].fill));
    assert_eq!(server, css(palette[1].fill));
    assert_ne!(client, server);

    // Top and bottom actor copies both carry the participant accent.
    assert!(
        occurrences(&svg, &format!("fill:{}", client)) >= 2,
        "client footer copy was not accented: {svg}"
    );
    assert!(
        occurrences(&svg, &format!("fill:{}", server)) >= 2,
        "server footer copy was not accented: {svg}"
    );

    // The activation rect is in the S lifeline column and must carry S's
    // fill, not C's.
    let activation = svg.find("class=\"activation0\"").expect("activation rect");
    let start = svg[..activation].rfind('<').expect("tag start");
    let end = svg[activation..].find('>').expect("tag end") + activation;
    let tag = &svg[start..=end];
    assert_eq!(
        effective_paint(tag, "fill").as_deref(),
        Some(server.as_str()),
        "activation did not inherit its participant accent: {tag}"
    );

    // Lifelines stay line-colored.
    let lifeline = svg.find("class=\"actor-line").expect("lifeline");
    let start = svg[..lifeline].rfind('<').expect("lifeline tag start");
    let end = svg[lifeline..].find('>').expect("lifeline tag end") + lifeline;
    let lifeline = &svg[start..=end];
    for accent in &palette {
        let accent_border = css(accent.border);
        assert!(
            effective_paint(lifeline, "stroke").as_deref() != Some(accent_border.as_str()),
            "lifeline was recolored: {lifeline}"
        );
    }
}

#[test]
fn deep_nested_activations_stay_with_their_participant() {
    let mut source = String::from(
        "sequenceDiagram\n    participant C as Client\n    participant S as Server\n    C->>S: one\n",
    );
    for index in 0..15 {
        source.push_str(&format!("    S->>S: inner {index}\n    activate S\n"));
    }
    for _ in 0..15 {
        source.push_str("    deactivate S\n");
    }

    let svg = accented(&source, &theme(false));
    let palette = accents();
    let server = group_fill(&svg, "S", Some("S"));
    assert_eq!(server, css(palette[1].fill));

    let rects = activation_rects(&svg);
    assert_eq!(rects.len(), 15, "expected all 15 nested S activations");
    for tag in rects {
        assert_eq!(
            effective_paint(tag, "fill").as_deref(),
            Some(server.as_str()),
            "deep nested activation left S's accent: {tag}"
        );
    }
}

#[test]
fn wide_activation_width_does_not_cross_actors() {
    let svg = accented(SEQUENCE_WIDE_ACTIVATION, &theme(false));
    let palette = accents();
    let client = group_fill(&svg, "C", Some("C"));
    let server = group_fill(&svg, "S", Some("S"));
    assert_eq!(client, css(palette[0].fill));
    assert_eq!(server, css(palette[1].fill));
    assert_ne!(client, server);

    let rects = activation_rects(&svg);
    assert_eq!(rects.len(), 2, "expected two nested S activations");
    for tag in rects {
        assert_eq!(
            effective_paint(tag, "fill").as_deref(),
            Some(server.as_str()),
            "wide activation crossed to another actor: {tag}"
        );
    }
}

#[test]
fn interleaved_activations_keep_start_order_and_owner() {
    let svg = accented(SEQUENCE_INTERLEAVED, &theme(false));
    let palette = accents();
    let client = group_fill(&svg, "C", Some("C"));
    let server = group_fill(&svg, "S", Some("S"));
    assert_eq!(client, css(palette[0].fill));
    assert_eq!(server, css(palette[1].fill));

    let rects = activation_rects(&svg);
    assert_eq!(rects.len(), 2, "expected C then S activations");
    assert_eq!(
        effective_paint(rects[0], "fill").as_deref(),
        Some(client.as_str()),
        "first completed activation must belong to C: {}",
        rects[0]
    );
    assert_eq!(
        effective_paint(rects[1], "fill").as_deref(),
        Some(server.as_str()),
        "second completed activation must belong to S: {}",
        rects[1]
    );
}

#[test]
fn unmatched_open_activation_does_not_shift_later_owners() {
    let svg = accented(SEQUENCE_UNMATCHED_OPEN, &theme(false));
    let palette = accents();
    let server = group_fill(&svg, "S", Some("S"));
    assert_eq!(server, css(palette[1].fill));

    let rects = activation_rects(&svg);
    assert_eq!(rects.len(), 1, "only the completed S activation is drawn");
    assert_eq!(
        effective_paint(rects[0], "fill").as_deref(),
        Some(server.as_str()),
        "unmatched C start shifted the S activation owner: {}",
        rects[0]
    );
}

#[test]
fn nested_activations_stay_with_their_participant() {
    let svg = accented(SEQUENCE_NESTED, &theme(false));
    let palette = accents();
    let server = group_fill(&svg, "S", Some("S"));
    let client = group_fill(&svg, "C", Some("C"));
    assert_eq!(server, css(palette[1].fill));
    assert_eq!(client, css(palette[0].fill));

    let rects = activation_rects(&svg);
    assert_eq!(rects.len(), 2, "expected both nested S activations");
    for tag in rects {
        assert_eq!(
            effective_paint(tag, "fill").as_deref(),
            Some(server.as_str()),
            "nested activation left S's accent: {tag}"
        );
    }
}

const FLOWCHART_TEXT_SHAPES: &[(&str, Option<&str>)] = &[
    ("A", None),
    ("B", None),
    ("C", None),
    ("D", None),
    ("E", None),
];
const SEQUENCE_TEXT_SHAPES: &[(&str, Option<&str>)] = &[("C", Some("C")), ("S", Some("S"))];
const CLASS_TEXT_SHAPES: &[(&str, Option<&str>)] =
    &[("Shape", None), ("Circle", None), ("Square", None)];
const STATE_TEXT_SHAPES: &[(&str, Option<&str>)] = &[("Idle", None), ("Running", None)];
const ER_TEXT_SHAPES: &[(&str, Option<&str>)] =
    &[("CUSTOMER", None), ("ORDER", None), ("LINE_ITEM", None)];
const MINDMAP_TEXT_SHAPES: &[(&str, Option<&str>)] = &[
    ("node_0", None),
    ("node_1", None),
    ("node_2", None),
    ("node_3", None),
    ("node_4", None),
    ("node_5", None),
    ("node_6", None),
];
const TEXT_SHAPES: [(&str, &str, &[(&str, Option<&str>)]); 6] = [
    ("flowchart", FLOWCHART, FLOWCHART_TEXT_SHAPES),
    ("sequence", SEQUENCE, SEQUENCE_TEXT_SHAPES),
    ("class", CLASS, CLASS_TEXT_SHAPES),
    ("state", STATE, STATE_TEXT_SHAPES),
    ("er", ER, ER_TEXT_SHAPES),
    ("mindmap", MINDMAP, MINDMAP_TEXT_SHAPES),
];

#[test]
fn every_shape_uses_its_pair_exact_text_and_border() {
    for dark in [false, true] {
        let palette = accents();
        let theme = theme(dark);
        for &(name, source, shapes) in &TEXT_SHAPES {
            let svg = accented(source, &theme);
            let mut textful = 0usize;
            for &(suffix, data_id) in shapes {
                assert!(
                    assert_shape_text(&svg, suffix, data_id, &palette),
                    "{name} ({dark}): {suffix} has no paired label text"
                );
                textful += 1;
            }
            assert_eq!(
                textful,
                shapes.len(),
                "{name} ({dark}): every listed shape must be checked"
            );
        }
    }
}

#[test]
fn state_pseudostates_stay_neutral_and_do_not_consume_palette() {
    let svg = accented(STATE, &theme(false));
    let palette = accents();
    assert_eq!(group_fill(&svg, "Idle", None), css(palette[0].fill));
    assert_eq!(group_fill(&svg, "Running", None), css(palette[1].fill));
    for suffix in ["root_start", "root_end"] {
        let (_, header_end, end) = group_bounds(&svg, suffix, None);
        let group = &svg[header_end..end];
        for accent in &palette {
            assert!(
                !group.contains(&format!("fill:{}", css(accent.fill))),
                "state pseudostate {suffix} consumed an accent"
            );
            assert!(
                !group.contains(&format!("fill:{}!important", css(accent.text))),
                "state pseudostate {suffix} got accent text"
            );
        }
    }
}

#[test]
fn inline_source_header_keeps_first_appearance_order() {
    let svg = accented(FLOWCHART_INLINE, &theme(false));
    let palette = accents();
    for (index, id) in ["A", "B", "C", "D"].iter().enumerate() {
        assert_eq!(
            group_fill(&svg, id, None),
            css(palette[index % palette.len()].fill),
            "inline header reordered {id}"
        );
    }
}

#[test]
fn sequence_alias_text_does_not_outrank_a_participant() {
    let first = accented(SEQUENCE_ALIAS, &theme(false));
    let edited = accented(SEQUENCE_ALIAS_EDITED, &theme(false));
    let palette = accents();
    assert_eq!(group_fill(&first, "A", Some("A")), css(palette[0].fill));
    assert_eq!(group_fill(&first, "B", Some("B")), css(palette[1].fill));
    assert_eq!(group_fill(&first, "Z", Some("Z")), css(palette[2].fill));
    // Editing A's display label must not recolor any participant.
    assert_eq!(group_fill(&edited, "A", Some("A")), css(palette[0].fill));
    assert_eq!(group_fill(&edited, "B", Some("B")), css(palette[1].fill));
    assert_eq!(group_fill(&edited, "Z", Some("Z")), css(palette[2].fill));
}

#[test]
fn frontmatter_and_directives_do_not_affect_order() {
    let svg = accented(FLOWCHART_FRONTMATTER, &theme(false));
    let palette = accents();
    assert_eq!(group_fill(&svg, "A", None), css(palette[0].fill));
    assert_eq!(group_fill(&svg, "B", None), css(palette[1].fill));
    assert_eq!(group_fill(&svg, "C", None), css(palette[2].fill));
}

#[test]
fn clusters_and_edges_stay_neutral() {
    let svg = accented(FLOWCHART_SUBGRAPH, &theme(false));
    let palette = accents();

    let cluster_at = svg
        .find("class=\"cluster ")
        .or_else(|| svg.find("class=\"cluster\""))
        .expect("cluster group");
    let cluster_start = svg[..cluster_at].rfind('<').expect("cluster tag start");
    let mut cluster_depth = 0usize;
    let mut cluster_limit = svg.len();
    let mut cursor = cluster_start;
    while let Some((tag_start, tag_end)) = tag_at(&svg, cursor) {
        let tag = &svg[tag_start..tag_end];
        if tag.starts_with("<g") {
            cluster_depth += 1;
        } else if tag.starts_with("</g") {
            cluster_depth -= 1;
            if cluster_depth == 0 {
                cluster_limit = tag_end;
                break;
            }
        }
        cursor = tag_end;
    }
    let cluster = &svg[cluster_start..cluster_limit];
    let Some(body) = cluster
        .find("<rect")
        .map(|at| {
            let start = at;
            let end = cluster[start..].find('>').expect("rect end") + start;
            &cluster[start..=end]
        })
        .or_else(|| {
            cluster.find("<path").map(|at| {
                let start = at;
                let end = cluster[start..].find('>').expect("path end") + start;
                &cluster[start..=end]
            })
        })
    else {
        panic!("cluster has no body");
    };
    let cluster_fill = effective_paint(body, "fill");
    let cluster_stroke = effective_paint(body, "stroke");
    for accent in &palette {
        let accent_fill = css(accent.fill);
        let accent_border = css(accent.border);
        assert_ne!(
            cluster_fill.as_deref(),
            Some(accent_fill.as_str()),
            "cluster fill was accented: {body}"
        );
        assert_ne!(
            cluster_stroke.as_deref(),
            Some(accent_border.as_str()),
            "cluster stroke was accented: {body}"
        );
    }

    let edge = svg
        .find("class=\"edge-thickness-normal")
        .or_else(|| svg.find("flowchart-link"))
        .expect("an edge path");
    let start = svg[..edge].rfind('<').expect("edge tag start");
    let end = svg[edge..].find('>').expect("edge tag end") + edge;
    let tag = &svg[start..=end];
    for accent in &palette {
        let accent_fill = css(accent.fill);
        let accent_border = css(accent.border);
        assert!(
            effective_paint(tag, "fill").as_deref() != Some(accent_fill.as_str()),
            "edge fill was recolored: {tag}"
        );
        assert!(
            effective_paint(tag, "stroke").as_deref() != Some(accent_border.as_str()),
            "edge stroke was recolored: {tag}"
        );
    }
}

#[test]
fn fallback_defs_text_uses_the_exact_accent_text_color() {
    let svg = accented(FLOWCHART, &theme(false));
    let palette = accents();
    let defs_start = svg.find("<defs>").expect("fallback defs");
    let defs = &svg[defs_start..];
    for accent in &palette {
        assert!(
            occurrences(defs, &format!("fill:{}", css(accent.text))) > 0,
            "linked fallback text missed accent.text {}",
            css(accent.text)
        );
    }
    // The linked definitions must still be referenced.
    assert!(svg.contains("<use "));
    assert!(svg.contains("merman-foreignobject-fallback-text"));
}

#[test]
fn accented_filters_keep_srgb_palette_pixels() {
    let svg = accented(SEQUENCE, &theme(false));
    let mut found = 0usize;
    let mut cursor = 0;
    while let Some(offset) = svg[cursor..].find("<filter") {
        let start = cursor + offset;
        let (_, end) = tag_at(&svg, start).expect("filter tag end");
        let tag = &svg[start..end];
        assert_eq!(
            attr(tag, "color-interpolation-filters"),
            Some("sRGB"),
            "filter must preserve exact accent pixels: {tag}"
        );
        found += 1;
        cursor = end;
    }
    assert!(
        found > 0,
        "sequence fixture must contain drop-shadow filters"
    );
}

#[test]
fn garbage_sources_never_panic_with_accents() {
    let accented_theme = theme(false);
    let long = "x".repeat(2048);
    let sources = [
        "flowchart TD\n A[unterminated",
        "sequenceDiagram\n ->>",
        "classDiagram\n class {",
        "stateDiagram-v2\n -->",
        "erDiagram\n ||--o{",
        "mindmap\n  (((",
        long.as_str(),
    ];
    for source in sources {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            if let Ok(svg) = render_svg(source, &theme(false)) {
                let _ = accents::apply(&svg, source, &accented_theme);
            }
        }));
        assert!(outcome.is_ok(), "panicked on {source:?}");
    }
}

#[test]
fn mindmap_branches_are_colored_in_source_order() {
    let svg = accented(MINDMAP, &theme(false));
    let palette = accents();
    assert_eq!(group_fill(&svg, "node_0", None), css(palette[0].fill));
    assert_eq!(group_fill(&svg, "node_1", None), css(palette[1].fill));
    assert_eq!(group_fill(&svg, "node_2", None), css(palette[2].fill));
}

#[test]
fn public_empty_palette_is_byte_identical_svg_and_raster() {
    for dark in [false, true] {
        let plain = neutral(dark);
        let mut empty = plain.clone();
        empty.accents = Vec::new();
        let plain_svg = render_svg(FLOWCHART, &plain).expect("plain svg");
        let empty_svg = render_svg(FLOWCHART, &empty).expect("empty-accents svg");
        assert_eq!(plain_svg, empty_svg, "empty palette changed the SVG");
        let plain_raster = tty59_mermaid::render(FLOWCHART, &plain, 1.0).expect("plain raster");
        let empty_raster = tty59_mermaid::render(FLOWCHART, &empty, 1.0).expect("empty raster");
        assert_eq!(
            plain_raster, empty_raster,
            "empty palette changed the raster"
        );
    }
}

#[test]
fn public_unsupported_families_are_byte_identical_svg_and_raster() {
    for (name, source) in UNSUPPORTED {
        for dark in [false, true] {
            let plain = neutral(dark);
            let colored = theme(dark);
            let plain_svg = render_svg(source, &plain).expect("plain svg");
            let colored_svg = render_svg(source, &colored).expect("accented svg");
            assert_eq!(plain_svg, colored_svg, "{name} ({dark}): SVG changed");
            let plain_raster = tty59_mermaid::render(source, &plain, 1.0).expect("plain raster");
            let colored_raster =
                tty59_mermaid::render(source, &colored, 1.0).expect("accented raster");
            assert_eq!(
                plain_raster, colored_raster,
                "{name} ({dark}): raster changed"
            );
        }
    }
}

fn contains_opaque_pixel(raster: &tty59_mermaid::Raster, color: Rgba) -> bool {
    assert_eq!(color.a, 255, "gallery palette must be opaque");
    let expected = [color.b, color.g, color.r, color.a];
    raster
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .any(|pixel| pixel == expected)
}

fn used_accents(svg: &str, shapes: &[(&str, Option<&str>)], palette: &[Accent]) -> Vec<Accent> {
    let mut used = Vec::new();
    for &(suffix, data_id) in shapes {
        let fill = group_fill(svg, suffix, data_id);
        let accent = *accent_for_fill(&fill, palette);
        if !used.iter().any(|known: &Accent| known.fill == accent.fill) {
            used.push(accent);
        }
    }
    used
}

/// These public-path assertions only become meaningful once the coordinated
/// `src/svg.rs` hook lands. They prove the GUI path (`render_svg`/`render`)
/// carries the accents for every supported family, light and dark.
#[test]
fn public_render_svg_applies_accents_per_family() {
    let palette = accents();
    for dark in [false, true] {
        let theme = theme(dark);
        for &(name, source, shapes) in &TEXT_SHAPES {
            let svg = render_svg(source, &theme)
                .unwrap_or_else(|error| panic!("{name} ({dark}) public render_svg: {error}"));
            let used = used_accents(&svg, shapes, &palette);
            assert!(
                used.len() >= 2,
                "{name} ({dark}): public SVG has fewer than two body colors"
            );
            for &(suffix, data_id) in shapes {
                assert!(
                    assert_shape_text(&svg, suffix, data_id, &palette),
                    "{name} ({dark}): public SVG missed {suffix}'s pair text"
                );
            }
        }
    }
}

#[test]
fn public_render_raster_changes_pixels_per_family() {
    let palette = accents();
    for (name, source) in SUPPORTED {
        for dark in [false, true] {
            let colored = theme(dark);
            let plain = neutral(dark);
            let shape_svg = render_svg(source, &colored)
                .unwrap_or_else(|error| panic!("{name} ({dark}) public render_svg: {error}"));
            let shapes = TEXT_SHAPES
                .iter()
                .find(|(fixture, _, _)| *fixture == name)
                .map(|(_, _, shapes)| *shapes)
                .unwrap_or(&[]);
            let used = used_accents(&shape_svg, shapes, &palette);
            assert!(
                used.len() >= 2,
                "{name} ({dark}): expected at least two used body colors"
            );

            let first = tty59_mermaid::render(source, &colored, 2.0)
                .unwrap_or_else(|error| panic!("{name} ({dark}) accented render: {error}"));
            let second = tty59_mermaid::render(source, &colored, 2.0)
                .unwrap_or_else(|error| panic!("{name} ({dark}) repeat render: {error}"));
            assert!(
                first == second,
                "{name} ({dark}): public raster is not deterministic"
            );
            assert!(
                (first.scale - 2.0).abs() < f32::EPSILON,
                "{name} ({dark}): scale changed"
            );
            assert!(
                first.width > 0 && first.height > 0,
                "{name} ({dark}): empty raster"
            );

            let plain_raster = tty59_mermaid::render(source, &plain, 2.0)
                .unwrap_or_else(|error| panic!("{name} ({dark}) neutral render: {error}"));
            assert!(
                first != plain_raster,
                "{name} ({dark}): public raster ignored accents"
            );
            assert_eq!(
                (first.width, first.height),
                (plain_raster.width, plain_raster.height),
                "{name} ({dark}): accents changed raster dimensions"
            );

            for accent in &used {
                assert!(
                    contains_opaque_pixel(&first, accent.fill),
                    "{name} ({dark}): body color {} missing from raster",
                    css(accent.fill)
                );
                assert!(
                    contains_opaque_pixel(&first, accent.text),
                    "{name} ({dark}): exact text color {} missing from raster",
                    css(accent.text)
                );
            }
        }
    }
}
