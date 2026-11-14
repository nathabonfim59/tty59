use gpui::{AnyElement, Context, Div, Entity, FontWeight, Stateful, div, prelude::*, px, rems};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex};

use crate::core::ssh_profile::ForwardRule;
use crate::daemon::protocol::{ForwardStatus, ManagedForward, SshForwardKind, SshForwardRule};
use crate::terminal::view::TerminalView;
use crate::ui::app::Tty59App;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::right_panel::{META, TEXT_MONO};
use crate::ui::text_elision::{ElidedLabel, Mode};

/// The widest the disconnected strip's reason grows before it wraps.
const REASON_MAX_W: f32 = 420.;

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
        let theme_popover = theme.popover;

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
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(foreground)
                    .child(if host.is_empty() {
                        t(L10nKey::ForwardDisconnected).to_string()
                    } else {
                        t_fmt(L10nKey::ForwardDisconnectedFrom, &[("host", &host)])
                    }),
            )
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
            .child(
                // The strip's one action, as v4's primary: inverted neutral on
                // the popover fill the pill floats on. The red is the edge
                // and the reason; the button does not repeat it.
                Button::new("ssh-reconnect")
                    .label(crate::ui::i18n::t(crate::ui::i18n::L10nKey::Reconnect))
                    .custom(crate::ui::theme::inverted_button(theme_popover, cx))
                    .small()
                    .on_click(
                        cx.listener(|this, _, window, cx| this.restart_ssh_session(window, cx)),
                    ),
            )
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
                let mut edit = edit;
                #[cfg(test)]
                crate::ui::paint_probe::note("ssh.edit-host", &mut edit);
                edit
            }));
        // The bar only. `body_area` anchors it, together with whatever else is
        // floating down there — this used to place itself at `bottom_4` and so
        // did the remote input notice, on the same container.
        Some(bar.into_any_element())
    }

    pub(crate) fn forward_row(
        &self,
        forward: &ManagedForward,
        mono: &gpui::SharedString,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
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
        let errored = !off && matches!(forward.status, ForwardStatus::Error(_));
        // Loopback is the unmarked case and the chip is the bare port; any other
        // bind says which interface it listens on.
        let bind_host = (!matches!(forward.bind_host.as_str(), "127.0.0.1" | "localhost" | ""))
            .then(|| gpui::SharedString::from(forward.bind_host.as_str()));
        let pane_id = forward.pane_id;
        let forward_id = forward.id;
        let forward_for_edit = forward.clone();
        let group = gpui::SharedString::from(format!("panel-forward-{forward_id}"));
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
                .text_color(if errored { theme.danger } else { muted })
        };
        // What the row says after the chip. A listening or switched-off rule
        // is a mapping: the target host gives way to the room it is handed and
        // its `:port` is a sibling that never does, so what the rule opens is
        // always readable. An error or a SOCKS proxy has no target to split.
        let target = match (&forward.status, forward.kind) {
            (ForwardStatus::Error(msg), _) if !off => mono_text(grows(div()).truncate())
                .child(msg.clone())
                .into_any_element(),
            (_, SshForwardKind::Dynamic) => mono_text(grows(div()).truncate())
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
            .when_some(forward.description.clone(), |this, desc| {
                this.child(
                    div()
                        .truncate()
                        .text_size(rems(META))
                        .text_color(muted)
                        .child(desc),
                )
            });
        #[cfg(test)]
        let mut column = column;
        #[cfg(test)]
        crate::ui::paint_probe::note("forwards.column", &mut column);

        let row = h_flex()
            .id(("panel-forward", forward_id as usize))
            .group(group.clone())
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
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.edit_managed_forward(forward_for_edit.clone(), window, cx)
            }))
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
            .child(
                div()
                    .flex_shrink_0()
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
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
                        .on_click(cx.listener(
                            move |this, _, _window, cx| {
                                this.remove_managed_forward(pane_id, forward_id, cx)
                            },
                        )),
                    ),
            );
        #[cfg(test)]
        let row = row.debug_selector(|| format!("forward-row-{forward_id}"));
        row
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
                                Button::new(("ssh-managed-forward-add", pane_id))
                                    .label(if editing {
                                        t(L10nKey::Save)
                                    } else {
                                        t(L10nKey::ForwardAdd)
                                    })
                                    // v4's primary: the panel's ink as the
                                    // fill, its opaque surface as the label.
                                    .custom(crate::ui::theme::inverted_button(
                                        gpui::rgb(sf.base).into(),
                                        cx,
                                    ))
                                    .xsmall()
                                    .disabled(!complete)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.add_managed_forward(pane_id, window, cx)
                                    })),
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

    /// The width the forward panel is drawn at in these tests: the 260px a
    /// narrow window gives it, above the panel's 240px floor.
    const PANEL_W: f32 = 260.;

    /// A panel wide enough for a short bind chip and a short target to share
    /// one line, which 260px is not once the switch, the remove tile and the
    /// fixed ports are counted.
    const WIDE_PANEL_W: f32 = 420.;

    fn set_panel_width(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) {
        set_width(app, vcx, PANEL_W);
    }

    fn set_width(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext, width: f32) {
        app.update_in(vcx, |app, _, cx| {
            app.right_panel_width.set(width);
            cx.notify();
        });
    }

    /// The panel is really `PANEL_W` wide: the width layout used, read back
    /// after the clamp that keeps it between its floor and its ceiling.
    fn assert_panel_width(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) {
        assert_width(app, vcx, PANEL_W);
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
        set_panel_width(app, vcx);
        draw(vcx);
        assert_panel_width(app, vcx);
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
    /// squeezed out beside it. A guarantee about the 260px panel; the wrap is
    /// what spares the target, so it never goes without a host character.
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
        set_panel_width(&app, &mut vcx);
        draw(&mut vcx);
        assert_panel_width(&app, &mut vcx);

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
}
