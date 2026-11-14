//! Test-only record of the styles builders put on their elements. GPUI's
//! test platform exposes measured bounds but not the painted scene, so
//! builders `note` an element's own `StyleRefinement` at construction; a
//! reverted `.bg(...)`/`.text_color(...)` fails an assertion instead of
//! compiling away silently. Read-only observation: notes never touch app
//! state or layout.
//!
//! Capture is off unless a test holds [`scope`] — renders outside a probe
//! scope record nothing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// One element's resolved paints. `fill`/`text`/`border` are the packed
/// `0xRRGGBB` values (alpha dropped, the way `contrast` reads only RGB);
/// `*_alpha` carry each paint's own alpha and `opacity` the element's
/// container opacity, so a dimmed-but-present paint is not mistaken for a
/// solid one. `None` where the element set none.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Painted {
    pub fill: Option<u32>,
    pub text: Option<u32>,
    pub border: Option<u32>,
    pub fill_alpha: Option<f32>,
    pub text_alpha: Option<f32>,
    pub border_alpha: Option<f32>,
    pub opacity: Option<f32>,
}

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static PAINTED: RefCell<HashMap<&'static str, Vec<Painted>>> = RefCell::new(HashMap::new());
}

pub(crate) fn hsla24(color: gpui::Hsla) -> u32 {
    let rgb = crate::terminal::palette::hsla_to_rgb(color);
    (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
}

/// Records the style `element` currently carries — call after the paints
/// under test are applied, before the element is wrapped.
pub(crate) fn note<E: gpui::Styled>(key: &'static str, element: &mut E) {
    if !ENABLED.get() {
        return;
    }
    let style = element.style();
    let solid = || {
        style
            .background
            .as_ref()
            .and_then(|f| f.color())
            .and_then(|b| b.as_solid())
    };
    let painted = Painted {
        fill: solid().map(hsla24),
        text: style.text.color.map(hsla24),
        border: style.border_color.map(hsla24),
        fill_alpha: solid().map(|c| c.a),
        text_alpha: style.text.color.map(|c| c.a),
        border_alpha: style.border_color.map(|c| c.a),
        opacity: style.opacity,
    };
    PAINTED.with(|p| p.borrow_mut().entry(key).or_default().push(painted));
}

/// Every note recorded for `key`, in the order the elements were built.
pub(crate) fn all(key: &'static str) -> Vec<Painted> {
    PAINTED.with(|p| p.borrow().get(key).cloned().unwrap_or_default())
}

/// Drops all notes; call between draws so a frame's notes stand alone.
pub(crate) fn clear() {
    PAINTED.with(|p| p.borrow_mut().clear());
}

/// Turns capture on for the current thread and drops stale notes; the
/// returned guard clears and disables again when the test ends.
pub(crate) fn scope() -> impl Drop {
    ENABLED.set(true);
    clear();
    ProbeScope
}

struct ProbeScope;

impl Drop for ProbeScope {
    fn drop(&mut self) {
        clear();
        ENABLED.set(false);
    }
}
