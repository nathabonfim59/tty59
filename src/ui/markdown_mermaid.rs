//! Mermaid diagrams in the editor's Markdown preview.
//!
//! The Markdown text view hands every fenced code block to our block parser.
//! This module recognises the ones whose info string starts with `mermaid`
//! (any case, backtick or tilde fences, metadata allowed) and turns each into
//! a [`MermaidBlock`]. Drawing itself lives in the private [`render`]
//! submodule, which package B2 owns.
//!
//! # Block identity
//!
//! [`MermaidBlock::block_id`] mixes the preview's identity — the buffer entity
//! id and the editor group slot — with the byte offset of the opening fence.
//! Editing the source inside one fence keeps that block's id, so its last-good
//! image survives typing. Two previews (or two groups) never share an id, and
//! two adjacent diagrams in one document differ by their offsets.
//!
//! Offsets shift when text *above* a fence changes, so an edit earlier in the
//! document may reset the placeholder for a later block. That is expected: the
//! source line itself did not move, but its document position did.

use gpui::{EntityId, IntoElement};
use gpui_component::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast};

mod render;

/// Stable node name registered with the text view.
const MERMAID_NODE: &str = "mermaid";

/// Identifies one rendered preview: a buffer in one editor group.
///
/// Built from the buffer entity id and the group slot at wiring time. The mix
/// is a fixed splitmix sequence, so the same buffer in the same group keeps
/// the same identity for the life of the process. `EntityId`s are assigned per
/// launch, so neither is stable across restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PreviewIdentity(u64);

impl PreviewIdentity {
    /// Derive the identity for `buffer` in editor group `slot`.
    pub(crate) fn from_buffer(buffer: EntityId, slot: usize) -> Self {
        Self(combine(buffer.as_u64(), slot as u64))
    }
}

/// One recognised mermaid fence in the Markdown preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MermaidBlock {
    /// The diagram source, without the opening and closing fences.
    pub(crate) source: String,
    /// The fence exactly as written, for copy and selection.
    pub(crate) original_fence: String,
    /// Stable identity for last-good-image bookkeeping (see module docs).
    pub(crate) block_id: u64,
}

/// The block plugin the editor's markdown preview installs.
#[derive(Clone)]
pub(crate) struct MermaidPlugin {
    identity: PreviewIdentity,
}

impl MermaidPlugin {
    pub(crate) fn new(identity: PreviewIdentity) -> Self {
        Self { identity }
    }
}

impl MarkdownPlugin for MermaidPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        MERMAID_NODE
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::Code(code) = node else {
            return None;
        };
        let fence = cx.node_source(node).unwrap_or_default();
        let offset = code
            .position
            .as_ref()
            .map_or(cx.offset(), |position| cx.offset() + position.start.offset);
        let block = mermaid_block(code, fence, offset, self.identity)?;
        Some(
            MarkdownNode::new(MERMAID_NODE, block)
                .text(code.value.clone())
                .markdown(fence.to_string()),
        )
    }

    fn render(
        &self,
        node: &MarkdownNode,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> impl IntoElement {
        match node.data::<MermaidBlock>() {
            Some(block) => render::render_block(block, window, cx),
            // Only our parser produces this name; an empty element beats a
            // panic if a stale node ever reaches the renderer.
            None => gpui::div().into_any_element(),
        }
    }
}

/// Turn an mdast `Code` node into a mermaid block when its language matches.
///
/// `original_fence` is the node's source slice (fences included) and
/// `document_offset` is the byte offset of the opening fence in the whole
/// document.
fn mermaid_block(
    code: &markdown_ast::Code,
    original_fence: &str,
    document_offset: usize,
    identity: PreviewIdentity,
) -> Option<MermaidBlock> {
    if !is_mermaid_info(code.lang.as_deref()) {
        return None;
    }
    Some(MermaidBlock {
        source: code.value.clone(),
        original_fence: original_fence.to_string(),
        block_id: combine(identity.0, document_offset as u64),
    })
}

/// `true` when the first word of a fence's info string is `mermaid`.
///
/// mdast normally puts just the language word in `lang`, but keeping the
/// first-word rule here makes the parser independent of that split and lets a
/// hand-built node carry metadata in the same string.
fn is_mermaid_info(info: Option<&str>) -> bool {
    info.and_then(|info| info.split_whitespace().next())
        .is_some_and(|word| word.eq_ignore_ascii_case(MERMAID_NODE))
}

/// Order-sensitive mix of two `u64`s into one.
///
/// FNV-1a over `[a, b]`, then the splitmix finalizer. Order matters (the two
/// words are folded in sequence), and an equal pair does not collapse to a
/// constant the way an XOR or an add of mirrored halves would.
fn combine(a: u64, b: u64) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for word in [a, b] {
        hash ^= word;
        hash = hash.wrapping_mul(PRIME);
    }
    mix(hash)
}

/// The splitmix64 finalizer: spreads adjacent inputs so ids derived from
/// neighbouring offsets or slots do not collide.
fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, Window};
    use gpui_component::text::TextView;
    use gpui_component::text::markdown_ast::Code;
    use std::sync::{Arc, Mutex};

    /// What one parsed node carried, recorded by the probe plugin below.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct SeenBlock {
        block: MermaidBlock,
        text: String,
        markdown: String,
    }

    /// Wraps the real [`MermaidPlugin`] and records what its `parse` returned.
    ///
    /// The text view calls this through `TextView::plugin`, so the parser is
    /// exercised by markdown-rs itself rather than by hand-built mdast nodes.
    #[derive(Clone)]
    struct ProbePlugin {
        inner: MermaidPlugin,
        seen: Arc<Mutex<Vec<SeenBlock>>>,
    }

    impl ProbePlugin {
        fn new(identity: PreviewIdentity, seen: Arc<Mutex<Vec<SeenBlock>>>) -> Self {
            Self {
                inner: MermaidPlugin::new(identity),
                seen,
            }
        }
    }

    impl MarkdownPlugin for ProbePlugin {
        fn is_block(&self) -> bool {
            self.inner.is_block()
        }

        fn name(&self) -> &str {
            self.inner.name()
        }

        fn parse(
            &self,
            node: &markdown_ast::Node,
            cx: &MarkdownParseContext<'_>,
        ) -> Option<MarkdownNode> {
            let parsed = self.inner.parse(node, cx)?;
            let block = parsed
                .data::<MermaidBlock>()
                .cloned()
                .expect("probe wraps the mermaid parser");
            self.seen.lock().unwrap().push(SeenBlock {
                block,
                text: parsed.as_text().to_string(),
                markdown: parsed.as_markdown().to_string(),
            });
            Some(parsed)
        }

        fn render(
            &self,
            _node: &MarkdownNode,
            _window: &mut Window,
            _cx: &mut gpui::App,
        ) -> impl IntoElement {
            gpui::div()
        }
    }

    /// A window root that renders one markdown preview.
    struct Preview {
        markdown: String,
        plugin: ProbePlugin,
    }

    impl Render for Preview {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            TextView::markdown("mermaid-integration-preview", self.markdown.clone())
                .plugin(self.plugin.clone())
        }
    }

    /// Parse a document through the real text view and return what was seen.
    fn parse_through_text_view(
        cx: &mut TestAppContext,
        document: &str,
        identity: PreviewIdentity,
    ) -> Vec<SeenBlock> {
        cx.update(gpui_component::init);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let plugin = ProbePlugin::new(identity, seen.clone());
        let (_, vcx) = cx.add_window_view(|_, _| Preview {
            markdown: document.to_string(),
            plugin,
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let seen = seen.lock().unwrap();
        // The text view reparses whenever its extension revision changes, and
        // each `Preview::render` installs the plugin afresh (the public API has
        // no idempotent registration), so the same document may be parsed more
        // than once. Collapse repeats by the stable block id; a genuine id
        // collision would wrongly collapse two different fences and fail below.
        let mut unique: Vec<SeenBlock> = Vec::new();
        for block in seen.iter().cloned() {
            if !unique
                .iter()
                .any(|seen| seen.block.block_id == block.block.block_id)
            {
                unique.push(block);
            }
        }
        unique
    }

    fn identity(buffer: u64, slot: usize) -> PreviewIdentity {
        PreviewIdentity::from_buffer(EntityId::from(buffer), slot)
    }

    /// A `Code` node with no position; tests pass the document offset directly.
    fn code(lang: Option<&str>, value: &str) -> Code {
        Code {
            value: value.to_string(),
            position: None,
            lang: lang.map(str::to_string),
            meta: None,
        }
    }

    fn parse_as(
        lang: Option<&str>,
        value: &str,
        fence: &str,
        offset: usize,
        identity: PreviewIdentity,
    ) -> Option<MermaidBlock> {
        mermaid_block(&code(lang, value), fence, offset, identity)
    }

    #[test]
    fn detects_backtick_and_tilde_fences_in_any_case() {
        let id = identity(1, 0);
        for (lang, fence) in [
            (Some("mermaid"), "```mermaid\ngraph TD; A-->B\n```"),
            (Some("Mermaid"), "```Mermaid\ngraph TD; A-->B\n```"),
            (Some("MERMAID"), "```MERMAID\ngraph TD; A-->B\n```"),
            (Some("mermaid"), "~~~mermaid\ngraph TD; A-->B\n~~~"),
            (Some("Mermaid"), "~~~Mermaid\ngraph TD; A-->B\n~~~"),
        ] {
            let block = parse_as(lang, "graph TD; A-->B\n", fence, 0, id)
                .unwrap_or_else(|| panic!("expected mermaid block for {lang:?}"));
            assert_eq!(block.original_fence, fence, "the fence must survive copy");
            assert_eq!(block.source, "graph TD; A-->B\n");
        }
    }

    #[test]
    fn accepts_info_metadata_and_uses_the_first_word() {
        // mdast normally splits `title=x` into `meta`, but the parser must not
        // depend on that: the first word is the language.
        let block = parse_as(
            Some("mermaid title=Flow"),
            "",
            "```mermaid title=Flow\n```",
            3,
            identity(1, 0),
        )
        .expect("mermaid block with metadata");
        assert_eq!(block.source, "");
        assert_eq!(block.original_fence, "```mermaid title=Flow\n```");
    }

    #[test]
    fn metadata_in_the_mdast_field_does_not_hide_the_language() {
        let mut node = code(Some("mermaid"), "graph TD; A-->B\n");
        node.meta = Some("title=Flow".to_string());
        let block = mermaid_block(&node, "```mermaid title=Flow\n...\n```", 0, identity(2, 3))
            .expect("mermaid block");
        assert_eq!(block.source, "graph TD; A-->B\n");
    }

    #[test]
    fn leaves_other_code_alone() {
        for lang in [
            Some("rust"),
            Some("mermaidx"),
            Some("merman"),
            Some(""),
            None,
        ] {
            assert!(
                parse_as(
                    lang,
                    "graph TD; A-->B\n",
                    "```rust\n...\n```",
                    0,
                    identity(1, 0)
                )
                .is_none(),
                "{lang:?} must stay an ordinary code block"
            );
        }
    }

    #[test]
    fn adjacent_blocks_get_distinct_ids() {
        let id = identity(7, 1);
        let first = parse_as(Some("mermaid"), "A", "```mermaid\nA\n```", 10, id).unwrap();
        let second = parse_as(Some("mermaid"), "B", "```mermaid\nB\n```", 30, id).unwrap();
        assert_ne!(first.block_id, second.block_id);
    }

    #[test]
    fn editing_a_blocks_source_keeps_its_id() {
        let id = identity(7, 1);
        let before = parse_as(
            Some("mermaid"),
            "graph TD; A-->B\n",
            "```mermaid\nA\n```",
            10,
            id,
        )
        .unwrap();
        let after = parse_as(
            Some("mermaid"),
            "graph TD; A-->B-->C\n",
            "```mermaid\nA-->B\n```",
            10,
            id,
        )
        .unwrap();
        assert_eq!(before.block_id, after.block_id);
    }

    #[test]
    fn a_moved_fence_changes_the_id() {
        let id = identity(7, 1);
        let first = parse_as(Some("mermaid"), "A", "```mermaid\nA\n```", 10, id).unwrap();
        let moved = parse_as(Some("mermaid"), "A", "```mermaid\nA\n```", 11, id).unwrap();
        assert_ne!(first.block_id, moved.block_id);
    }

    #[test]
    fn distinct_previews_do_not_share_identity() {
        assert_eq!(identity(1, 0), identity(1, 0));
        assert_ne!(identity(1, 0), identity(1, 1), "groups must differ");
        assert_ne!(identity(1, 0), identity(2, 0), "buffers must differ");
        assert_ne!(identity(1, 0), identity(2, 1));
    }

    /// Regression: the first `combine` XORed `mix(a)` and `mix(b)`, which is
    /// symmetric and collapses every equal pair to `mix(0)`. Buffer 1 / slot 1
    /// and buffer 2 / slot 2 (or 3/3, 4/4, …) got the same preview identity.
    #[test]
    fn identity_mix_is_order_sensitive_and_keeps_equal_pairs_apart() {
        assert_ne!(
            combine(1, 1),
            combine(2, 2),
            "equal-argument pairs must not collapse to one identity"
        );
        assert_ne!(
            identity(1, 1),
            identity(2, 2),
            "equal buffer/slot pairs must not collapse to one identity"
        );
        assert_ne!(identity(3, 3), identity(4, 4));
        assert_ne!(combine(1, 2), combine(2, 1), "argument order must matter");
        assert_ne!(combine(7, 9), combine(9, 7));
        // Different positions still differ from one another.
        assert_ne!(combine(1, 1), combine(1, 2));
        assert_ne!(combine(1, 1), combine(2, 1));
    }

    #[test]
    fn ids_of_two_previews_at_the_same_offset_differ() {
        let first = parse_as(
            Some("mermaid"),
            "A",
            "```mermaid\nA\n```",
            5,
            identity(1, 0),
        )
        .unwrap();
        let second = parse_as(
            Some("mermaid"),
            "A",
            "```mermaid\nA\n```",
            5,
            identity(2, 0),
        )
        .unwrap();
        assert_ne!(first.block_id, second.block_id);
    }

    /// End-to-end through `TextView::markdown(...).plugin(...)`: the real
    /// markdown-rs parser has to turn each fence into a node our `parse`
    /// recognises, and the original fence has to survive on `.markdown()`.
    #[gpui::test]
    fn text_view_parses_backtick_tilde_and_metadata_fences(cx: &mut TestAppContext) {
        let document = "```mermaid\ngraph TD; A-->B\n```\n\n~~~Mermaid\nsequenceDiagram\n    A->>B: hi\n~~~\n\n```mermaid title=Flow\nflowchart LR\n    A --> B\n```\n\n```rust\nfn main() {}\n```\n";

        let seen = parse_through_text_view(cx, document, identity(9, 2));

        assert_eq!(seen.len(), 3, "the rust fence must stay ordinary code");
        assert!(
            seen.iter().all(|seen| !seen.markdown.contains("fn main")),
            "non-mermaid code must never reach the plugin"
        );

        // Backtick fence, lower case. mdast's `value` has no trailing newline.
        assert_eq!(seen[0].block.source, "graph TD; A-->B");
        assert_eq!(
            seen[0].text, "graph TD; A-->B",
            ".text() is the diagram source without fences"
        );
        assert_eq!(
            seen[0].markdown, "```mermaid\ngraph TD; A-->B\n```",
            ".markdown() keeps the original fence for copy"
        );

        // Tilde fence, mixed case.
        assert_eq!(seen[1].block.source, "sequenceDiagram\n    A->>B: hi");
        assert!(
            seen[1].markdown.starts_with("~~~Mermaid\n"),
            "tilde fence and its case must survive: {:?}",
            seen[1].markdown
        );
        assert!(seen[1].markdown.ends_with("~~~"));

        // Info-string metadata after the language word.
        assert_eq!(seen[2].block.source, "flowchart LR\n    A --> B");
        assert!(
            seen[2].markdown.starts_with("```mermaid title=Flow\n"),
            "metadata must survive in .markdown(): {:?}",
            seen[2].markdown
        );

        assert_ne!(seen[0].block.block_id, seen[1].block.block_id);
        assert_ne!(seen[1].block.block_id, seen[2].block.block_id);
        assert_ne!(seen[0].block.block_id, seen[2].block.block_id);
    }

    /// A document with no mermaid fence must produce no custom nodes at all.
    #[gpui::test]
    fn text_view_leaves_plain_code_blocks_alone(cx: &mut TestAppContext) {
        let seen = parse_through_text_view(
            cx,
            "```rust\nfn main() {}\n```\n\n```\nplain\n```\n",
            identity(4, 0),
        );
        assert!(seen.is_empty());
    }
}
