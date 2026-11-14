//! The wheel-to-zoom decision, shared by every surface that zooms.
//!
//! The terminal has always turned one modifier plus the wheel into font-size
//! steps (see [`crate::terminal::view`]); the code editor and its Markdown
//! preview zoom the same way, so the two helpers that decide "is this wheel
//! event a zoom" and "how many steps is it worth" live here rather than in
//! the terminal's own module. Nothing in here touches a view — callers bring
//! their own gesture latch and debt, the way a caller brings a mutex.

use crate::core::config::MouseZoomModifier;
use gpui::Modifiers;

/// How far a trackpad has to travel, in lines, to earn one zoom step while
/// the platform modifier is held. A wheel detent is one step on its own, so
/// this only ever applies to the continuous stream a gesture produces.
pub(crate) const ZOOM_SCROLL_LINES: f32 = 3.0;

/// How long a wheel gesture stays "live" after its last event. Long enough
/// to bridge the gaps in a momentum tail, short enough that reaching for the
/// wheel right after a swipe is not mistaken for more of the swipe. The
/// terminal keeps its own copy for its scroll gestures; this one is what the
/// editor's zoom latch counts against.
pub(crate) const SCROLL_GESTURE_IDLE: std::time::Duration = std::time::Duration::from_millis(150);

/// Whether this wheel event is a zoom rather than a scroll.
///
/// Exactly one modifier, and it has to be the configured one: anything
/// alongside it is somebody else's gesture — shift in particular is the escape
/// hatch that scrolls the scrollback out from under a mouse-reporting program.
///
/// Off macOS the platform modifier *is* Ctrl, so `Platform` and `Ctrl` describe
/// the same key there; the setting still round-trips, so a config file shared
/// with a Mac keeps meaning what it meant.
pub(crate) fn zoom_wheel(modifier: MouseZoomModifier, mods: &Modifiers) -> bool {
    if mods.number_of_modifiers() != 1 {
        return false;
    }
    match modifier {
        MouseZoomModifier::Platform => mods.secondary(),
        MouseZoomModifier::Ctrl => mods.control,
        MouseZoomModifier::Alt => mods.alt,
        MouseZoomModifier::None => false,
    }
}

/// How many zoom steps a wheel event is worth, and what is left over for
/// the next one.
///
/// A wheel detent is a discrete click of intent, so it is one step whatever the
/// platform says it covers — macOS calls a single notch five lines, and five
/// steps of font per notch would be unusable. A trackpad has no detents and
/// spends a flick over dozens of events, so those accumulate in `debt` and
/// only pay out once the fingers have travelled [`ZOOM_SCROLL_LINES`].
pub(crate) fn zoom_scroll_steps(lines: f32, debt: f32, gesturing: bool) -> (i32, f32) {
    if !gesturing {
        let step = if lines > 0. {
            1
        } else if lines < 0. {
            -1
        } else {
            0
        };
        return (step, 0.);
    }
    let total = debt + lines;
    let steps = (total / ZOOM_SCROLL_LINES).trunc();
    (steps as i32, total - steps * ZOOM_SCROLL_LINES)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #668: the platform modifier is the wrong key to hardwire a zoom to —
    /// on a Mac it is ⌘, which people are already holding for something
    /// else — so which key zooms is a setting, all the way down to none.
    #[test]
    fn the_zoom_modifier_is_configurable_and_can_be_turned_off() {
        let secondary = Modifiers::secondary_key();
        let alt = Modifiers::alt();
        assert!(zoom_wheel(MouseZoomModifier::Platform, &secondary));
        assert!(
            !zoom_wheel(MouseZoomModifier::None, &secondary),
            "off is off"
        );
        assert!(!zoom_wheel(MouseZoomModifier::Alt, &secondary));
        assert!(zoom_wheel(MouseZoomModifier::Alt, &alt));
        assert!(!zoom_wheel(MouseZoomModifier::Platform, &alt));
        assert!(zoom_wheel(MouseZoomModifier::Ctrl, &Modifiers::control()));
        assert_eq!(
            zoom_wheel(MouseZoomModifier::Platform, &Modifiers::control()),
            !cfg!(target_os = "macos"),
            "off macOS the platform modifier is Ctrl itself"
        );
        // A second modifier is somebody else's gesture, whichever key is bound.
        let both = Modifiers { shift: true, ..alt };
        assert!(!zoom_wheel(MouseZoomModifier::Alt, &both));
        assert!(!zoom_wheel(MouseZoomModifier::Platform, &Modifiers::none()));
    }

    /// A detent is one step however many lines the platform bills it as —
    /// macOS calls a single notch five.
    #[test]
    fn a_wheel_detent_zooms_by_exactly_one_step() {
        assert_eq!(zoom_scroll_steps(4.9, 0., false), (1, 0.));
        assert_eq!(zoom_scroll_steps(-4.9, 0., false), (-1, 0.));
        assert_eq!(zoom_scroll_steps(0., 0., false), (0, 0.));
    }

    /// A trackpad has no detents, so a flick arrives as a stream of slivers.
    /// Paying out a step per sliver would run the font from end to end.
    #[test]
    fn a_trackpad_flick_adds_up_to_whole_steps() {
        let (mut debt, mut steps) = (0., 0);
        for _ in 0..20 {
            let (s, d) = zoom_scroll_steps(1. / 3., debt, true);
            steps += s;
            debt = d;
        }
        assert_eq!(steps, 2, "twenty thirds of a line is two steps, not twenty");
        assert!(debt > 0., "and the remainder was dropped instead of kept");
    }
}
