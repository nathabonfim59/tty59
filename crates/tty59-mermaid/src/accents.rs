//! Per-shape accent colors for merman's SVG output.
//!
//! This is a bounded, quote-aware post-processor over the SVG merman already
//! produced. It never parses or draws Mermaid itself; it:
//!
//! 1. finds the diagram family from merman's own SVG root
//!    `aria-roledescription`, not from re-lexing the Mermaid source,
//! 2. recovers each shape's *source* identity from the SVG ids/attributes,
//! 3. orders those identities with merman's typed semantic model (nodes,
//!    actors, classes, entities in source-creation order; a label edit cannot
//!    move them; the SVG walk is only a fallback),
//! 4. rewrites the actual paint declarations of the shape bodies, their
//!    outlines and their labels, including the `<text>` nodes merman's
//!    `foreignObject` fallback keeps in `<defs>` and links with `<use>`.
//!
//! Clusters, edges, arrows, lifelines and unsupported families are left
//! alone. An empty accent list is handled by the caller before this module
//! runs, so the neutral output stays byte-for-byte identical.
//!
//! Written from scratch against the public merman SVG (MIT OR Apache-2.0).
//! Never copy from Zed's GPL `mermaid_render` crate.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use merman::diagrams::sequence::SequenceMessageKind;
use merman::{Engine, ParseOptions, RenderSemanticModel};

use crate::{Accent, MermaidTheme};

/// Diagram families this module knows how to color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Flowchart,
    Sequence,
    Class,
    State,
    Er,
    Mindmap,
    Unsupported,
}

/// Entry point used by the SVG hook: returns `svg` untouched unless there is
/// at least one accent and the family has per-shape identity.
pub(crate) fn apply<'a>(svg: &'a str, source: &str, theme: &MermaidTheme) -> Cow<'a, str> {
    if theme.accents.is_empty() {
        return Cow::Borrowed(svg);
    }
    let Some(diagram_id) = root_diagram_id(svg) else {
        return Cow::Borrowed(svg);
    };
    let family = detect_family(svg);
    if family == Family::Unsupported {
        return Cow::Borrowed(svg);
    }

    let (groups, def_identities) = collect_shapes(svg, family, &diagram_id);
    if groups.is_empty() {
        return Cow::Borrowed(svg);
    }

    let model = model_order(source);
    let groups: Vec<ShapeGroup> = groups
        .into_iter()
        .filter(|group| !model.excluded.contains(&group.identity))
        .collect();
    let identities: HashSet<String> = groups
        .iter()
        .map(|group| group.identity.clone())
        .filter(|identity| !model.excluded.contains(identity))
        .collect();
    let order = shape_order(&identities, &model.order, &groups);
    let accents: HashMap<String, Accent> = order
        .into_iter()
        .enumerate()
        .map(|(index, identity)| (identity, theme.accents[index % theme.accents.len()]))
        .collect();

    let mut group_starts: HashMap<usize, Accent> = HashMap::with_capacity(groups.len());
    for group in &groups {
        if let Some(&accent) = accents.get(&group.identity) {
            group_starts.insert(group.start, accent);
        }
    }

    let def_starts: HashMap<String, Accent> = def_identities
        .into_iter()
        .filter_map(|(def_id, identity)| accents.get(&identity).map(|accent| (def_id, *accent)))
        .collect();

    let activation_accents: Vec<Option<Accent>> = model
        .activations
        .iter()
        .map(|identity| accents.get(identity).copied())
        .collect();

    Cow::Owned(rewrite(
        svg,
        &group_starts,
        &def_starts,
        &activation_accents,
    ))
}

// ---------------------------------------------------------------------------
// Bounded, quote-aware SVG tag scanner
// ---------------------------------------------------------------------------

struct Tag {
    start: usize,
    end: usize,
    name: String,
    closing: bool,
    self_closing: bool,
    attrs: Vec<(String, String)>,
    /// Comments, PIs, declarations and whole `<style>` blocks: copy verbatim.
    passthrough: bool,
}

impl Tag {
    fn passthrough(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            name: String::new(),
            closing: false,
            self_closing: true,
            attrs: Vec::new(),
            passthrough: true,
        }
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn has_class(&self, class: &str) -> bool {
        self.attr("class").is_some_and(|value| {
            value
                .split(|c: char| c.is_whitespace())
                .any(|token| token == class)
        })
    }

    fn classes(&self) -> impl Iterator<Item = &str> {
        self.attr("class")
            .into_iter()
            .flat_map(|value| value.split(|c: char| c.is_whitespace()))
    }
}

struct Scanner<'a> {
    svg: &'a str,
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(svg: &'a str) -> Self {
        Self { svg, pos: 0 }
    }

    fn next(&mut self) -> Option<Tag> {
        let bytes = self.svg.as_bytes();
        while self.pos < bytes.len() && bytes[self.pos] != b'<' {
            self.pos += 1;
        }
        if self.pos >= bytes.len() {
            return None;
        }
        let start = self.pos;

        if self.svg[start..].starts_with("<!--") {
            let end = self
                .svg
                .get(start + 4..)
                .and_then(|rest| rest.find("-->"))
                .map(|rel| start + 4 + rel + 3)
                .unwrap_or(bytes.len());
            self.pos = end;
            return Some(Tag::passthrough(start, end));
        }
        if self.svg[start..].starts_with("<![CDATA[") {
            let end = self
                .svg
                .get(start + 9..)
                .and_then(|rest| rest.find("]]>"))
                .map(|rel| start + 9 + rel + 3)
                .unwrap_or(bytes.len());
            self.pos = end;
            return Some(Tag::passthrough(start, end));
        }
        if self.svg[start..].starts_with("<!") || self.svg[start..].starts_with("<?") {
            let end = self.svg[start..]
                .find('>')
                .map(|rel| start + rel + 1)
                .unwrap_or(bytes.len());
            self.pos = end;
            return Some(Tag::passthrough(start, end));
        }

        // Find the closing `>` outside quotes.
        let mut index = start + 1;
        let mut quote = 0u8;
        while index < bytes.len() {
            let byte = bytes[index];
            if quote != 0 {
                if byte == quote {
                    quote = 0;
                }
            } else if byte == b'"' || byte == b'\'' {
                quote = byte;
            } else if byte == b'>' {
                break;
            }
            index += 1;
        }
        if index >= bytes.len() {
            self.pos = bytes.len();
            return Some(Tag::passthrough(start, bytes.len()));
        }
        let end = index + 1;
        self.pos = end;

        let inner = &self.svg[start + 1..index];
        let closing = inner.starts_with('/');
        let body = if closing {
            inner[1..].trim_start()
        } else {
            inner.trim_start()
        };
        let name_end = body
            .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .unwrap_or(body.len());
        let name = body[..name_end].to_string();

        let self_closing = inner.trim_end().ends_with('/');
        if !closing
            && name.eq_ignore_ascii_case("style")
            && !self_closing
            && let Some(rel) = self.svg[end..].find("</style>")
        {
            let close_end = end + rel + "</style>".len();
            self.pos = close_end;
            return Some(Tag::passthrough(start, close_end));
        }
        if !closing
            && name.eq_ignore_ascii_case("foreignObject")
            && !self_closing
            && let Some(rel) = self.svg[end..].find("</foreignObject>")
        {
            // The resvg-safe pipeline replaces these with a `<defs>` text
            // overlay plus a `<use>`; no need (or desire) to walk the HTML.
            let close_end = end + rel + "</foreignObject>".len();
            self.pos = close_end;
            return Some(Tag::passthrough(start, close_end));
        }
        let attrs = if closing {
            Vec::new()
        } else {
            parse_attrs(&body[name_end..])
        };

        Some(Tag {
            start,
            end,
            name,
            closing,
            self_closing,
            attrs,
            passthrough: false,
        })
    }
}

fn parse_attrs(input: &str) -> Vec<(String, String)> {
    let bytes = input.as_bytes();
    let mut attrs = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] == b'/' || bytes[index] == b'>' {
            break;
        }
        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && bytes[index] != b'='
            && bytes[index] != b'/'
            && bytes[index] != b'>'
        {
            index += 1;
        }
        let name = input[name_start..index].to_string();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let mut value = String::new();
        if index < bytes.len() && bytes[index] == b'=' {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index < bytes.len() && (bytes[index] == b'"' || bytes[index] == b'\'') {
                let quote = bytes[index];
                index += 1;
                let value_start = index;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
                value = input[value_start..index].to_string();
                if index < bytes.len() {
                    index += 1;
                }
            } else {
                let value_start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && bytes[index] != b'/'
                    && bytes[index] != b'>'
                {
                    index += 1;
                }
                value = input[value_start..index].to_string();
            }
        }
        if !name.is_empty() {
            attrs.push((name, value));
        }
    }
    attrs
}

fn render_start(name: &str, attrs: &[(String, String)], self_closing: bool) -> String {
    let mut out = String::with_capacity(32 + name.len());
    out.push('<');
    out.push_str(name);
    for (key, value) in attrs {
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        // Normalizing single-quoted attribute values to double quotes must not
        // let a raw `"` escape the attribute.
        out.push_str(&value.replace('"', "&quot;"));
        out.push('"');
    }
    if self_closing {
        out.push_str("/>");
    } else {
        out.push('>');
    }
    out
}

fn set_attr(attrs: &mut Vec<(String, String)>, name: &str, value: String) {
    if let Some(entry) = attrs
        .iter_mut()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
    {
        entry.1 = value;
    } else {
        attrs.push((name.to_string(), value));
    }
}

// ---------------------------------------------------------------------------
// Shape collection
// ---------------------------------------------------------------------------

struct ShapeGroup {
    identity: String,
    start: usize,
}

#[derive(Clone, Copy, Default)]
struct CollectFrame {
    start: Option<usize>,
    is_g: bool,
    identity: Option<usize>,
    skip: bool,
}

fn root_diagram_id(svg: &str) -> Option<String> {
    let mut scanner = Scanner::new(svg);
    while let Some(tag) = scanner.next() {
        if tag.passthrough || tag.closing {
            continue;
        }
        if tag.name.eq_ignore_ascii_case("svg") {
            return tag.attr("id").map(unescape_xml);
        }
    }
    None
}

/// Family from merman's own root marker. This survives inline headers,
/// frontmatter and directives, which a source-line lexer does not.
fn detect_family(svg: &str) -> Family {
    let mut scanner = Scanner::new(svg);
    while let Some(tag) = scanner.next() {
        if tag.passthrough || tag.closing {
            continue;
        }
        if !tag.name.eq_ignore_ascii_case("svg") {
            continue;
        }
        let role = tag.attr("aria-roledescription").unwrap_or("");
        return match role {
            "flowchart-v2" | "flowchart" | "flowchart-elk" => Family::Flowchart,
            "sequence" => Family::Sequence,
            "classDiagram" | "classDiagram-v2" => Family::Class,
            "stateDiagram" | "stateDiagram-v2" => Family::State,
            "er" | "erDiagram" => Family::Er,
            "mindmap" => Family::Mindmap,
            _ => Family::Unsupported,
        };
    }
    Family::Unsupported
}

fn collect_shapes(
    svg: &str,
    family: Family,
    diagram_id: &str,
) -> (Vec<ShapeGroup>, Vec<(String, String)>) {
    let prefix = format!("{diagram_id}-");
    let mut groups = Vec::new();
    let mut identities: Vec<String> = Vec::new();
    let mut def_identities: Vec<(String, String)> = Vec::new();
    let mut scanner = Scanner::new(svg);
    let mut stack: Vec<CollectFrame> = Vec::new();

    while let Some(tag) = scanner.next() {
        if tag.passthrough {
            continue;
        }
        if tag.closing {
            if let Some(frame) = stack.pop()
                && frame.is_g
                && let (Some(start), Some(index)) = (frame.start, frame.identity)
            {
                groups.push(ShapeGroup {
                    identity: identities[index].clone(),
                    start,
                });
            }
            continue;
        }

        let parent_skip = stack.last().is_some_and(|frame| frame.skip);
        let skip = parent_skip
            || tag.name.eq_ignore_ascii_case("defs")
            || tag.name.eq_ignore_ascii_case("foreignObject")
            || is_fallback_group(&tag);

        let mut frame = CollectFrame {
            start: None,
            is_g: false,
            identity: None,
            skip,
        };

        if tag.name.eq_ignore_ascii_case("use")
            && let Some(href) = tag.attr("href").or_else(|| tag.attr("xlink:href"))
            && let Some(index) = stack.iter().rev().find_map(|frame| frame.identity)
        {
            let id = href.trim_start_matches('#');
            if !id.is_empty() {
                def_identities.push((id.to_string(), identities[index].clone()));
            }
        }

        if !skip {
            if tag.name.eq_ignore_ascii_case("g") {
                frame.start = Some(tag.start);
                frame.is_g = true;
                let identity = match family {
                    Family::Sequence => {
                        if tag.attr("data-et") == Some("participant") {
                            tag.attr("data-id").map(unescape_xml)
                        } else if tag.has_class("actor") || tag.has_class("actor-man") {
                            tag.attr("name").map(unescape_xml)
                        } else {
                            None
                        }
                    }
                    _ => {
                        if tag.has_class("node") {
                            tag.attr("id")
                                .map(unescape_xml)
                                .and_then(|id| decode_identity(family, &id, &prefix))
                        } else {
                            None
                        }
                    }
                };
                if let Some(identity) = identity {
                    frame.identity = Some(push_identity(&mut identities, identity));
                }
            }

            // Some sequence footer copies use an anonymous `<g>` whose actor
            // rectangle carries the participant id in `name`.
            if family == Family::Sequence
                && tag.name.eq_ignore_ascii_case("rect")
                && tag.has_class("actor")
                && let Some(name) = tag.attr("name")
                && let Some(pending) = stack.last_mut()
                && pending.is_g
                && !pending.skip
                && pending.identity.is_none()
            {
                pending.identity = Some(push_identity(&mut identities, unescape_xml(name)));
            }
        }

        if !tag.self_closing {
            stack.push(frame);
        }
    }

    (groups, def_identities)
}

fn push_identity(identities: &mut Vec<String>, identity: String) -> usize {
    if let Some(index) = identities.iter().position(|known| *known == identity) {
        index
    } else {
        identities.push(identity);
        identities.len() - 1
    }
}

fn decode_identity(family: Family, raw_id: &str, prefix: &str) -> Option<String> {
    let suffix = raw_id.strip_prefix(prefix).unwrap_or(raw_id);
    match family {
        Family::Flowchart => {
            if let Some(rest) = suffix.strip_prefix("flowchart-")
                && let Some((node, index)) = rest.rsplit_once('-')
                && !node.is_empty()
                && index.bytes().all(|b| b.is_ascii_digit())
            {
                return Some(node.to_string());
            }
            Some(suffix.to_string())
        }
        Family::Class => {
            let rest = suffix.strip_prefix("classId-")?;
            Some(strip_trailing_index(rest).to_string())
        }
        Family::State => {
            let rest = suffix.strip_prefix("state-")?;
            let rest = strip_trailing_index(rest);
            if rest.ends_with("-note") || rest.ends_with("-group") {
                None
            } else {
                Some(rest.to_string())
            }
        }
        Family::Er => {
            let rest = suffix.strip_prefix("entity-")?;
            Some(strip_trailing_index(rest).to_string())
        }
        Family::Mindmap => suffix.strip_prefix("node_").map(str::to_string),
        Family::Sequence | Family::Unsupported => None,
    }
}

fn strip_trailing_index(value: &str) -> &str {
    match value.rsplit_once('-') {
        Some((head, tail))
            if !head.is_empty() && !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) =>
        {
            head
        }
        _ => value,
    }
}

// ---------------------------------------------------------------------------
// Source-first-appearance ordering
// ---------------------------------------------------------------------------

/// Model-derived order, plus identities that are drawn as chrome rather than
/// source shapes (state `[*]` pseudostates) and must stay neutral.
#[derive(Default)]
struct ModelOrder {
    order: Vec<String>,
    excluded: HashSet<String>,
    /// Completed sequence activations in document (start-message) order.
    activations: Vec<String>,
}

fn model_identity_order(order: Vec<String>) -> ModelOrder {
    ModelOrder {
        order,
        excluded: HashSet::new(),
        activations: Vec::new(),
    }
}

/// Order from merman's typed semantic model. These vectors/maps are in source
/// creation order and label edits cannot reorder them; the SVG walk is only a
/// fallback for identities the model does not expose.
fn model_order(source: &str) -> ModelOrder {
    let parsed = match catch_unwind(AssertUnwindSafe(|| {
        Engine::new().parse_diagram_for_render_model_sync(source, ParseOptions::strict())
    })) {
        Ok(Ok(Some(parsed))) => parsed,
        _ => return ModelOrder::default(),
    };
    match parsed.model() {
        RenderSemanticModel::Flowchart(model) => {
            model_identity_order(model.nodes.iter().map(|node| node.id.clone()).collect())
        }
        RenderSemanticModel::Sequence(model) => {
            let mut slots: Vec<Option<String>> = Vec::new();
            let mut stacks: HashMap<String, Vec<usize>> = HashMap::new();
            for message in &model.messages {
                match message.semantic_kind() {
                    SequenceMessageKind::ActivationStart => {
                        if let Some(actor) = &message.from {
                            let slot = slots.len();
                            slots.push(None);
                            stacks.entry(actor.clone()).or_default().push(slot);
                        }
                    }
                    SequenceMessageKind::ActivationEnd => {
                        if let Some(actor) = &message.from
                            && let Some(slot) = stacks.get_mut(actor).and_then(|stack| stack.pop())
                        {
                            slots[slot] = Some(actor.clone());
                        }
                    }
                    _ => {}
                }
            }
            ModelOrder {
                order: model.actor_order.clone(),
                excluded: HashSet::new(),
                activations: slots.into_iter().flatten().collect(),
            }
        }
        RenderSemanticModel::Class(model) => {
            model_identity_order(model.classes.keys().cloned().collect())
        }
        RenderSemanticModel::State(model) => {
            let mut order = Vec::with_capacity(model.nodes.len());
            let mut excluded = HashSet::new();
            for node in &model.nodes {
                if node.shape == "stateStart" || node.shape == "stateEnd" {
                    // `[*]` is diagram chrome (a dot), not a state box: keep
                    // it neutral and do not let it consume a palette slot.
                    excluded.insert(node.id.clone());
                } else {
                    order.push(node.id.clone());
                }
            }
            ModelOrder {
                order,
                excluded,
                activations: Vec::new(),
            }
        }
        RenderSemanticModel::Er(model) => model_identity_order(
            model
                .entities
                .keys()
                .map(|key| er_entity_identity(key))
                .collect(),
        ),
        RenderSemanticModel::Mindmap(model) => {
            model_identity_order(model.nodes.iter().map(|node| node.id.clone()).collect())
        }
        _ => ModelOrder::default(),
    }
}

fn er_entity_identity(key: &str) -> String {
    let rest = key.strip_prefix("entity-").unwrap_or(key);
    strip_trailing_index(rest).to_string()
}

fn shape_order(
    identities: &HashSet<String>,
    model_order: &[String],
    groups: &[ShapeGroup],
) -> Vec<String> {
    let mut ordered = Vec::with_capacity(identities.len());
    let mut seen = HashSet::with_capacity(identities.len());
    for identity in model_order {
        if identities.contains(identity) && seen.insert(identity.clone()) {
            ordered.push(identity.clone());
        }
    }
    for group in groups {
        if seen.insert(group.identity.clone()) {
            ordered.push(group.identity.clone());
        }
    }
    ordered
}

// ---------------------------------------------------------------------------
// Rewriting
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Frame {
    accent: Option<Accent>,
    label: bool,
    suppress: bool,
    def_text: Option<Accent>,
    in_defs: bool,
}

fn rewrite(
    svg: &str,
    group_starts: &HashMap<usize, Accent>,
    def_starts: &HashMap<String, Accent>,
    activation_accents: &[Option<Accent>],
) -> String {
    let mut out = String::with_capacity(svg.len() + svg.len() / 8 + 64);
    let mut last = 0;
    let mut scanner = Scanner::new(svg);
    let mut stack: Vec<Frame> = Vec::new();
    let mut next_activation = 0usize;

    while let Some(tag) = scanner.next() {
        if last < tag.start {
            out.push_str(&svg[last..tag.start]);
        }
        last = tag.end;

        if tag.passthrough {
            out.push_str(&svg[tag.start..tag.end]);
            continue;
        }
        if tag.closing {
            out.push_str(&svg[tag.start..tag.end]);
            stack.pop();
            continue;
        }

        let parent = stack.last().copied().unwrap_or_default();
        let mut frame = parent;
        if tag.name.eq_ignore_ascii_case("defs") {
            frame.in_defs = true;
        }
        if tag.name.eq_ignore_ascii_case("foreignObject") || is_fallback_group(&tag) {
            frame.suppress = true;
        }
        if tag.name.eq_ignore_ascii_case("g") && tag.has_class("label") {
            frame.label = true;
        }
        if let Some(&accent) = group_starts.get(&tag.start) {
            frame.accent = Some(accent);
        }
        if frame.in_defs
            && tag.name.eq_ignore_ascii_case("g")
            && let Some(id) = tag.attr("id")
            && let Some(&accent) = def_starts.get(&unescape_xml(id))
        {
            frame.def_text = Some(accent);
        }

        let activation = tag.name.eq_ignore_ascii_case("rect")
            && frame.accent.is_none()
            && !frame.suppress
            && tag
                .attr("class")
                .is_some_and(|class| class.trim_start().starts_with("activation"));
        let activation_accent = if activation {
            let accent = activation_accents.get(next_activation).copied().flatten();
            next_activation = next_activation.saturating_add(1);
            accent
        } else {
            None
        };

        let rewritten = if let Some(accent) = activation_accent {
            Some(paint_shape(&tag, accent))
        } else {
            rewrite_tag(&tag, frame)
        };

        match rewritten {
            Some(value) => out.push_str(&value),
            None => out.push_str(&svg[tag.start..tag.end]),
        }
        if !tag.self_closing {
            stack.push(frame);
        }
    }
    if last < svg.len() {
        out.push_str(&svg[last..]);
    }
    out
}

fn rewrite_tag(tag: &Tag, frame: Frame) -> Option<String> {
    if tag.name.eq_ignore_ascii_case("filter") {
        // usvg defaults filter color interpolation to linearRGB and converts
        // the whole filtered subtree through gamma tables, so an authored
        // accent fill no longer lands as that exact sRGB pixel. Keep the
        // filter chain (shadows) but process it in sRGB so the shape/text
        // paint stays exactly the supplied accent color.
        let mut attrs = tag.attrs.clone();
        set_attr(
            &mut attrs,
            "color-interpolation-filters",
            "sRGB".to_string(),
        );
        return Some(render_start(&tag.name, &attrs, tag.self_closing));
    }
    if tag.name.eq_ignore_ascii_case("text") || tag.name.eq_ignore_ascii_case("tspan") {
        let accent = frame
            .def_text
            .or(if frame.suppress { None } else { frame.accent })?;
        return Some(paint_text(tag, accent));
    }
    if frame.suppress || frame.label {
        return None;
    }
    let accent = frame.accent?;
    if tag.name.eq_ignore_ascii_case("line") {
        return Some(paint_stroke(tag, accent));
    }
    if !matches!(
        tag.name.to_ascii_lowercase().as_str(),
        "rect" | "circle" | "ellipse" | "polygon" | "polyline" | "path"
    ) {
        return None;
    }
    let divider = tag.classes().any(|class| {
        matches!(
            class,
            "divider" | "collapsed-separator" | "collapsed-indicator" | "note-edge"
        )
    });
    if divider {
        return Some(paint_shape(tag, accent));
    }
    if tag.classes().any(|class| {
        matches!(
            class,
            "anchor" | "icon" | "image" | "marker" | "arrowMarkerPath"
        )
    }) {
        return None;
    }
    let fill_none = tag
        .attr("fill")
        .is_some_and(|fill| fill.eq_ignore_ascii_case("none"));
    let stroke_none = tag
        .attr("stroke")
        .is_some_and(|stroke| stroke.eq_ignore_ascii_case("none"));
    if fill_none && stroke_none {
        return None;
    }
    Some(paint_shape(tag, accent))
}

fn paint_shape(tag: &Tag, accent: Accent) -> String {
    let mut attrs = tag.attrs.clone();
    set_attr(&mut attrs, "fill", accent.fill.to_css());
    let stroke_none = tag
        .attr("stroke")
        .is_some_and(|stroke| stroke.eq_ignore_ascii_case("none"));
    let mut declarations = format!("fill:{}!important;", accent.fill.to_css());
    if !stroke_none {
        set_attr(&mut attrs, "stroke", accent.border.to_css());
        declarations.push_str(&format!("stroke:{}!important;", accent.border.to_css()));
    }
    merge_style(&mut attrs, tag, &declarations, &["fill", "stroke"]);
    render_start(&tag.name, &attrs, tag.self_closing)
}

fn paint_stroke(tag: &Tag, accent: Accent) -> String {
    let mut attrs = tag.attrs.clone();
    set_attr(&mut attrs, "stroke", accent.border.to_css());
    merge_style(
        &mut attrs,
        tag,
        &format!("stroke:{}!important;", accent.border.to_css()),
        &["stroke"],
    );
    render_start(&tag.name, &attrs, tag.self_closing)
}

fn paint_text(tag: &Tag, accent: Accent) -> String {
    let mut attrs = tag.attrs.clone();
    let css = accent.text.to_css();
    set_attr(&mut attrs, "fill", css.clone());
    merge_style(
        &mut attrs,
        tag,
        &format!("fill:{css}!important;color:{css}!important;"),
        &["fill", "color"],
    );
    render_start(&tag.name, &attrs, tag.self_closing)
}

fn merge_style(
    attrs: &mut Vec<(String, String)>,
    tag: &Tag,
    declarations: &str,
    replaced: &[&str],
) {
    let existing = tag.attr("style").unwrap_or("");
    let mut merged = String::with_capacity(existing.len() + declarations.len() + 1);
    for declaration in existing.split(';') {
        let declaration = declaration.trim();
        if declaration.is_empty() {
            continue;
        }
        let property = declaration
            .split_once(':')
            .map(|(property, _)| property.trim())
            .unwrap_or("");
        if replaced
            .iter()
            .any(|name| property.eq_ignore_ascii_case(name))
        {
            continue;
        }
        merged.push_str(declaration);
        merged.push(';');
    }
    merged.push_str(declarations);
    set_attr(attrs, "style", merged);
}

fn is_fallback_group(tag: &Tag) -> bool {
    tag.name.eq_ignore_ascii_case("g")
        && (tag.attr("data-merman-foreignobject") == Some("fallback")
            || tag.has_class("merman-foreignobject-fallback"))
}

fn unescape_xml(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

// ---------------------------------------------------------------------------
// Small string helper (bounded, byte-accurate copies)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn scanner_reads_quoted_attributes_and_skips_style_text() {
        let svg = r#"<svg id="d"><style>#d rect{fill:#fff;}</style><g id="d-A" class="node"><rect x="0"/></g></svg>"#;
        let mut scanner = Scanner::new(svg);
        let mut names = Vec::new();
        while let Some(tag) = scanner.next() {
            names.push(tag.name.clone());
        }
        assert_eq!(names, ["svg", "", "g", "rect", "g", "svg"]);
    }

    #[test]
    fn model_order_is_first_appearance_for_flowchart() {
        let source = "flowchart TD\n  Z[z] --> Y[y]\n  X[x] --> Z\n";
        let identities: HashSet<String> = ["X", "Y", "Z"].into_iter().map(str::to_string).collect();
        let groups = [
            ShapeGroup {
                identity: "X".into(),
                start: 0,
            },
            ShapeGroup {
                identity: "Y".into(),
                start: 0,
            },
            ShapeGroup {
                identity: "Z".into(),
                start: 0,
            },
        ];
        let order = shape_order(&identities, &model_order(source).order, &groups);
        assert_eq!(order, ["Z", "Y", "X"]);
    }
}
