//! An element that lays its child out against a different rem.
//!
//! The Markdown preview zooms by scaling the rem the text view resolves its
//! `rems(..)` lengths against — the window's rem belongs to `ui_font_size`,
//! which the rest of the chrome must keep. This element pushes a private
//! rem around one child for all three draw phases, the same way Zed's
//! `WithRemSize` does: styles are registered under it in `request_layout`,
//! and measured children — the text view's inline flows, whose taffy
//! closures read `window.rem_size()` when the enclosing view's layout
//! computes during `prepaint` — resolve against it too.
//!
//! Anything that should not grow with the text (a scrollbar, fixed frame
//! padding) stays outside the scope.

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

/// Lay `child` out as though the window's rem were `rem`.
pub(crate) struct RemScope {
    rem: Pixels,
    child: AnyElement,
}

impl RemScope {
    pub(crate) fn new(rem: impl Into<Pixels>, child: AnyElement) -> Self {
        Self {
            rem: rem.into(),
            child,
        }
    }
}

impl Element for RemScope {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child = window.with_rem_size(Some(self.rem), |window| {
            self.child.request_layout(window, cx)
        });
        (child, child)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _child: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rem_size(Some(self.rem), |window| {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rem_size(Some(self.rem), |window| {
            self.child.paint(window, cx);
        });
    }
}

impl IntoElement for RemScope {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, ParentElement as _, Styled as _, TestAppContext, px};
    use gpui_component::text::TextView;

    /// Records the bounds layout gave the wrapped element, standing in for
    /// the preview's column: full available width, own height.
    struct Measured {
        inner: AnyElement,
        recorded: std::sync::Arc<std::sync::Mutex<Option<gpui::Bounds<Pixels>>>>,
    }

    impl Element for Measured {
        type RequestLayoutState = LayoutId;
        type PrepaintState = ();

        fn id(&self) -> Option<ElementId> {
            None
        }

        fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, Self::RequestLayoutState) {
            let child = self.inner.request_layout(window, cx);
            let mut style = gpui::Style::default();
            style.size.width = gpui::relative(1.).into();
            (window.request_layout(style, Some(child), cx), child)
        }

        fn prepaint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            child: &mut LayoutId,
            window: &mut Window,
            cx: &mut App,
        ) {
            self.inner.prepaint(window, cx);
            let bounds = window.layout_bounds(*child);
            *self.recorded.lock().unwrap() = Some(bounds);
        }

        fn paint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut LayoutId,
            _: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) {
            self.inner.paint(window, cx);
        }
    }

    impl IntoElement for Measured {
        type Element = Self;

        fn into_element(self) -> Self {
            self
        }
    }

    /// The window's root: one markdown text view, optionally inside a
    /// [`RemScope`], in a fixed-width column. A raw `VisualTestContext::draw`
    /// has no rendered view on the element stack (`window.current_view()`),
    /// which the text view's inline state needs — so the probe draws as a
    /// real root view, the way the preview does.
    struct ProbeHost {
        scoped: bool,
        column: f32,
        recorded: std::sync::Arc<std::sync::Mutex<Option<gpui::Bounds<Pixels>>>>,
    }

    impl gpui::Render for ProbeHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let text = TextView::markdown(
                "probe-body",
                "One two three four five six seven eight nine ten \
                 eleven twelve thirteen fourteen fifteen sixteen.",
            )
            .into_any_element();
            let inner = if self.scoped {
                RemScope::new(px(32.), text).into_any_element()
            } else {
                text
            };
            gpui::div().w(px(self.column)).child(Measured {
                inner,
                recorded: self.recorded.clone(),
            })
        }
    }

    /// The zoom lever the preview relies on: a text view inside the scope
    /// resolves its body text against the scoped rem, so doubling the rem
    /// grows the height the paragraphs lay out to. If this stops holding —
    /// a gpui-component change that resolves the rem at view creation, say
    /// — the preview's zoom must fall back to explicit sizes.
    #[gpui::test]
    fn rem_scope_grows_markdown_body_text(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let column = 300.;
        let measure = |scoped: bool, cx: &mut TestAppContext| -> Pixels {
            let recorded: std::sync::Arc<std::sync::Mutex<Option<gpui::Bounds<Pixels>>>> =
                std::sync::Arc::new(std::sync::Mutex::new(None));
            let sink = recorded.clone();
            let (_, vcx) = cx.add_window_view(|window, cx| {
                let host = cx.new(|_| ProbeHost {
                    scoped,
                    column,
                    recorded: sink,
                });
                gpui_component::Root::new(host, window, cx)
            });
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            recorded
                .lock()
                .unwrap()
                .take()
                .expect("prepaint records the child bounds")
                .size
                .height
        };

        let base = measure(false, cx);
        let scoped = measure(true, cx);

        assert!(
            scoped > base * 1.6,
            "scoped rem must grow body text: base {} px, scoped {} px",
            base.as_f32(),
            scoped.as_f32()
        );
    }

    /// A window root holding one code-editor Input, optionally scoped.
    struct InputHost {
        state: gpui::Entity<gpui_component::input::InputState>,
        scoped: bool,
    }

    impl gpui::Render for InputHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let input = gpui_component::input::Input::new(&self.state)
                .appearance(false)
                .size_full()
                .into_any_element();
            let inner = if self.scoped {
                RemScope::new(px(32.), input).into_any_element()
            } else {
                input
            };
            gpui::div().w(px(300.)).h(px(400.)).child(inner)
        }
    }

    /// The zoom lever the source view relies on: an Input lays its lines out
    /// at `rems(1.25)`, so inside the scope the line pitch scales with the
    /// rem. The pitch is measured through the caret's own bounds — the same
    /// layout the line numbers and gutter markers are placed against.
    #[gpui::test]
    fn rem_scope_scales_the_input_line_pitch(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let pitch = |scoped: bool, cx: &mut TestAppContext| -> f32 {
            let (host, mut vcx) = cx.add_window_view(|window, cx| {
                let state = cx.new(|cx| {
                    gpui_component::input::InputState::new(window, cx)
                        .multi_line(true)
                        .default_value("one\ntwo\nthree\nfour\n")
                });
                InputHost { state, scoped }
            });
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            vcx.run_until_parked();
            let top = |line: u32, vcx: &mut gpui::VisualTestContext| {
                vcx.update(|_, cx| {
                    host.read_with(cx, |host, cx| {
                        // `range_to_bounds` speaks byte offsets; the sample's
                        // line starts are 0, 4, 8 and 14.
                        let start = match line {
                            0 => 0,
                            1 => 4,
                            2 => 8,
                            _ => 14,
                        };
                        host.state
                            .read(cx)
                            .range_to_bounds(&(start..start))
                            .expect("the caret has bounds once laid out")
                            .top()
                            .as_f32()
                    })
                })
            };
            (top(3, &mut vcx) - top(0, &mut vcx)) / 3.
        };

        let base = pitch(false, cx);
        let scoped = pitch(true, cx);
        assert!(
            (scoped - base * 2.).abs() < 1.,
            "the line pitch must scale with the scoped rem: base {base} px, scoped {scoped} px"
        );
    }
}
