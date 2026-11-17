use gpui::{
    AnyElement, App, Context, Div, Entity, FontWeight, KeyDownEvent, Stateful, Window, div,
    prelude::*, px, rems,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};

use crate::core::ssh_profile::ForwardRule;
use crate::daemon::protocol::{ForwardStatus, ManagedForward, SshForwardKind, SshForwardRule};
use crate::terminal::view::TerminalView;
use crate::ui::app::Tty59App;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::right_panel::{META, TEXT_MONO};
use crate::ui::text_elision::{ElidedLabel, Mode};

/// The widest the disconnected strip's reason grows before it wraps.
const REASON_MAX_W: f32 = 420.;

/// The strip title's tooltip: the whole sentence, bounded by the window.
fn title_tooltip(
    text: gpui::SharedString,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> gpui::AnyView {
    crate::ui::dialog::bounded_text_tooltip(text, "ssh-strip-title-tooltip-text", window, cx)
}

/// What a forward row and its remove tile tell the dispatch tree they are, so
/// the walk off a removed row can ask where focus landed. They name where
/// focus is, not a binding: no key is bound to either.
const FORWARD_ROW_CONTEXT: &str = "ForwardRow";
const FORWARD_REMOVE_CONTEXT: &str = "ForwardRemove";
const FORWARD_SWITCH_CONTEXT: &str = "ForwardSwitch";

/// Which edits the rows have asked the app for, in a test: the request only,
/// read at the one place the panel hands it on, so a callback that fires twice,
/// or a tile click that bubbles into its row, is seen where the final editor
/// state could not show it.
#[cfg(test)]
pub(crate) mod edit_requests {
    use std::cell::RefCell;

    thread_local! {
        static ASKED: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn note(pane_id: u64, forward_id: u64) {
        ASKED.with(|asked| asked.borrow_mut().push((pane_id, forward_id)));
    }

    pub(crate) fn take() -> Vec<(u64, u64)> {
        ASKED.with(|asked| std::mem::take(&mut *asked.borrow_mut()))
    }
}

/// The focus handle each forward's row was drawn with, in a test: whatever
/// the render itself keyed, noted as it went, never one a test made.
#[cfg(test)]
pub(crate) mod row_handles {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static DRAWN: RefCell<HashMap<u64, gpui::WeakFocusHandle>> = RefCell::new(HashMap::new());
    }

    pub(crate) fn note(forward_id: u64, handle: &gpui::FocusHandle) {
        DRAWN.with(|drawn| drawn.borrow_mut().insert(forward_id, handle.downgrade()));
    }

    pub(crate) fn get(forward_id: u64) -> Option<gpui::FocusHandle> {
        DRAWN.with(|drawn| {
            drawn
                .borrow()
                .get(&forward_id)
                .and_then(|weak| weak.upgrade())
        })
    }

    pub(crate) fn clear() {
        DRAWN.with(|drawn| drawn.borrow_mut().clear());
    }
}

/// The text each failed row's error tooltip was built with, in a test: noted
/// by the real tooltip builder when a real hover asks for it, never changing
/// what it returns.
#[cfg(test)]
pub(crate) mod error_tooltips {
    use std::cell::RefCell;

    thread_local! {
        static BUILT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn note(text: &gpui::SharedString) {
        BUILT.with(|built| built.borrow_mut().push(text.to_string()));
    }

    pub(crate) fn take() -> Vec<String> {
        BUILT.with(|built| std::mem::take(&mut *built.borrow_mut()))
    }
}

/// Which removals the rows have asked the app for, in a test: the request
/// only, read at the one place the panel hands it on. What came of a request
/// is the route's answer and is looked at elsewhere.
#[cfg(test)]
pub(crate) mod remove_requests {
    use std::cell::RefCell;

    thread_local! {
        static ASKED: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn note(pane_id: u64, forward_id: u64) {
        ASKED.with(|asked| asked.borrow_mut().push((pane_id, forward_id)));
    }

    pub(crate) fn take() -> Vec<(u64, u64)> {
        ASKED.with(|asked| std::mem::take(&mut *asked.borrow_mut()))
    }
}

/// The managed-forward form's five text fields, read out of their inputs.
///
/// Split out so the question "do these make a rule?" can be asked without a
/// `Window` and answered the same way twice: `add_managed_forward` needs the
/// rule, and `forward_form` needs to know whether there is one yet — that is
/// what decides whether Add is live and whether the form says what is missing.
pub(crate) struct ForwardFields {
    /// Whether the whole `ssh -L` grammar is on screen. With it off the form
    /// is one number and the rule is derived from it.
    pub(crate) advanced: bool,
    pub(crate) kind: SshForwardKind,
    pub(crate) bind_host: String,
    pub(crate) bind_port: String,
    pub(crate) target_host: String,
    pub(crate) target_port: String,
    pub(crate) description: String,
}

impl ForwardFields {
    /// The rule these fields describe, or `None` while they do not describe
    /// one yet.
    ///
    /// The same conditions the settings sheet's `ForwardRuleForm::collect`
    /// applies, so a rule typed here and a rule typed there are accepted or
    /// refused alike — including port 0, which parses as a `u16` but asks the
    /// OS to pick the port, and there is nowhere in either form to say which
    /// one it picked.
    pub(crate) fn collect(&self) -> Option<SshForwardRule> {
        if !self.advanced {
            return self.simple_rule();
        }
        let bind_port: u16 = self.bind_port.trim().parse().ok().filter(|p| *p > 0)?;
        let (target_host, target_port) = if self.kind == SshForwardKind::Dynamic {
            (String::new(), 0)
        } else {
            let port: u16 = self.target_port.trim().parse().ok().filter(|p| *p > 0)?;
            let host = self.target_host.trim();
            if host.is_empty() {
                return None;
            }
            (host.to_string(), port)
        };
        let bind_host = match self.bind_host.trim() {
            // The panel's own default, and the one the strip's tooltip
            // promises: an empty bind host is loopback, not every interface.
            "" => "127.0.0.1".to_string(),
            host => host.to_string(),
        };
        let description = self.description.trim();
        Some(SshForwardRule {
            kind: self.kind,
            bind_host,
            bind_port,
            target_host,
            target_port,
            description: (!description.is_empty()).then(|| description.to_string()),
            enabled: true,
        })
    }

    /// The rule the short form's single number describes: bring the far
    /// side's own `:port` over to the same number here.
    ///
    /// The bind port is that same number rather than 0 so the address is one
    /// anybody can predict — remote :3000 is localhost:3000. When it is taken
    /// on this machine `add_managed_forward` retries with 0 and the OS picks,
    /// which is a better answer than handing the collision back to be solved
    /// by hand.
    fn simple_rule(&self) -> Option<SshForwardRule> {
        let port: u16 = self.target_port.trim().parse().ok().filter(|p| *p > 0)?;
        Some(SshForwardRule {
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".to_string(),
            bind_port: port,
            target_host: "localhost".to_string(),
            target_port: port,
            // No field for it on screen, so nothing to carry: a description
            // left over from a trip through the advanced form is not something
            // this rule was given.
            description: None,
            enabled: true,
        })
    }

    /// Whether the form is still empty enough that saying what is missing
    /// would be nagging rather than helping — the same restraint the settings
    /// sheet shows through `ForwardRuleForm::is_blank`.
    pub(crate) fn is_blank(&self) -> bool {
        if !self.advanced {
            return self.target_port.trim().is_empty();
        }
        [
            &self.bind_host,
            &self.bind_port,
            &self.target_host,
            &self.target_port,
            &self.description,
        ]
        .iter()
        .all(|v| v.trim().is_empty())
    }
}

/// The whole target as the rule holds it, for the tooltip the elided row hands
/// back. The host keeps the spelling it was saved with, so an IPv6 literal is
/// `fe80::1:8080` here as everywhere else the app writes a forward — the port
/// is a field of its own, never recovered by splitting this string.
fn target_address(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

/// The entry a forward request just appended: the one the panel did not have
/// before it asked.
///
/// Ids come from a counter that only goes up, so "none of the ids from before"
/// names the new entry exactly — and it is the new entry that says whether the
/// rule is listening or why it is not.
pub(crate) fn added_forward<'a>(
    before: &[u64],
    list: &'a [ManagedForward],
) -> Option<&'a ManagedForward> {
    list.iter().find(|m| !before.contains(&m.id))
}

/// The rule a live forward was made from.
///
/// An edit removes the old forward before adding the new one, so when the new
/// one will not come up this is what puts the old one back.
pub(crate) fn rule_of(forward: &ManagedForward) -> SshForwardRule {
    SshForwardRule {
        kind: forward.kind,
        bind_host: forward.bind_host.clone(),
        bind_port: forward.bind_port,
        target_host: forward.target_host.clone(),
        target_port: forward.target_port,
        description: forward.description.clone(),
        enabled: forward.enabled,
    }
}

/// Which of a saved host's rules a live forward was opened from, among
/// those not already switched to `enabled` — the one a switch in the panel
/// has to be written back to.
///
/// Matched on the whole mapping, since a live forward carries no reference to
/// the rule it came from. A saved rule always names its port, so the port the
/// forward bound is the one the rule asked for. A forward added in the panel
/// matches nothing and stays as temporary as it always was.
pub(crate) fn saved_rule_index(
    rules: &[ForwardRule],
    forward: &ManagedForward,
    enabled: bool,
) -> Option<usize> {
    rules.iter().position(|r| {
        let saved = crate::ui::ssh_connect::map_forward(r);
        saved.enabled != enabled
            && saved.kind == forward.kind
            && saved.bind_host == forward.bind_host
            && saved.bind_port == forward.bind_port
            && saved.target_host == forward.target_host
            && saved.target_port == forward.target_port
    })
}

/// The error a forward's row shows beneath its mapping: the daemon's own
/// message, only while the forward is switched on and failed.
///
/// A switched-off forward reports an error only because it binds nothing, so it
/// has none to show. Borrowed from the forward, so asking costs nothing for the
/// rows that are not failing.
///
/// Backported from l0ng-ai/tty7 e7fee512 (which kept the mapping on the first
/// line and put the error on a second one), minus its flat mapping, which would
/// lose the bind chip and the elided host this row's mapping is built from.
fn row_error(forward: &ManagedForward) -> Option<&str> {
    match &forward.status {
        ForwardStatus::Error(msg) if forward.enabled => Some(msg.as_str()),
        _ => None,
    }
}

/// The box of one forward row: the element `forward_row` fills in with its
/// badge, text, switch and remove tile and its handlers, and the one the ring
/// scene tests mount on its own. `own` is the row's focus handle, `revealed`
/// whether focus is anywhere in the row, `hover_fill` the sidebar's hover rung.
fn forward_row_shell(
    forward_id: usize,
    group: gpui::SharedString,
    own: &gpui::FocusHandle,
    revealed: bool,
    hover_fill: u32,
) -> Stateful<Div> {
    h_flex()
        .id(("panel-forward", forward_id))
        .group(group)
        .items_center()
        .gap(px(8.))
        // The port and process rows' box: 26px, padded `ROW_INSET` so the
        // text lands on the panel's 20px column and the trailing tile ends
        // 20px from the edge, on the panel's row corner. A described
        // forward grows by its second line instead of being clipped to one.
        .min_h(px(26.))
        .py(px(3.))
        .px(px(crate::ui::right_panel::ROW_INSET))
        .rounded(crate::ui::right_panel::ROW_FILL_RADIUS)
        .cursor_pointer()
        .track_focus(own)
        .key_context(FORWARD_ROW_CONTEXT)
        .hover(move |s| s.bg(gpui::rgb(hover_fill)))
        .when(revealed, move |s| s.bg(gpui::rgb(hover_fill)))
}

/// The shared keyboard ring around a forward row, shown while the row itself
/// (not the remove tile or the switch inside it, which draw their own) has
/// focus. The one call `forward_row` and the ring scene tests both make.
fn ring_row(own: &gpui::FocusHandle, row: impl IntoElement) -> crate::ui::theme::FocusRing {
    crate::ui::theme::FocusRing::new(
        own,
        crate::ui::theme::RingPlacement::Outside,
        crate::ui::right_panel::ROW_FILL_RADIUS,
        row,
    )
}

/// Take the focus off a forward row that is about to go, onto its
/// neighbour: the next row, else the previous one. Says whether it found one.
///
/// Walks the tab stops the last frame mounted, so "next" is whatever is
/// next on screen — first, last, only or reordered row alike. Nothing about
/// a row's children is assumed, not their number nor their order: a step
/// that lands inside a row (on its remove tile, its switch, whatever is
/// made focusable in it) is stepped past until focus rests on a row
/// itself, told apart by the contexts the dispatch tree already holds for
/// the focused element. The walk gives up on no progress, a wrap onto a
/// stop already visited, no focus at all, or leaving the rows, and the
/// caller takes the key from there.
fn walk_to_neighbour_row(own: &gpui::FocusHandle, window: &mut Window, cx: &mut App) -> bool {
    let in_rows = |window: &Window| {
        window
            .context_stack()
            .iter()
            .any(|c| c.contains(FORWARD_ROW_CONTEXT))
    };
    // On a row's own stop, not on anything inside one.
    let on_row_itself = |window: &Window| {
        let stack = window.context_stack();
        let has = |name: &str| stack.iter().any(|c| c.contains(name));
        has(FORWARD_ROW_CONTEXT) && !has(FORWARD_REMOVE_CONTEXT) && !has(FORWARD_SWITCH_CONTEXT)
    };
    // `true` once focus rests on the row itself of some row other than
    // `own`.
    let walk = |window: &mut Window, cx: &mut App, step: fn(&mut Window, &mut App)| {
        let mut seen = vec![window.focused(cx)];
        loop {
            if !own.contains_focused(window, cx) {
                if on_row_itself(&*window) {
                    return true;
                }
                if !in_rows(&*window) {
                    return false;
                }
            }
            step(window, cx);
            let now = window.focused(cx);
            if now.is_none() || seen.contains(&now) {
                return false;
            }
            seen.push(now);
        }
    };
    if walk(&mut *window, &mut *cx, |w, cx| w.focus_next(cx)) {
        return true;
    }
    window.focus(own, cx);
    walk(&mut *window, &mut *cx, |w, cx| w.focus_prev(cx))
}

impl Tty59App {
    pub(crate) fn render_ssh_status_strip(
        &self,
        leaf: &Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = leaf.read(cx);
        view.ssh_phase()?;
        if !view.ssh_disconnected() {
            return None;
        }
        let host = view
            .terminal
            .ssh_endpoint()
            .map(|(h, _)| h)
            .or_else(|| view.remote_context().map(|c| c.target))
            .unwrap_or_default();

        // Why it ended. The strip used to say only that it *had* ended, so a
        // rejected key and a dropped network read identically — and the reason
        // scrolled away with the pane's own output.
        let reason = match view.ssh_phase() {
            Some(crate::daemon::protocol::SshPhase::Failed { reason }) if !reason.is_empty() => {
                Some(reason)
            }
            _ => None,
        };
        // A saved connection is editable; a one-off `user@host` is not, and
        // offering to edit one would open an empty page.
        let profile = view
            .ssh_spec()
            .and_then(|s| s.profile_id.clone())
            .and_then(|id| uuid::Uuid::parse_str(&id).ok());

        let theme = cx.theme();
        let (danger, foreground, muted) = (theme.danger, theme.foreground, theme.muted_foreground);

        // The chord was spelled "⌘⇧R" here. That is one platform's
        // rendering of one default: off macOS it names a modifier the
        // keyboard does not have, and either way it kept saying ⌘⇧R after
        // the user had rebound Reconnect to something else. Ask the keymap
        // for the first stroke, and say nothing at all when the action carries
        // no binding.
        let chord = crate::ui::keymap::effective_key("RestartSshSession", cx)
            .and_then(|spec| crate::ui::keymap::key_chords(&spec).into_iter().next())
            .map(|keys| crate::ui::dialog::chord(keys, cx));
        // A chord that is built is not a chord that is drawn: this is what lets a
        // test ask the real window whether it was laid out.
        #[cfg(test)]
        let chord = chord.map(|chord| chord.debug_selector(|| "ssh-strip-chord".into()));

        let bar = crate::ui::notice::pill(danger, cx)
            .child({
                // The title names the host, and a long host name has no break to
                // wrap at: it is elided in the middle, which keeps the end of the
                // name that tells one host from another, and the tooltip is where
                // the whole sentence is.
                let full: gpui::SharedString = if host.is_empty() {
                    t(L10nKey::ForwardDisconnected).to_string()
                } else {
                    t_fmt(L10nKey::ForwardDisconnectedFrom, &[("host", &host)])
                }
                .into();
                let tip = full.clone();
                let title = div()
                    .id("ssh-strip-title")
                    .min_w_0()
                    .max_w_full()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(foreground)
                    .tooltip(move |window, cx| title_tooltip(tip.clone(), window, cx))
                    .child(ElidedLabel::new("ssh-strip-title-text", full, Mode::Middle));
                #[cfg(test)]
                let title = title.debug_selector(|| "ssh-strip-title".into());
                title
            })
            .children(reason.map(|reason| {
                // The reason is whatever ssh said, and it runs long more often
                // than not — a rejected key's account used to be one truncated
                // line with "…" for the rest, its pane's own output having
                // scrolled away. It wraps to two lines instead, in the muted
                // ink: the dot is the strip's one red. The tooltip still gives
                // back the whole sentence past the second line.
                let full = reason.clone();
                let line = div()
                    .id("ssh-strip-reason")
                    .min_w_0()
                    .max_w(px(REASON_MAX_W))
                    .line_clamp(2)
                    .text_color(muted)
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                    })
                    .child(reason);
                #[cfg(test)]
                let mut line = line;
                #[cfg(test)]
                crate::ui::paint_probe::note("ssh.reason", &mut line);
                #[cfg(test)]
                let line = line.debug_selector(|| "ssh-strip-reason".into());
                line
            }))
            .children(chord)
            .child({
                // The strip's one action, as v4's primary: inverted neutral on
                // the popover fill the pill floats on. The red is the edge
                // and the reason; the button does not repeat it.
                let reconnect = crate::ui::dialog::primary_button(
                    "ssh-reconnect",
                    crate::ui::i18n::t(crate::ui::i18n::L10nKey::Reconnect),
                    true,
                    crate::ui::dialog::popover_rungs(cx),
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.restart_ssh_session(window, cx)));
                #[cfg(test)]
                let reconnect = reconnect.debug_selector(|| "ssh-strip-reconnect".into());
                reconnect
            })
            .children(profile.map(|id| {
                // Quiet until pointed at, so Reconnect is the one thing that
                // leads; the hover is the ghost button's own.
                let edit = Button::new("ssh-edit-profile")
                    .label(crate::ui::i18n::t(crate::ui::i18n::L10nKey::SshEditProfile))
                    .ghost()
                    .small()
                    .text_color(muted)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_ssh_profile_in_settings(id, window, cx)
                    }));
                #[cfg(test)]
                let mut edit = edit.debug_selector(|| "ssh-strip-edit-host".into());
                #[cfg(test)]
                crate::ui::paint_probe::note("ssh.edit-host", &mut edit);
                edit
            }));
        #[cfg(test)]
        let bar = bar.debug_selector(|| "ssh-strip-pill".into());
        // The bar only. `body_area` anchors it, together with whatever else is
        // floating down there — this used to place itself at `bottom_4` and so
        // did the remote input notice, on the same container.
        Some(bar.into_any_element())
    }

    /// Take the focus off a forward row that is about to go: onto its
    /// neighbour row, else the pane. See `walk_to_neighbour_row`.
    fn leave_forward_row(&self, own: &gpui::FocusHandle, window: &mut Window, cx: &mut App) {
        if !walk_to_neighbour_row(own, window, cx) {
            self.focus_active(window, cx);
        }
    }

    /// The one way a forward row asks for its forward to go, from the mouse
    /// and the keyboard alike: the existing removal, once, and then the focus
    /// off the row if and only if it really went. A removal the far side did
    /// not answer leaves the list, the row and the focus where they were.
    fn remove_forward_from_row(
        &mut self,
        pane_id: u64,
        forward_id: u64,
        own: &gpui::FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inside = own.contains_focused(window, cx);
        self.remove_managed_forward(pane_id, forward_id, cx);
        let gone = !self
            .loopback_panel
            .managed
            .iter()
            .any(|m| m.id == forward_id);
        if inside && gone {
            self.leave_forward_row(own, window, cx);
        }
    }

    pub(crate) fn forward_row(
        &self,
        forward: &ManagedForward,
        mono: &gpui::SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> crate::ui::theme::FocusRing {
        // The row's name, which hover already groups it by: one per daemon id,
        // and the daemon's ids only ever go up.
        let group = gpui::SharedString::from(format!("panel-forward-{}", forward.id));
        // Kept by the framework for as long as the forward's row is drawn,
        // under that same name, so a row that moves in the list keeps its
        // focus and one that is gone takes it away.
        let own = window
            .use_keyed_state(group.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone()
            .tab_stop(true);
        // Focus anywhere in the row — the row, or its remove tile — shows the
        // remove tile the way the pointer does.
        let revealed = own.contains_focused(window, cx);
        // What the real render keyed, for a test to say which row has focus.
        #[cfg(test)]
        row_handles::note(forward.id, &own);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let sf = cx.global::<crate::ui::presets::Surfaces>().sidebar;
        let letter = match forward.kind {
            SshForwardKind::Local => "L",
            SshForwardKind::Remote => "R",
            SshForwardKind::Dynamic => "D",
        };
        // A switched-off forward has an error status only because it binds
        // nothing; it is drawn quiet, not red.
        let off = !forward.enabled;
        let error = row_error(forward);
        let errored = error.is_some();
        // Loopback is the unmarked case and the chip is the bare port; any other
        // bind says which interface it listens on.
        let bind_host = (!matches!(forward.bind_host.as_str(), "127.0.0.1" | "localhost" | ""))
            .then(|| gpui::SharedString::from(forward.bind_host.as_str()));
        let pane_id = forward.pane_id;
        let forward_id = forward.id;
        let forward_for_edit = forward.clone();
        // A switched-off rule reads dimmer through its muted ink and the
        // absence of the chip's wash, never through a container fade: half
        // opacity dropped muted text under 4.5:1 on soft palettes. A live one
        // wears the same neutral wash and floored ink as a port's chip; the
        // accent is not spent on a number that is not an error.
        let (chip_fill, chip_ink) = match off {
            true => (gpui::transparent_black(), muted),
            false => (
                gpui::Hsla::from(gpui::rgb(crate::ui::theme::fill_pole(cx)))
                    .opacity(crate::ui::presets::PORT_CHIP_WASH),
                gpui::rgb(crate::ui::theme::chrome_ink(cx).port_chip_ink).into(),
            ),
        };
        // A bind that names an interface makes a chip too wide to share the
        // line with a target on a narrow panel, and what each would lose to
        // fit is a choice with no right answer. The line wraps instead: the
        // target takes its own full-width line below the chip when the two
        // cannot sit side by side, and nothing gives way to the other. The
        // target then sizes from its content (`flex_auto`) rather than from
        // zero (`flex_1`), which would never be wider than the room left on
        // the chip's line and so would never wrap.
        let wraps = bind_host.is_some();
        let grows = |line: Div| {
            if wraps {
                line.flex_auto()
            } else {
                line.flex_1()
            }
        };
        let mono_text = |line: Div| {
            line.min_w_0()
                .text_size(rems(TEXT_MONO))
                .font_family(mono.clone())
                .text_color(muted)
        };
        // What the row says after the chip. A listening or switched-off rule
        // is a mapping: the target host gives way to the room it is handed and
        // its `:port` is a sibling that never does, so what the rule opens is
        // always readable, whatever the forward's status: a failed rule says
        // where it goes and the error has a line of its own below. A SOCKS
        // proxy has no target to split.
        let target = match forward.kind {
            SshForwardKind::Dynamic => mono_text(grows(div()).truncate())
                .child("SOCKS")
                .into_any_element(),
            _ => {
                // The host as the rule holds it: an IPv6 literal keeps the
                // spelling it was saved with, and the port is a field of its
                // own, so no `:` is ever taken for a separator. The tooltip
                // builds the whole address only when it is asked for.
                let host = gpui::SharedString::from(forward.target_host.as_str());
                let port = forward.target_port;
                let full_host = host.clone();
                let port_text = div().flex_none().child(format!(":{port}"));
                #[cfg(test)]
                let port_text = port_text.debug_selector(|| format!("forward-port-{forward_id}"));
                let line = mono_text(grows(h_flex()).overflow_hidden())
                    .id(("panel-forward-target-row", forward_id as usize))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(target_address(&full_host, port))
                            .build(window, cx)
                    })
                    .child(div().flex_none().mr(px(5.)).child("→"))
                    .child(ElidedLabel::new(
                        ("panel-forward-target", forward_id as usize),
                        host,
                        Mode::Middle,
                    ))
                    .child(port_text);
                #[cfg(test)]
                let mut line = line;
                #[cfg(test)]
                crate::ui::paint_probe::note("forwards.mapping", &mut line);
                #[cfg(test)]
                let line = line.debug_selector(|| format!("forward-target-{forward_id}"));
                line.into_any_element()
            }
        };
        let bind_port = forward.bind_port;
        let chip = match bind_host {
            None => {
                crate::ui::right_panel::info_chip(&bind_port.to_string(), chip_fill, chip_ink, mono)
            }
            // The same pill, its host elided against the room it is handed and
            // its port a sibling that never gives way. The tooltip builds the
            // whole address when asked.
            Some(host) => {
                let full_host = host.clone();
                let bind_port_text = div().flex_none().child(format!(":{bind_port}"));
                #[cfg(test)]
                let bind_port_text =
                    bind_port_text.debug_selector(|| format!("forward-bind-port-{forward_id}"));
                let shell = crate::ui::right_panel::pill_shell(chip_fill, chip_ink, mono)
                    .id(("panel-forward-bind", forward_id as usize))
                    .flex()
                    .items_center()
                    // Past the line itself the host inside is elided, the
                    // port after it kept; short of that it is whole.
                    .max_w_full()
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(target_address(&full_host, bind_port))
                            .build(window, cx)
                    })
                    .child(ElidedLabel::new(
                        ("panel-forward-bind-host", forward_id as usize),
                        host,
                        Mode::Middle,
                    ))
                    .child(bind_port_text);
                #[cfg(test)]
                let mut shell = shell;
                #[cfg(test)]
                crate::ui::paint_probe::note("forwards.bind-chip", &mut shell);
                #[cfg(test)]
                let shell = shell.debug_selector(|| format!("forward-chip-{forward_id}"));
                shell.into_any_element()
            }
        };
        let column = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(1.))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.))
                    .when(wraps, |line| line.flex_wrap())
                    .child(chip)
                    .child(target),
            )
            // The error is the more urgent of the two lines under the mapping,
            // so while a forward is on and failing it takes the description's
            // place; the description is a click away in the edit form. The
            // mapping line above is whatever it already was, wrapped or not.
            .when_some(
                error.map(|msg| {
                    // One string for what is drawn and for the whole message
                    // the tooltip gives back.
                    let shown = gpui::SharedString::from(msg);
                    let tip = shown.clone();
                    let line = div()
                        .id(("panel-forward-error", forward_id as usize))
                        .truncate()
                        .text_size(rems(META))
                        // The cached chrome role, derived once per theme change
                        // and floored on the sidebar's resting and hover fills;
                        // not the semantic danger ink nor the toolkit's raw one.
                        .text_color(gpui::rgb(
                            crate::ui::theme::chrome_ink(cx).forward_error_ink,
                        ))
                        .tooltip(move |window, cx| {
                            let text = tip.clone();
                            #[cfg(test)]
                            error_tooltips::note(&text);
                            gpui_component::tooltip::Tooltip::new(text).build(window, cx)
                        })
                        .child(shown);
                    #[cfg(test)]
                    let mut line = line;
                    #[cfg(test)]
                    crate::ui::paint_probe::note("forwards.error", &mut line);
                    #[cfg(test)]
                    let line = line.debug_selector(|| format!("forward-error-{forward_id}"));
                    line
                }),
                |this, line| this.child(line),
            )
            .when(error.is_none(), |this| {
                this.when_some(forward.description.clone(), |this, desc| {
                    let line = div()
                        .truncate()
                        .text_size(rems(META))
                        .text_color(muted)
                        .child(desc);
                    #[cfg(test)]
                    let line = line.debug_selector(|| format!("forward-description-{forward_id}"));
                    this.child(line)
                })
            });
        #[cfg(test)]
        let mut column = column;
        #[cfg(test)]
        crate::ui::paint_probe::note("forwards.column", &mut column);

        let row = forward_row_shell(forward_id as usize, group.clone(), &own, revealed, sf.hover)
            // Enter and Space on the focused row come through here as a click
            // (gpui turns the key-up into one), so the keyboard opens the same
            // editor the pointer does. Only the row's own focus does: a key on
            // the remove tile or the switch is theirs.
            .on_click(cx.listener(move |this, _, window, cx| {
                this.edit_managed_forward(forward_for_edit.clone(), window, cx)
            }))
            // Delete (on macOS, where the key labelled delete is `backspace`,
            // that too) from the row or anything inside it asks for the remove
            // tile's own removal. Scoped to the row's element, so the terminal
            // and every input, which are not inside it, never see it taken.
            .on_key_down({
                let own = own.clone();
                cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    let key = event.keystroke.key.as_str();
                    let is_delete =
                        key == "delete" || (cfg!(target_os = "macos") && key == "backspace");
                    if is_delete && !event.keystroke.modifiers.modified() {
                        cx.stop_propagation();
                        this.remove_forward_from_row(pane_id, forward_id, &own, window, cx);
                    }
                })
            })
            .child(crate::ui::right_panel::git_badge(
                letter,
                if errored { theme.danger } else { muted },
                mono,
            ))
            .child(column)
            // Always shown, unlike Remove beside it: whether a forward is on is
            // state to read at a glance, not an action to go looking for.
            .child({
                let tip = if off {
                    t(L10nKey::ForwardTooltipTurnOn)
                } else {
                    t(L10nKey::ForwardTooltipTurnOff)
                };
                div()
                    .id(("panel-forward-enabled-tip", forward_id as usize))
                    .flex_shrink_0()
                    // Whatever is made focusable in here is part of the row,
                    // not a row: the walk off a removed row steps past it.
                    .key_context(FORWARD_SWITCH_CONTEXT)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tip).build(window, cx)
                    })
                    .child(
                        crate::ui::settings::kit::switch((
                            "panel-forward-enabled",
                            forward_id as usize,
                        ))
                        .checked(forward.enabled)
                        .small()
                        .on_click(cx.listener(
                            move |this, on: &bool, window, cx| {
                                this.set_managed_forward_enabled(
                                    pane_id, forward_id, *on, window, cx,
                                )
                            },
                        )),
                    )
            })
            .child({
                let slot = div()
                    .flex_shrink_0()
                    .key_context(FORWARD_REMOVE_CONTEXT)
                    // Shown for the pointer and for focus anywhere in the row,
                    // at once: it is not a thing to fade in under the key.
                    .opacity(if revealed { 1. } else { 0. })
                    .group_hover(group, |s| s.opacity(1.))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
                #[cfg(test)]
                let mut slot = slot;
                #[cfg(test)]
                crate::ui::paint_probe::note("forwards.remove-slot", &mut slot);
                slot.child(
                    crate::ui::tab_strip::chrome_tile(
                        Button::new(("panel-forward-del", forward_id as usize))
                            .icon(IconName::Close)
                            .xsmall(),
                        false,
                        cx,
                    )
                    .w(px(crate::ui::tab_strip::MIN_TARGET))
                    .h(px(crate::ui::tab_strip::MIN_TARGET))
                    .rounded(px(crate::ui::tab_strip::RAIL_TILE_RADIUS))
                    .tooltip(t(L10nKey::ForwardTooltipRemove))
                    .on_click({
                        let own = own.clone();
                        cx.listener(move |this, _, window, cx| {
                            this.remove_forward_from_row(pane_id, forward_id, &own, window, cx)
                        })
                    }),
                )
            });
        #[cfg(test)]
        let row = row.debug_selector(|| format!("forward-row-{forward_id}"));
        let ringed = ring_row(&own, row);
        #[cfg(test)]
        let ringed = ringed.tagged(format!("forward-ring-{forward_id}"));
        ringed
    }

    /// The form that adds a forward.
    ///
    /// Short by default — one number, because "bring the remote's :3000 over
    /// here" is what nearly every hand-built forward is, and collecting one
    /// number through five fields is what made this read like paperwork. The
    /// full `ssh -L` grammar is one disclosure away for the rest.
    pub(crate) fn forward_form(&self, pane_id: u64, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let sf = cx.global::<crate::ui::presets::Surfaces>().sidebar;
        let kind = self.loopback_panel.mf_kind;
        let editing = self.loopback_panel.mf_editing.is_some();
        let advanced = self.loopback_panel.mf_advanced;
        let fields = self.managed_forward_fields(cx);
        // The form used to accept a click on Add and then do nothing at all
        // when the fields did not make a rule. Now Add is only live when there
        // is something to add, and the line below the form says what is still
        // missing — but not while the form has barely been touched.
        let rule = fields.collect();
        let complete = rule.is_some();
        let incomplete = !complete && !fields.is_blank();
        let selected = match kind {
            SshForwardKind::Local => 0,
            SshForwardKind::Remote => 1,
            SshForwardKind::Dynamic => 2,
        };
        let needs_target = kind != SshForwardKind::Dynamic;

        let pair = |label: &'static str,
                    host: &Entity<gpui_component::input::InputState>,
                    port: &Entity<gpui_component::input::InputState>| {
            h_flex()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .flex_none()
                        .w(px(30.))
                        .text_size(rems(META))
                        .text_color(muted)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(Input::new(host).xsmall()))
                .child(div().text_size(rems(META)).text_color(muted).child(":"))
                .child(div().w(px(52.)).child(Input::new(port).xsmall()))
        };

        // Form text, not rows: on the panel's text column.
        v_flex()
            .px(px(crate::ui::right_panel::TEXT_INSET))
            .pt(px(6.))
            .pb(px(2.))
            .gap(px(5.))
            // Escape backs out of the form, the way it backs out of the sftp
            // edit box and every sheet the app puts up. Return is answered by
            // the boxes themselves — see `arm_managed_forward_form` — because
            // an Input takes Return before it can bubble to here.
            .on_key_down(
                cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.close_managed_forward_form(window, cx);
                    }
                }),
            )
            .when(!advanced, |form| {
                let port_input = div()
                    .w(px(64.))
                    .child(Input::new(&self.loopback_panel.mf_target_port).xsmall());
                #[cfg(test)]
                let port_input = port_input.debug_selector(|| "forward-port-input".into());
                let port_row = h_flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_none()
                            .text_size(rems(META))
                            .text_color(muted)
                            .child(t(L10nKey::ForwardPortLabel)),
                    )
                    .child(port_input);
                #[cfg(test)]
                let port_row = port_row.debug_selector(|| "forward-port-row".into());
                form.child(port_row)
                    // Where the port will come out, said before the click
                    // rather than after it, and on a line of its own: beside
                    // the field it was the sentence a 260px panel cut off. The
                    // number is the same one on both ends unless it is taken
                    // here, which is the case the row itself reports once the
                    // forward exists.
                    .children(rule.as_ref().map(|r| {
                        let hint = div().text_size(rems(META)).text_color(muted).child(t_fmt(
                            L10nKey::ForwardPortHere,
                            &[("port", &r.bind_port.to_string())],
                        ));
                        #[cfg(test)]
                        let hint = hint.debug_selector(|| "forward-port-hint".into());
                        hint
                    }))
            })
            .when(advanced, |form| {
                form.child(self.segmented(
                    "ssh-managed-forward-kind",
                    &[
                        t(L10nKey::ForwardLocal),
                        t(L10nKey::ForwardRemote),
                        t(L10nKey::ForwardDynamic),
                    ],
                    selected,
                    cx,
                    move |this, ix, _window, cx| {
                        let kind = match ix {
                            1 => SshForwardKind::Remote,
                            2 => SshForwardKind::Dynamic,
                            _ => SshForwardKind::Local,
                        };
                        this.set_managed_forward_kind(kind, cx);
                    },
                ))
                .child({
                    let bind_pair = pair(
                        t(L10nKey::ForwardBindLabel),
                        &self.loopback_panel.mf_bind_host,
                        &self.loopback_panel.mf_bind_port,
                    );
                    #[cfg(test)]
                    let mut bind_pair = bind_pair;
                    #[cfg(test)]
                    crate::ui::paint_probe::note("forwards.pair", &mut bind_pair);
                    bind_pair
                })
                // A Dynamic rule has no target to name, so its pair is left
                // out instead of drawn faded: no legible label sits below the
                // floor. Local and Remote keep theirs.
                .when(needs_target, |form| {
                    let target_pair = pair(
                        t(L10nKey::ForwardToLabel),
                        &self.loopback_panel.mf_target_host,
                        &self.loopback_panel.mf_target_port,
                    );
                    #[cfg(test)]
                    let mut target_pair = target_pair;
                    #[cfg(test)]
                    crate::ui::paint_probe::note("forwards.pair", &mut target_pair);
                    form.child(target_pair)
                })
                .child(Input::new(&self.loopback_panel.mf_description).xsmall())
            })
            .when(incomplete, |form| {
                form.child(div().text_size(rems(META)).text_color(danger).child(
                    match (advanced, needs_target) {
                        (false, _) => t(L10nKey::ForwardNeedsPort),
                        (true, true) => t(L10nKey::SettingsFwdNeedsBoth),
                        (true, false) => t(L10nKey::SettingsFwdNeedsListen),
                    },
                ))
            })
            .when_some(self.loopback_panel.mf_error.clone(), |form, msg| {
                form.child(div().text_size(rems(META)).text_color(danger).child(msg))
            })
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(4.))
                    .pt(px(1.))
                    // An edit is always shown whole — the short form cannot
                    // spell what an existing rule may contain — so there is
                    // nothing to disclose and no toggle to offer.
                    .child(div().when(!editing, |slot| {
                        slot.child(
                            Button::new(("ssh-managed-forward-advanced", pane_id))
                                .label(if advanced {
                                    t(L10nKey::ForwardSimpleToggle)
                                } else {
                                    t(L10nKey::ForwardAdvancedToggle)
                                })
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.toggle_managed_forward_advanced(cx)
                                })),
                        )
                    }))
                    .child(
                        h_flex()
                            .gap(px(4.))
                            .child(
                                Button::new(("ssh-managed-forward-cancel", pane_id))
                                    .label(t(L10nKey::Cancel))
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.close_managed_forward_form(window, cx)
                                    })),
                            )
                            .child(
                                // The panel's primary, on the panel's own
                                // surface: the same library button, id and
                                // disabled gate, in the shared recipe.
                                crate::ui::dialog::primary_button(
                                    ("ssh-managed-forward-add", pane_id),
                                    if editing {
                                        t(L10nKey::Save)
                                    } else {
                                        t(L10nKey::ForwardAdd)
                                    },
                                    complete,
                                    sf,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.add_managed_forward(pane_id, window, cx)
                                    },
                                )),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(kind: SshForwardKind, bind_port: &str, host: &str, port: &str) -> ForwardFields {
        ForwardFields {
            advanced: true,
            kind,
            bind_host: "127.0.0.1".to_string(),
            bind_port: bind_port.to_string(),
            target_host: host.to_string(),
            target_port: port.to_string(),
            description: String::new(),
        }
    }

    fn managed(id: u64, bind_port: u16) -> ManagedForward {
        ManagedForward {
            id,
            pane_id: 7,
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".to_string(),
            bind_port,
            target_host: "10.0.0.5".to_string(),
            target_port: 80,
            description: Some("the staging box".to_string()),
            status: ForwardStatus::Listening,
            enabled: true,
        }
    }

    #[test]
    fn a_complete_local_rule_is_collected() {
        let rule = fields(SshForwardKind::Local, "8080", "10.0.0.5", "80")
            .collect()
            .expect("a bind port and a target make a rule");
        assert_eq!(rule.bind_port, 8080);
        assert_eq!(rule.target_host, "10.0.0.5");
        assert_eq!(rule.target_port, 80);
        assert_eq!(rule.description, None);
    }

    #[test]
    fn a_half_typed_rule_is_not_a_rule() {
        assert!(
            fields(SshForwardKind::Local, "", "10.0.0.5", "80")
                .collect()
                .is_none()
        );
        assert!(
            fields(SshForwardKind::Local, "8080", "", "80")
                .collect()
                .is_none()
        );
        assert!(
            fields(SshForwardKind::Local, "8080", "10.0.0.5", "")
                .collect()
                .is_none()
        );
        assert!(
            fields(SshForwardKind::Local, "http", "10.0.0.5", "80")
                .collect()
                .is_none(),
            "a service name is not a port"
        );
    }

    #[test]
    fn port_zero_is_refused_rather_than_quietly_ephemeral() {
        assert!(
            fields(SshForwardKind::Local, "0", "10.0.0.5", "80")
                .collect()
                .is_none(),
            "there is nowhere in this form to say which port the OS picked"
        );
        assert!(
            fields(SshForwardKind::Local, "8080", "10.0.0.5", "0")
                .collect()
                .is_none()
        );
    }

    /// The short form: one number, and the rule it makes reaches the far
    /// side's own loopback and comes out here under the same number.
    #[test]
    fn one_number_is_a_whole_rule_in_the_short_form() {
        let mut form = fields(SshForwardKind::Local, "", "", "3000");
        form.advanced = false;
        // Whatever a trip through the advanced form left behind is not part of
        // what the short form was asked for.
        form.bind_host = "0.0.0.0".to_string();
        form.description = "left over".to_string();
        let rule = form.collect().expect("a port is enough");
        assert_eq!(rule.kind, SshForwardKind::Local);
        assert_eq!(rule.bind_host, "127.0.0.1");
        assert_eq!(rule.bind_port, 3000, "the address has to be predictable");
        assert_eq!(rule.target_host, "localhost");
        assert_eq!(rule.target_port, 3000);
        assert_eq!(rule.description, None);
    }

    #[test]
    fn the_short_form_is_blank_until_the_port_is_typed() {
        let mut form = fields(SshForwardKind::Local, "8080", "10.0.0.5", "");
        form.advanced = false;
        assert!(
            form.is_blank(),
            "fields the short form does not show cannot make it dirty"
        );
        assert!(form.collect().is_none());
        form.target_port = "http".to_string();
        assert!(
            !form.is_blank(),
            "a typed port that is not one still counts"
        );
        assert!(form.collect().is_none());
    }

    #[test]
    fn a_socks_proxy_needs_nothing_but_a_port_to_listen_on() {
        let rule = fields(SshForwardKind::Dynamic, "1080", "", "")
            .collect()
            .expect("a dynamic forward has no target");
        assert_eq!(rule.bind_port, 1080);
        assert_eq!(rule.target_host, "");
        assert_eq!(rule.target_port, 0);
    }

    #[test]
    fn an_untouched_form_is_blank_and_a_touched_one_is_not() {
        let mut form = fields(SshForwardKind::Local, "", "", "");
        form.bind_host = String::new();
        assert!(form.is_blank());
        form.description = "  ".to_string();
        assert!(form.is_blank(), "whitespace is not typing");
        form.bind_port = "8".to_string();
        assert!(!form.is_blank());
    }

    #[test]
    fn a_rule_survives_the_round_trip_through_a_live_forward() {
        let rule = rule_of(&managed(3, 8080));
        assert_eq!(rule.kind, SshForwardKind::Local);
        assert_eq!(rule.bind_host, "127.0.0.1");
        assert_eq!(rule.bind_port, 8080);
        assert_eq!(rule.target_host, "10.0.0.5");
        assert_eq!(rule.target_port, 80);
        assert_eq!(rule.description.as_deref(), Some("the staging box"));
    }

    #[test]
    fn the_entry_an_add_appended_is_the_one_that_was_not_there_before() {
        let list = vec![managed(1, 8080), managed(4, 9090)];
        let added = added_forward(&[1], &list).expect("the new entry");
        assert_eq!(added.id, 4);
        assert!(
            added_forward(&[1, 4], &list).is_none(),
            "nothing was added, so there is nothing to point at"
        );
    }

    #[test]
    fn a_switched_off_forward_keeps_its_switch_through_an_edit() {
        let mut off = managed(3, 8080);
        off.enabled = false;
        assert!(!rule_of(&off).enabled);
        assert!(rule_of(&managed(3, 8080)).enabled);
    }

    /// The panel's switch is written back to the saved host only for the rule
    /// the forward was opened from, and a rule added in the panel — which no
    /// saved rule describes — is left as temporary as it always was.
    #[test]
    fn a_switch_is_written_back_to_the_saved_rule_it_came_from() {
        use crate::core::ssh_profile::{ForwardKind, HostPort};
        let saved = |target: &str, enabled: bool| ForwardRule {
            kind: ForwardKind::Local,
            bind: HostPort::new("127.0.0.1", 8080),
            target: HostPort::new(target, 80),
            description: "the staging box".to_string(),
            enabled,
        };
        let rules = vec![saved("10.0.0.9", true), saved("10.0.0.5", true)];
        let live = managed(3, 8080);
        assert_eq!(saved_rule_index(&rules, &live, false), Some(1));
        assert_eq!(
            saved_rule_index(&rules, &live, true),
            None,
            "already on: nothing to write"
        );

        let mut elsewhere = managed(3, 8081);
        elsewhere.description = None;
        assert_eq!(saved_rule_index(&rules, &elsewhere, false), None);

        let mut remote = managed(3, 8080);
        remote.kind = SshForwardKind::Remote;
        assert_eq!(saved_rule_index(&rules, &remote, false), None);
    }
}

/// Remote F21 and F10 through the real app render: the Info tab is drawn with
/// forward rows and the advanced form, and the paints the builders assigned are
/// read back from the scoped probe.
#[cfg(test)]
mod contrast_tests {
    use gpui::TestAppContext;
    use gpui_component::ActiveTheme as _;

    use crate::daemon::protocol::{ForwardStatus, ManagedForward, SshForwardKind};
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::dialog::test_themes as themes;
    use crate::ui::paint_probe as probe;
    use crate::ui::presets::PORT_CHIP_WASH;
    use crate::ui::right_panel::test_panel::{open_info, port};

    fn forward(id: u64, bind_port: u16, enabled: bool) -> ManagedForward {
        ManagedForward {
            id,
            pane_id: 1,
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            bind_port,
            target_host: "10.0.0.5".into(),
            target_port: 80,
            description: Some("db tunnel".into()),
            status: match enabled {
                true => ForwardStatus::Listening,
                false => ForwardStatus::Error("not bound".into()),
            },
            enabled,
        }
    }

    #[gpui::test]
    fn an_off_forward_is_quiet_and_a_live_one_wears_the_neutral_chip(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_info(
            &app,
            &mut vcx,
            vec![port(3000, 42, "node")],
            vec![forward(9, 8080, false), forward(10, 8081, true)],
        );
        let _probe = probe::scope();

        for theme in &themes::themes() {
            themes::apply(&mut vcx, theme);
            let (muted, ink, pole, accent) = vcx.update(|_, cx| {
                (
                    probe::hsla24(cx.theme().muted_foreground),
                    crate::ui::theme::chrome_ink(cx),
                    crate::ui::theme::fill_pole(cx),
                    probe::hsla24(cx.theme().accent),
                )
            });
            probe::clear();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let id = theme.id.as_str();

            let columns = probe::all("forwards.column");
            assert_eq!(
                columns.len(),
                2,
                "{id}: both forward rows reached the real render"
            );
            for (n, column) in columns.iter().enumerate() {
                assert_eq!(
                    column.opacity, None,
                    "{id}: forward row {n} is not dimmed by a container fade"
                );
            }

            let chips = probe::all("ui.chip");
            assert_eq!(chips.len(), 2, "{id}: one bind chip per forward row");
            let (off, live) = (&chips[0], &chips[1]);
            assert_eq!(
                off.fill_alpha,
                Some(0.0),
                "{id}: the off row's chip carries no fill"
            );
            assert_eq!(
                off.text,
                Some(muted),
                "{id}: the off row's bind is muted ink"
            );
            assert_eq!(
                off.text_alpha,
                Some(1.0),
                "{id}: the off row's bind is full strength"
            );
            assert_eq!(
                live.fill,
                Some(pole),
                "{id}: the live chip washes with the shared fill pole"
            );
            assert_ne!(
                live.fill,
                Some(accent),
                "{id}: an ordinary mapping does not spend the accent"
            );
            assert!(
                live.fill_alpha
                    .is_some_and(|a| (a - PORT_CHIP_WASH).abs() < 0.005),
                "{id}: the live chip keeps the shared wash alpha, got {:?}",
                live.fill_alpha
            );
            assert_eq!(
                live.text,
                Some(ink.port_chip_ink),
                "{id}: the live chip wears the floored port ink"
            );
            assert_eq!(live.text_alpha, Some(1.0), "{id}: the ink is solid");
            assert!(
                probe::all("port.chip.hover").len() == 1,
                "{id}: only the port row's chip answers the row's hover; forward chips do not"
            );
        }
    }

    #[gpui::test]
    fn the_advanced_form_draws_a_target_pair_only_for_rules_that_have_a_target(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, vec![port(3000, 42, "node")], Vec::new());
        let _probe = probe::scope();

        for (kind, pairs) in [
            (SshForwardKind::Dynamic, 1),
            (SshForwardKind::Local, 2),
            (SshForwardKind::Remote, 2),
        ] {
            app.update_in(&mut vcx, |app, _, cx| {
                app.loopback_panel.form_pane_id = Some(1);
                app.loopback_panel.mf_advanced = true;
                app.loopback_panel.mf_kind = kind;
                cx.notify();
            });
            probe::clear();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let drawn = probe::all("forwards.pair");
            assert_eq!(
                drawn.len(),
                pairs,
                "{kind:?}: the form drew {} field pairs in the real render",
                drawn.len()
            );
            assert!(
                drawn.iter().all(|pair| pair.opacity.is_none()),
                "{kind:?}: no pair stands in faded"
            );
        }
    }
}

/// Remote F10, F12 and F19 through the real render: what each label painted,
/// and where layout put it.
///
/// `TestAppContext` shapes through gpui's `NoopTextSystem`, where every `char`
/// advances the same fraction of the font size, so widths are compared with one
/// another and with the labels' own prepared text, never with constants. What
/// a real font does to them is for the native captures, and a tooltip's
/// appearance under a real pointer is too: nothing here hovers.
#[cfg(test)]
mod layout_tests {
    use gpui::{ElementId, TestAppContext, VisualTestContext, px};

    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::paint_probe as probe;
    use crate::ui::right_panel::test_panel::{open_info, port};
    use crate::ui::text_elision::observe;

    const LONG_HOST: &str = "reporting-replica-eu-west-1b.internal.platform.example.com";

    fn mapping(id: u64, bind_host: &str, host: &str, port: u16) -> ManagedForward {
        ManagedForward {
            id,
            pane_id: 1,
            kind: SshForwardKind::Local,
            bind_host: bind_host.into(),
            bind_port: 5432,
            target_host: host.into(),
            target_port: port,
            description: None,
            status: ForwardStatus::Listening,
            enabled: true,
        }
    }

    /// Lets the app settle, forgets what earlier frames observed, and draws.
    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        observe::clear();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn label(id: u64) -> Option<observe::Label> {
        observe::label(&ElementId::from(("panel-forward-target", id as usize)))
    }

    /// The narrow width the forward panel is asked for in these tests: the
    /// 260px a narrow window gives it. It is a request, not a promise: the
    /// product never draws the panel under its own measured minimum
    /// (`right_panel_min_px`), and on macOS that minimum, which counts the
    /// panel toggle beside the tabs, is wider than 260px. The width a narrow
    /// scene really asks for is `narrow_panel_request`: this, or that minimum
    /// where it is the larger. The product floor is never lowered or bypassed.
    const PANEL_W: f32 = 260.;

    /// A panel wide enough for a short bind chip and a short target to share
    /// one line, which 260px is not once the switch, the remove tile and the
    /// fixed ports are counted.
    const WIDE_PANEL_W: f32 = 420.;

    /// The narrowest legal request for these scenes: `PANEL_W`, or the panel's
    /// own measured minimum where that is wider (macOS). The minimum follows
    /// the tab metrics the tab row is drawn from, so an ordinary frame is drawn
    /// first to refresh them for the scene's locale and font. Captured here,
    /// before anything is set or drawn at it, and never derived from the width
    /// the panel ends up with. Where the floor is not the macOS one, the
    /// request is exactly `PANEL_W`: a larger one on Linux or Windows is a
    /// failure to investigate, not a width to follow.
    fn narrow_panel_request(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) -> f32 {
        draw(vcx);
        let floor = app.update_in(vcx, |app, window, cx| app.right_panel_min_px(window, cx));
        let request = PANEL_W.max(floor);
        if cfg!(not(target_os = "macos")) {
            assert_eq!(
                request, PANEL_W,
                "off macOS the panel floor {floor} is under the 260px request"
            );
        }
        request
    }

    /// Asks for the narrow request and returns it, for `assert_panel_width`.
    fn set_panel_width(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) -> f32 {
        let request = narrow_panel_request(app, vcx);
        set_width(app, vcx, request);
        request
    }

    fn set_width(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext, width: f32) {
        app.update_in(vcx, |app, _, cx| {
            app.right_panel_width.set(width);
            cx.notify();
        });
    }

    /// The panel is really `request` wide, the width `set_panel_width` captured
    /// before drawing: the width layout used, read back after the clamp that
    /// keeps it between its floor and its ceiling, and the box the frame drew,
    /// read from the `right-panel` selector.
    fn assert_panel_width(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext, request: f32) {
        assert_width(app, vcx, request);
        let drawn = vcx
            .debug_bounds("right-panel")
            .expect("the right panel was drawn");
        assert_eq!(
            drawn.size.width,
            px(request),
            "the panel was drawn {:?} wide, not the {request}px asked for",
            drawn.size.width
        );
    }

    fn assert_width(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext, width: f32) {
        let used = app.update_in(vcx, |app, window, cx| app.right_panel_px(window, cx));
        assert_eq!(used, width, "the panel was clamped away from {width}");
    }

    fn show(
        app: &gpui::Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        forwards: Vec<ManagedForward>,
    ) {
        app.update_in(vcx, |app, _, cx| {
            app.loopback_panel.managed = forwards;
            cx.notify();
        });
        let request = set_panel_width(app, vcx);
        draw(vcx);
        assert_panel_width(app, vcx, request);
    }

    /// What `…` kept of `source` on each side. Both are derived from the text
    /// that was cut, so the assertion is "this is a real head and a real tail
    /// of it", not a guess at how many characters a width buys.
    fn ends(shown: &str, source: &str) -> (String, String) {
        let (head, tail) = shown
            .split_once('…')
            .unwrap_or_else(|| panic!("{shown:?} was not elided"));
        assert!(!head.is_empty(), "{shown:?} kept no head of {source:?}");
        assert!(!tail.is_empty(), "{shown:?} kept no tail of {source:?}");
        assert!(
            source.starts_with(head),
            "{head:?} is not a head of {source:?}"
        );
        assert!(
            source.ends_with(tail),
            "{tail:?} is not a tail of {source:?}"
        );
        assert!(head.len() + tail.len() < source.len(), "nothing was cut");
        (head.to_string(), tail.to_string())
    }

    /// The host gives way and the port does not: beside a host that fits, the
    /// port of a host that was cut is exactly as wide, and sits inside the row.
    #[gpui::test]
    fn a_long_host_is_elided_while_its_port_stays_whole(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, vec![port(3000, 42, "node")], Vec::new());
        show(
            &app,
            &mut vcx,
            vec![
                mapping(9, "127.0.0.1", LONG_HOST, 5432),
                mapping(10, "127.0.0.1", "db", 5432),
            ],
        );

        let cut = label(9).expect("the long host's label was drawn");
        assert!(cut.painted, "{cut:?}");
        ends(&cut.text, LONG_HOST);
        let whole = label(10).expect("the short host's label was drawn");
        assert_eq!(&*whole.text, "db");
        assert!(whole.painted);

        let long_port = vcx
            .debug_bounds("forward-port-9")
            .expect("long port laid out");
        let short_port = vcx
            .debug_bounds("forward-port-10")
            .expect("short port laid out");
        let row = vcx
            .debug_bounds("forward-target-9")
            .expect("target row laid out");
        assert_eq!(
            long_port.size.width, short_port.size.width,
            "the port is as wide beside a cut host as beside a whole one"
        );
        assert!(long_port.size.width > px(0.));
        assert!(
            long_port.right() <= row.right(),
            "{long_port:?} inside {row:?}"
        );
    }

    /// An IPv6 literal is the field as saved: nothing is bracketed, split or
    /// reparsed, so a host that fits is painted verbatim and a long one is cut
    /// by the same elision, its kept ends being real ends of the literal.
    #[gpui::test]
    fn an_ipv6_literal_keeps_its_spelling_and_its_port(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, vec![port(3000, 42, "node")], Vec::new());
        let scoped = "fe80::a00:27ff:fe4e:66a1%enp0s31f6-with-a-very-long-scope-name";
        show(
            &app,
            &mut vcx,
            vec![
                mapping(9, "127.0.0.1", "::1", 8080),
                mapping(10, "127.0.0.1", scoped, 8080),
            ],
        );

        let short = label(9).expect("drawn");
        assert_eq!(
            &*short.text, "::1",
            "a literal that fits is painted as saved"
        );
        let cut = label(10).expect("drawn");
        ends(&cut.text, scoped);
        assert!(
            !cut.text.contains('[') && !short.text.contains('['),
            "no bracket is invented"
        );
        let a = vcx.debug_bounds("forward-port-9").expect("laid out");
        let b = vcx.debug_bounds("forward-port-10").expect("laid out");
        assert_eq!(a.size.width, b.size.width, "the port is whole on both");
        // The whole address the tooltip hands back, spelled the way the rule
        // holds it. Only the string is checked: hovering is for the native run.
        assert_eq!(target_address("fe80::1", 8080), "fe80::1:8080");
        assert_eq!(target_address(scoped, 8080), format!("{scoped}:8080"));
    }

    fn bind_label(id: u64) -> Option<observe::Label> {
        observe::label(&ElementId::from(("panel-forward-bind-host", id as usize)))
    }

    /// A bind host as long as a person can type: the chip is as wide as its
    /// line and no wider, its host elided inside it with the `:port` kept, and
    /// the target wrapped onto a line of its own below it instead of being
    /// squeezed out beside it. A guarantee about the narrowest legal panel (the
    /// 260px request, or the product's wider floor on macOS); the wrap is what
    /// spares the target, so it never goes without a host character.
    #[gpui::test]
    fn a_long_bind_host_is_elided_in_its_chip_and_the_target_wraps_below(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, vec![port(3000, 42, "node")], Vec::new());
        show(&app, &mut vcx, vec![mapping(9, "0.0.0.0", LONG_HOST, 5432)]);
        let target_port_width = vcx
            .debug_bounds("forward-port-9")
            .expect("laid out")
            .size
            .width;
        let bind_port_width = vcx
            .debug_bounds("forward-bind-port-9")
            .expect("laid out")
            .size
            .width;

        let mut wrapped_text = None;
        let mut chip_widths = Vec::new();
        let mut bind_kept_all = Vec::new();
        let mut bind_was_cut = false;
        for extra in 0..60 {
            let bind = format!("lan-{}-gateway", "x".repeat(extra));
            show(
                &app,
                &mut vcx,
                vec![
                    mapping(9, &bind, LONG_HOST, 5432),
                    mapping(10, "127.0.0.1", "db", 5432),
                ],
            );

            let target = label(9).expect("the target label is drawn at every width");
            assert!(
                target.text.chars().any(|c| c != '…'),
                "{extra}: no target host is left"
            );
            let host = bind_label(9).expect("the bind host label is drawn at every width");
            let bind_kept = host.text.chars().filter(|c| *c != '…').count();
            assert!(bind_kept >= 1, "{extra}: no bind host is left");
            bind_kept_all.push(bind_kept);
            if host.text.contains('…') {
                bind_was_cut = true;
                ends(&host.text, &bind);
            }

            let row = vcx.debug_bounds("forward-row-9").expect("row laid out");
            let chip = vcx.debug_bounds("forward-chip-9").expect("chip laid out");
            let target_row = vcx.debug_bounds("forward-target-9").expect("laid out");
            let target_port = vcx.debug_bounds("forward-port-9").expect("laid out");
            let bind_port = vcx.debug_bounds("forward-bind-port-9").expect("laid out");
            assert_eq!(target_port.size.width, target_port_width, "{extra}");
            assert_eq!(bind_port.size.width, bind_port_width, "{extra}");
            assert!(
                target_port.right() <= target_row.right()
                    && target_port.left() >= target_row.left(),
                "{extra}: {target_port:?} is outside {target_row:?}"
            );
            assert!(
                bind_port.right() <= chip.right() && bind_port.left() >= chip.left(),
                "{extra}: {bind_port:?} is outside {chip:?}"
            );
            assert!(
                chip.right() <= row.right() && target_row.right() <= row.right(),
                "{extra}: the line outgrew its row: {chip:?} {target_row:?} in {row:?}"
            );
            chip_widths.push(chip.size.width);

            // Wrapped: the target is below the chip, not beside it, and the
            // same text is shown at every width that wraps, because it then
            // has the whole line whatever the chip is doing.
            if chip.bottom() <= target_row.top() {
                assert!(
                    chip.size.width <= target_row.size.width,
                    "{extra}: the chip is wider than the line it sits on"
                );
                match &wrapped_text {
                    None => wrapped_text = Some(target.text.clone()),
                    Some(first) => assert_eq!(
                        &target.text, first,
                        "{extra}: the wrapped target changed with the chip"
                    ),
                }
            }
        }
        assert!(bind_was_cut, "the sweep never made the bind host give way");
        assert!(wrapped_text.is_some(), "the target never wrapped below");
        assert!(
            bind_kept_all
                .iter()
                .rev()
                .take(10)
                .all(|k| *k == bind_kept_all[59]),
            "once the chip is as wide as its line the host gets the same room: {bind_kept_all:?}"
        );
        let cap = chip_widths[chip_widths.len() - 1];
        assert!(
            chip_widths.iter().rev().take(10).all(|w| *w == cap),
            "the chip kept growing past its line: {chip_widths:?}"
        );
    }

    /// A short bind host is whole however long the target is: the target
    /// wraps below it and is the one that gives way. The row grows by the
    /// second line, and the next row starts under it rather than over it.
    #[gpui::test]
    fn a_short_bind_host_stays_whole_and_the_target_wraps_below_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, vec![port(3000, 42, "node")], Vec::new());
        show(
            &app,
            &mut vcx,
            vec![
                mapping(9, "0.0.0.0", LONG_HOST, 5432),
                mapping(10, "127.0.0.1", "db", 5432),
            ],
        );
        let bind = bind_label(9).expect("drawn");
        assert_eq!(&*bind.text, "0.0.0.0");
        assert!(bind.painted);
        ends(&label(9).expect("drawn").text, LONG_HOST);

        let chip = vcx.debug_bounds("forward-chip-9").expect("laid out");
        let target = vcx.debug_bounds("forward-target-9").expect("laid out");
        let row = vcx.debug_bounds("forward-row-9").expect("laid out");
        let next = vcx.debug_bounds("forward-row-10").expect("laid out");
        assert!(
            chip.bottom() <= target.top(),
            "the target wrapped below the chip: {chip:?} {target:?}"
        );
        assert!(
            row.size.height >= chip.size.height + target.size.height,
            "the row grew by the second line: {row:?}"
        );
        assert!(
            next.top() >= row.bottom(),
            "the next row starts under the wrapped one: {next:?} {row:?}"
        );
        assert!(
            row.size.height > next.size.height,
            "the wrapped row is taller than the one-line row"
        );
    }

    /// Wrapping is for the line that needs it: where a short bind chip and a
    /// short target fit together they stay one line. At 260px they do not (the
    /// chip, the target's arrow and ports and the row's switch and tile are
    /// more than the column), so this is drawn at a wider panel, read back.
    #[gpui::test]
    fn a_short_bind_chip_and_a_short_target_share_one_line(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, vec![port(3000, 42, "node")], Vec::new());
        app.update_in(&mut vcx, |app, _, cx| {
            app.loopback_panel.managed = vec![mapping(9, "0.0.0.0", "db", 80)];
            cx.notify();
        });
        set_width(&app, &mut vcx, WIDE_PANEL_W);
        draw(&mut vcx);
        assert_width(&app, &mut vcx, WIDE_PANEL_W);
        let chip = vcx.debug_bounds("forward-chip-9").expect("laid out");
        let target = vcx.debug_bounds("forward-target-9").expect("laid out");
        assert!(
            target.top() < chip.bottom(),
            "they share a line: {chip:?} {target:?}"
        );
        assert_eq!(&*bind_label(9).expect("drawn").text, "0.0.0.0");
        assert_eq!(&*label(9).expect("drawn").text, "db");
    }

    /// F19: the hint is a line of its own under the 64px field, in the form's
    /// own column, rather than a flex sibling the row cuts off.
    #[gpui::test]
    fn the_simple_form_hint_has_its_own_line_under_the_port_field(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, vec![port(3000, 42, "node")], Vec::new());
        app.update_in(&mut vcx, |app, window, cx| {
            app.loopback_panel.form_pane_id = Some(1);
            app.loopback_panel.mf_advanced = false;
            app.loopback_panel
                .mf_target_port
                .update(cx, |input, cx| input.set_value("8080", window, cx));
            cx.notify();
        });
        let request = set_panel_width(&app, &mut vcx);
        draw(&mut vcx);
        assert_panel_width(&app, &mut vcx, request);

        let row = vcx
            .debug_bounds("forward-port-row")
            .expect("port row laid out");
        let input = vcx
            .debug_bounds("forward-port-input")
            .expect("input laid out");
        let hint = vcx
            .debug_bounds("forward-port-hint")
            .expect("hint laid out");
        assert_eq!(input.size.width, px(64.), "the field keeps its width");
        assert!(
            hint.top() >= row.bottom(),
            "the hint sits under the row: {hint:?} vs {row:?}"
        );
        assert_eq!(hint.left(), row.left(), "in the form's own column");
        assert!(
            hint.size.width >= row.size.width / 2.,
            "it is a line of the column, not a flex sibling squeezed beside the field"
        );
    }

    /// A live ssh pane from a saved host (or a one-off address), in the app's
    /// own window the way the tab tests build one.
    fn live_ssh_app(
        cx: &mut TestAppContext,
        profile: Option<uuid::Uuid>,
    ) -> (
        gpui::Entity<Tty59App>,
        VisualTestContext,
        gpui::Entity<crate::terminal::view::TerminalView>,
        crate::daemon::transport::Stream,
    ) {
        use crate::terminal::view::quiet_test_ssh_pane_of;
        use crate::ui::app::{Tab, test_window::harness};
        use crate::ui::pane::{Pane, PaneSlot};

        let (app, mut vcx) = harness(cx);
        let (pane, daemon) = app.update_in(&mut vcx, |app, window, cx| {
            let (view, end) = quiet_test_ssh_pane_of(1, profile, window, cx);
            app.tabs
                .push(Tab::new(Pane::leaf(PaneSlot::Ready(view.clone()))));
            app.active = 0;
            cx.notify();
            (view, end)
        });
        vcx.background_executor.run_until_parked();
        (app, vcx, pane, daemon)
    }

    /// The connect fails with `reason` and the link goes away, which is how a
    /// real pane learns it is over without a child having exited: the reader
    /// tears down and sends its exit event. Returns once the pane says so.
    fn fail_ssh_pane(
        vcx: &mut VisualTestContext,
        pane: &gpui::Entity<crate::terminal::view::TerminalView>,
        mut daemon: crate::daemon::transport::Stream,
        reason: &str,
    ) {
        use crate::daemon::protocol::{DaemonMsg, SshPhase};

        DaemonMsg::SshStatus {
            phase: SshPhase::Failed {
                reason: reason.into(),
            },
        }
        .encode(&mut daemon)
        .unwrap();
        drop(daemon);
        let ended = (0..200).any(|_| {
            vcx.run_until_parked();
            let ended = pane.update(vcx, |view, _| {
                view.ssh_phase().is_some() && view.ssh_disconnected()
            });
            if !ended {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            ended
        });
        assert!(ended, "the pane never reported failed and disconnected");
    }

    /// The whole fixture in one: an ssh pane that failed and ended, in a
    /// window that has not yet drawn the strip.
    fn failed_ssh_app(
        cx: &mut TestAppContext,
        profile: Option<uuid::Uuid>,
        reason: &str,
    ) -> (gpui::Entity<Tty59App>, VisualTestContext) {
        let (app, mut vcx, pane, daemon) = live_ssh_app(cx, profile);
        fail_ssh_pane(&mut vcx, &pane, daemon, reason);
        (app, vcx)
    }

    /// The whole path: an ssh pane from a saved host, the daemon's `Failed`
    /// status, the link dropping so the pane ends through its own event path,
    /// and the strip the app's render draws for it — reason and Edit host
    /// included.
    #[gpui::test]
    fn a_failed_ssh_pane_draws_the_strip_through_the_apps_render(cx: &mut TestAppContext) {
        let _probe = probe::scope();
        let (_app, mut vcx, pane, daemon) = live_ssh_app(cx, Some(uuid::Uuid::nil()));
        let muted = vcx.update(|_, cx| probe::hsla24(cx.theme().muted_foreground));

        draw(&mut vcx);
        assert!(
            vcx.debug_bounds("ssh-strip-reason").is_none(),
            "a live pane has no strip"
        );

        let reason = "connect to 127.0.0.1:2299 failed: Connection refused (os error 111)";
        fail_ssh_pane(&mut vcx, &pane, daemon, reason);
        probe::clear();
        draw(&mut vcx);
        let drawn = vcx
            .debug_bounds("ssh-strip-reason")
            .expect("the strip's reason was drawn by the app's render");
        assert!(drawn.size.width <= px(REASON_MAX_W));
        let paint = probe::all("ssh.reason");
        assert_eq!(paint.len(), 1);
        assert_eq!(paint[0].text, Some(muted), "the reason is muted ink");
        assert_eq!(probe::all("ssh.edit-host").len(), 1, "a saved connection");
        assert!(
            !probe::all("dialog.keycap").is_empty(),
            "the chord is keycaps"
        );
        assert!(
            vcx.debug_bounds("ssh-strip-chord").is_some(),
            "the chord is drawn by the app's render, not only built"
        );
    }

    #[gpui::test]
    fn a_long_reason_wraps_to_two_muted_lines_within_its_width(cx: &mut TestAppContext) {
        let _probe = probe::scope();
        let short = "refused";
        let medium =
            "connect to 127.0.0.1:2299 failed: Connection refused (os error 111) ".repeat(3);
        let endless =
            "connect to 127.0.0.1:2299 failed: Connection refused (os error 111) ".repeat(20);

        let mut heights = Vec::new();
        for reason in [short.to_string(), medium, endless] {
            let (_app, mut vcx) = failed_ssh_app(cx, None, &reason);
            let muted = vcx.update(|_, cx| probe::hsla24(cx.theme().muted_foreground));
            let danger = vcx.update(|_, cx| probe::hsla24(cx.theme().danger));
            probe::clear();
            draw(&mut vcx);
            let bounds = vcx
                .debug_bounds("ssh-strip-reason")
                .expect("reason laid out");
            assert!(
                bounds.size.width <= px(REASON_MAX_W),
                "{:?} is past the strip's maximum",
                bounds.size.width
            );
            heights.push(bounds.size.height);
            let paint = probe::all("ssh.reason");
            assert_eq!(paint.len(), 1);
            assert_eq!(paint[0].text, Some(muted), "the reason is muted ink");
            assert_ne!(
                paint[0].text,
                Some(danger),
                "the dot is the strip's one red"
            );
        }
        assert!(heights[1] > heights[0], "a long reason wraps: {heights:?}");
        assert_eq!(
            heights[2], heights[1],
            "and stops at two lines however long it is: {heights:?}"
        );
    }

    #[gpui::test]
    fn the_reconnect_chord_is_the_effective_binding_as_keycaps(cx: &mut TestAppContext) {
        use crate::core::config::{Config, KeybindingOverride};

        let _probe = probe::scope();
        let (app, mut vcx) = failed_ssh_app(cx, None, "refused");
        // Every keycap the window draws, the strip's chord included: the
        // chord's own is what the binding adds to the count with it unbound.
        let caps = |app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext| {
            app.update_in(vcx, |_, _, cx| cx.notify());
            probe::clear();
            draw(vcx);
            probe::all("dialog.keycap").len()
        };
        let bind = |vcx: &mut VisualTestContext, keys: Vec<String>| {
            vcx.update(|_, cx| {
                let mut cfg = cx.global::<Config>().clone();
                cfg.keybindings
                    .insert("RestartSshSession".into(), KeybindingOverride::Exact(keys));
                cx.set_global(cfg);
            });
        };

        // The shipped chord, read from the keymap before anything overrides it.
        let shipped = vcx.update(|_, cx| {
            crate::ui::keymap::effective_key("RestartSshSession", cx)
                .and_then(|spec| crate::ui::keymap::key_chords(&spec).into_iter().next())
                .map_or(0, |keys| keys.len())
        });
        assert!(shipped >= 2, "a combination is one cap per key");
        let with_default = caps(&app, &mut vcx);
        // The paint probe notes a keycap when it is BUILT, so counts alone would
        // also be satisfied by a chord that is built and then never drawn. The
        // window's own layout answers whether it was drawn.
        let default_chord = vcx
            .debug_bounds("ssh-strip-chord")
            .expect("the default chord is drawn");
        assert!(default_chord.size.width > px(0.), "{default_chord:?}");

        bind(&mut vcx, Vec::new());
        let without = caps(&app, &mut vcx);
        assert_eq!(with_default, without + shipped, "one cap per key");
        assert!(
            vcx.debug_bounds("ssh-strip-chord").is_none(),
            "an unbound action draws no chord"
        );

        bind(&mut vcx, vec!["alt-x".to_string()]);
        assert_eq!(caps(&app, &mut vcx), without + 2, "a rebind moves the caps");
        let rebound = vcx
            .debug_bounds("ssh-strip-chord")
            .expect("the rebound chord is drawn");
        assert!(
            px(0.) < rebound.size.width && rebound.size.width < default_chord.size.width,
            "two caps are narrower than the shipped combination: {rebound:?} vs {default_chord:?}"
        );
    }

    #[gpui::test]
    fn edit_host_is_quiet_and_only_offered_for_a_saved_connection(cx: &mut TestAppContext) {
        let _probe = probe::scope();

        let (_one_off, mut vcx) = failed_ssh_app(cx, None, "refused");
        probe::clear();
        draw(&mut vcx);
        assert!(
            vcx.debug_bounds("ssh-strip-reason").is_some(),
            "the strip is drawn, so its missing button means something"
        );
        assert!(
            probe::all("ssh.edit-host").is_empty(),
            "a one-off address has no host to edit"
        );

        let (_saved, mut vcx) = failed_ssh_app(cx, Some(uuid::Uuid::nil()), "refused");
        let muted = vcx.update(|_, cx| probe::hsla24(cx.theme().muted_foreground));
        probe::clear();
        draw(&mut vcx);
        let edit = probe::all("ssh.edit-host");
        assert_eq!(edit.len(), 1);
        assert_eq!(
            edit[0].text,
            Some(muted),
            "muted until the button's own hover"
        );
    }

    // ---- F22: the strip inside the terminal a narrow window leaves ----------

    /// What an ssh failure really says when it runs long: several clauses, no
    /// natural break near the width of a 440px terminal.
    const LONG_REASON: &str = "connect to 127.0.0.1:2299 failed: Connection refused (os error 111); the host key was rejected, the agent refused every identity and the jump host closed the channel before authentication finished";

    /// The window of the original finding: 960x600, the left rail 260px wide and
    /// the right panel 260px wide, which leaves the terminal 440px.
    fn narrow_scene(
        cx: &mut TestAppContext,
        locale: &str,
        ui_font_size: f32,
        theme: &crate::ui::presets::Theme,
        reason: &str,
    ) -> (Entity<Tty59App>, VisualTestContext) {
        narrow_scene_host(cx, locale, ui_font_size, theme, reason, "")
    }

    /// [`narrow_scene`] for a pane that dialled `host`, which the strip's title
    /// names ("" = the fixture's pane, which names nothing).
    fn narrow_scene_host(
        cx: &mut TestAppContext,
        locale: &str,
        ui_font_size: f32,
        theme: &crate::ui::presets::Theme,
        reason: &str,
        host: &str,
    ) -> (Entity<Tty59App>, VisualTestContext) {
        scene(
            cx,
            locale,
            ui_font_size,
            theme,
            reason,
            host,
            (960., 600.),
            Some((260., 260.)),
        )
    }

    /// The scene of the original finding with its terminal made exactly 440px,
    /// for the two tests that are about that width and no other. 440px is the
    /// terminal's content width; the outer window that leaves it is a platform
    /// matter, because the rail, the panel's floor and the window chrome differ
    /// (on macOS the panel's floor is wider than 260px). So the scene is built
    /// as [`narrow_scene_host`] builds it (960x600, 260px rail and panel asked
    /// for), the terminal and the viewport it really got are read back, and the
    /// window's width is corrected once by what the terminal is short of or over
    /// 440px. Height and the requests are unchanged; there is no search. The
    /// tests' own precondition still measures the terminal at 440px. English at
    /// UI16, in the theme given (the tests use the dark one).
    fn narrow_scene_exact_440(
        cx: &mut TestAppContext,
        theme: &crate::ui::presets::Theme,
        reason: &str,
        host: &str,
    ) -> (Entity<Tty59App>, VisualTestContext) {
        let (app, mut vcx) = narrow_scene_host(cx, "en", 16., theme, reason, host);
        let viewport = vcx.update(|window, _| window.viewport_size());
        let terminal = terminal_area(&app, &mut vcx);
        let outer = viewport.width + (px(440.) - terminal.size.width);
        vcx.simulate_resize(gpui::size(outer, viewport.height));
        draw(&mut vcx);
        (app, vcx)
    }

    /// A saved host whose name is as long as real ones get: no break a layout
    /// can use, longer than the terminal.
    const LONG_SAVED_HOST: &str = "demo@this-host-name-is-deliberately-long-so-the-failure-reason-wraps-in-a-narrow-terminal.invalid";

    /// [`failed_ssh_app`] for a pane that dialled `host`: the daemon reports the
    /// remote context the way a real native-ssh pane gets one, which is where
    /// the strip reads the name it puts in its title.
    fn failed_ssh_app_host(
        cx: &mut TestAppContext,
        profile: Option<uuid::Uuid>,
        reason: &str,
        host: &str,
    ) -> (Entity<Tty59App>, VisualTestContext) {
        use crate::daemon::protocol::{DaemonMsg, RemoteContext, RemoteKind};
        let (app, mut vcx, pane, mut daemon) = live_ssh_app(cx, profile);
        if !host.is_empty() {
            DaemonMsg::RemoteContext(Some(RemoteContext {
                kind: RemoteKind::NativeSsh,
                argv: Vec::new(),
                target: host.into(),
            }))
            .encode(&mut daemon)
            .unwrap();
            vcx.run_until_parked();
        }
        fail_ssh_pane(&mut vcx, &pane, daemon, reason);
        (app, vcx)
    }

    /// The same failed ssh pane in a window of `window` size, with the left rail
    /// and the right panel asked for `panels` pixels, or the right panel left
    /// closed. What the panels really get is what the layout makes of the ask
    /// (the right panel's floor follows the locale and the UI size), so the
    /// terminal a scene leaves is read back, never assumed.
    fn scene(
        cx: &mut TestAppContext,
        locale: &str,
        ui_font_size: f32,
        theme: &crate::ui::presets::Theme,
        reason: &str,
        host: &str,
        window: (f32, f32),
        panels: Option<(f32, f32)>,
    ) -> (Entity<Tty59App>, VisualTestContext) {
        crate::ui::i18n::set_locale(locale);
        let (app, mut vcx) = failed_ssh_app_host(cx, Some(uuid::Uuid::nil()), reason, host);
        crate::ui::dialog::test_themes::apply(&mut vcx, theme);
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<crate::core::config::Config>().clone();
            cfg.ui_font_size = ui_font_size;
            cx.set_global(cfg);
        });
        vcx.simulate_resize(gpui::size(px(window.0), px(window.1)));
        app.update_in(&mut vcx, |app, _, cx| {
            if let Some((rail, panel)) = panels {
                app.sidebar_width.set(rail);
                app.right_panel_width.set(panel);
            }
            app.right_panel_visible = panels.is_some();
            cx.notify();
        });
        draw(&mut vcx);
        // The window's rem is the UI font size: every `rems(..)` in the strip is
        // measured against it, so a scene that did not take the size would
        // prove nothing about UI24.
        let rem = vcx.update(|window, _| window.rem_size().as_f32());
        assert!(
            (rem - ui_font_size).abs() < 0.5,
            "the window drew at rem {rem}, not the UI font size {ui_font_size}"
        );
        (app, vcx)
    }

    fn overlaps(a: gpui::Bounds<gpui::Pixels>, b: gpui::Bounds<gpui::Pixels>) -> bool {
        let eps = px(0.5);
        a.left() < b.right() - eps
            && b.left() < a.right() - eps
            && a.top() < b.bottom() - eps
            && b.top() < a.bottom() - eps
    }

    fn terminal_area(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
    ) -> gpui::Bounds<gpui::Pixels> {
        app.update_in(vcx, |app, _, _| app.pane_area.get())
            .expect("the terminal body was painted")
    }

    fn within(inner: gpui::Bounds<gpui::Pixels>, outer: gpui::Bounds<gpui::Pixels>) -> bool {
        let eps = px(0.5);
        inner.left() >= outer.left() - eps
            && inner.right() <= outer.right() + eps
            && inner.top() >= outer.top() - eps
            && inner.bottom() <= outer.bottom() + eps
    }

    fn both_themes() -> Vec<crate::ui::presets::Theme> {
        let all = crate::ui::presets::builtins();
        let dark = all
            .iter()
            .find(|t| t.dark)
            .expect("a dark built-in")
            .clone();
        let light = all
            .iter()
            .find(|t| !t.dark)
            .expect("a light built-in")
            .clone();
        vec![dark, light]
    }

    /// How wide each action is when nothing constrains it: the same pane in a
    /// wide window with no right panel. A narrow terminal may move an action
    /// to another line; it may not make it narrower than itself.
    fn natural_actions(
        cx: &mut TestAppContext,
        locale: &str,
        ui: f32,
        theme: &crate::ui::presets::Theme,
    ) -> (gpui::Pixels, gpui::Pixels) {
        let (_app, mut vcx) = scene(cx, locale, ui, theme, "refused", "", (1800., 900.), None);
        let mut width = |selector: &'static str| {
            vcx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} laid out in the wide window"))
                .size
                .width
        };
        (width("ssh-strip-reconnect"), width("ssh-strip-edit-host"))
    }

    /// Everything the strip must hold in whatever terminal it was drawn in:
    /// every node inside it, the reason readable and wrapped, each action as
    /// wide as it is when free, the two in reading order, and no two of them
    /// on top of one another.
    fn strip_misses(
        case: &str,
        vcx: &mut VisualTestContext,
        area: gpui::Bounds<gpui::Pixels>,
        natural: (gpui::Pixels, gpui::Pixels),
        ui: f32,
    ) -> Vec<String> {
        let mut misses = Vec::new();
        let mut get = |selector: &'static str| vcx.debug_bounds(selector);
        for (what, selector) in [
            ("pill", "ssh-strip-pill"),
            ("reason", "ssh-strip-reason"),
            ("Reconnect", "ssh-strip-reconnect"),
            ("Edit host", "ssh-strip-edit-host"),
        ] {
            match get(selector) {
                None => misses.push(format!("{case}: {what} was not laid out")),
                Some(b) if !within(b, area) => {
                    misses.push(format!("{case}: {what} {b:?} leaves the terminal {area:?}"));
                }
                Some(_) => {}
            }
        }
        // The title names the host and a long name is the one thing in the strip
        // with no break to wrap at: it stays inside the pill, not merely inside
        // the terminal the pill itself may overhang.
        if let (Some(pill), Some(title)) = (get("ssh-strip-pill"), get("ssh-strip-title")) {
            if !within(title, pill) {
                misses.push(format!(
                    "{case}: the title {title:?} leaves its pill {pill:?}"
                ));
            }
        }
        let (Some(reason), Some(reconnect), Some(edit)) = (
            get("ssh-strip-reason"),
            get("ssh-strip-reconnect"),
            get("ssh-strip-edit-host"),
        ) else {
            return misses;
        };
        if reason.size.width < px(120.) {
            misses.push(format!(
                "{case}: the reason is squeezed to {:?}",
                reason.size.width
            ));
        }
        if reason.size.height < px(ui * 1.5) {
            misses.push(format!(
                "{case}: a long reason did not wrap: {:?} high",
                reason.size.height
            ));
        }
        let slack = px(0.5);
        if reconnect.size.width + slack < natural.0 {
            misses.push(format!(
                "{case}: Reconnect is {:?} wide where it is {:?} free",
                reconnect.size.width, natural.0
            ));
        }
        if edit.size.width + slack < natural.1 {
            misses.push(format!(
                "{case}: Edit host is {:?} wide where it is {:?} free",
                edit.size.width, natural.1
            ));
        }
        let first = reconnect.top() < edit.top() - slack
            || ((reconnect.top() - edit.top()).abs() <= slack && reconnect.left() < edit.left());
        if !first {
            misses.push(format!(
                "{case}: Edit host {edit:?} is before Reconnect {reconnect:?}"
            ));
        }
        let mut pairs = vec![
            ("reason", reason),
            ("Reconnect", reconnect),
            ("Edit host", edit),
        ];
        if let Some(chord) = get("ssh-strip-chord") {
            pairs.push(("chord", chord));
        }
        for (i, (a, ab)) in pairs.iter().enumerate() {
            for (b, bb) in &pairs[i + 1..] {
                if overlaps(*ab, *bb) {
                    misses.push(format!("{case}: {a} {ab:?} overlaps {b} {bb:?}"));
                }
            }
        }
        misses
    }

    /// The scene of the original finding: en, UI16, dark, the rail and the
    /// right panel asked for 260px, in a window whose outer width is corrected
    /// once (`narrow_scene_exact_440`) so that the terminal is 440px. The 440px
    /// is the terminal's CONTENT width, the one the regression was written
    /// about; the outer window may be wider than the historical 960px, because
    /// macOS chrome and its wider panel floor take more of it. A different
    /// terminal here is a failed precondition, not a pass or a layout miss.
    #[gpui::test]
    fn the_original_440px_scene_keeps_the_strip_and_both_actions_inside(cx: &mut TestAppContext) {
        let _probe = probe::scope();
        let theme = both_themes().remove(0);
        let natural = natural_actions(cx, "en", 16., &theme);
        let (app, mut vcx) = narrow_scene_exact_440(cx, &theme, LONG_REASON, "");
        let area = terminal_area(&app, &mut vcx);
        assert!(
            (area.size.width - px(440.)).abs() < px(1.),
            "precondition: the scene leaves a {:?} terminal, not 440px",
            area.size.width
        );
        let misses = strip_misses("en UI16 dark 440", &mut vcx, area, natural, 16.);
        assert!(
            misses.is_empty(),
            "{} misses\n{}",
            misses.len(),
            misses.join("\n")
        );
    }

    /// The same checks in every language, UI size and theme the finding names,
    /// in the terminal each really leaves. The right panel's minimum follows
    /// the locale and the font, so 260px is only a request: the terminal each
    /// scene ends up with is printed, and every node is held to it.
    #[gpui::test]
    fn the_strip_stays_inside_the_terminal_in_every_language_size_and_theme(
        cx: &mut TestAppContext,
    ) {
        let _probe = probe::scope();
        let mut misses = Vec::new();
        for locale in ["en", "ru-RU"] {
            for ui in [16., 24.] {
                for theme in &both_themes() {
                    let natural = natural_actions(cx, locale, ui, theme);
                    let (app, mut vcx) = narrow_scene(cx, locale, ui, theme, LONG_REASON);
                    let area = terminal_area(&app, &mut vcx);
                    let case = format!("{locale} UI{ui} {} area {:?}", theme.id, area.size.width);
                    eprintln!("terminal for {case}");
                    assert!(area.size.width > px(0.), "{case}: no terminal at all");
                    misses.extend(strip_misses(&case, &mut vcx, area, natural, ui));
                }
            }
        }
        crate::ui::i18n::set_locale("en");
        assert!(
            misses.is_empty(),
            "{} misses\n{}",
            misses.len(),
            misses.join("\n")
        );
    }

    /// Reconnect is clickable where it is painted in the narrow terminal: a
    /// press at the middle of its bounds reaches the pane's respawn, which
    /// replaces the dead leaf.
    #[gpui::test]
    fn reconnect_is_clickable_in_the_narrow_terminal(cx: &mut TestAppContext) {
        let theme = both_themes().remove(0);
        let (app, mut vcx) = narrow_scene(cx, "en", 16., &theme, LONG_REASON);
        let area = terminal_area(&app, &mut vcx);
        let button = vcx
            .debug_bounds("ssh-strip-reconnect")
            .expect("Reconnect was laid out");
        assert!(within(button, area), "{button:?} leaves {area:?}");
        let leaf = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
            app.update_in(vcx, |app, _, _| {
                app.tabs[0]
                    .pane
                    .terminals()
                    .iter()
                    .map(|t| t.entity_id())
                    .collect::<Vec<_>>()
            })
        };
        let before = leaf(&app, &mut vcx);
        let notified = |vcx: &mut VisualTestContext| {
            use gpui_component::WindowExt as _;
            vcx.update(|window, cx| window.notifications(cx).len())
        };
        let notices = notified(&mut vcx);
        vcx.simulate_click(button.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        // The click reached the pane's respawn: it replaced the dead leaf, or,
        // where the test host cannot start an ssh session, reported that it
        // could not. Nothing else in the window answers a press on Reconnect.
        assert!(
            leaf(&app, &mut vcx) != before || notified(&mut vcx) > notices,
            "the click neither restarted the pane nor reported that it could not"
        );
    }

    /// How wide the title is when nothing constrains it: the pane that dialled
    /// `host` in a window wider than any title.
    fn natural_title(
        cx: &mut TestAppContext,
        locale: &str,
        ui: f32,
        theme: &crate::ui::presets::Theme,
        host: &str,
    ) -> gpui::Pixels {
        let (_app, mut vcx) = scene(cx, locale, ui, theme, "refused", host, (2400., 700.), None);
        vcx.debug_bounds("ssh-strip-title")
            .expect("the title is laid out in the wide window")
            .size
            .width
    }

    /// A long saved-host name in the scene of the original finding: the title
    /// that names it stays inside the pill, and so inside the terminal, with the
    /// reason and both actions holding every rule of the other scenes. The pane
    /// really dialled the long host: its title is wider than the terminal when
    /// nothing constrains it. The 440px is the terminal's CONTENT width, the one
    /// the regression was written about; the outer window is corrected once
    /// (`narrow_scene_exact_440`) and, on macOS, may be wider than the
    /// historical 960px to compensate for chrome.
    #[gpui::test]
    fn a_long_saved_host_name_stays_inside_the_pill_in_the_original_440px_scene(
        cx: &mut TestAppContext,
    ) {
        let _probe = probe::scope();
        let theme = both_themes().remove(0);
        let natural = natural_actions(cx, "en", 16., &theme);
        let free = natural_title(cx, "en", 16., &theme, LONG_SAVED_HOST);
        let (app, mut vcx) = narrow_scene_exact_440(cx, &theme, LONG_REASON, LONG_SAVED_HOST);
        let area = terminal_area(&app, &mut vcx);
        assert!(
            (area.size.width - px(440.)).abs() < px(1.),
            "precondition: the scene leaves a {:?} terminal, not 440px",
            area.size.width
        );
        assert!(
            free > area.size.width,
            "precondition: the title is {free:?} free, no wider than the terminal, so the host never reached it"
        );
        let misses = strip_misses("en UI16 dark 440 long host", &mut vcx, area, natural, 16.);
        assert!(
            misses.is_empty(),
            "{} misses\n{}",
            misses.len(),
            misses.join("\n")
        );
    }

    /// The same in every language, UI size and theme, in the terminal each
    /// leaves.
    #[gpui::test]
    fn a_long_saved_host_name_stays_inside_the_pill_in_every_language_size_and_theme(
        cx: &mut TestAppContext,
    ) {
        let _probe = probe::scope();
        let mut misses = Vec::new();
        for locale in ["en", "ru-RU"] {
            for ui in [16., 24.] {
                for theme in &both_themes() {
                    let natural = natural_actions(cx, locale, ui, theme);
                    let free = natural_title(cx, locale, ui, theme, LONG_SAVED_HOST);
                    let (app, mut vcx) =
                        narrow_scene_host(cx, locale, ui, theme, LONG_REASON, LONG_SAVED_HOST);
                    let area = terminal_area(&app, &mut vcx);
                    let case = format!("{locale} UI{ui} {} area {:?}", theme.id, area.size.width);
                    assert!(
                        free > area.size.width,
                        "{case}: the title is {free:?} free, no wider than the terminal"
                    );
                    misses.extend(strip_misses(&case, &mut vcx, area, natural, ui));
                }
            }
        }
        crate::ui::i18n::set_locale("en");
        assert!(
            misses.is_empty(),
            "{} misses\n{}",
            misses.len(),
            misses.join("\n")
        );
    }

    /// Where the title fits it is left alone: in a wide window the long name is
    /// shown at the width it has when nothing constrains it, and the pill holds it.
    #[gpui::test]
    fn a_wide_window_leaves_a_long_title_whole(cx: &mut TestAppContext) {
        let _probe = probe::scope();
        let theme = both_themes().remove(0);
        let free = natural_title(cx, "en", 16., &theme, LONG_SAVED_HOST);
        let (app, mut vcx) = scene(
            cx,
            "en",
            16.,
            &theme,
            "refused",
            LONG_SAVED_HOST,
            (1800., 700.),
            None,
        );
        let area = terminal_area(&app, &mut vcx);
        let title = vcx.debug_bounds("ssh-strip-title").expect("the title");
        let pill = vcx.debug_bounds("ssh-strip-pill").expect("the pill");
        assert!(
            (title.size.width - free).abs() < px(0.5),
            "the title is {:?} wide where it is {free:?} free",
            title.size.width
        );
        assert!(within(title, pill), "{title:?} leaves {pill:?}");
        assert!(within(pill, area), "{pill:?} leaves {area:?}");
    }

    /// The real tooltip of the title: the mouse moved onto it, the clock past
    /// the show delay, a frame drawn. Where the tooltip put its content, or
    /// `None` if it is not there.
    fn hover_title_tooltip(vcx: &mut VisualTestContext) -> Option<gpui::Bounds<gpui::Pixels>> {
        // Off every element first, so no tooltip of an earlier hover is up.
        vcx.simulate_mouse_move(
            gpui::point(px(2.), px(2.)),
            None,
            gpui::Modifiers::default(),
        );
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(700));
        vcx.run_until_parked();
        let title = vcx
            .debug_bounds("ssh-strip-title")
            .expect("the title is drawn, so it can be hovered");
        vcx.simulate_mouse_move(title.center(), None, gpui::Modifiers::default());
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(700));
        vcx.run_until_parked();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.debug_bounds("ssh-strip-title-tooltip-text")
    }

    /// A long host's title is elided in the strip, so its tooltip is where the
    /// whole name is: it exists, it is no wider than the hover maximum, and
    /// every edge of it is inside the window, in the widest language at the
    /// largest UI size as in the plainest.
    #[gpui::test]
    fn the_titles_tooltip_is_there_and_inside_the_window(cx: &mut TestAppContext) {
        let _probe = probe::scope();
        let theme = both_themes().remove(0);
        for (locale, ui) in [("en", 16.), ("ru-RU", 24.)] {
            let (_app, mut vcx) =
                narrow_scene_host(cx, locale, ui, &theme, LONG_REASON, LONG_SAVED_HOST);
            let window = vcx.update(|window, _| window.viewport_size());
            let content = hover_title_tooltip(&mut vcx)
                .unwrap_or_else(|| panic!("{locale} UI{ui}: the title has no tooltip"));
            let rem = vcx.update(|window, _| window.rem_size().as_f32());
            // The selector is on the content. What surrounds it, per side, in
            // the component's own card: the outer margin (0.75 rem), the
            // padding (0.625 rem across, 0.25 rem above and below) and the
            // 1px border. The whole card, margin included, is inside the
            // window on all four sides.
            let across = 0.75 * rem + 0.625 * rem + 1.;
            let above = 0.75 * rem + 0.25 * rem + 1.;
            let (window_w, window_h) = (f32::from(window.width), f32::from(window.height));
            assert!(
                f32::from(content.left()) - across >= -0.5
                    && f32::from(content.right()) + across <= window_w + 0.5,
                "{locale} UI{ui}: the card, content {content:?} and {across}px of padding, border \
                 and margin each side, is not inside the {window_w}px window"
            );
            assert!(
                f32::from(content.top()) - above >= -0.5
                    && f32::from(content.bottom()) + above <= window_h + 0.5,
                "{locale} UI{ui}: the card, content {content:?} and {above}px of padding, border \
                 and margin above and below, is not inside the {window_h}px window"
            );
            // The visible card (content with its padding and border, not the
            // margin) is at most the hover maximum.
            let card = f32::from(content.size.width) + 2. * (0.625 * rem + 1.);
            assert!(
                card <= 420. + 0.5,
                "{locale} UI{ui}: the visible card is {card}px wide, over the 420px hover maximum"
            );
        }
        crate::ui::i18n::set_locale("en");
    }

    /// A 440x300 terminal stand-in that draws what `body_area` draws for
    /// notices: `notice::anchor` over pills. This tests the shared anchor's
    /// geometry, not the lease or the remote-status behavior that decides
    /// which notices exist; the real ssh strip's mount is covered by the app
    /// scenes above.
    struct AnchorHost {
        long: &'static str,
    }

    impl gpui::Render for AnchorHost {
        fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = cx.theme();
            let (danger, warning, foreground) = (theme.danger, theme.warning, theme.foreground);
            // An ssh-like bar: a long wrapping reason and two actions that
            // must stay whole, as the strip draws them.
            let strip = crate::ui::notice::pill(danger, cx)
                .child(div().min_w_0().child(self.long))
                .child(div().flex_none().text_color(foreground).child("Reconnect"))
                .child(div().flex_none().text_color(foreground).child("Edit host"))
                .debug_selector(|| "stack-strip".into());
            let notice = crate::ui::notice::pill(warning, cx)
                .child("Not connected: typing has no effect")
                .debug_selector(|| "stack-notice".into());
            div()
                .relative()
                .w(px(440.))
                .h(px(300.))
                .overflow_hidden()
                .debug_selector(|| "stack-host".into())
                .children(crate::ui::notice::anchor(vec![
                    notice.into_any_element(),
                    strip.into_any_element(),
                ]))
        }
    }

    /// Two notices share one anchor in a 440px terminal: each inside it, one
    /// above the other, neither on top of the other.
    #[gpui::test]
    fn two_notices_stack_inside_a_440px_terminal(cx: &mut TestAppContext) {
        let _probe = probe::scope();
        let (_app, _vcx) = crate::ui::app::test_window::harness(cx);
        let window = cx.add_window(|_, _| AnchorHost { long: LONG_REASON });
        let mut host = VisualTestContext::from_window(window.into(), cx);
        draw(&mut host);
        let area = host.debug_bounds("stack-host").expect("the host");
        let strip = host.debug_bounds("stack-strip").expect("the strip pill");
        let notice = host.debug_bounds("stack-notice").expect("the notice pill");
        assert!(within(strip, area), "{strip:?} leaves {area:?}");
        assert!(within(notice, area), "{notice:?} leaves {area:?}");
        assert!(!overlaps(strip, notice), "{strip:?} overlaps {notice:?}");
    }
}

/// One key as the platform delivers it: key-down, then key-up. The window's
/// `simulate_keystrokes` sends the down alone, and a click made from the
/// keyboard — Enter or Space on a focused element — fires on the key-up, so a
/// test of one uses this.
#[cfg(test)]
fn press(vcx: &mut gpui::VisualTestContext, keystroke: &str) {
    vcx.simulate_keystrokes(keystroke);
    vcx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse(keystroke).expect("a keystroke"),
    });
}

/// The first thing the pane sent its daemon, if anything, within a moment.
#[cfg(test)]
fn pty_input(
    vcx: &mut gpui::VisualTestContext,
    stream: &mut crate::daemon::transport::Stream,
) -> Option<Vec<u8>> {
    use std::io::ErrorKind;

    vcx.run_until_parked();
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(250)))
        .expect("the pane's stream takes a timeout");
    loop {
        match crate::daemon::protocol::ClientMsg::read(stream) {
            Ok(crate::daemon::protocol::ClientMsg::Input(bytes)) => return Some(bytes),
            Ok(_) => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return None;
            }
            Err(e) => panic!("the pane's stream failed before any input: {e}"),
        }
    }
}

/// The way in from the terminal: the palette's own chord, the localized name
/// of the Info command typed as keys, Enter. Nothing but dispatched keys.
#[cfg(test)]
fn show_info_through_the_palette(vcx: &mut gpui::VisualTestContext) {
    let chord = vcx
        .update(|_, cx| crate::ui::keymap::effective_key("TogglePalette", cx))
        .expect("the palette is bound");
    vcx.simulate_keystrokes(&chord);
    let name = t(L10nKey::CmdRightPanelInfo).to_lowercase();
    for ch in name.chars().filter(|c| c.is_alphanumeric() || *c == ' ') {
        let key = if ch == ' ' {
            "space".to_string()
        } else {
            ch.to_string()
        };
        vcx.simulate_keystrokes(&key);
    }
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
}

/// Forward rows by keyboard, through the real app in a real window: keys are
/// dispatched into it (key-down; `press` adds the key-up where a click needs
/// it), rows are drawn by the panel's own render, and what a test reads back
/// is the window's focus and the panel's own state.
///
/// Tests here that read `remove_requests` only prove that a removal was
/// *asked for*, once and for the right forward; they are named `…asks…` so
/// they cannot stand in for the wire cases at the end, which are the ones that
/// show a forward really going and the focus really moving.
#[cfg(test)]
mod keyboard_tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::daemon::protocol::{ForwardStatus, ManagedForward, SshForwardKind};
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::paint_probe as probe;
    use crate::ui::right_panel::test_panel::open_info;

    fn forward(id: u64) -> ManagedForward {
        ManagedForward {
            id,
            pane_id: 1,
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            bind_port: 8000 + id as u16,
            target_host: "10.0.0.5".into(),
            target_port: 80,
            description: None,
            status: ForwardStatus::Listening,
            enabled: true,
        }
    }

    fn forwards(ids: &[u64]) -> Vec<ManagedForward> {
        ids.iter().map(|id| forward(*id)).collect()
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }

    fn show(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext, ids: &[u64]) {
        app.update_in(vcx, |app, _, cx| {
            app.right_panel_width.set(260.);
            app.loopback_panel.managed = forwards(ids);
            cx.notify();
        });
        draw(vcx);
    }

    /// The app with `ids` drawn as forward rows, and nothing yet asked for.
    fn setup(
        cx: &mut TestAppContext,
        ids: &[u64],
    ) -> (
        gpui::Entity<Tty59App>,
        VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        row_handles::clear();
        remove_requests::take();
        edit_requests::take();
        let (app, mut vcx, streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, Vec::new(), forwards(ids));
        show(&app, &mut vcx, ids);
        // Whatever the setup itself asked for is not the test's.
        edit_requests::take();
        (app, vcx, streams)
    }

    #[derive(Debug, PartialEq)]
    enum At {
        Row(u64),
        Tile(u64),
        /// Inside a row, on neither the row nor its remove tile.
        Inside(u64),
        Elsewhere,
    }

    fn at(vcx: &mut VisualTestContext, ids: &[u64]) -> At {
        vcx.update(|window, cx| {
            let focused = window.focused(cx);
            for id in ids {
                let Some(handle) = row_handles::get(*id) else {
                    continue;
                };
                if focused.as_ref() == Some(&handle) {
                    return At::Row(*id);
                }
                if handle.contains_focused(window, cx) {
                    let on_tile = window
                        .context_stack()
                        .iter()
                        .any(|c| c.contains(FORWARD_REMOVE_CONTEXT));
                    return if on_tile {
                        At::Tile(*id)
                    } else {
                        At::Inside(*id)
                    };
                }
            }
            At::Elsewhere
        })
    }

    /// Setup only: put focus on a row the way a Tab from outside would have.
    fn focus_row(vcx: &mut VisualTestContext, id: u64) {
        let handle = row_handles::get(id).expect("the row was drawn");
        vcx.update(|window, cx| window.focus(&handle, cx));
        draw(vcx);
    }

    fn focus_pane(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) {
        app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
        draw(vcx);
    }

    /// True while focus is on forward `id`'s own switch: the live context path
    /// of the focused element carries the switch stop's test-only marker, its
    /// element id.
    fn switch_has_focus(vcx: &mut VisualTestContext, id: u64) -> bool {
        let want = format!("panel-forward-enabled-{id}");
        vcx.update(|window, _| {
            window.context_stack().iter().any(|c| {
                c.get(crate::ui::settings::kit::STOP_MARK)
                    .is_some_and(|v| v.as_ref() == want.as_str())
            })
        })
    }

    /// From row `id`'s own stop, exactly two Tabs: the row's switch (its second
    /// stop, named by the live marker), then its remove tile.
    fn tab_from_the_row_through_its_switch_to_its_tile(
        vcx: &mut VisualTestContext,
        ids: &[u64],
        id: u64,
    ) {
        assert_eq!(at(vcx, ids), At::Row(id), "setup: focus is on the row");
        vcx.simulate_keystrokes("tab");
        assert_eq!(
            at(vcx, ids),
            At::Inside(id),
            "the first Tab from the row is a part of it, not the tile"
        );
        assert!(
            switch_has_focus(vcx, id),
            "and that part is the row's own switch"
        );
        vcx.simulate_keystrokes("tab");
        assert_eq!(
            at(vcx, ids),
            At::Tile(id),
            "the second Tab reaches the remove tile"
        );
    }

    #[gpui::test]
    fn tab_walks_a_row_then_its_remove_tile_then_the_next_row(cx: &mut TestAppContext) {
        let (_app, mut vcx, _streams) = setup(cx, &[9, 10]);
        let ids = [9, 10];
        focus_row(&mut vcx, 9);
        assert_eq!(at(&mut vcx, &ids), At::Row(9));

        // Whatever else is made a stop later, the row's own remove tile comes
        // before the next row, and the next row is reached.
        let mut saw_tile = false;
        let mut reached = false;
        for _ in 0..6 {
            vcx.simulate_keystrokes("tab");
            match at(&mut vcx, &ids) {
                At::Tile(9) => saw_tile = true,
                At::Row(10) => {
                    reached = true;
                    break;
                }
                At::Row(9) | At::Inside(9) => {}
                other => panic!("Tab left the rows early: {other:?}"),
            }
        }
        assert!(saw_tile, "the remove tile is a stop between the two rows");
        assert!(reached, "Tab reaches the next row");

        // And back: Shift+Tab undoes it step for step.
        vcx.simulate_keystrokes("shift-tab");
        assert!(
            matches!(at(&mut vcx, &ids), At::Tile(9) | At::Inside(9)),
            "Shift+Tab from the second row comes back into the first"
        );
    }

    #[gpui::test]
    fn the_remove_tile_is_hidden_until_focus_is_anywhere_in_its_row(cx: &mut TestAppContext) {
        let _probe = probe::scope();
        let (app, mut vcx, _streams) = setup(cx, &[9, 10]);
        let opacities = |vcx: &mut VisualTestContext| {
            probe::clear();
            draw(vcx);
            probe::all("forwards.remove-slot")
                .iter()
                .map(|p| p.opacity)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            opacities(&mut vcx),
            vec![Some(0.), Some(0.)],
            "nothing focused: both hidden"
        );
        focus_row(&mut vcx, 9);
        assert_eq!(
            opacities(&mut vcx),
            vec![Some(1.), Some(0.)],
            "focus on the row reveals its own tile at once and no other"
        );
        let mut on_tile = false;
        for _ in 0..4 {
            vcx.simulate_keystrokes("tab");
            if matches!(at(&mut vcx, &[9, 10]), At::Tile(9)) {
                on_tile = true;
                break;
            }
        }
        assert!(on_tile, "Tab reached the remove tile");
        assert_eq!(
            opacities(&mut vcx),
            vec![Some(1.), Some(0.)],
            "focus on the tile itself keeps it shown"
        );
        focus_pane(&app, &mut vcx);
        assert_eq!(
            opacities(&mut vcx),
            vec![Some(0.), Some(0.)],
            "focus gone: hidden again"
        );
    }

    #[gpui::test]
    fn enter_and_space_on_the_row_open_its_editor_and_remove_nothing(cx: &mut TestAppContext) {
        for key in ["enter", "space"] {
            let (app, mut vcx, _streams) = setup(cx, &[9, 10]);
            focus_row(&mut vcx, 10);
            app.read_with(&vcx, |app, _| {
                assert!(app.loopback_panel.mf_editing.is_none())
            });
            press(&mut vcx, key);
            app.read_with(&vcx, |app, _| {
                assert_eq!(
                    app.loopback_panel.mf_editing.as_ref().map(|f| f.id),
                    Some(10),
                    "{key} on the focused row opens that forward's editor"
                );
            });
            assert!(
                remove_requests::take().is_empty(),
                "{key} on the row removes nothing"
            );
            assert_eq!(
                edit_requests::take(),
                vec![(1, 10)],
                "{key} on the focused row reaches the real edit handler exactly once"
            );
        }
    }

    #[gpui::test]
    fn enter_on_the_remove_tile_asks_for_the_removal_and_opens_no_editor(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = setup(cx, &[9, 10]);
        focus_row(&mut vcx, 9);
        for _ in 0..4 {
            vcx.simulate_keystrokes("tab");
            if matches!(at(&mut vcx, &[9, 10]), At::Tile(9)) {
                break;
            }
        }
        assert_eq!(at(&mut vcx, &[9, 10]), At::Tile(9));
        press(&mut vcx, "enter");
        assert_eq!(
            remove_requests::take(),
            vec![(1, 9)],
            "Enter on the tile is the tile's own click, once, for its forward"
        );
        app.read_with(&vcx, |app, _| {
            assert!(
                app.loopback_panel.mf_editing.is_none(),
                "Enter on the tile did not bubble into the row's editor"
            );
        });
        assert_eq!(
            edit_requests::take(),
            Vec::<(u64, u64)>::new(),
            "Enter on the tile reaches the real edit handler zero times, not even in passing"
        );
    }

    #[gpui::test]
    fn delete_on_a_focused_row_asks_for_that_forward_once(cx: &mut TestAppContext) {
        // Backspace is the delete key's name on macOS only; anywhere else it
        // is Backspace, and a row is not a place to erase.
        for (key, removes) in [("delete", true), ("backspace", cfg!(target_os = "macos"))] {
            let (_app, mut vcx, _streams) = setup(cx, &[9, 10, 11]);
            focus_row(&mut vcx, 10);
            vcx.simulate_keystrokes(key);
            let expected = if removes { vec![(1, 10)] } else { Vec::new() };
            assert_eq!(
                remove_requests::take(),
                expected,
                "{key}: the focused row's forward once, or nothing where the key is not delete"
            );
        }
    }

    #[gpui::test]
    fn delete_on_the_focused_remove_tile_asks_for_its_forward_once(cx: &mut TestAppContext) {
        let (_app, mut vcx, _streams) = setup(cx, &[9, 10]);
        focus_row(&mut vcx, 10);
        for _ in 0..4 {
            vcx.simulate_keystrokes("tab");
            if matches!(at(&mut vcx, &[9, 10]), At::Tile(10)) {
                break;
            }
        }
        assert_eq!(at(&mut vcx, &[9, 10]), At::Tile(10));
        vcx.simulate_keystrokes("delete");
        assert_eq!(remove_requests::take(), vec![(1, 10)]);
    }

    #[gpui::test]
    fn a_modified_delete_is_not_a_removal(cx: &mut TestAppContext) {
        let (_app, mut vcx, _streams) = setup(cx, &[9]);
        focus_row(&mut vcx, 9);
        for chord in ["shift-delete", "secondary-delete", "alt-backspace"] {
            vcx.simulate_keystrokes(chord);
        }
        assert!(remove_requests::take().is_empty());
    }

    #[gpui::test]
    fn with_no_forwards_tab_and_delete_ask_for_nothing(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = setup(cx, &[]);
        focus_pane(&app, &mut vcx);
        for key in ["tab", "shift-tab", "delete", "backspace", "enter"] {
            vcx.simulate_keystrokes(key);
        }
        assert!(remove_requests::take().is_empty());
        assert!(row_handles::get(9).is_none(), "no row was ever drawn");
    }

    /// The positive control for the row cases above is the same keystroke
    /// with a row focused (`delete_on_a_focused_row_…`); here the terminal has
    /// focus, and the same keys must reach it as exactly what the terminal
    /// encodes them as — the pane's own stream shows it live by taking an
    /// ordinary key first.
    #[gpui::test]
    fn delete_and_backspace_with_the_terminal_focused_reach_the_terminal_not_the_rows(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, mut streams) = setup(cx, &[9]);
        focus_pane(&app, &mut vcx);
        vcx.simulate_keystrokes("x");
        assert_eq!(
            pty_input(&mut vcx, &mut streams[0]),
            Some(b"x".to_vec()),
            "control: the terminal is live, x arrives"
        );

        for (key, bytes) in [("delete", &b"\x1b[3~"[..]), ("backspace", &b"\x7f"[..])] {
            vcx.simulate_keystrokes(key);
            assert!(
                remove_requests::take().is_empty(),
                "{key} aimed at the terminal removed nothing"
            );
            assert_eq!(
                pty_input(&mut vcx, &mut streams[0]),
                Some(bytes.to_vec()),
                "{key} reached the PTY as exactly its own sequence"
            );
        }
    }

    /// A field in the forward form is not inside any row: Backspace edits its
    /// text, and the row beside it is not asked to go.
    #[gpui::test]
    fn backspace_in_the_form_field_edits_the_text_and_removes_nothing(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = setup(cx, &[9]);
        app.update_in(&mut vcx, |app, window, cx| {
            app.edit_managed_forward(forward(9), window, cx);
        });
        draw(&mut vcx);
        let field = app.read_with(&vcx, |app, _| app.loopback_panel.mf_target_port.clone());
        vcx.update(|window, cx| field.update(cx, |input, cx| input.focus(window, cx)));
        draw(&mut vcx);
        let before = field.read_with(&vcx, |input, _| input.value().to_string());
        assert_eq!(
            before, "80",
            "control: the field holds the rule's target port"
        );

        vcx.simulate_keystrokes("backspace");
        let after = field.read_with(&vcx, |input, _| input.value().to_string());
        assert_eq!(after, "8", "Backspace edited the field");
        assert!(remove_requests::take().is_empty());
    }

    #[gpui::test]
    fn a_reordered_list_keeps_focus_on_the_same_forward_and_delete_still_means_it(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = setup(cx, &[9, 10, 11]);
        focus_row(&mut vcx, 10);
        show(&app, &mut vcx, &[11, 10, 9]);
        assert_eq!(
            at(&mut vcx, &[9, 10, 11]),
            At::Row(10),
            "the row moved; its focus went with it"
        );
        vcx.simulate_keystrokes("delete");
        assert_eq!(remove_requests::take(), vec![(1, 10)]);
    }

    /// The pin for the app's own lost-focus repair (`on_focus_lost` in
    /// `Tty59App::new`): a row the daemon drops while it holds focus must not
    /// leave the window with a key handler for nothing. Not a witness for
    /// anything this change adds; it fails only if that repair goes.
    #[gpui::test]
    fn a_row_removed_from_under_focus_hands_it_to_the_terminal(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = setup(cx, &[9, 10]);
        focus_row(&mut vcx, 10);
        show(&app, &mut vcx, &[9]);
        vcx.run_until_parked();

        assert_eq!(at(&mut vcx, &[9, 10]), At::Elsewhere);
        vcx.simulate_keystrokes("x");
        let typed = pty_input(&mut vcx, &mut streams[0]);
        assert!(
            typed.as_ref().is_some_and(|b| b.contains(&b'x')),
            "a key after the row vanished reaches the terminal, got {typed:?}"
        );
    }

    #[gpui::test]
    fn closing_and_reopening_the_panel_rebinds_the_rows_to_the_keyboard(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = setup(cx, &[9]);
        focus_row(&mut vcx, 9);

        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_visible = false;
            cx.notify();
        });
        draw(&mut vcx);
        assert_eq!(
            at(&mut vcx, &[9]),
            At::Elsewhere,
            "the row went with the panel"
        );

        row_handles::clear();
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_visible = true;
            cx.notify();
        });
        draw(&mut vcx);
        focus_row(&mut vcx, 9);
        vcx.simulate_keystrokes("delete");
        assert_eq!(
            remove_requests::take(),
            vec![(1, 9)],
            "after reopening, the row takes the same keys"
        );
    }

    // ---- the way into the panel from the terminal -----------------------

    fn info_has_focus(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
        let root = app.read_with(vcx, |app, _| app.info_focus.clone());
        vcx.update(|window, _| root.is_focused(window))
    }

    fn inside_info(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
        let root = app.read_with(vcx, |app, _| app.info_focus.clone());
        vcx.update(|window, cx| root.contains_focused(window, cx))
    }

    /// Where the right panel is when the keyboard is at the terminal.
    fn start_from(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext, state: &str) {
        use crate::core::config::RightPanelTab;

        app.update_in(vcx, |app, window, cx| {
            match state {
                "closed" => app.right_panel_visible = false,
                "files" => {
                    app.right_panel_visible = true;
                    app.right_panel_tab = RightPanelTab::Files;
                }
                _ => {}
            }
            app.focus_active(window, cx);
            cx.notify();
        });
        draw(vcx);
    }

    #[gpui::test]
    fn terminal_tab_and_backtab_still_reach_the_pty_exactly(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = setup(cx, &[9]);
        focus_pane(&app, &mut vcx);
        vcx.simulate_keystrokes("x");
        assert_eq!(
            pty_input(&mut vcx, &mut streams[0]),
            Some(b"x".to_vec()),
            "control: the terminal is live"
        );
        for (key, bytes) in [("tab", &b"\t"[..]), ("shift-tab", &b"\x1b[Z"[..])] {
            vcx.simulate_keystrokes(key);
            assert_eq!(
                pty_input(&mut vcx, &mut streams[0]),
                Some(bytes.to_vec()),
                "{key} from the terminal is the PTY's, exactly"
            );
        }
        assert!(
            !inside_info(&app, &mut vcx),
            "and it never entered the panel"
        );
    }

    #[gpui::test]
    fn the_palette_item_moves_the_keyboard_into_info_from_every_starting_state(
        cx: &mut TestAppContext,
    ) {
        for state in ["closed", "files", "info"] {
            let (app, mut vcx, _streams) = setup(cx, &[9, 10]);
            start_from(&app, &mut vcx, state);
            assert!(
                !inside_info(&app, &mut vcx),
                "{state}: the keyboard starts at the terminal"
            );

            show_info_through_the_palette(&mut vcx);
            draw(&mut vcx);

            assert!(
                info_has_focus(&app, &mut vcx),
                "{state}: the keyboard is on the Info root"
            );
            app.read_with(&vcx, |app, _| {
                assert!(app.right_panel_visible, "{state}: the panel is shown");
                assert_eq!(
                    app.right_panel_tab,
                    crate::core::config::RightPanelTab::Info,
                    "{state}: on the Info tab"
                );
            });
        }
    }

    #[gpui::test]
    fn the_show_info_action_moves_the_keyboard_in_and_the_other_tab_actions_do_not(
        cx: &mut TestAppContext,
    ) {
        use crate::core::actions::{ShowRightPanelFiles, ShowRightPanelInfo};

        for state in ["closed", "files"] {
            let (app, mut vcx, _streams) = setup(cx, &[9]);
            start_from(&app, &mut vcx, state);
            vcx.dispatch_action(ShowRightPanelInfo);
            draw(&mut vcx);
            assert!(
                info_has_focus(&app, &mut vcx),
                "{state}: ShowRightPanelInfo puts the keyboard on the Info root"
            );
        }

        let (app, mut vcx, _streams) = setup(cx, &[9]);
        start_from(&app, &mut vcx, "info");
        vcx.dispatch_action(ShowRightPanelFiles);
        draw(&mut vcx);
        assert!(
            !inside_info(&app, &mut vcx) && !info_has_focus(&app, &mut vcx),
            "control: showing another tab does not move the keyboard"
        );
    }

    #[gpui::test]
    fn tab_from_the_info_root_stays_inside_the_panel_and_reaches_the_forward_rows(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = setup(cx, &[9, 10]);
        start_from(&app, &mut vcx, "files");
        show_info_through_the_palette(&mut vcx);
        draw(&mut vcx);
        assert!(info_has_focus(&app, &mut vcx));

        let mut reached = false;
        for step in 1..=40 {
            vcx.simulate_keystrokes("tab");
            assert!(
                inside_info(&app, &mut vcx),
                "Tab {step} left the Info panel"
            );
            if at(&mut vcx, &[9, 10]) == At::Row(9) {
                reached = true;
                break;
            }
        }
        assert!(
            reached,
            "Tab from the Info root reaches the first forward row"
        );
    }

    #[gpui::test]
    fn escape_in_the_info_panel_gives_the_keyboard_back_to_the_pane(cx: &mut TestAppContext) {
        for place in ["root", "row", "tile"] {
            let (app, mut vcx, mut streams) = setup(cx, &[9, 10]);
            start_from(&app, &mut vcx, "closed");
            show_info_through_the_palette(&mut vcx);
            draw(&mut vcx);
            if place != "root" {
                let mut on_row = false;
                for _ in 0..40 {
                    vcx.simulate_keystrokes("tab");
                    if at(&mut vcx, &[9, 10]) == At::Row(9) {
                        on_row = true;
                        break;
                    }
                }
                assert!(on_row, "{place}: Tab reached a forward row");
            }
            if place == "tile" {
                tab_from_the_row_through_its_switch_to_its_tile(&mut vcx, &[9, 10], 9);
                assert_eq!(at(&mut vcx, &[9, 10]), At::Tile(9));
            }
            assert!(
                inside_info(&app, &mut vcx),
                "{place}: the keyboard is in the panel"
            );

            vcx.simulate_keystrokes("escape");
            draw(&mut vcx);
            assert!(
                !inside_info(&app, &mut vcx),
                "{place}: Escape left the panel"
            );
            vcx.simulate_keystrokes("x");
            assert_eq!(
                pty_input(&mut vcx, &mut streams[0]),
                Some(b"x".to_vec()),
                "{place}: and the next key is the terminal's"
            );
        }
    }

    /// The forward form has its own Escape (`forward_form`): it closes the form,
    /// drops an edit in progress and hands the keyboard back, through
    /// `close_managed_forward_form`. A field with something to dismiss keeps
    /// its Escape; one with nothing lets it through, and the form's handler
    /// takes it. Only that handler can close the form, so this is told apart
    /// from the Info root's Escape, which leaves the form alone (the row cases
    /// of `escape_in_the_info_panel_gives_the_keyboard_back_to_the_pane`).
    #[gpui::test]
    fn escape_in_a_form_field_closes_the_form_through_its_own_handler(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = setup(cx, &[9]);
        app.update_in(&mut vcx, |app, window, cx| {
            app.edit_managed_forward(forward(9), window, cx);
        });
        draw(&mut vcx);
        let field = app.read_with(&vcx, |app, _| app.loopback_panel.mf_target_port.clone());
        vcx.update(|window, cx| field.update(cx, |input, cx| input.focus(window, cx)));
        draw(&mut vcx);
        assert!(
            inside_info(&app, &mut vcx),
            "control: the field is in the panel"
        );
        app.read_with(&vcx, |app, _| {
            assert!(
                app.loopback_panel.form_pane_id.is_some()
                    && app.loopback_panel.mf_editing.is_some(),
                "control: the form is open on an edit"
            );
        });

        vcx.simulate_keystrokes("escape");
        draw(&mut vcx);

        app.read_with(&vcx, |app, _| {
            assert!(
                app.loopback_panel.form_pane_id.is_none(),
                "the form's own Escape closed the form"
            );
            assert!(
                app.loopback_panel.mf_editing.is_none(),
                "and dropped the edit"
            );
        });
        assert_eq!(
            field.read_with(&vcx, |input, _| input.value().to_string()),
            "",
            "the existing cancellation cleared the field"
        );
        assert!(
            !inside_info(&app, &mut vcx),
            "the keyboard is out of the panel"
        );
        vcx.simulate_keystrokes("x");
        assert_eq!(pty_input(&mut vcx, &mut streams[0]), Some(b"x".to_vec()));
    }

    /// The Info root is not drawn at all on the empty home (no pane, so no
    /// panel): the action's focus then goes to a handle with nothing mounted,
    /// and the window's existing lost-focus repair puts the keyboard back where
    /// keys work.
    #[gpui::test]
    fn the_info_action_on_the_empty_home_leaves_the_keys_usable(cx: &mut TestAppContext) {
        use crate::core::actions::ShowRightPanelInfo;

        row_handles::clear();
        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.focus_active(window, cx);
            cx.notify();
        });
        draw(&mut vcx);
        let home = app.read_with(&vcx, |app, _| app.home_focus.clone());
        assert!(
            vcx.update(|window, _| home.is_focused(window)),
            "control: the empty home has the keyboard"
        );

        vcx.dispatch_action(ShowRightPanelInfo);
        draw(&mut vcx);
        vcx.run_until_parked();

        assert!(
            !info_has_focus(&app, &mut vcx),
            "the unmounted Info root does not keep the keyboard"
        );
        assert!(
            vcx.update(|window, _| home.is_focused(window)),
            "the repair put it back on the home, where the app's keys are bound"
        );
    }

    /// `[left, top, right, bottom]` of laid-out bounds, in logical pixels.
    fn edges(b: gpui::Bounds<gpui::Pixels>) -> [f32; 4] {
        [
            b.left().as_f32(),
            b.top().as_f32(),
            b.right().as_f32(),
            b.bottom().as_f32(),
        ]
    }

    /// `of` grown by `by` on every side (shrunk when negative).
    fn grown(of: [f32; 4], by: f32) -> [f32; 4] {
        [of[0] - by, of[1] - by, of[2] + by, of[3] + by]
    }

    fn close(got: [f32; 4], want: [f32; 4]) -> bool {
        got.iter()
            .zip(want.iter())
            .all(|(g, w)| (g - w).abs() < 0.02)
    }

    /// The shared ring is drawn around the row while the ROW has focus, out past
    /// its edge by the installed gap and the stroke, and is not drawn around a row
    /// whose remove tile has it (the tile draws its own) or around a row nobody
    /// has focused. Asserted on the real app's layout: the ring's own bounds.
    #[gpui::test]
    fn a_row_with_keyboard_focus_draws_the_ring_and_a_row_whose_tile_has_it_does_not(
        cx: &mut TestAppContext,
    ) {
        use gpui_component::ActiveTheme as _;

        let (_app, mut vcx, _streams) = setup(cx, &[9, 10]);
        let ids = [9, 10];
        assert!(
            vcx.debug_bounds("forward-ring-9-frame").is_some(),
            "control: the row sits in its ring wrapper"
        );
        assert!(
            vcx.debug_bounds("forward-ring-9").is_none(),
            "a row nobody focused has no ring"
        );

        focus_row(&mut vcx, 9);
        assert_eq!(at(&mut vcx, &ids), At::Row(9), "control: the row has focus");
        let gap = vcx.update(|_, cx| cx.theme().focus_ring_offset().as_f32());
        let row = vcx.debug_bounds("forward-row-9").expect("the row");
        let frame = vcx
            .debug_bounds("forward-ring-9-frame")
            .expect("the row's ring wrapper");
        assert!(
            close(edges(frame), edges(row)),
            "the wrapper is exactly the row's box: {frame:?} {row:?}"
        );
        let ring = vcx
            .debug_bounds("forward-ring-9")
            .expect("the row with focus draws its ring");
        assert!(
            close(edges(ring), grown(edges(row), 1.5 + gap)),
            "the ring is the row grown by the stroke 1.5 and the installed gap {gap}: {ring:?} {row:?}"
        );
        assert!(
            vcx.debug_bounds("forward-ring-10").is_none(),
            "only the focused row has one"
        );

        tab_from_the_row_through_its_switch_to_its_tile(&mut vcx, &ids, 9);
        assert_eq!(
            at(&mut vcx, &ids),
            At::Tile(9),
            "control: the remove tile has focus"
        );
        assert!(
            vcx.debug_bounds("forward-ring-9").is_none(),
            "the tile draws its own ring; the row's is not drawn too"
        );
    }

    /// The Info root draws the shared ring just inside the panel body while it
    /// has focus, and not while a row inside it, the terminal or nothing does.
    #[gpui::test]
    fn the_info_root_draws_the_ring_while_it_has_focus_and_not_when_a_row_or_the_terminal_has_it(
        cx: &mut TestAppContext,
    ) {
        use gpui_component::ActiveTheme as _;

        let (app, mut vcx, _streams) = setup(cx, &[9, 10]);
        start_from(&app, &mut vcx, "closed");
        show_info_through_the_palette(&mut vcx);
        draw(&mut vcx);
        assert!(
            info_has_focus(&app, &mut vcx),
            "control: the Info root has the keyboard"
        );
        let gap = vcx.update(|_, cx| cx.theme().focus_ring_offset().as_f32());
        let frame = vcx
            .debug_bounds("info-ring-frame")
            .expect("the Info body sits in its ring wrapper");
        let ring = vcx
            .debug_bounds("info-ring")
            .expect("the focused Info root draws its ring");
        assert!(
            close(edges(ring), grown(edges(frame), -gap)),
            "the ring is the body's box inset by the installed gap {gap}: {ring:?} {frame:?}"
        );

        // Focus on a row inside the panel is not focus on the root.
        for _ in 0..40 {
            vcx.simulate_keystrokes("tab");
            if at(&mut vcx, &[9, 10]) == At::Row(9) {
                break;
            }
        }
        assert_eq!(at(&mut vcx, &[9, 10]), At::Row(9), "control: on the row");
        assert!(
            vcx.debug_bounds("info-ring").is_none(),
            "the root has no ring while a row inside it has the focus"
        );

        vcx.simulate_keystrokes("escape");
        draw(&mut vcx);
        assert!(
            vcx.debug_bounds("info-ring").is_none(),
            "no ring once the keyboard is back at the terminal"
        );
    }

    /// Installs a gap of 5 and reads it back, so the frame that follows is drawn
    /// with the value the test means (a control: if it ever fails the setup is
    /// wrong, not the ring).
    fn install_a_gap_of_5(vcx: &mut VisualTestContext) {
        use gpui_component::ActiveTheme as _;

        vcx.update(|_, cx| {
            gpui_component::Theme::global_mut(cx).focus_ring_offset = px(5.);
        });
        let read = vcx.update(|_, cx| cx.theme().focus_ring_offset().as_f32());
        assert_eq!(
            read, 5.,
            "control: the installed gap reads back as 5 before the frame is drawn"
        );
    }

    /// Neither ring has the gap or the colour written into it: a later change
    /// of the installed gap moves both on the next frame.
    ///
    /// The gap is installed again after the palette command has run. Observed on
    /// Boat: running a palette command bumps its usage in the `Config`, the
    /// app's config observer (`reload_from_config`) re-applies the theme on every
    /// `Config` change, and `apply_theme` installs the policy gap again, so a 5
    /// set before the command is back to 2 by the time the Info root is drawn.
    /// The test does not depend on that happening, only on installing the 5
    /// after whatever may have re-applied the theme.
    #[gpui::test]
    fn the_row_and_root_rings_follow_the_installed_gap_in_the_real_app(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = setup(cx, &[9, 10]);
        install_a_gap_of_5(&mut vcx);
        focus_row(&mut vcx, 9);
        let row = vcx.debug_bounds("forward-row-9").expect("the row");
        let ring = vcx
            .debug_bounds("forward-ring-9")
            .expect("the row with focus draws its ring");
        assert!(
            close(edges(ring), grown(edges(row), 1.5 + 5.)),
            "the row's ring follows a gap of 5: {ring:?} {row:?}"
        );

        start_from(&app, &mut vcx, "closed");
        show_info_through_the_palette(&mut vcx);
        draw(&mut vcx);
        assert!(
            info_has_focus(&app, &mut vcx),
            "control: the Info root has the keyboard"
        );
        install_a_gap_of_5(&mut vcx);
        draw(&mut vcx);
        let frame = vcx
            .debug_bounds("info-ring-frame")
            .expect("the Info body's wrapper");
        let ring = vcx
            .debug_bounds("info-ring")
            .expect("the focused Info root draws its ring");
        assert!(
            close(edges(ring), grown(edges(frame), -5.)),
            "the root's ring follows a gap of 5: {ring:?} {frame:?}"
        );
    }
}

/// The removal itself, end to end: a real `RemoveForward` over the real
/// daemon transport to a fixture daemon that answers like the real one, and
/// the focus where the walk off a removed row put it.
///
/// Every case runs alone in a child process of the test binary. The daemon
/// socket lives in the process's config directory, which is process-global and
/// fixed once set, so a socket bound here would otherwise be shared with — and
/// seen by — every other test in the run. The child's directory is its own
/// (`pin_test_config_dir` names it after the child's pid) and the child proves
/// that before it relies on it.
#[cfg(all(test, unix))]
mod wire_tests {
    use std::io::ErrorKind;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::daemon::protocol::{ClientMsg, DaemonMsg, ForwardStatus, SshForwardKind};
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::right_panel::test_panel::open_info;

    const CHILD: &str = "TTY59_FORWARDS_WIRE_CHILD";

    /// How long a child gets. The fixture daemon's own cap cannot unblock a
    /// UI thread that is stuck, so the parent holds the clock.
    const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

    /// `N` from the `test result: ok. N passed; …` line libtest prints.
    fn passed_count(stdout: &str) -> Option<u32> {
        stdout.lines().find_map(|line| {
            let rest = line.strip_prefix("test result: ok. ")?;
            rest.split_once(" passed;")?.0.trim().parse().ok()
        })
    }

    /// In the parent: run `case` again, alone, in a child, and require that it
    /// ran and passed. In the child: return `true` and let the case go on.
    fn in_child(case: &str) -> bool {
        use std::io::Read;
        use std::process::Stdio;

        if std::env::var_os(CHILD).is_some() {
            return true;
        }
        let module = module_path!()
            .split_once("::")
            .map(|(_, rest)| rest)
            .expect("a module path has a crate");
        let name = format!("{module}::{case}");
        let mut child = Command::new(std::env::current_exe().expect("locate the test binary"))
            .args(["--exact", &name, "--nocapture"])
            .env(CHILD, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the child test process");
        let drain = |mut pipe: Box<dyn Read + Send>| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = pipe.read_to_end(&mut bytes);
                String::from_utf8_lossy(&bytes).into_owned()
            })
        };
        let stdout = drain(Box::new(child.stdout.take().expect("piped stdout")));
        let stderr = drain(Box::new(child.stderr.take().expect("piped stderr")));

        let give_up = Instant::now() + CHILD_TIMEOUT;
        let status = loop {
            match child.try_wait().expect("poll the child") {
                Some(status) => break Some(status),
                None if Instant::now() >= give_up => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        let stdout = stdout.join().expect("collect the child's stdout");
        let stderr = stderr.join().expect("collect the child's stderr");

        let Some(status) = status else {
            panic!(
                "child {name} timed out after {CHILD_TIMEOUT:?} and was killed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
            );
        };
        assert!(
            status.success(),
            "child {name} failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        // libtest exits 0 when `--exact` matches nothing, which would turn a
        // stale name into a silent pass; and exactly one ran, not "11".
        assert_eq!(
            passed_count(&stdout),
            Some(1),
            "child {name} did not run exactly one test:\n{stdout}"
        );
        false
    }

    #[derive(Default)]
    struct State {
        forwards: Vec<ManagedForward>,
        refuse_remove: bool,
        requests: Vec<String>,
        failures: Vec<String>,
    }

    /// A daemon that answers the two forward requests the panel makes, over
    /// the same socket path and framing as the real one.
    struct Fixture {
        state: Arc<Mutex<State>>,
        stop: Arc<AtomicBool>,
        server: Option<JoinHandle<()>>,
    }

    impl Fixture {
        fn start(forwards: Vec<ManagedForward>, refuse_remove: bool) -> Fixture {
            // The path the app's own transport will dial, proven rather than
            // assumed: this process's config dir, which `pin_test_config_dir`
            // names after the pid, and the socket the transport resolves in it.
            crate::core::config::pin_test_config_dir();
            let dir = tty59_core::core::config::config_dir_path().expect("a config dir");
            assert_eq!(
                dir,
                std::env::temp_dir().join(format!("tty59-covtest-{}", std::process::id())),
                "the child's config dir is its own"
            );
            let socket = dir.join("daemon.sock");
            assert_eq!(
                crate::daemon::transport::endpoint_display(),
                socket.display().to_string(),
                "the transport dials the socket in that dir"
            );
            crate::daemon::transport::remove_stale_endpoint();
            let listener = crate::daemon::transport::bind().expect("bind the fixture daemon");
            listener
                .set_nonblocking(true)
                .expect("the listener goes non-blocking");

            let state = Arc::new(Mutex::new(State {
                forwards,
                refuse_remove,
                ..State::default()
            }));
            let stop = Arc::new(AtomicBool::new(false));
            let server = std::thread::spawn({
                let (state, stop) = (state.clone(), stop.clone());
                move || serve(listener, state, stop)
            });
            Fixture {
                state,
                stop,
                server: Some(server),
            }
        }

        fn requests(&self) -> Vec<String> {
            self.state.lock().unwrap().requests.clone()
        }

        /// Stop the server, wait for it, and fail on anything it could not
        /// serve.
        fn finish(mut self) {
            self.shutdown();
            let state = self.state.lock().unwrap();
            assert!(
                state.failures.is_empty(),
                "the fixture daemon hit transport failures: {:?}",
                state.failures
            );
        }

        fn shutdown(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(server) = self.server.take() {
                server.join().expect("the fixture daemon thread joined");
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // A case that failed still joins its server.
            self.shutdown();
        }
    }

    fn serve(
        listener: crate::daemon::transport::Listener,
        state: Arc<Mutex<State>>,
        stop: Arc<AtomicBool>,
    ) {
        // Bounded: a case that never finishes cannot hold the child forever.
        let give_up = Instant::now() + Duration::from_secs(120);
        while !stop.load(Ordering::SeqCst) && Instant::now() < give_up {
            match listener.accept() {
                Ok((mut stream, _)) => answer(&mut stream, &state),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => {
                    state.lock().unwrap().failures.push(format!("accept: {e}"));
                    return;
                }
            }
        }
    }

    fn answer(stream: &mut crate::daemon::transport::Stream, state: &Arc<Mutex<State>>) {
        let fail = |what: String| state.lock().unwrap().failures.push(what);
        if let Err(e) = stream
            .set_nonblocking(false)
            .and_then(|()| stream.set_read_timeout(Some(Duration::from_secs(2))))
            .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(2))))
        {
            return fail(format!("configure the connection: {e}"));
        }
        let message = match ClientMsg::read(stream) {
            Ok(message) => message,
            // A connection that closes before saying anything is a probe, not
            // a failure.
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => return,
            Err(e) => return fail(format!("read a request: {e}")),
        };
        let reply = {
            let mut state = state.lock().unwrap();
            match message {
                ClientMsg::ListForwards { pane_id } => {
                    state.requests.push(format!("list:{pane_id}"));
                    DaemonMsg::ForwardList(state.forwards.clone())
                }
                ClientMsg::RemoveForward {
                    pane_id,
                    forward_id,
                } => {
                    state
                        .requests
                        .push(format!("remove:{pane_id}:{forward_id}"));
                    if state.refuse_remove {
                        DaemonMsg::Error("the far side refused".into())
                    } else {
                        state.forwards.retain(|f| f.id != forward_id);
                        DaemonMsg::ForwardList(state.forwards.clone())
                    }
                }
                _ => {
                    state.requests.push("other".into());
                    DaemonMsg::Error("the fixture does not serve this".into())
                }
            }
        };
        use std::io::Write as _;
        if let Err(e) = reply.encode(stream).and_then(|()| stream.flush()) {
            fail(format!("write a reply: {e}"));
        }
    }

    fn forward(id: u64) -> ManagedForward {
        ManagedForward {
            id,
            pane_id: 1,
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            bind_port: 8000 + id as u16,
            target_host: "10.0.0.5".into(),
            target_port: 80,
            description: None,
            status: ForwardStatus::Listening,
            enabled: true,
        }
    }

    fn ids(app: &gpui::Entity<Tty59App>, vcx: &VisualTestContext) -> Vec<u64> {
        app.read_with(vcx, |app, _| {
            app.loopback_panel.managed.iter().map(|f| f.id).collect()
        })
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }

    /// The app showing the fixture's forwards, after proving the fixture is
    /// the daemon the app talks to: a real list request over the real
    /// transport comes back with the fixture's list.
    fn app_on(
        cx: &mut TestAppContext,
        fixture: &Fixture,
        listed: &[u64],
    ) -> (
        gpui::Entity<Tty59App>,
        VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        row_handles::clear();
        remove_requests::take();
        edit_requests::take();
        let live = crate::terminal::RemoteTerminal::list_forwards(1);
        assert_eq!(
            live.iter().map(|f| f.id).collect::<Vec<_>>(),
            listed,
            "control: the fixture answers the app's own transport"
        );
        assert_eq!(fixture.requests(), vec!["list:1".to_string()]);

        let (app, mut vcx, streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, Vec::new(), live);
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_width.set(260.);
            cx.notify();
        });
        draw(&mut vcx);
        // Whatever the setup itself asked for is not the test's.
        edit_requests::take();
        (app, vcx, streams)
    }

    fn focused_row(vcx: &mut VisualTestContext, among: &[u64]) -> Option<u64> {
        vcx.update(|window, cx| {
            let focused = window.focused(cx)?;
            among
                .iter()
                .find(|id| row_handles::get(**id).is_some_and(|h| h == focused))
                .copied()
        })
    }

    fn focus_row(vcx: &mut VisualTestContext, id: u64) {
        let handle = row_handles::get(id).expect("the row was drawn");
        vcx.update(|window, cx| window.focus(&handle, cx));
        draw(vcx);
    }

    fn terminal_has_focus(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
        app.update_in(vcx, |app, window, cx| {
            let leaf = app.tabs[app.active].focus_target().expect("a pane");
            leaf.focus_handle(cx).contains_focused(window, cx)
        })
    }

    #[gpui::test]
    fn removing_the_middle_row_removes_it_over_the_wire_and_focuses_the_next(
        cx: &mut TestAppContext,
    ) {
        if !in_child("removing_the_middle_row_removes_it_over_the_wire_and_focuses_the_next") {
            return;
        }
        let fixture = Fixture::start(vec![forward(9), forward(10), forward(11)], false);
        let (app, mut vcx, _streams) = app_on(cx, &fixture, &[9, 10, 11]);
        focus_row(&mut vcx, 10);

        vcx.simulate_keystrokes("delete");
        draw(&mut vcx);

        assert_eq!(
            fixture.requests(),
            vec!["list:1".to_string(), "remove:1:10".to_string()],
            "exactly one RemoveForward went over the wire, for the focused forward"
        );
        assert_eq!(
            ids(&app, &vcx),
            vec![9, 11],
            "the daemon's answer is the list"
        );
        assert_eq!(
            focused_row(&mut vcx, &[9, 11]),
            Some(11),
            "focus moved to the next row"
        );

        // The same scenario by the remove tile's keyboard path: Tab onto row
        // 11's tile, Enter. That is the tile's own click, so it removes over
        // the wire once, opens no editor, and (11 being the last row) hands
        // focus back to row 9.
        let mut on_tile = false;
        for _ in 0..4 {
            vcx.simulate_keystrokes("tab");
            if vcx.update(|window, _| {
                window
                    .context_stack()
                    .iter()
                    .any(|c| c.contains(FORWARD_REMOVE_CONTEXT))
            }) {
                on_tile = true;
                break;
            }
        }
        assert!(on_tile, "Tab reached row 11's remove tile");
        press(&mut vcx, "enter");
        draw(&mut vcx);
        assert_eq!(
            fixture.requests(),
            vec![
                "list:1".to_string(),
                "remove:1:10".to_string(),
                "remove:1:11".to_string()
            ],
            "Enter on the tile sent exactly one more RemoveForward, for its own forward"
        );
        assert_eq!(ids(&app, &vcx), vec![9]);
        app.read_with(&vcx, |app, _| {
            assert!(
                app.loopback_panel.mf_editing.is_none(),
                "Enter on the tile did not open the row's editor"
            );
        });
        assert_eq!(
            edit_requests::take(),
            Vec::<(u64, u64)>::new(),
            "Enter on the tile reached the real edit handler zero times"
        );
        assert_eq!(
            focused_row(&mut vcx, &[9]),
            Some(9),
            "focus left the removed row for the previous one"
        );
        fixture.finish();
    }

    #[gpui::test]
    fn removing_the_last_row_focuses_the_previous_and_the_only_row_the_terminal(
        cx: &mut TestAppContext,
    ) {
        if !in_child("removing_the_last_row_focuses_the_previous_and_the_only_row_the_terminal") {
            return;
        }
        let fixture = Fixture::start(vec![forward(9), forward(10)], false);
        let (app, mut vcx, _streams) = app_on(cx, &fixture, &[9, 10]);

        focus_row(&mut vcx, 10);
        vcx.simulate_keystrokes("delete");
        draw(&mut vcx);
        assert_eq!(ids(&app, &vcx), vec![9]);
        assert_eq!(
            focused_row(&mut vcx, &[9]),
            Some(9),
            "no next row: focus went to the previous one, on the row itself"
        );

        vcx.simulate_keystrokes("delete");
        draw(&mut vcx);
        assert_eq!(ids(&app, &vcx), Vec::<u64>::new());
        assert!(
            terminal_has_focus(&app, &mut vcx),
            "the only row went: the terminal has the keyboard"
        );
        assert_eq!(
            fixture.requests(),
            vec![
                "list:1".to_string(),
                "remove:1:10".to_string(),
                "remove:1:9".to_string()
            ]
        );
        fixture.finish();
    }

    #[gpui::test]
    fn a_removal_the_far_side_refuses_keeps_the_row_and_the_focus(cx: &mut TestAppContext) {
        if !in_child("a_removal_the_far_side_refuses_keeps_the_row_and_the_focus") {
            return;
        }
        let fixture = Fixture::start(vec![forward(9), forward(10)], true);
        let (app, mut vcx, _streams) = app_on(cx, &fixture, &[9, 10]);
        focus_row(&mut vcx, 9);

        vcx.simulate_keystrokes("delete");
        draw(&mut vcx);

        assert_eq!(
            fixture.requests(),
            vec!["list:1".to_string(), "remove:1:9".to_string()],
            "the request really went out and was refused"
        );
        assert_eq!(ids(&app, &vcx), vec![9, 10], "the row is still there");
        assert_eq!(
            focused_row(&mut vcx, &[9, 10]),
            Some(9),
            "and still has the focus"
        );
        fixture.finish();
    }

    /// The whole way by keyboard, from the terminal and back, against the real
    /// wire: the palette's chord and the localized Info command (from the Files
    /// tab, so the panel has to change tab as well as take the keyboard), Tab
    /// through the panel to the first forward row and onto its remove tile,
    /// Enter, then Delete on the row focus lands on, and the terminal at the
    /// end. No row is focused any way but by keys.
    #[gpui::test]
    fn from_the_terminal_through_the_palette_and_info_to_a_removal_and_back(
        cx: &mut TestAppContext,
    ) {
        if !in_child("from_the_terminal_through_the_palette_and_info_to_a_removal_and_back") {
            return;
        }
        let fixture = Fixture::start(vec![forward(9), forward(10)], false);
        let (app, mut vcx, mut streams) = app_on(cx, &fixture, &[9, 10]);
        app.update_in(&mut vcx, |app, window, cx| {
            app.right_panel_visible = true;
            app.right_panel_tab = crate::core::config::RightPanelTab::Files;
            app.focus_active(window, cx);
            cx.notify();
        });
        draw(&mut vcx);

        let info = app.read_with(&vcx, |app, _| app.info_focus.clone());
        let in_info = |vcx: &mut VisualTestContext| {
            vcx.update(|window, cx| info.contains_focused(window, cx))
        };
        assert!(!in_info(&mut vcx), "the keyboard starts at the terminal");

        show_info_through_the_palette(&mut vcx);
        draw(&mut vcx);
        assert!(
            vcx.update(|window, _| info.is_focused(window)),
            "the palette's Info command put the keyboard on the Info root"
        );

        let mut on_row = false;
        for step in 1..=40 {
            vcx.simulate_keystrokes("tab");
            assert!(in_info(&mut vcx), "Tab {step} left the panel");
            if focused_row(&mut vcx, &[9, 10]) == Some(9) {
                on_row = true;
                break;
            }
        }
        assert!(on_row, "Tab reached the first forward row");

        let mut on_tile = false;
        for _ in 0..4 {
            vcx.simulate_keystrokes("tab");
            if vcx.update(|window, _| {
                window
                    .context_stack()
                    .iter()
                    .any(|c| c.contains(FORWARD_REMOVE_CONTEXT))
            }) {
                on_tile = true;
                break;
            }
        }
        assert!(on_tile, "Tab reached the row's remove tile");

        press(&mut vcx, "enter");
        draw(&mut vcx);
        assert_eq!(
            edit_requests::take(),
            Vec::<(u64, u64)>::new(),
            "Enter on the tile reached the real edit handler zero times"
        );
        assert_eq!(
            ids(&app, &vcx),
            vec![10],
            "Enter removed row 9 over the wire"
        );
        assert_eq!(
            focused_row(&mut vcx, &[10]),
            Some(10),
            "focus moved on to the next row"
        );

        vcx.simulate_keystrokes("delete");
        draw(&mut vcx);
        assert_eq!(
            ids(&app, &vcx),
            Vec::<u64>::new(),
            "Delete removed the last row"
        );
        assert!(
            terminal_has_focus(&app, &mut vcx),
            "with no row left the keyboard is back at the terminal"
        );
        vcx.simulate_keystrokes("x");
        assert_eq!(
            pty_input(&mut vcx, &mut streams[0]),
            Some(b"x".to_vec()),
            "and a typed key is the terminal's"
        );
        assert_eq!(
            fixture.requests(),
            vec![
                "list:1".to_string(),
                "remove:1:9".to_string(),
                "remove:1:10".to_string()
            ],
            "exactly the two removals went over the wire"
        );
        fixture.finish();
    }
}

/// The walk off a removed row, run on a mounted focus tree whose rows carry
/// more parts than the real ones do today: the real tree has one focusable
/// thing in a row besides the row (the remove tile), and the switch will be a
/// second. This host mounts rows made as the real ones are — the row's own
/// stop under `ForwardRow`, its remove tile under `ForwardRemove`, anything
/// else focusable in it under `ForwardSwitch` — with a stop before and after
/// the list as the panel has, and calls the real `Tty59App::leave_forward_row`
/// on the existing app harness. It decides nothing itself: where
/// focus comes to rest is read from the window.
///
/// The real app and wire cases elsewhere are untouched by it; this is the
/// "no child-count assumption" regression, with its own test count.
#[cfg(test)]
mod walk_tests {
    use gpui::{FocusHandle, Render, TestAppContext, VisualTestContext};

    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;

    struct Host {
        header: FocusHandle,
        rows: Vec<FocusHandle>,
        /// Per row: its remove tile, then `extras` more parts.
        parts: Vec<Vec<FocusHandle>>,
        footer: FocusHandle,
    }

    impl Host {
        fn new(rows: usize, extras: usize, cx: &mut Context<Self>) -> Self {
            let header = cx.focus_handle().tab_stop(true);
            let mut own = Vec::new();
            let mut parts = Vec::new();
            for _ in 0..rows {
                own.push(cx.focus_handle().tab_stop(true));
                let mut row_parts = Vec::new();
                for _ in 0..1 + extras {
                    row_parts.push(cx.focus_handle().tab_stop(true));
                }
                parts.push(row_parts);
            }
            let footer = cx.focus_handle().tab_stop(true);
            Host {
                header,
                rows: own,
                parts,
                footer,
            }
        }
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut list = div()
                .flex()
                .flex_col()
                .size_full()
                .child(div().id("header").h(px(20.)).track_focus(&self.header));
            for (i, (row, parts)) in self.rows.iter().zip(&self.parts).enumerate() {
                let mut line = div()
                    .id(("row", i))
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .track_focus(row)
                    .key_context(FORWARD_ROW_CONTEXT);
                for (j, part) in parts.iter().enumerate() {
                    let context = if j == 0 {
                        FORWARD_REMOVE_CONTEXT
                    } else {
                        FORWARD_SWITCH_CONTEXT
                    };
                    line = line.child(
                        div().key_context(context).child(
                            div()
                                .id(("part", i * 100 + j))
                                .size(px(16.))
                                .track_focus(part),
                        ),
                    );
                }
                list = list.child(line);
            }
            list.child(div().id("footer").h(px(20.)).track_focus(&self.footer))
        }
    }

    fn mounted(
        cx: &mut TestAppContext,
        rows: usize,
        extras: usize,
    ) -> (Vec<FocusHandle>, VisualTestContext) {
        let window = cx.add_window(|_, cx| Host::new(rows, extras, cx));
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        let handles = window.update(cx, |host, _, _| host.rows.clone()).unwrap();
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (handles, vcx)
    }

    /// Focus `own` in the host window, call the app's real `leave_forward_row`
    /// there, and read where focus came to rest.
    fn leave(
        app: &gpui::Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        own: &FocusHandle,
    ) -> Option<FocusHandle> {
        vcx.update(|window, cx| {
            window.focus(own, cx);
            app.update(cx, |app, cx| app.leave_forward_row(own, window, cx));
        });
        vcx.update(|window, cx| window.focused(cx))
    }

    /// The handle the app's own fallback focuses: the active pane's terminal.
    fn terminal(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) -> FocusHandle {
        app.update(vcx, |app, cx| {
            app.tabs[app.active]
                .focus_target()
                .expect("the app has a pane")
                .focus_handle(cx)
        })
    }

    #[gpui::test]
    fn the_walk_rests_on_the_neighbouring_row_however_many_parts_a_row_has(
        cx: &mut TestAppContext,
    ) {
        let (app, _app_window, _streams) = harness_with_tabs(cx, 1);
        for extras in [0, 1, 3] {
            let (rows, mut vcx) = mounted(cx, 3, extras);

            assert_eq!(
                leave(&app, &mut vcx, &rows[0]),
                Some(rows[1].clone()),
                "{extras} extra parts: off the first row, the next row itself"
            );
            assert_eq!(
                leave(&app, &mut vcx, &rows[1]),
                Some(rows[2].clone()),
                "{extras} extra parts: off the middle row, the next row itself"
            );
            assert_eq!(
                leave(&app, &mut vcx, &rows[2]),
                Some(rows[1].clone()),
                "{extras} extra parts: off the last row, the previous row itself, past its parts"
            );
        }
    }

    #[gpui::test]
    fn off_the_only_row_the_app_gives_the_focus_to_the_terminal(cx: &mut TestAppContext) {
        let (app, mut app_window, _streams) = harness_with_tabs(cx, 1);
        let terminal = terminal(&app, &mut app_window);
        for extras in [0, 2] {
            let (rows, mut vcx) = mounted(cx, 1, extras);
            assert_eq!(
                leave(&app, &mut vcx, &rows[0]),
                Some(terminal.clone()),
                "{extras} extra parts: with no other row the pane takes the key"
            );
        }
    }
}

/// The failed-forward row (backported from l0ng-ai/tty7 e7fee512, without its
/// flat mapping or its context menu): the mapping stays as the row builds it
/// and the error has a line of its own beneath, through the real app render.
#[cfg(test)]
mod error_tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::dialog::test_themes::{self as themes, Misses, TEXT_FLOOR};
    use crate::ui::paint_probe as probe;
    use crate::ui::presets::PORT_CHIP_WASH;
    use crate::ui::right_panel::test_panel::open_info;

    const REASON: &str = "bind 127.0.0.1:8080: address already in use (os error 98)";
    const LONG_HOST: &str = "reporting-replica-eu-west-1b.internal.platform.example.com";

    fn long_reason() -> String {
        format!(
            "channel 0: open failed: connect failed: {}",
            "a long ssh error that runs well past the panel ".repeat(6)
        )
    }

    fn made(
        id: u64,
        enabled: bool,
        status: ForwardStatus,
        description: Option<&str>,
    ) -> ManagedForward {
        ManagedForward {
            id,
            pane_id: 1,
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            bind_port: 8000 + id as u16,
            target_host: "10.0.0.5".into(),
            target_port: 80,
            description: description.map(str::to_string),
            status,
            enabled,
        }
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        probe::clear();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn open(
        cx: &mut TestAppContext,
        list: Vec<ManagedForward>,
    ) -> (
        gpui::Entity<Tty59App>,
        VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        // The pane's streams go back to the test with the app: dropped here
        // they would end the pane and could reshuffle what the fixtures read.
        let (app, mut vcx, streams) = harness_with_tabs(cx, 1);
        open_info(&app, &mut vcx, Vec::new(), list.clone());
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_width.set(260.);
            app.loopback_panel.managed = list;
            cx.notify();
        });
        draw(&mut vcx);
        (app, vcx, streams)
    }

    /// A failed one with a description, a listening one with one, a switched
    /// off one that "fails" only because it binds nothing, also with one.
    fn three() -> Vec<ManagedForward> {
        vec![
            made(
                9,
                true,
                ForwardStatus::Error(REASON.into()),
                Some("db tunnel"),
            ),
            made(10, true, ForwardStatus::Listening, Some("other tunnel")),
            made(
                11,
                false,
                ForwardStatus::Error("not bound".into()),
                Some("off tunnel"),
            ),
        ]
    }

    #[test]
    fn row_error_is_the_borrowed_message_only_while_the_forward_is_on_and_failed() {
        let failed = made(1, true, ForwardStatus::Error(REASON.into()), None);
        let shown = row_error(&failed).expect("a failed forward that is on has an error to show");
        assert_eq!(shown, REASON);
        let ForwardStatus::Error(held) = &failed.status else {
            unreachable!()
        };
        assert!(
            std::ptr::eq(shown.as_ptr(), held.as_ptr()),
            "it is the forward's own string, not a copy"
        );

        assert_eq!(
            row_error(&made(1, true, ForwardStatus::Listening, None)),
            None,
            "a listening forward has nothing to report"
        );
        assert_eq!(
            row_error(&made(1, false, ForwardStatus::Error(REASON.into()), None)),
            None,
            "a switched-off forward fails only because it binds nothing"
        );
        let mut socks = made(1, true, ForwardStatus::Error(REASON.into()), None);
        socks.kind = SshForwardKind::Dynamic;
        assert_eq!(row_error(&socks), Some(REASON), "a SOCKS proxy fails too");
    }

    #[gpui::test]
    fn a_failed_forward_keeps_its_mapping_and_puts_the_error_beneath_it(cx: &mut TestAppContext) {
        let _probe = probe::scope();
        let (_app, mut vcx, _streams) = open(cx, three());
        let muted = vcx.update(|_, cx| probe::hsla24(cx.theme().muted_foreground));
        let error_ink = vcx.update(|_, cx| crate::ui::theme::chrome_ink(cx).forward_error_ink);

        let row = vcx.debug_bounds("forward-row-9").expect("row laid out");
        let target = vcx
            .debug_bounds("forward-target-9")
            .expect("the failed row still has its mapping");
        vcx.debug_bounds("forward-port-9")
            .expect("and its :port, a sibling the host gives way to");
        let error = vcx
            .debug_bounds("forward-error-9")
            .expect("the error has a line of its own");
        assert!(
            error.top() >= target.bottom(),
            "the error is beneath the mapping: {error:?} {target:?}"
        );
        assert!(
            error.left() >= row.left()
                && error.right() <= row.right()
                && error.bottom() <= row.bottom(),
            "the error line is inside its row: {error:?} {row:?}"
        );
        let next = vcx.debug_bounds("forward-row-10").expect("next row");
        assert!(
            next.top() >= row.bottom(),
            "the next row starts under the grown one"
        );

        let chips = probe::all("ui.chip");
        assert_eq!(chips.len(), 3, "one bind chip per row");
        assert_eq!(
            (chips[0].fill, chips[0].fill_alpha, chips[0].text),
            (chips[1].fill, chips[1].fill_alpha, chips[1].text),
            "the failed row's chip is the live row's neutral chip, not an error chip"
        );
        assert!(
            chips[0]
                .fill_alpha
                .is_some_and(|a| (a - PORT_CHIP_WASH).abs() < 0.005),
            "and wears the shared wash"
        );
        let mapping = probe::all("forwards.mapping");
        assert_eq!(mapping.len(), 3);
        for (n, m) in mapping.iter().enumerate() {
            assert_eq!(
                m.text,
                Some(muted),
                "row {n}: the mapping is muted, the error no longer borrows it"
            );
        }
        let errors = probe::all("forwards.error");
        assert_eq!(errors.len(), 1, "only the failed, enabled row has an error");
        assert_eq!(
            errors[0].text,
            Some(error_ink),
            "in the cached forward error ink"
        );
        assert!(
            vcx.debug_bounds("forward-error-10").is_none()
                && vcx.debug_bounds("forward-error-11").is_none(),
            "a listening row and a switched-off one have no error line"
        );
    }

    #[gpui::test]
    fn the_error_takes_the_descriptions_place_only_while_the_forward_is_on_and_failing(
        cx: &mut TestAppContext,
    ) {
        let _probe = probe::scope();
        let (_app, mut vcx, _streams) = open(cx, three());
        assert!(
            vcx.debug_bounds("forward-description-9").is_none(),
            "failing and on: the error replaces the description"
        );
        assert!(
            vcx.debug_bounds("forward-description-10").is_some(),
            "listening: the description stays"
        );
        assert!(
            vcx.debug_bounds("forward-description-11").is_some(),
            "switched off: the description stays, there is no error to replace it"
        );
    }

    /// The wrapped mapping and the long error, at the default UI font and at
    /// 22px: the window's rem size really changes (read back), and the panel
    /// is asked for the legal request, 260px or the wider floor its tab labels
    /// need at that font, captured before the width is set and the frame is
    /// drawn. The layout width and the mounted box must both equal that
    /// request, and every bound is asserted again at each legal width. The
    /// exact 260px is claimed only for the default font off macOS; macOS's
    /// wider floor and the 22px font are asked for their own floor, and
    /// neither is forced down to 260.
    #[gpui::test]
    fn a_wrapped_mapping_stays_wrapped_and_a_long_error_is_one_truncated_line(
        cx: &mut TestAppContext,
    ) {
        use crate::ui::text_elision::observe;

        let _probe = probe::scope();
        let mut wrapped = made(9, true, ForwardStatus::Error(long_reason()), None);
        wrapped.bind_host = "lan-gateway-with-a-rather-long-name".into();
        wrapped.target_host = LONG_HOST.into();
        let plain = made(10, true, ForwardStatus::Error("refused".into()), None);
        let (app, mut vcx, _streams) = open(cx, vec![wrapped, plain]);

        for font in [crate::core::config::UI_FONT_SIZE_DEFAULT, 22.0_f32] {
            vcx.update(|_, cx| {
                let mut config = cx.global::<crate::core::config::Config>().clone();
                config.ui_font_size = font;
                cx.set_global(config);
            });
            // An ordinary frame at this font first, so the tab metrics the
            // panel's floor is measured from are this font's. The request is
            // captured here, from that floor, BEFORE the width is set and the
            // frame the assertions read is drawn; it is never derived from the
            // width the panel ends up with.
            vcx.run_until_parked();
            draw(&mut vcx);
            let floor = app.update_in(&mut vcx, |app, window, cx| {
                app.right_panel_min_px(window, cx)
            });
            let request = 260_f32.max(floor);
            // 260px is the ask. The product never draws the panel under its
            // own floor, so where that floor is wider (macOS, whose floor
            // counts the panel toggle beside the tabs, and any platform at
            // 22px) the legal ask is the floor itself. Only the default font
            // off macOS is exactly 260; a wider request there is a failure
            // to investigate, not a width to follow, and 22px is not forced
            // down to 260.
            if font == crate::core::config::UI_FONT_SIZE_DEFAULT && cfg!(not(target_os = "macos")) {
                assert_eq!(
                    request, 260.,
                    "off macOS the default font's panel floor {floor} is under the 260 asked for"
                );
            }
            app.update_in(&mut vcx, |app, _, cx| {
                app.right_panel_width.set(request);
                cx.notify();
            });
            vcx.run_until_parked();
            observe::clear();
            draw(&mut vcx);

            let rem = vcx.update(|window, _| window.rem_size());
            assert_eq!(
                rem,
                gpui::px(font),
                "{font}px: the window's rem really is the UI font"
            );
            // The width the layout used, after the clamp, and the box the
            // frame mounted: each is the request captured above.
            let used = app.update_in(&mut vcx, |app, window, cx| app.right_panel_px(window, cx));
            assert_eq!(
                used, request,
                "{font}px: the panel layout used {used}px, not the {request}px asked for (floor {floor})"
            );
            let mounted = vcx
                .debug_bounds("right-panel")
                .expect("the right panel was drawn");
            assert_eq!(
                mounted.size.width,
                gpui::px(request),
                "{font}px: the panel was drawn {:?} wide, not the {request}px asked for",
                mounted.size.width
            );

            let row = vcx.debug_bounds("forward-row-9").expect("row");
            assert!(
                row.size.width > gpui::px(0.) && row.size.width <= gpui::px(request),
                "{font}px: the mounted row ({:?}) fits the panel actually drawn ({request}px)",
                row.size.width
            );
            let chip = vcx.debug_bounds("forward-chip-9").expect("chip");
            let bind_port = vcx.debug_bounds("forward-bind-port-9").expect("bind port");
            let target_row = vcx.debug_bounds("forward-target-9").expect("mapping kept");
            let port = vcx.debug_bounds("forward-port-9").expect("port kept");
            let error = vcx.debug_bounds("forward-error-9").expect("error line");
            assert!(
                chip.bottom() <= target_row.top(),
                "{font}px: the mapping is wrapped below its chip, as it is without an error: {chip:?} {target_row:?}"
            );
            assert!(
                port.size.width > gpui::px(0.)
                    && port.left() >= target_row.left()
                    && port.right() <= target_row.right(),
                "{font}px: the :port keeps its room inside the mapping line: {port:?} {target_row:?}"
            );
            assert!(
                bind_port.size.width > gpui::px(0.)
                    && bind_port.left() >= chip.left()
                    && bind_port.right() <= chip.right(),
                "{font}px: the chip's port keeps its room inside the chip: {bind_port:?} {chip:?}"
            );
            assert!(
                chip.right() <= row.right() && target_row.right() <= row.right(),
                "{font}px: the mapping does not outgrow the row"
            );
            for (key, what) in [
                (("panel-forward-target", 9usize), "target host"),
                (("panel-forward-bind-host", 9usize), "bind host"),
            ] {
                let label = observe::label(&gpui::ElementId::from(key))
                    .unwrap_or_else(|| panic!("{font}px: the {what} label is drawn"));
                assert!(
                    label.text.chars().any(|c| c != '…'),
                    "{font}px: some of the {what} is left"
                );
            }
            assert!(
                error.top() >= target_row.bottom() && error.top() >= chip.bottom(),
                "{font}px: the error is beneath both lines of the mapping"
            );
            assert!(
                error.left() >= row.left()
                    && error.right() <= row.right()
                    && error.bottom() <= row.bottom(),
                "{font}px: the long error does not outgrow its row: {error:?} {row:?}"
            );
            let short = vcx.debug_bounds("forward-error-10").expect("short error");
            assert_eq!(
                error.size.height, short.size.height,
                "{font}px: a long error is one truncated line, as tall as a short one"
            );
            let next = vcx.debug_bounds("forward-row-10").expect("next row");
            assert!(
                next.top() >= row.bottom(),
                "{font}px: the next row starts under the grown one"
            );
        }
    }

    #[gpui::test]
    fn the_error_tooltip_carries_the_whole_message(cx: &mut TestAppContext) {
        let reason = long_reason();
        let (_app, mut vcx, _streams) = open(
            cx,
            vec![made(9, true, ForwardStatus::Error(reason.clone()), None)],
        );
        error_tooltips::take();
        let error = vcx.debug_bounds("forward-error-9").expect("error line");
        vcx.simulate_mouse_move(
            error.center(),
            None::<gpui::MouseButton>,
            gpui::Modifiers::none(),
        );
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(900));
        vcx.run_until_parked();
        draw(&mut vcx);
        let built = error_tooltips::take();
        assert!(
            !built.is_empty(),
            "a real hover on the error line built its tooltip"
        );
        assert!(
            built.iter().all(|text| *text == reason),
            "the tooltip's text is the whole message, never a prefix: {built:?}"
        );
    }

    #[gpui::test]
    fn the_error_ink_clears_the_text_floor_on_the_surfaces_it_is_painted_on(
        cx: &mut TestAppContext,
    ) {
        let (_app, mut vcx, _streams) = open(cx, three());
        let mut misses = Misses::default();
        for theme in &themes::themes() {
            themes::apply(&mut vcx, theme);
            let (ink, panel, base, hover) = vcx.update(|_, cx| {
                let sidebar = cx.global::<crate::ui::presets::Surfaces>().sidebar;
                (
                    crate::ui::theme::chrome_ink(cx).forward_error_ink,
                    probe::hsla24(crate::ui::theme::workspace_surface_color(cx)),
                    sidebar.base,
                    sidebar.hover,
                )
            });
            let id = theme.id.as_str();
            assert_eq!(
                ink,
                theme.chrome_ink().forward_error_ink,
                "{id}: apply_theme installs the role the presets derive"
            );
            misses.floor(
                id,
                "error ink on the panel's resting surface",
                ink,
                panel,
                TEXT_FLOOR,
            );
            misses.floor(id, "error ink on the sidebar base", ink, base, TEXT_FLOOR);
            misses.floor(
                id,
                "error ink on the hover and focus fill",
                ink,
                hover,
                TEXT_FLOOR,
            );
        }
        misses.assert_none("the failed-forward error line");
    }

    /// After `apply_theme` the semantic danger ink, the toolkit's own danger and
    /// (on most themes) the cached chrome role can be one colour, so a fixture
    /// that writes three different values into the three globals, after the
    /// theme has been applied, is what shows which one the line reads. Only the
    /// test's own globals are written; nothing in production is touched.
    #[gpui::test]
    fn the_error_ink_is_the_cached_chrome_role_not_the_semantic_or_toolkit_danger(
        cx: &mut TestAppContext,
    ) {
        let _probe = probe::scope();
        let (_app, mut vcx, _streams) = open(cx, three());
        let toolkit = gpui::hsla(0.33, 1.0, 0.5, 1.0);
        const CHROME_SENTINEL: u32 = 0x12ab34;
        const SEMANTIC_SENTINEL: u32 = 0xab1234;
        let toolkit_rgb = probe::hsla24(toolkit);
        vcx.update(|_, cx| {
            gpui_component::Theme::global_mut(cx).danger = toolkit;
            let mut semantics = crate::ui::theme::semantics(cx);
            semantics.danger.ink = SEMANTIC_SENTINEL;
            cx.set_global(semantics);
            let mut chrome = crate::ui::theme::chrome_ink(cx);
            chrome.forward_error_ink = CHROME_SENTINEL;
            cx.set_global(chrome);
        });
        assert!(
            CHROME_SENTINEL != SEMANTIC_SENTINEL
                && CHROME_SENTINEL != toolkit_rgb
                && SEMANTIC_SENTINEL != toolkit_rgb,
            "control: the three globals tell each other apart"
        );
        draw(&mut vcx);
        let errors = probe::all("forwards.error");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].text,
            Some(CHROME_SENTINEL),
            "the line reads the cached chrome role, not the semantic danger ink ({SEMANTIC_SENTINEL:06x}) \
             nor the toolkit's danger ({toolkit_rgb:06x})"
        );
    }
}

/// The shared keyboard ring as it is PAINTED: the production builders the app
/// uses for a forward row (`forward_row_shell` and `ring_row`) and for the Info
/// panel body (`panel_scroll_body` and `info_ring`), mounted in a headless window
/// whose renderer keeps the quads of the last frame. Every expectation is written
/// here from the policy, in logical pixels times the window's scale factor: a
/// 1.5 stroke, a 2 gap, the row's 6 radius grown to 9.5; the installed colour is
/// read, and the installed gap and colour are required to be those (the R role).
///
/// The headless window's scale factor is fixed (2), so nothing here claims 1x;
/// native 1x and 2x captures cover the rest.
#[cfg(test)]
mod ring_scene_tests {
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{Arc, Mutex},
    };

    use gpui::{
        AnyWindowHandle, AppContext as _, Background, Context, DevicePixels, HeadlessAppContext,
        Hsla, InteractiveElement as _, IntoElement, NoopTextSystem, ParentElement as _,
        PlatformAtlas, PlatformHeadlessRenderer, Quad, Render, Scene, Size, Styled as _, Window,
        div, px, rgb, size,
    };
    use gpui_component::{ActiveTheme as _, v_flex};

    use crate::core::config::{Config, CoreConfig};
    use crate::ui::presets;
    use crate::ui::theme::apply_theme;

    /// The host's padding, the window, and the row it mounts: the row is exactly
    /// the window's inner width, so its ring wrapper is exactly the row's box.
    const PAD: f32 = 20.;
    const WIN_W: f32 = 200.;
    const WIN_H: f32 = 140.;
    const ROW_W: f32 = WIN_W - 2. * PAD;
    const ROW_H: f32 = 30.;
    /// What the policy says, written out: stroke, gap, the row's own radius.
    const BAND: f32 = 1.5;
    const GAP: f32 = 2.;
    const ROW_RADIUS: f32 = 6.;
    /// A fill nothing else paints, for the neighbour's content.
    const MARKER: u32 = 0x12ab34;
    const MARKER_SIDE: f32 = 6.;

    /// An atlas that stores nothing: nothing under test draws a glyph.
    struct NoAtlas;

    impl PlatformAtlas for NoAtlas {
        fn get_or_insert_with<'a>(
            &self,
            _key: &gpui::AtlasKey,
            _build: &mut dyn FnMut() -> anyhow::Result<
                Option<(Size<DevicePixels>, std::borrow::Cow<'a, [u8]>)>,
            >,
        ) -> anyhow::Result<Option<gpui::AtlasTile>> {
            Ok(None)
        }

        fn remove(&self, _key: &gpui::AtlasKey) {}
    }

    /// Records the quads of the most recent scene instead of rendering it.
    struct QuadRecorder {
        quads: Arc<Mutex<Vec<Quad>>>,
    }

    impl QuadRecorder {
        /// Fails when the log is poisoned: a stale scene must never be read back
        /// as the frame just drawn.
        fn record(&self, scene: &Scene) -> anyhow::Result<()> {
            let mut quads = self
                .quads
                .lock()
                .map_err(|_| anyhow::anyhow!("quad log mutex poisoned"))?;
            *quads = scene.quads.clone();
            Ok(())
        }
    }

    impl PlatformHeadlessRenderer for QuadRecorder {
        fn render_scene_to_image(
            &mut self,
            scene: &Scene,
            _size: Size<DevicePixels>,
        ) -> anyhow::Result<image::RgbaImage> {
            self.record(scene)?;
            Ok(image::RgbaImage::new(1, 1))
        }

        fn render_scene(&mut self, scene: &Scene, _size: Size<DevicePixels>) -> anyhow::Result<()> {
            self.record(scene)
        }

        fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
            Arc::new(NoAtlas)
        }
    }

    #[derive(Clone, Copy)]
    enum Which {
        /// One forward row.
        Row,
        /// Two forward rows one pixel apart, the second carrying content.
        Stacked,
        /// The same two rows, the second painting the hover fill the pointer
        /// gives a row (the production shell's own revealed fill), under the
        /// first row's ring.
        StackedHover,
        /// The Info panel body, under its ring.
        Info,
        /// The real panel scroll body holding two forward rows and filler, taller
        /// than the body so the list can be scrolled.
        Scrolled,
    }

    /// One production row, as `forward_row` builds it: the shared shell sized
    /// for the host, in the shared ring.
    fn row_of(
        id: usize,
        handle: &gpui::FocusHandle,
        marker: bool,
        filled: bool,
        window: &Window,
        cx: &gpui::App,
    ) -> gpui::AnyElement {
        let hover = cx.global::<presets::Surfaces>().sidebar.hover;
        // `filled` stands for the pointer on the row: the shell paints the same
        // hover fill through the same `revealed` path.
        let revealed = filled || handle.contains_focused(window, cx);
        let shell = super::forward_row_shell(
            id,
            format!("ring-scene-{id}").into(),
            handle,
            revealed,
            hover,
        )
        .w(px(ROW_W))
        .h(px(ROW_H));
        let shell = match marker {
            true => shell.child(
                div()
                    .flex_none()
                    .w(px(MARKER_SIDE))
                    .h(px(MARKER_SIDE))
                    .bg(rgb(MARKER)),
            ),
            false => shell,
        };
        super::ring_row(handle, shell).into_any_element()
    }

    struct Host {
        which: Which,
        first: gpui::FocusHandle,
        second: gpui::FocusHandle,
        scroll: gpui::ScrollHandle,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            match self.which {
                Which::Row => div()
                    .size_full()
                    .p(px(PAD))
                    .child(row_of(1, &self.first, false, false, window, cx))
                    .into_any_element(),
                Which::Stacked => v_flex()
                    .size_full()
                    .p(px(PAD))
                    .gap(px(1.))
                    .child(row_of(1, &self.first, false, false, window, cx))
                    .child(row_of(2, &self.second, true, false, window, cx))
                    .into_any_element(),
                Which::StackedHover => v_flex()
                    .size_full()
                    .p(px(PAD))
                    .gap(px(1.))
                    .child(row_of(1, &self.first, false, false, window, cx))
                    .child(row_of(2, &self.second, true, true, window, cx))
                    .into_any_element(),
                Which::Scrolled => {
                    // Two real rows at the top of a list taller than its body:
                    // scrolled down a little, the first is half out of the list.
                    let inner = v_flex()
                        .child(row_of(1, &self.first, false, false, window, cx))
                        .child(row_of(2, &self.second, true, false, window, cx))
                        .child(div().h(px(ROW_H)).w_full())
                        .child(div().h(px(ROW_H)).w_full())
                        .child(div().h(px(ROW_H)).w_full())
                        .into_any_element();
                    let title = div().h(px(20.)).into_any_element();
                    v_flex()
                        .size_full()
                        .p(px(PAD))
                        .child(crate::ui::right_panel::panel_scroll_body(
                            &self.scroll,
                            inner,
                            title,
                        ))
                        .into_any_element()
                }
                Which::Info => {
                    let inner = v_flex()
                        .id("right-panel-info")
                        .track_focus(&self.first)
                        .child(div().h(px(40.)).w_full())
                        .into_any_element();
                    let title = div().h(px(20.)).into_any_element();
                    v_flex()
                        .size_full()
                        .p(px(PAD))
                        .child(crate::ui::right_panel::info_ring(
                            &self.first,
                            crate::ui::right_panel::panel_scroll_body(&self.scroll, inner, title),
                        ))
                        .into_any_element()
                }
            }
        }
    }

    struct Harness {
        cx: HeadlessAppContext,
        window: AnyWindowHandle,
        quads: Arc<Mutex<Vec<Quad>>>,
        handles: Rc<RefCell<Vec<gpui::FocusHandle>>>,
        /// The host's scroll handle, for the scrolled scene.
        scroll: gpui::ScrollHandle,
    }

    impl Harness {
        /// A headless window with `theme` installed through the real `apply_theme`.
        fn open(theme: &presets::Theme, which: Which) -> Self {
            crate::core::config::pin_test_config_dir();
            let quads = Arc::new(Mutex::new(Vec::new()));
            let recorder_quads = quads.clone();
            let mut cx = HeadlessAppContext::with_platform(
                Arc::new(NoopTextSystem::new()),
                Arc::new(()),
                move || {
                    Some(Box::new(QuadRecorder {
                        quads: recorder_quads.clone(),
                    }) as Box<dyn PlatformHeadlessRenderer>)
                },
            );
            cx.update(|cx| {
                gpui_component::init(cx);
                cx.set_global(presets::Themes(vec![theme.clone()]));
                cx.set_global(Config(CoreConfig {
                    theme_follow_system: false,
                    theme_preset: theme.id.clone(),
                    ..Default::default()
                }));
                apply_theme(None, cx);
            });
            let handles: Rc<RefCell<Vec<gpui::FocusHandle>>> = Rc::default();
            let published = handles.clone();
            let scroll = gpui::ScrollHandle::new();
            let host_scroll = scroll.clone();
            let window = cx
                .open_window(size(px(WIN_W), px(WIN_H)), move |_, cx| {
                    cx.new(|cx| {
                        let first = cx.focus_handle();
                        let second = cx.focus_handle();
                        published
                            .borrow_mut()
                            .extend([first.clone(), second.clone()]);
                        Host {
                            which,
                            first,
                            second,
                            scroll: host_scroll,
                        }
                    })
                })
                .unwrap();
            let mut harness = Self {
                cx,
                window: window.into(),
                quads,
                handles,
                scroll,
            };
            // First frame: mounts the handles.
            harness.draw();
            harness
        }

        /// Draw a frame and return the quads painted by it.
        fn draw(&mut self) -> Vec<Quad> {
            self.cx
                .update_window(self.window, |_, window, cx| {
                    window.draw(cx).clear();
                    window.render_to_image().map(|_| ())
                })
                .and_then(|drawn| drawn)
                .unwrap();
            self.quads.lock().unwrap().clone()
        }

        /// Focus the host's `index`th handle, the way a Tab or a programmatic
        /// landing would.
        fn focus(&mut self, index: usize) {
            let handle = self.handles.borrow()[index].clone();
            self.cx
                .update_window(self.window, |_, window, cx| window.focus(&handle, cx))
                .unwrap();
        }

        fn scale(&mut self) -> f32 {
            self.cx
                .update_window(self.window, |_, window, _| window.scale_factor())
                .unwrap()
        }

        /// The border-only quads whose stroke is exactly the ring's on every side.
        fn rings(&mut self, quads: &[Quad]) -> Vec<Quad> {
            let width = BAND * self.scale();
            quads
                .iter()
                .filter(|quad| {
                    let w = quad.border_widths;
                    [w.top, w.right, w.bottom, w.left]
                        .iter()
                        .all(|side| (side.0 - width).abs() < 0.01)
                })
                .copied()
                .collect()
        }

        /// The installed ring colour.
        fn ink(&mut self) -> Hsla {
            self.cx.update(|cx| cx.theme().focus_ring_color())
        }
    }

    type Edges4 = [f32; 4];

    /// `[left, top, right, bottom]` of a quad's bounds, in device pixels.
    fn edges(quad: &Quad) -> Edges4 {
        let b = quad.bounds;
        [
            b.origin.x.0,
            b.origin.y.0,
            b.origin.x.0 + b.size.width.0,
            b.origin.y.0 + b.size.height.0,
        ]
    }

    /// `e` grown by `by` device pixels on every side (shrunk when negative).
    fn grown(e: Edges4, by: f32) -> Edges4 {
        [e[0] - by, e[1] - by, e[2] + by, e[3] + by]
    }

    fn same_edges(a: Edges4, b: Edges4) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 0.01)
    }

    fn same_colour(a: Hsla, b: Hsla) -> bool {
        [(a.h, b.h), (a.s, b.s), (a.l, b.l), (a.a, b.a)]
            .iter()
            .all(|(x, y)| (x - y).abs() < 1e-4)
    }

    fn theme(id: &str) -> presets::Theme {
        presets::builtins()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap_or_else(|| panic!("the {id} built-in"))
    }

    fn assert_no_misses(what: &str, misses: Vec<String>) {
        assert!(
            misses.is_empty(),
            "{what}: {} misses\n{}",
            misses.len(),
            misses.join("\n")
        );
    }

    /// The first row's box in device pixels.
    fn first_row(scale: f32) -> Edges4 {
        [
            PAD * scale,
            PAD * scale,
            (PAD + ROW_W) * scale,
            (PAD + ROW_H) * scale,
        ]
    }

    /// A focused forward row, from the production shell in the production ring:
    /// one ring quad, the installed colour and opaque, the policy's 1.5 stroke,
    /// exactly 3.5 out from the row's edge on every side (so exactly the 2 gap
    /// between the row and the ring), its corners the row's 6 grown by gap and
    /// stroke.
    #[test]
    fn a_focused_forward_row_paints_the_ring_at_the_installed_gap_band_colour_and_concentric_radius()
     {
        let mut misses = Vec::new();
        for id in ["dark", "light"] {
            let mut harness = Harness::open(&theme(id), Which::Row);
            let scale = harness.scale();
            let ink = harness.ink();
            harness.focus(0);
            let quads = harness.draw();
            let rings = harness.rings(&quads);
            if rings.len() != 1 {
                misses.push(format!(
                    "{id}: {} ring quads painted for the focused row, expected exactly 1",
                    rings.len()
                ));
                continue;
            }
            let ring = rings[0];
            if !same_colour(ring.border_color, ink) {
                misses.push(format!(
                    "{id}: ring painted {:?}, the installed focus ring colour is {ink:?}",
                    ring.border_color
                ));
            }
            if (ring.border_color.a - 1.0).abs() > 1e-6 {
                misses.push(format!("{id}: ring alpha {}", ring.border_color.a));
            }
            let outer = grown(first_row(scale), (GAP + BAND) * scale);
            if !same_edges(edges(&ring), outer) {
                misses.push(format!(
                    "{id}: ring edges {:?}, expected the row grown by {}: {outer:?}",
                    edges(&ring),
                    (GAP + BAND) * scale
                ));
            }
            // What the ring leaves between itself and the row is exactly the gap.
            let inner = grown(edges(&ring), -BAND * scale);
            if !same_edges(inner, grown(first_row(scale), GAP * scale)) {
                misses.push(format!(
                    "{id}: the ring's inner edge {inner:?} is not {} out from the row",
                    GAP * scale
                ));
            }
            let want = (ROW_RADIUS + GAP + BAND) * scale;
            let radii = ring.corner_radii;
            for (corner, got) in [
                ("top left", radii.top_left.0),
                ("top right", radii.top_right.0),
                ("bottom right", radii.bottom_right.0),
                ("bottom left", radii.bottom_left.0),
            ] {
                if (got - want).abs() > 0.01 {
                    misses.push(format!("{id}: {corner} radius {got}, expected {want}"));
                }
            }
            // The ring is the policy's, not merely whatever is installed.
            let (installed_gap, installed_ring) = harness.cx.update(|cx| {
                (
                    cx.theme().focus_ring_offset().as_f32(),
                    cx.theme().focus_ring.is_some(),
                )
            });
            if (installed_gap - GAP).abs() > 1e-4 || !installed_ring {
                misses.push(format!(
                    "{id}: the installed gap is {installed_gap} (policy {GAP}), ring role installed: {installed_ring}"
                ));
            }
        }
        assert_no_misses("the focused row's ring", misses);
    }

    /// No ring until the row has focus, and none on a row whose neighbour has it:
    /// only the focused row is ringed, and the ring is around that row.
    #[test]
    fn an_unfocused_row_paints_no_ring_and_a_focused_neighbour_ringed_alone() {
        let mut harness = Harness::open(&theme("dark"), Which::Stacked);
        let scale = harness.scale();
        let quads = harness.draw();
        let none = harness.rings(&quads);
        assert!(
            none.is_empty(),
            "nothing focused: {} ring quads painted, expected none",
            none.len()
        );

        harness.focus(1);
        let quads = harness.draw();
        let rings = harness.rings(&quads);
        assert_eq!(
            rings.len(),
            1,
            "the second row has focus: exactly one ring, around it"
        );
        let second = {
            let first = first_row(scale);
            // The second row starts 1 logical pixel under the first.
            [
                first[0],
                first[3] + 1. * scale,
                first[2],
                first[3] + (1. + ROW_H) * scale,
            ]
        };
        assert!(
            same_edges(edges(&rings[0]), grown(second, (GAP + BAND) * scale)),
            "the ring is around the focused second row, not the first: {:?} vs {:?}",
            edges(&rings[0]),
            grown(second, (GAP + BAND) * scale)
        );
    }

    /// The Info root under the production scroll body: the ring is inside the
    /// body's edge by the gap, all the way round, the policy's stroke and the
    /// installed colour, with square corners; and not before the root has focus.
    #[test]
    fn the_focused_info_root_paints_the_ring_inside_the_real_scroll_body() {
        let mut misses = Vec::new();
        for id in ["dark", "light"] {
            let mut harness = Harness::open(&theme(id), Which::Info);
            let scale = harness.scale();
            let ink = harness.ink();
            let before = harness.draw();
            let none = harness.rings(&before);
            if !none.is_empty() {
                misses.push(format!(
                    "{id}: {} ring quads before the root had focus",
                    none.len()
                ));
            }
            harness.focus(0);
            let quads = harness.draw();
            let rings = harness.rings(&quads);
            if rings.len() != 1 {
                misses.push(format!(
                    "{id}: {} ring quads painted for the focused Info root, expected exactly 1",
                    rings.len()
                ));
                continue;
            }
            let ring = rings[0];
            if !same_colour(ring.border_color, ink) {
                misses.push(format!(
                    "{id}: ring painted {:?}, installed {ink:?}",
                    ring.border_color
                ));
            }
            let body = [
                PAD * scale,
                PAD * scale,
                (WIN_W - PAD) * scale,
                (WIN_H - PAD) * scale,
            ];
            let want = grown(body, -GAP * scale);
            if !same_edges(edges(&ring), want) {
                misses.push(format!(
                    "{id}: ring edges {:?}, expected the body inset by {}: {want:?}",
                    edges(&ring),
                    GAP * scale
                ));
            }
            let radii = ring.corner_radii;
            for (corner, got) in [
                ("top left", radii.top_left.0),
                ("top right", radii.top_right.0),
                ("bottom right", radii.bottom_right.0),
                ("bottom left", radii.bottom_left.0),
            ] {
                if got.abs() > 0.01 {
                    misses.push(format!("{id}: {corner} radius {got}, expected square"));
                }
            }
        }
        assert_no_misses("the focused Info root's ring", misses);
    }

    /// Rows sit one pixel apart, so a ring 3.5 out reaches into the next row's
    /// box by 2.5, and must stop short of its content: the neighbour's marker is
    /// painted whole and the ring ends before it. The neighbour is the row the
    /// pointer is on: it paints the hover fill the production shell gives it, and
    /// the focused row's ring must be painted OVER that fill, not under it (a later
    /// sibling's fill painted after the ring covers its bottom stroke).
    #[test]
    fn a_rows_ring_overlaps_its_neighbours_box_by_two_and_a_half_but_not_its_content() {
        let mut harness = Harness::open(&theme("dark"), Which::StackedHover);
        let scale = harness.scale();
        let hover = harness
            .cx
            .update(|cx| cx.global::<presets::Surfaces>().sidebar.hover);
        harness.focus(0);
        let quads = harness.draw();
        let rings = harness.rings(&quads);
        assert_eq!(rings.len(), 1, "one ring, around the focused first row");
        let ring = edges(&rings[0]);
        let first = first_row(scale);
        let second_top = first[3] + 1. * scale;
        let overlap = ring[3] - second_top;
        assert!(
            (overlap - 2.5 * scale).abs() < 0.01,
            "the ring reaches {overlap} device pixels into the neighbour's box, expected {}",
            2.5 * scale
        );
        let want_marker = format!("{:?}", Background::from(Hsla::from(rgb(MARKER))));
        let markers: Vec<&Quad> = quads
            .iter()
            .filter(|quad| format!("{:?}", quad.background) == want_marker)
            .collect();
        assert_eq!(markers.len(), 1, "the neighbour's content is painted once");
        let marker = edges(markers[0]);
        assert!(
            ring[3] <= marker[1] + 0.01,
            "the ring's bottom {} runs into the neighbour's content at {}",
            ring[3],
            marker[1]
        );

        // The neighbour's own hover fill, at its own box: the quad the ring's
        // bottom stroke lies inside.
        let second = [first[0], second_top, first[2], second_top + ROW_H * scale];
        let want_fill = format!("{:?}", Background::from(Hsla::from(rgb(hover))));
        let fills: Vec<&Quad> = quads
            .iter()
            .filter(|quad| {
                format!("{:?}", quad.background) == want_fill && same_edges(edges(quad), second)
            })
            .collect();
        assert_eq!(
            fills.len(),
            1,
            "control: the neighbour paints its hover fill at its own box, found {}",
            fills.len()
        );
        assert!(
            ring[3] > second[1] && ring[3] <= second[3],
            "control: the ring's bottom edge {} is inside the neighbour's fill box {:?}",
            ring[3],
            second
        );
        assert!(
            rings[0].order > fills[0].order,
            "the ring is painted over the neighbour hover fill: ring draw order {} is not above the neighbour fill's {}",
            rings[0].order,
            fills[0].order
        );
    }

    /// A focused row half scrolled out of the real panel scroll body: its ring is
    /// clipped to the list, never painted above the list's top (over the panel's
    /// header) or below its bottom. The ring quad's own clip must lie inside the
    /// scroll body's bounds.
    #[test]
    fn the_ring_of_a_row_scrolled_half_out_of_the_list_is_clipped_to_the_list() {
        let mut harness = Harness::open(&theme("dark"), Which::Scrolled);
        let scale = harness.scale();
        // One frame lays the list out, so the scroll state knows its content.
        harness.draw();
        harness.scroll.set_offset(gpui::point(px(0.), px(-15.)));
        harness.focus(0);
        let quads = harness.draw();
        let rings = harness.rings(&quads);
        assert_eq!(rings.len(), 1, "one ring, around the focused first row");
        let body = harness.scroll.bounds();
        let list = [
            body.origin.x.as_f32() * scale,
            body.origin.y.as_f32() * scale,
            (body.origin.x.as_f32() + body.size.width.as_f32()) * scale,
            (body.origin.y.as_f32() + body.size.height.as_f32()) * scale,
        ];
        let ring = edges(&rings[0]);
        assert!(
            ring[1] < list[1],
            "control: the first row is scrolled half out, its ring starts at {} above the list's top {}",
            ring[1],
            list[1]
        );
        let clip = rings[0].content_mask.bounds;
        let clip = [
            clip.origin.x.0,
            clip.origin.y.0,
            clip.origin.x.0 + clip.size.width.0,
            clip.origin.y.0 + clip.size.height.0,
        ];
        assert!(
            clip[0] >= list[0] - 0.5
                && clip[1] >= list[1] - 0.5
                && clip[2] <= list[2] + 0.5
                && clip[3] <= list[3] + 0.5,
            "the ring's clip {clip:?} reaches outside the list {list:?}: a half-scrolled row's ring must not paint over what is above or below the list"
        );
    }

    /// Neither the gap nor the colour is written into the ring: a later change of
    /// the installed values moves both on the next frame.
    #[test]
    fn the_painted_ring_follows_a_later_change_of_the_installed_gap_and_colour() {
        let mut harness = Harness::open(&theme("dark"), Which::Row);
        let scale = harness.scale();
        let sentinel = gpui::hsla(0.33, 1.0, 0.5, 1.0);
        assert!(
            !same_colour(sentinel, harness.ink()),
            "control: the sentinel is not the installed ring"
        );
        harness.cx.update(|cx| {
            let theme = gpui_component::Theme::global_mut(cx);
            theme.focus_ring = Some(sentinel);
            theme.focus_ring_offset = px(5.);
        });
        harness.focus(0);
        let quads = harness.draw();
        let rings = harness.rings(&quads);
        assert_eq!(rings.len(), 1, "one ring, around the focused row");
        assert!(
            same_colour(rings[0].border_color, sentinel),
            "the ring is painted {:?}, not the colour installed after the change {sentinel:?}",
            rings[0].border_color
        );
        let want = grown(first_row(scale), (5. + BAND) * scale);
        assert!(
            same_edges(edges(&rings[0]), want),
            "the ring edges {:?}, expected the row grown by {}: {want:?}",
            edges(&rings[0]),
            (5. + BAND) * scale
        );
    }
}
