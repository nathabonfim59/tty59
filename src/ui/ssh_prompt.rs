use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, IntoElement, ParentElement as _, SharedString,
    Styled as _, Subscription, Window, div, prelude::*, px, rems,
};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, FocusTrapElement as _, Sizable as _, Size, h_flex, v_flex};

use crate::core::keychain::{CredentialStore as _, OsCredentialStore};
use crate::daemon::protocol::{AuthPromptKind, AuthResponse, SshPhase};
use crate::terminal::view::TerminalView;

use super::app::Tty59App;
use super::dialog::{self, Tone};
use super::i18n::{L10nKey, t, t_fmt};
use super::right_panel::{META, META_MONO, TEXT};

/// The sheet's width, and the width of the banners stacked above it, so the
/// column reads as one.
const SHEET_W: f32 = 440.;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KiRow {
    pub text: String,
    pub echo: bool,
}

/// The connection a prompt belongs to, which is also the key the keychain
/// files its password under. A password prompt names its own user and host,
/// but nothing on the wire carries the port and a keyboard-interactive prompt
/// names none of the three — so whoever raises the sheet has to supply what it
/// knows about the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptEndpoint {
    pub user: String,
    pub host: String,
    pub port: u16,
}

/// How a private key's identity reads on the passphrase sheet: the file name
/// in the title, the whole path in a line of its own under the field.
///
/// Built once, from the path, when the prompt model is, so painting the sheet
/// splits and copies nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyDisplay {
    pub name: SharedString,
    pub path: SharedString,
}

impl KeyDisplay {
    fn of(key_path: &str) -> Self {
        Self {
            name: SharedString::from(crate::ui::path_display::split_path_leaf(key_path).1),
            path: SharedString::from(key_path.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromptModel {
    Password {
        user: String,
        host: String,
        port: u16,
        rejected: bool,
    },
    KeyPassphrase {
        /// The key's path exactly as the daemon sent it: what the keychain is
        /// keyed by and what is sent back. Never shown from here; the sheet
        /// reads [`KeyDisplay`].
        key_path: String,
        display: KeyDisplay,
        comment: String,
        rejected: bool,
    },
    KeyboardInteractive {
        name: String,
        instructions: String,
        prompts: Vec<KiRow>,
        /// `None` when nothing told the sheet which connection is asking — a
        /// route the GUI could not resolve to an SSH hop. Without it there is
        /// no keychain entry to name, so a rejected stored password can only
        /// be re-typed, not forgotten.
        endpoint: Option<PromptEndpoint>,
        stored_rejected: bool,
    },
    HostKeyUnknown {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
        /// Set when the host is already on file under some other algorithm, so
        /// the sheet can say why a known host is offering an unseen key.
        previously_known_as: Option<String>,
    },
    HostKeyChanged {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
        old_fingerprint: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeychainWrite {
    None,
    SetPassword {
        user: String,
        host: String,
        port: u16,
        secret: String,
    },
    DeletePassword {
        user: String,
        host: String,
        port: u16,
    },
    SetKeyPassphrase {
        key_path: String,
        secret: String,
    },
    DeleteKeyPassphrase {
        key_path: String,
    },
}

impl PromptModel {
    pub(crate) fn from_prompt(
        kind: AuthPromptKind,
        endpoint: Option<PromptEndpoint>,
        auto_supplied_password: bool,
    ) -> Option<PromptModel> {
        let port = endpoint.as_ref().map(|e| e.port).unwrap_or(22);
        Some(match kind {
            AuthPromptKind::Password { user, host } => PromptModel::Password {
                user,
                host,
                port,
                rejected: auto_supplied_password,
            },
            AuthPromptKind::KeyPassphrase {
                key_path,
                comment,
                rejected,
            } => PromptModel::KeyPassphrase {
                display: KeyDisplay::of(&key_path),
                key_path,
                comment,
                rejected,
            },
            AuthPromptKind::KeyboardInteractive {
                name,
                instructions,
                prompts,
                stored_rejected,
            } => PromptModel::KeyboardInteractive {
                name,
                instructions,
                prompts: prompts
                    .into_iter()
                    .map(|p| KiRow {
                        text: p.text,
                        echo: p.echo,
                    })
                    .collect(),
                endpoint,
                stored_rejected,
            },
            AuthPromptKind::HostKeyUnknown {
                host,
                port,
                algorithm,
                fingerprint_sha256,
                previously_known_as,
            } => PromptModel::HostKeyUnknown {
                host,
                port,
                algorithm,
                fingerprint: fingerprint_sha256,
                previously_known_as,
            },
            AuthPromptKind::HostKeyChanged {
                host,
                port,
                algorithm,
                fingerprint_sha256,
                old_fingerprint_sha256,
            } => PromptModel::HostKeyChanged {
                host,
                port,
                algorithm,
                fingerprint: fingerprint_sha256,
                old_fingerprint: old_fingerprint_sha256,
            },
            AuthPromptKind::Banner { .. } => return None,
        })
    }

    fn input_count(&self) -> usize {
        match self {
            PromptModel::Password { .. } | PromptModel::KeyPassphrase { .. } => 1,
            PromptModel::KeyboardInteractive { prompts, .. } => prompts.len(),
            PromptModel::HostKeyUnknown { .. } => 0,
            PromptModel::HostKeyChanged { .. } => 1,
        }
    }
}

pub(crate) fn password_submit(
    user: &str,
    host: &str,
    port: u16,
    secret: String,
    remember: bool,
    rejected: bool,
) -> (AuthResponse, KeychainWrite) {
    let write = if remember {
        KeychainWrite::SetPassword {
            user: user.to_string(),
            host: host.to_string(),
            port,
            secret: secret.clone(),
        }
    } else if rejected {
        KeychainWrite::DeletePassword {
            user: user.to_string(),
            host: host.to_string(),
            port,
        }
    } else {
        KeychainWrite::None
    };
    (AuthResponse::Secret(secret), write)
}

pub(crate) fn passphrase_submit(
    key_path: &str,
    secret: String,
    remember: bool,
    rejected: bool,
) -> (AuthResponse, KeychainWrite) {
    let write = if remember {
        KeychainWrite::SetKeyPassphrase {
            key_path: key_path.to_string(),
            secret: secret.clone(),
        }
    } else if rejected {
        // The stored passphrase is the reason this sheet is up, and the user
        // has just declined to save the replacement. Leaving the old one
        // behind would hand the same dead secret to the next connection.
        KeychainWrite::DeleteKeyPassphrase {
            key_path: key_path.to_string(),
        }
    } else {
        KeychainWrite::None
    };
    (AuthResponse::Secret(secret), write)
}

/// Keyboard-interactive answers are never saved — one of them is as likely to
/// be a one-time code as a password — so this side only ever *forgets*: the
/// stored password the daemon says the server just turned down.
pub(crate) fn ki_submit(
    endpoint: Option<&PromptEndpoint>,
    answers: Vec<String>,
    stored_rejected: bool,
) -> (AuthResponse, KeychainWrite) {
    let write = match endpoint.filter(|_| stored_rejected) {
        Some(e) => KeychainWrite::DeletePassword {
            user: e.user.clone(),
            host: e.host.clone(),
            port: e.port,
        },
        None => KeychainWrite::None,
    };
    (AuthResponse::Secrets(answers), write)
}

pub(crate) fn host_key_unknown_decision(trust: bool) -> AuthResponse {
    AuthResponse::HostKeyDecision {
        accept: trust,
        remember: trust,
    }
}

pub(crate) fn changed_confirmed(typed: &str) -> bool {
    typed.trim().eq_ignore_ascii_case("yes")
}

pub(crate) fn host_key_changed_decision(typed: &str) -> AuthResponse {
    if changed_confirmed(typed) {
        AuthResponse::HostKeyDecision {
            accept: true,
            remember: true,
        }
    } else {
        AuthResponse::HostKeyDecision {
            accept: false,
            remember: false,
        }
    }
}

pub(crate) struct SshPromptState {
    pane: Option<Entity<TerminalView>>,
    pane_id: Option<u64>,
    request_id: u64,
    model: Option<PromptModel>,
    banners: Vec<String>,
    inputs: Vec<Entity<InputState>>,
    remember: bool,
    phase: Option<SshPhase>,
    routed: Option<crate::ui::remote_connect::PendingAuth>,
    routed_host: Option<tty59_core::host::HostId>,
    /// The card, and the trap's container: focus inside it stays inside it.
    /// Not a tab stop itself.
    focus_handle: FocusHandle,
    /// The footer's two actions, one handle each for the life of the app:
    /// the back-out (Cancel, Abort) and the answer (Submit, Trust,
    /// Override). Whichever sheet is up lends them to its buttons, and a
    /// button that is not drawn, or is disabled, simply does not hold one.
    cancel_focus: FocusHandle,
    accept_focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl SshPromptState {
    pub(crate) fn new(cx: &mut Context<Tty59App>) -> Self {
        Self {
            pane: None,
            pane_id: None,
            request_id: 0,
            model: None,
            banners: Vec::new(),
            inputs: Vec::new(),
            remember: false,
            phase: None,
            routed: None,
            routed_host: None,
            focus_handle: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            accept_focus: cx.focus_handle(),
            _subs: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.pane = None;
        self.pane_id = None;
        self.request_id = 0;
        self.model = None;
        self.inputs.clear();
        self.remember = false;
        self._subs.clear();
        if let Some(pending) = self.routed.take() {
            pending.answer(AuthResponse::Cancelled);
        }
    }
}

impl Tty59App {
    pub(crate) fn on_auth_prompt_ready(
        &mut self,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_id = view.read(cx).pane_id;
        let (endpoint, auto_supplied, phase, banners, next) = {
            let term = &view.read(cx).terminal;
            let mut banners = Vec::new();
            let mut next: Option<(u64, AuthPromptKind)> = None;
            let want_prompt = self.ssh_prompt.model.is_none();
            loop {
                if want_prompt {
                    match term.take_auth_prompt() {
                        Some((_, AuthPromptKind::Banner { text })) => banners.push(text),
                        Some(p) => {
                            next = Some(p);
                            break;
                        }
                        None => break,
                    }
                } else {
                    match term.take_auth_banner() {
                        Some(text) => banners.push(text),
                        None => break,
                    }
                }
            }
            let endpoint = term.ssh_endpoint().map(|(host, port)| PromptEndpoint {
                user: term.ssh_user().unwrap_or_default(),
                host,
                port,
            });
            (
                endpoint,
                term.auto_supplied_password(),
                term.ssh_phase(),
                banners,
                next,
            )
        };

        self.ssh_prompt.banners.extend(banners);
        if phase.is_some() {
            self.ssh_prompt.phase = phase;
        }

        if let Some((request_id, kind)) = next
            && let Some(model) = PromptModel::from_prompt(kind, endpoint, auto_supplied)
        {
            let inputs = build_inputs(&model, window, cx);
            let mut subs = Vec::new();
            for input in &inputs {
                subs.push(cx.subscribe_in(
                    input,
                    window,
                    |this, _input, ev: &InputEvent, window, cx| match ev {
                        InputEvent::PressEnter { .. } => this.submit_ssh_prompt(window, cx),
                        // The changed-host sheet enables Override off the
                        // typed text, so the flag has to be recomputed
                        // between keystrokes rather than at whatever
                        // repaint happened to come along.
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    },
                ));
            }
            self.ssh_prompt.pane = Some(view.clone());
            self.ssh_prompt.pane_id = Some(pane_id);
            self.ssh_prompt.request_id = request_id;
            self.ssh_prompt.model = Some(model);
            self.ssh_prompt.inputs = inputs;
            self.ssh_prompt.remember = false;
            self.ssh_prompt._subs = subs;
            self.focus_prompt(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn raise_routed_auth(
        &mut self,
        pending: crate::ui::remote_connect::PendingAuth,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> crate::ui::remote_workspace::SheetOutcome {
        use crate::ui::remote_workspace::SheetOutcome;
        if self.ssh_prompt.model.is_some() {
            return SheetOutcome::GiveBack(pending);
        }
        let Some(model) = PromptModel::from_prompt(
            pending.prompt.clone(),
            pending.endpoint.clone(),
            pending.auto_supplied_password,
        ) else {
            if let AuthPromptKind::Banner { text } = &pending.prompt {
                self.ssh_prompt.banners.push(text.clone());
            }
            pending.answer(AuthResponse::Cancelled);
            cx.notify();
            return SheetOutcome::Raised;
        };
        let inputs = build_inputs(&model, window, cx);
        let mut subs = Vec::new();
        for input in &inputs {
            subs.push(cx.subscribe_in(
                input,
                window,
                |this, _input, ev: &InputEvent, window, cx| match ev {
                    InputEvent::PressEnter { .. } => this.submit_ssh_prompt(window, cx),
                    // Same reason as the pane-routed path above: Override's
                    // enabled state is read off the input on every repaint.
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ));
        }
        self.ssh_prompt.pane = None;
        self.ssh_prompt.pane_id = None;
        self.ssh_prompt.request_id = 0;
        self.ssh_prompt.model = Some(model);
        self.ssh_prompt.inputs = inputs;
        self.ssh_prompt.remember = false;
        self.ssh_prompt._subs = subs;
        self.ssh_prompt.routed_host = Some(pending.host);
        self.ssh_prompt.routed = Some(pending);
        self.focus_prompt(window, cx);
        cx.notify();
        SheetOutcome::Raised
    }

    /// The routed sheet on screen belongs to an attempt that has stopped
    /// waiting for it.
    pub(crate) fn routed_auth_abandoned(&self) -> bool {
        self.ssh_prompt
            .routed
            .as_ref()
            .is_some_and(|pending| pending.is_abandoned())
    }

    /// Take down a routed sheet nobody is waiting on any more, and move on to
    /// whatever else is asking. Answering it would have sent the secret into a
    /// channel with no one on the other end, and closed it only to show the
    /// next dead one queued behind it.
    pub(crate) fn retract_abandoned_routed_auth(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.routed_auth_abandoned() {
            return;
        }
        self.dismiss_and_advance(window, cx);
    }

    pub(crate) fn submit_ssh_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.ssh_prompt.model.clone() else {
            return;
        };
        let remember = self.ssh_prompt.remember;
        let values: Vec<String> = self
            .ssh_prompt
            .inputs
            .iter()
            .map(|i| i.read(cx).value().to_string())
            .collect();

        // A changed host key is the one prompt where submitting the wrong thing
        // is indistinguishable from aborting: the decision it sends for
        // anything but "yes" is byte-for-byte what Abort sends, and the sheet
        // closed either way. So Enter on a half-typed answer looked like the
        // app had swallowed the connection. Leave the sheet up instead; the
        // rejection stays available on Abort, where the user meant it.
        if let PromptModel::HostKeyChanged { .. } = &model
            && !changed_confirmed(values.first().map(String::as_str).unwrap_or_default())
        {
            return;
        }

        let (response, write) = match &model {
            PromptModel::Password {
                user,
                host,
                port,
                rejected,
            } => {
                let secret = values.first().cloned().unwrap_or_default();
                password_submit(user, host, *port, secret, remember, *rejected)
            }
            PromptModel::KeyPassphrase {
                key_path, rejected, ..
            } => {
                let secret = values.first().cloned().unwrap_or_default();
                passphrase_submit(key_path, secret, remember, *rejected)
            }
            PromptModel::KeyboardInteractive {
                endpoint,
                stored_rejected,
                ..
            } => ki_submit(endpoint.as_ref(), values, *stored_rejected),
            PromptModel::HostKeyUnknown { .. } => {
                (host_key_unknown_decision(true), KeychainWrite::None)
            }
            PromptModel::HostKeyChanged { .. } => {
                let typed = values.first().cloned().unwrap_or_default();
                (host_key_changed_decision(&typed), KeychainWrite::None)
            }
        };

        self.apply_keychain_write(write);
        self.respond_active(response, cx);
        self.dismiss_and_advance(window, cx);
    }

    pub(crate) fn cancel_ssh_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.ssh_prompt.model.clone() else {
            return;
        };
        let response = match model {
            PromptModel::HostKeyUnknown { .. } => host_key_unknown_decision(false),
            PromptModel::HostKeyChanged { .. } => AuthResponse::HostKeyDecision {
                accept: false,
                remember: false,
            },
            _ => AuthResponse::Cancelled,
        };
        self.respond_active(response, cx);
        self.dismiss_and_advance(window, cx);
    }

    pub(crate) fn trust_ssh_host_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.respond_active(host_key_unknown_decision(true), cx);
        self.dismiss_and_advance(window, cx);
    }

    pub(crate) fn toggle_ssh_remember(&mut self, cx: &mut Context<Self>) {
        self.ssh_prompt.remember = !self.ssh_prompt.remember;
        cx.notify();
    }

    /// Where focus goes when a sheet comes up, once its model is installed:
    /// a host-key sheet opens on Abort, so an Enter that is already on its
    /// way can only refuse; a credential sheet opens on its first field.
    /// Runs when the sheet is raised, never while it is drawn.
    fn focus_prompt(&self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.ssh_prompt.model {
            Some(PromptModel::HostKeyUnknown { .. } | PromptModel::HostKeyChanged { .. }) => {
                window.focus(&self.ssh_prompt.cancel_focus, cx);
            }
            Some(_) => {
                if let Some(first) = self.ssh_prompt.inputs.first() {
                    first.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            None => {}
        }
    }

    /// A footer button's action as the focus-aware button wants it: run on
    /// the app, from a press the button took itself.
    fn ssh_action(
        app: gpui::WeakEntity<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        move |window, cx| {
            let _ = app.update(cx, |this, cx| action(this, window, cx));
        }
    }

    pub(crate) fn push_ssh_connect_error(&mut self, reason: String, cx: &mut Context<Self>) {
        self.ssh_prompt.banners.push(reason);
        cx.notify();
    }

    pub(crate) fn dismiss_ssh_banner(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.ssh_prompt.banners.len() {
            self.ssh_prompt.banners.remove(ix);
            cx.notify();
        }
    }

    fn respond_active(&mut self, response: AuthResponse, cx: &Context<Self>) {
        if let Some(pending) = self.ssh_prompt.routed.take() {
            pending.answer(response);
            return;
        }
        if let (Some(pane), id) = (&self.ssh_prompt.pane, self.ssh_prompt.request_id) {
            pane.read(cx).terminal.respond_auth(id, response);
        }
    }

    fn dismiss_and_advance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.ssh_prompt.pane.clone();
        if let Some(host) = self.ssh_prompt.routed_host.take() {
            crate::ui::remote_workspace::release_auth_sheet(host, cx);
        }
        self.ssh_prompt.clear();
        if let Some(pane) = pane {
            self.on_auth_prompt_ready(pane, window, cx);
        }
        if self.ssh_prompt.model.is_none() {
            let waiting = self
                .tabs
                .iter()
                .flat_map(|t| t.pane.terminals())
                .find(|l| l.read(cx).terminal.has_pending_auth());
            if let Some(view) = waiting {
                self.on_auth_prompt_ready(view, window, cx);
            }
        }
        if self.ssh_prompt.model.is_none() {
            // Nothing else is asking, so the sheet stops rendering and the
            // focus it held goes with it. Every other overlay hands focus back
            // on the way out; this one left it on an element that was no
            // longer there, and `submit_ssh_prompt` comes through here too —
            // so after a successful login, with the pane connected and
            // waiting, the next keystroke went nowhere until the user clicked.
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    fn apply_keychain_write(&self, write: KeychainWrite) {
        let store = OsCredentialStore;
        match write {
            KeychainWrite::None => {}
            KeychainWrite::SetPassword {
                user,
                host,
                port,
                secret,
            } => {
                if let Err(e) = store.set_password(&user, &host, port, &secret) {
                    log::warn!("could not save password to keychain: {e}");
                }
            }
            KeychainWrite::DeletePassword { user, host, port } => {
                // The other three arms say when the keychain refuses them. A
                // delete that keeps failing leaves the rejected password in
                // place, so the same wrong secret is offered next connect and
                // nothing anywhere records why.
                if let Err(e) = store.delete_password(&user, &host, port) {
                    log::warn!("could not forget password in keychain: {e}");
                }
            }
            KeychainWrite::SetKeyPassphrase { key_path, secret } => {
                let path = crate::core::ssh_profile::expand_tilde(&key_path);
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let account = crate::core::keychain::key_account_from_contents(&bytes);
                        if let Err(e) = store.set_key_passphrase(&account, &secret) {
                            log::warn!("could not save key passphrase to keychain: {e}");
                        }
                    }
                    Err(e) => log::warn!("not remembering passphrase; cannot read {path}: {e}"),
                }
            }
            KeychainWrite::DeleteKeyPassphrase { key_path } => {
                // Keyed by the key file's contents, exactly as the set arm
                // above is — the keychain account for a passphrase is a hash
                // of the file, not its path. A delete that keeps failing
                // leaves the rejected passphrase in place, and the key stays
                // locked out on every later connection with nothing anywhere
                // recording why.
                let path = crate::core::ssh_profile::expand_tilde(&key_path);
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let account = crate::core::keychain::key_account_from_contents(&bytes);
                        if let Err(e) = store.delete_key_passphrase(&account) {
                            log::warn!("could not forget key passphrase in keychain: {e}");
                        }
                    }
                    Err(e) => log::warn!("not forgetting passphrase; cannot read {path}: {e}"),
                }
            }
        }
    }

    pub(crate) fn render_ssh_prompt_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.ssh_prompt.model.is_none() && self.ssh_prompt.banners.is_empty() {
            return None;
        }
        let focused_pane_id = self
            .tabs
            .get(self.active)
            .and_then(|t| t.pane.focused_or_first(window, cx))
            .map(|p| p.read(cx).pane_id);
        if self.ssh_prompt.pane_id.is_some() && focused_pane_id != self.ssh_prompt.pane_id {
            return None;
        }

        let mut stack = v_flex().gap(px(8.)).items_center();

        for (ix, banner) in self.ssh_prompt.banners.iter().enumerate() {
            stack = stack.child(self.render_ssh_banner(ix, banner, cx));
        }

        if let Some(model) = &self.ssh_prompt.model {
            stack = stack.child(self.render_ssh_sheet(model, cx));
        }

        Some(
            div()
                .absolute()
                .inset_0()
                // This is the one modal that must not be dismissed by a stray
                // click — a mis-aimed click would abandon an authentication
                // attempt mid-handshake. It gets the scrim, so it reads as
                // modal, and Escape stays the only way out.
                .bg(crate::ui::presets::scrim_fill(cx))
                .flex()
                .flex_col()
                .items_center()
                .justify_start()
                // Window-level now, so the old 48px — measured from the top of
                // the terminal area — would ride up under the title bar. The
                // same drop the switcher, the palette and the worktree prompt
                // take.
                .pt(px(crate::ui::switcher::CARD_TOP))
                .child(stack)
                .into_any_element(),
        )
    }

    fn render_ssh_banner(&self, ix: usize, text: &str, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .occlude()
            .w(px(SHEET_W))
            .items_center()
            .gap(px(10.))
            .min_h(px(dialog::HEADER_H))
            .pl(px(dialog::INSET))
            .pr(px(10.))
            .py(px(6.))
            .map(|panel| crate::ui::theme::floating_surface(panel, cx))
            .rounded(px(dialog::CARD_RADIUS))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(TEXT))
                    .child(text.to_string()),
            )
            .child(dialog::button(
                ("ssh-banner-dismiss", ix),
                t(L10nKey::Dismiss),
                Tone::Secondary,
                true,
                dialog::popover_rungs(cx),
                cx,
                cx.listener(move |this, _, _w, cx| this.dismiss_ssh_banner(ix, cx)),
            ))
            .into_any_element()
    }

    fn render_ssh_sheet(&self, model: &PromptModel, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (danger, muted, fg) = (theme.danger, theme.muted_foreground, theme.foreground);
        let rungs = dialog::popover_rungs(cx);
        let (title, danger_sheet) = match model {
            PromptModel::Password { user, host, .. } => (
                t_fmt(
                    L10nKey::SshPromptPasswordFor,
                    &[("user", user), ("host", host)],
                ),
                false,
            ),
            // The file name, not the path: a long path end-truncated the one
            // part that says which key this is. The path is under the field.
            PromptModel::KeyPassphrase { display, .. } => (
                t_fmt(
                    L10nKey::SshPromptPassphraseFor,
                    &[("key_path", &display.name)],
                ),
                false,
            ),
            PromptModel::KeyboardInteractive { name, .. } => {
                let label = if name.is_empty() {
                    t(L10nKey::SshPromptTwoFactor).to_string()
                } else {
                    name.clone()
                };
                (label, false)
            }
            PromptModel::HostKeyUnknown { host, .. } => (
                t_fmt(L10nKey::SshPromptUnknownHost, &[("host", host)]),
                false,
            ),
            PromptModel::HostKeyChanged { .. } => {
                (t(L10nKey::SshPromptHostKeyChanged).to_string(), true)
            }
        };

        // One line of the sheet's prose: caption size, in the ink it is told.
        let line =
            |text: String, ink: gpui::Hsla| div().text_size(rems(META)).text_color(ink).child(text);
        // A host line or a fingerprint, set in mono inside the detail well.
        // The theme's mono family, not the CSS keyword: gpui resolves
        // `"monospace"` as a family name, finds none, and falls back to the
        // UI face — the fingerprint was being checked in proportional type.
        let mono_family = cx.theme().mono_font_family.clone();
        let mono = |text: String, ink: gpui::Hsla| {
            div()
                .text_size(rems(META_MONO))
                .font_family(mono_family.clone())
                .text_color(ink)
                .child(text)
        };

        // A modal: Tab and Shift+Tab cycle inside the card, over whatever
        // inside it is a tab stop. The trap tracks the card's handle itself,
        // and that handle is not a stop.
        let card = dialog::card("ssh-prompt-card", title.clone(), SHEET_W, cx)
            .focus_trap("ssh-prompt-trap", &self.ssh_prompt.focus_handle)
            .key_context("SshPrompt")
            // The changed-key sheet is the one card allowed a coloured edge:
            // it is what a man-in-the-middle looks like.
            .when(danger_sheet, |panel| panel.border_color(danger))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    // A held Escape repeats into whatever is raised next, so
                    // only a fresh press backs out.
                    if !ev.is_held {
                        this.cancel_ssh_prompt(window, cx);
                    }
                }
            }))
            .child(dialog::header(
                div()
                    .when(danger_sheet, |d| d.text_color(danger))
                    .child(title),
                cx,
            ));
        #[cfg(test)]
        let card = gpui::InteractiveElement::debug_selector(card, || "ssh-prompt-card".into());

        let (body, footer) = match model {
            PromptModel::Password { rejected, .. } => {
                let body = dialog::body()
                    .when(*rejected, |c| {
                        c.child(line(t(L10nKey::StoredPasswordRejected).to_string(), danger))
                    })
                    .child(self.render_ssh_input(0, cx))
                    .child(self.render_ssh_remember(cx));
                (
                    body,
                    self.render_ssh_actions(t(L10nKey::SshPromptConnect), cx),
                )
            }
            PromptModel::KeyPassphrase {
                comment,
                rejected,
                display,
                ..
            } => {
                // The whole path, in secondary caption type: it wraps rather
                // than truncates, so a deep path still says where the key is.
                let path_line = div()
                    .text_size(rems(META))
                    .text_color(muted)
                    .child(display.path.clone());
                #[cfg(test)]
                let path_line =
                    gpui::InteractiveElement::debug_selector(path_line, || "ssh-key-path".into());
                let body = dialog::body()
                    .when(*rejected, |c| {
                        c.child(line(
                            t(L10nKey::StoredPassphraseRejected).to_string(),
                            danger,
                        ))
                    })
                    .when(!comment.is_empty(), |c| {
                        c.child(line(comment.clone(), muted))
                    })
                    .child(self.render_ssh_input(0, cx))
                    .child(path_line)
                    .child(self.render_ssh_remember(cx));
                (
                    body,
                    self.render_ssh_actions(t(L10nKey::SshPromptUnlock), cx),
                )
            }
            PromptModel::KeyboardInteractive {
                instructions,
                prompts,
                stored_rejected,
                ..
            } => {
                let mut body = dialog::body()
                    .when(*stored_rejected, |c| {
                        c.child(line(t(L10nKey::StoredPasswordRejected).to_string(), danger))
                    })
                    .when(!instructions.is_empty(), |c| {
                        c.child(line(instructions.clone(), fg))
                    });
                // Each prompt is the name of the field under it.
                for (i, row) in prompts.iter().enumerate() {
                    body = body.child(
                        v_flex()
                            .gap(px(6.))
                            .child(dialog::label(row.text.clone(), cx))
                            .child(self.render_ssh_input(i, cx)),
                    );
                }
                (
                    body,
                    self.render_ssh_actions(t(L10nKey::SshPromptSubmit), cx),
                )
            }
            PromptModel::HostKeyUnknown {
                algorithm,
                fingerprint,
                port,
                host,
                previously_known_as,
            } => {
                let body = dialog::body()
                    .child(
                        dialog::well(cx)
                            .child(mono(format!("{host}:{port}  {algorithm}"), muted))
                            .child(mono(fingerprint.clone(), fg)),
                    )
                    // A host that already has an entry under another algorithm
                    // is the ordinary way a server grows an ed25519 key beside
                    // its old ssh-rsa one. Saying so is the difference between
                    // "who is this?" and "this is the host you know, with a
                    // second key".
                    .when_some(previously_known_as.as_ref(), |c, previous| {
                        c.child(line(
                            t_fmt(
                                L10nKey::SshPromptHostKeyNewAlgorithm,
                                &[("previous_algorithm", previous), ("algorithm", algorithm)],
                            ),
                            muted,
                        ))
                    });
                let (abort, trust) = (
                    self.ssh_prompt.cancel_focus.clone(),
                    self.ssh_prompt.accept_focus.clone(),
                );
                let row = [abort.clone(), trust.clone()];
                let app = cx.entity().downgrade();
                let footer = dialog::footer(cx)
                    .child(dialog::focus_button(
                        "ssh-hk-abort",
                        t(L10nKey::Abort),
                        Tone::Secondary,
                        rungs,
                        &dialog::ButtonFocus {
                            enabled: true,
                            handle: &abort,
                            row: &row,
                        },
                        cx,
                        Self::ssh_action(app.clone(), |this, window, cx| {
                            this.cancel_ssh_prompt(window, cx)
                        }),
                    ))
                    .child(dialog::focus_button(
                        "ssh-hk-trust",
                        t(L10nKey::Trust),
                        Tone::Primary,
                        rungs,
                        &dialog::ButtonFocus {
                            enabled: true,
                            handle: &trust,
                            row: &row,
                        },
                        cx,
                        Self::ssh_action(app, |this, window, cx| {
                            this.trust_ssh_host_key(window, cx)
                        }),
                    ))
                    .into_any_element();
                (body, footer)
            }
            PromptModel::HostKeyChanged {
                algorithm,
                fingerprint,
                old_fingerprint,
                port,
                host,
            } => {
                // Override without "yes" typed used to send the *rejection* —
                // the same bytes Abort sends — and close the sheet, so the
                // button read as a way through and behaved as a way out. It is
                // dead until the word is there, which is what the line above
                // the field has been claiming all along.
                let typed = self
                    .ssh_prompt
                    .inputs
                    .first()
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                let can_override = changed_confirmed(&typed);
                let body = dialog::body()
                    .child(line(
                        t_fmt(
                            L10nKey::SshPromptHostKeyChangedBody,
                            &[("host", host), ("port", &port.to_string())],
                        ),
                        danger,
                    ))
                    .child(
                        dialog::well(cx)
                            .child(mono(format!("{host}:{port}  {algorithm}"), muted))
                            .child(mono(
                                t_fmt(L10nKey::SshPromptNewKey, &[("fingerprint", fingerprint)]),
                                fg,
                            ))
                            .child(mono(
                                t_fmt(
                                    L10nKey::SshPromptOldKey,
                                    &[("old_fingerprint", old_fingerprint)],
                                ),
                                muted,
                            )),
                    )
                    .child(
                        v_flex()
                            .gap(px(6.))
                            .child(line(t(L10nKey::HostKeyOverrideMessage).to_string(), fg))
                            .child(self.render_ssh_input(0, cx)),
                    );
                // Abort stays the emphasized one and also sits where the eye
                // lands last: a changed host key is the one prompt where the
                // safe answer wants both.
                //
                // Override is the app's one `danger` button, and it is the
                // site that earns it: the sheet is what a man-in-the-middle
                // looks like, and this was the only control in the product
                // that could act on that with no colour on it at all. It is
                // inert until the word is typed, so it stays faint until armed
                // and then goes red — the emphasis arrives exactly when the
                // button does.
                //
                // The arrows and Tab visit only what is enabled, so the row
                // has Override in it exactly when it is armed.
                let (abort, over) = (
                    self.ssh_prompt.cancel_focus.clone(),
                    self.ssh_prompt.accept_focus.clone(),
                );
                let row: Vec<FocusHandle> = if can_override {
                    vec![over.clone(), abort.clone()]
                } else {
                    vec![abort.clone()]
                };
                let app = cx.entity().downgrade();
                let footer = dialog::footer(cx)
                    .child(dialog::focus_button(
                        "ssh-hkc-override",
                        t(L10nKey::Override),
                        Tone::Danger,
                        rungs,
                        &dialog::ButtonFocus {
                            enabled: can_override,
                            handle: &over,
                            row: &row,
                        },
                        cx,
                        Self::ssh_action(app.clone(), |this, window, cx| {
                            this.submit_ssh_prompt(window, cx)
                        }),
                    ))
                    .child(dialog::focus_button(
                        "ssh-hkc-abort",
                        t(L10nKey::Abort),
                        Tone::Primary,
                        rungs,
                        &dialog::ButtonFocus {
                            enabled: true,
                            handle: &abort,
                            row: &row,
                        },
                        cx,
                        Self::ssh_action(app, |this, window, cx| {
                            this.cancel_ssh_prompt(window, cx)
                        }),
                    ))
                    .into_any_element();
                (body, footer)
            }
        };

        card.child(body).child(footer).into_any_element()
    }

    fn render_ssh_input(&self, ix: usize, cx: &App) -> AnyElement {
        match self.ssh_prompt.inputs.get(ix) {
            Some(state) => dialog::field(Input::new(state), cx).into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn render_ssh_remember(&self, cx: &mut Context<Self>) -> AnyElement {
        let row = h_flex();
        #[cfg(test)]
        let row = gpui::InteractiveElement::debug_selector(row, || "ssh-remember-row".into());
        row.child(
            Checkbox::new("ssh-remember")
                .label(t(L10nKey::SshPromptSaveToKeychain))
                .with_size(Size::Medium)
                .unchecked_border(cx.theme().muted_foreground)
                .checked(self.ssh_prompt.remember)
                .on_click(cx.listener(|this, _, _w, cx| this.toggle_ssh_remember(cx))),
        )
        .into_any_element()
    }

    /// The action sits on the right, backing out on its left.
    ///
    /// `ui::confirm_answers` settled that arrangement for every native alert
    /// the app raises; a sheet the app draws itself has no reason to mirror it.
    fn render_ssh_actions(&self, submit_label: &str, cx: &mut Context<Self>) -> AnyElement {
        let rungs = dialog::popover_rungs(cx);
        let (cancel, submit) = (
            self.ssh_prompt.cancel_focus.clone(),
            self.ssh_prompt.accept_focus.clone(),
        );
        let row = [cancel.clone(), submit.clone()];
        let app = cx.entity().downgrade();
        dialog::footer(cx)
            .child(dialog::focus_button(
                "ssh-cancel",
                t(L10nKey::Cancel),
                Tone::Secondary,
                rungs,
                &dialog::ButtonFocus {
                    enabled: true,
                    handle: &cancel,
                    row: &row,
                },
                cx,
                Self::ssh_action(app.clone(), |this, window, cx| {
                    this.cancel_ssh_prompt(window, cx)
                }),
            ))
            .child(dialog::focus_button(
                "ssh-submit",
                submit_label.to_string(),
                Tone::Primary,
                rungs,
                &dialog::ButtonFocus {
                    enabled: true,
                    handle: &submit,
                    row: &row,
                },
                cx,
                Self::ssh_action(app, |this, window, cx| this.submit_ssh_prompt(window, cx)),
            ))
            .into_any_element()
    }
}

/// The placeholder a sheet's field starts with, by what the sheet asks for.
fn input_placeholder(model: &PromptModel) -> Option<L10nKey> {
    match model {
        PromptModel::Password { .. } => Some(L10nKey::SettingsPassword),
        PromptModel::KeyPassphrase { .. } => Some(L10nKey::SshPromptPassphrasePlaceholder),
        PromptModel::KeyboardInteractive { .. }
        | PromptModel::HostKeyUnknown { .. }
        | PromptModel::HostKeyChanged { .. } => None,
    }
}

fn build_inputs(
    model: &PromptModel,
    window: &mut Window,
    cx: &mut Context<Tty59App>,
) -> Vec<Entity<InputState>> {
    let count = model.input_count();
    (0..count)
        .map(|i| {
            let masked = match model {
                PromptModel::Password { .. } | PromptModel::KeyPassphrase { .. } => true,
                PromptModel::KeyboardInteractive { prompts, .. } => {
                    prompts.get(i).map(|p| !p.echo).unwrap_or(true)
                }
                PromptModel::HostKeyUnknown { .. } | PromptModel::HostKeyChanged { .. } => false,
            };
            // What the field wants, named once at construction: a blank well
            // says nothing about whether it takes a password or a passphrase.
            // Keyboard-interactive rows carry their own label above the field.
            let placeholder = input_placeholder(model);
            cx.new(|cx| {
                let input = InputState::new(window, cx).masked(masked);
                match placeholder {
                    Some(key) => input.placeholder(t(key)),
                    None => input,
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint() -> PromptEndpoint {
        PromptEndpoint {
            user: "deploy".into(),
            host: "10.0.0.5".into(),
            port: 2222,
        }
    }

    #[test]
    fn password_prompt_carries_port_from_endpoint_and_marks_rejection() {
        let m = PromptModel::from_prompt(
            AuthPromptKind::Password {
                user: "deploy".into(),
                host: "10.0.0.5".into(),
            },
            Some(endpoint()),
            true,
        )
        .unwrap();
        assert_eq!(
            m,
            PromptModel::Password {
                user: "deploy".into(),
                host: "10.0.0.5".into(),
                port: 2222,
                rejected: true,
            }
        );
    }

    #[test]
    fn banner_is_not_a_blocking_model() {
        assert!(
            PromptModel::from_prompt(AuthPromptKind::Banner { text: "hi".into() }, None, false)
                .is_none()
        );
    }

    #[test]
    fn fr_a6_remember_overwrites() {
        let (resp, write) = password_submit("u", "h", 22, "new".into(), true, true);
        assert!(matches!(resp, AuthResponse::Secret(_)));
        assert_eq!(
            write,
            KeychainWrite::SetPassword {
                user: "u".into(),
                host: "h".into(),
                port: 22,
                secret: "new".into(),
            }
        );
    }

    #[test]
    fn fr_a6_rejected_without_remember_deletes_stale_entry() {
        let (_resp, write) = password_submit("u", "h", 22, "new".into(), false, true);
        assert_eq!(
            write,
            KeychainWrite::DeletePassword {
                user: "u".into(),
                host: "h".into(),
                port: 22,
            }
        );
    }

    #[test]
    fn non_rejection_without_remember_never_touches_keychain() {
        let (_resp, write) = password_submit("u", "h", 22, "pw".into(), false, false);
        assert_eq!(write, KeychainWrite::None);
    }

    #[test]
    fn passphrase_remember_stores_by_key_path() {
        let (_resp, write) = passphrase_submit("/home/u/.ssh/id_ed25519", "pp".into(), true, false);
        assert_eq!(
            write,
            KeychainWrite::SetKeyPassphrase {
                key_path: "/home/u/.ssh/id_ed25519".into(),
                secret: "pp".into(),
            }
        );
        let (_r, w) = passphrase_submit("/k", "pp".into(), false, false);
        assert_eq!(w, KeychainWrite::None);
    }

    /// The passphrase side of `fr_a6_rejected_without_remember_deletes_stale_entry`
    /// above. It matters more here than it does for a password: a key
    /// passphrase the daemon cannot use is not one wrong login, it is a key
    /// that never opens again, and before this the sheet had no way at all to
    /// let go of one.
    #[test]
    fn passphrase_rejected_without_remember_deletes_stale_entry() {
        let (resp, write) = passphrase_submit("~/.ssh/id_ed25519", "right".into(), false, true);
        assert!(matches!(resp, AuthResponse::Secret(_)));
        assert_eq!(
            write,
            KeychainWrite::DeleteKeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
            }
        );

        // Remembering still wins: the new secret replaces the old one, so
        // there is nothing left to delete.
        let (_r, w) = passphrase_submit("~/.ssh/id_ed25519", "right".into(), true, true);
        assert_eq!(
            w,
            KeychainWrite::SetKeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
                secret: "right".into(),
            }
        );
    }

    #[test]
    fn ki_submit_bundles_all_answers() {
        let (resp, write) = ki_submit(Some(&endpoint()), vec!["a".into(), "b".into()], false);
        assert_eq!(resp, AuthResponse::Secrets(vec!["a".into(), "b".into()]));
        assert_eq!(write, KeychainWrite::None);
    }

    /// Keyboard-interactive has no "remember" of its own — an answer may well
    /// be a one-time code — so the only keychain move it makes is letting go
    /// of the stored password the server just turned down. Without it that
    /// password was replayed into every later connection, and the prompt the
    /// user answered here never became the one the next attempt sent.
    #[test]
    fn ki_answer_forgets_the_stored_password_the_server_rejected() {
        let (resp, write) = ki_submit(Some(&endpoint()), vec!["typed".into()], true);
        assert_eq!(resp, AuthResponse::Secrets(vec!["typed".into()]));
        assert_eq!(
            write,
            KeychainWrite::DeletePassword {
                user: "deploy".into(),
                host: "10.0.0.5".into(),
                port: 2222,
            }
        );

        // A prompt nobody could tie to an endpoint has no entry to name, so it
        // must not guess one — deleting the wrong account is worse than
        // leaving the right one in place.
        let (_r, w) = ki_submit(None, vec!["typed".into()], true);
        assert_eq!(w, KeychainWrite::None);
    }

    /// The port has to come from the connection, not from the default: a
    /// prompt for `:2222` that files its keychain entry under `:22` writes a
    /// secret nothing ever reads back.
    #[test]
    fn keyboard_interactive_carries_the_endpoint_and_the_rejection_through() {
        let m = PromptModel::from_prompt(
            AuthPromptKind::KeyboardInteractive {
                name: "2FA".into(),
                instructions: "code please".into(),
                prompts: vec![],
                stored_rejected: true,
            },
            Some(endpoint()),
            false,
        )
        .unwrap();
        assert_eq!(
            m,
            PromptModel::KeyboardInteractive {
                name: "2FA".into(),
                instructions: "code please".into(),
                prompts: vec![],
                endpoint: Some(endpoint()),
                stored_rejected: true,
            }
        );
    }

    #[test]
    fn passphrase_prompt_carries_the_daemons_rejection() {
        let m = PromptModel::from_prompt(
            AuthPromptKind::KeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
                comment: String::new(),
                rejected: true,
            },
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            m,
            PromptModel::KeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
                display: KeyDisplay {
                    name: "id_ed25519".into(),
                    path: "~/.ssh/id_ed25519".into(),
                },
                comment: String::new(),
                rejected: true,
            }
        );
    }

    #[test]
    fn each_sheet_names_what_its_field_asks_for() {
        let model = |kind| PromptModel::from_prompt(kind, Some(endpoint()), false).unwrap();
        assert_eq!(
            input_placeholder(&model(AuthPromptKind::Password {
                user: "deploy".into(),
                host: "10.0.0.5".into(),
            })),
            Some(L10nKey::SettingsPassword)
        );
        assert_eq!(
            input_placeholder(&model(AuthPromptKind::KeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
                comment: String::new(),
                rejected: false,
            })),
            Some(L10nKey::SshPromptPassphrasePlaceholder)
        );
        // Keyboard-interactive rows carry their own label; host-key sheets
        // have no field to describe.
        assert_eq!(
            input_placeholder(&model(AuthPromptKind::KeyboardInteractive {
                name: String::new(),
                instructions: String::new(),
                prompts: vec![],
                stored_rejected: false,
            })),
            None
        );
        assert_eq!(
            input_placeholder(&model(AuthPromptKind::HostKeyUnknown {
                host: "h".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                fingerprint_sha256: "SHA256:x".into(),
                previously_known_as: None,
            })),
            None
        );
    }

    #[test]
    fn passphrase_display_is_the_file_name_and_the_untouched_path() {
        let deep = format!("/srv/{}/id_deploy", "d".repeat(300));
        let cases: [(&str, &str); 6] = [
            ("/home/u/.ssh/id_deploy", "id_deploy"),
            ("/home/ü/鍵/まい/id_ed25519", "id_ed25519"),
            ("C:\\Users\\dev/.ssh\\id_rsa", "id_rsa"),
            ("id_bare", "id_bare"),
            (&deep, "id_deploy"),
            // No file name after the last separator: the path itself is all
            // there is to show.
            ("/home/u/.ssh/", "/home/u/.ssh/"),
        ];
        for (path, name) in cases {
            let model = PromptModel::from_prompt(
                AuthPromptKind::KeyPassphrase {
                    key_path: path.into(),
                    comment: String::new(),
                    rejected: false,
                },
                None,
                false,
            )
            .unwrap();
            let PromptModel::KeyPassphrase {
                key_path, display, ..
            } = model
            else {
                panic!("a key passphrase prompt builds a KeyPassphrase model");
            };
            assert_eq!(&*display.name, name, "{path}");
            assert_eq!(&*display.path, path, "{path}");
            // What the keychain files the secret under is the daemon's string.
            assert_eq!(key_path, path, "{path}");
        }
    }

    #[test]
    fn unknown_host_trust_accepts_and_remembers_abort_rejects() {
        assert_eq!(
            host_key_unknown_decision(true),
            AuthResponse::HostKeyDecision {
                accept: true,
                remember: true
            }
        );
        assert_eq!(
            host_key_unknown_decision(false),
            AuthResponse::HostKeyDecision {
                accept: false,
                remember: false
            }
        );
    }

    #[test]
    fn changed_host_never_auto_accepts() {
        assert!(changed_confirmed("yes"));
        assert!(changed_confirmed("  YES "));
        assert!(!changed_confirmed(""));
        assert!(!changed_confirmed("y"));
        assert!(!changed_confirmed("no"));
        assert_eq!(
            host_key_changed_decision(""),
            AuthResponse::HostKeyDecision {
                accept: false,
                remember: false
            }
        );
        assert_eq!(
            host_key_changed_decision("yes"),
            AuthResponse::HostKeyDecision {
                accept: true,
                remember: true
            }
        );
    }

    /// `changed_confirmed` is also what enables the Override button, so the
    /// button and the decision can never disagree about what "yes" means — the
    /// bug was a button that offered a way through and sent the rejection.
    #[test]
    fn override_is_enabled_by_exactly_what_accepts() {
        for typed in ["", " ", "y", "no", "yesss"] {
            assert!(
                !changed_confirmed(typed),
                "{typed:?} must leave Override disabled"
            );
            assert_eq!(
                host_key_changed_decision(typed),
                AuthResponse::HostKeyDecision {
                    accept: false,
                    remember: false
                }
            );
        }
        for typed in ["yes", "YES", " yes "] {
            assert!(changed_confirmed(typed), "{typed:?} must enable Override");
        }
    }

    /// The host is known, just not by this algorithm — the mild confirmation,
    /// carrying the algorithm it *is* known by, and never the danger sheet.
    #[test]
    fn a_new_algorithm_raises_the_unknown_host_sheet_not_the_changed_one() {
        let m = PromptModel::from_prompt(
            AuthPromptKind::HostKeyUnknown {
                host: "example.com".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                fingerprint_sha256: "SHA256:new".into(),
                previously_known_as: Some("ssh-rsa".into()),
            },
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            m,
            PromptModel::HostKeyUnknown {
                host: "example.com".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                fingerprint: "SHA256:new".into(),
                previously_known_as: Some("ssh-rsa".into()),
            }
        );
        assert_eq!(m.input_count(), 0);
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::session::Session;
    use gpui::{TestAppContext, VisualTestContext, WindowHandle};

    fn harness(cx: &mut TestAppContext) -> (WindowHandle<Tty59App>, VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
        });
        let window = cx.add_window(|window, cx| {
            Tty59App::with_session(None, Some(Session::default()), window, cx)
        });
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.background_executor.run_until_parked();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (window, vcx)
    }

    fn host_key_sheet() -> PromptModel {
        PromptModel::HostKeyUnknown {
            host: "example".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:zzz".into(),
            previously_known_as: None,
        }
    }

    /// #820. A routed sheet whose attempt has stopped waiting is taken down
    /// rather than left for the user to type a password into nothing — and a
    /// sheet somebody is still waiting on is left exactly where it is.
    #[gpui::test]
    fn an_abandoned_routed_sheet_is_taken_down(cx: &mut TestAppContext) {
        let (window, _vcx) = harness(cx);
        let host = tty59_core::host::HostId(0x820);
        let prompt = AuthPromptKind::Password {
            user: "me".into(),
            host: "example".into(),
        };
        let (pending, _rx, asker) = crate::ui::remote_connect::PendingAuth::for_test(host, prompt);

        window
            .update(cx, |app, window, cx| {
                // Host-key model for the same reason as the test below: no
                // `InputState` in this harness.
                app.ssh_prompt.model = Some(host_key_sheet());
                app.ssh_prompt.routed_host = Some(host);
                app.ssh_prompt.routed = Some(pending);
                app.retract_abandoned_routed_auth(window, cx);
                assert!(
                    app.ssh_prompt.model.is_some(),
                    "somebody is still waiting on this one"
                );
            })
            .unwrap();

        drop(asker);
        window
            .update(cx, |app, window, cx| {
                assert!(app.routed_auth_abandoned());
                app.retract_abandoned_routed_auth(window, cx);
                assert!(
                    app.ssh_prompt.model.is_none(),
                    "an answer to this sheet would go nowhere, so it is gone"
                );
                assert!(app.ssh_prompt.routed.is_none());
            })
            .unwrap();
    }

    /// The sheet takes focus into an input it owns, and dismissing it drops
    /// that input. Every other overlay in the app hands focus back on the way
    /// out; this one left it on a handle whose element had stopped rendering.
    /// `submit_ssh_prompt` shares the path, so the same thing happened after a
    /// *successful* login, where the pane is alive and waiting — and the next
    /// keystroke went nowhere until the user clicked.
    #[gpui::test]
    fn dismissing_the_last_prompt_hands_focus_back(cx: &mut TestAppContext) {
        let (window, _vcx) = harness(cx);

        let pane_focus = window
            .update(cx, |app, window, cx| {
                // The host-key sheet is the one model with no text fields, so
                // it stands in for a raised sheet without needing an
                // `InputState` — which this harness cannot build, its window
                // root being the app rather than gpui-component's `Root`.
                app.ssh_prompt.model = Some(PromptModel::HostKeyUnknown {
                    host: "example".into(),
                    port: 22,
                    algorithm: "ssh-ed25519".into(),
                    fingerprint: "SHA256:zzz".into(),
                    previously_known_as: None,
                });
                window.focus(&app.ssh_prompt.focus_handle, cx);
                // A headless harness has no live pane, so `focus_active`
                // falls through to the home screen's handle. Which target it
                // picks is not what is under test — that it picks one is.
                app.home_focus.clone()
            })
            .expect("the app window stays open");
        cx.background_executor.run_until_parked();

        // Sanity: the sheet holds focus while it is up.
        assert!(
            !window
                .update(cx, |_, window, _| pane_focus.is_focused(window))
                .unwrap(),
            "the sheet should hold focus while it is up"
        );

        window
            .update(cx, |app, window, cx| app.cancel_ssh_prompt(window, cx))
            .expect("the app window stays open");
        cx.background_executor.run_until_parked();

        assert!(
            window
                .update(cx, |_, window, _| pane_focus.is_focused(window))
                .unwrap(),
            "dismissing the last prompt must hand focus back to the app"
        );
    }
}

/// The changed-key sheet in the real app tree: its height while the user
/// types the confirmation, and that "yes" still arms Override exactly as it
/// always did.
#[cfg(test)]
mod changed_card_tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::session::Session;
    use gpui::{AppContext, TestAppContext, VisualTestContext};

    fn harness(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
            crate::ui::keymap::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| Tty59App::with_session(None, Some(Session::default()), window, cx));
            gpui_component::Root::new(app, window, cx)
        });
        cx.background_executor.run_until_parked();
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<Tty59App>()
                    .unwrap_or_else(|_| panic!("window root wraps a Tty59App"))
            })
            .unwrap();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (app, vcx)
    }

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        let _ = vcx.update(|window, cx| window.draw(cx));
        vcx.run_until_parked();
    }

    fn type_into_sheet(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, text: &'static str) {
        app.update_in(vcx, |app, window, cx| {
            let input = app.ssh_prompt.inputs[0].clone();
            input.update(cx, |s, cx| s.set_value(text, window, cx));
        });
        settle(vcx);
    }

    fn card_height(vcx: &mut VisualTestContext) -> f32 {
        let bounds = vcx
            .debug_bounds("ssh-prompt-card")
            .expect("the changed-key sheet is drawn by the app's render");
        f32::from(bounds.size.height)
    }

    #[gpui::test]
    fn the_changed_key_card_keeps_its_height_and_yes_still_arms_override(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        let (pending, rx, _asker) = crate::ui::remote_connect::PendingAuth::for_test(
            tty59_core::host::HostId(0x14),
            AuthPromptKind::HostKeyChanged {
                host: "127.0.0.1".into(),
                port: 2222,
                algorithm: "ssh-ed25519".into(),
                fingerprint_sha256: "SHA256:new".into(),
                old_fingerprint_sha256: "SHA256:old".into(),
            },
        );
        app.update_in(&mut vcx, |app, window, cx| {
            let outcome = app.raise_routed_auth(pending, window, cx);
            assert!(matches!(
                outcome,
                crate::ui::remote_workspace::SheetOutcome::Raised
            ));
        });
        settle(&mut vcx);

        let empty = card_height(&mut vcx);
        for typed in ["y", "ye", "yes"] {
            type_into_sheet(&app, &mut vcx, typed);
            assert_eq!(
                card_height(&mut vcx),
                empty,
                "typing {typed:?} must not grow or shrink the card"
            );
        }

        // Override is armed by exactly "yes", as before: half a word, Enter
        // and nothing is sent and the sheet stays up.
        type_into_sheet(&app, &mut vcx, "ye");
        app.update_in(&mut vcx, |app, window, cx| {
            app.submit_ssh_prompt(window, cx)
        });
        assert!(rx.try_recv().is_err(), "\"ye\" must not answer the prompt");
        assert!(app.read_with(&vcx, |app, _| app.ssh_prompt.model.is_some()));

        type_into_sheet(&app, &mut vcx, "yes");
        app.update_in(&mut vcx, |app, window, cx| {
            app.submit_ssh_prompt(window, cx)
        });
        assert_eq!(
            rx.try_recv().expect("\"yes\" answers the prompt"),
            AuthResponse::HostKeyDecision {
                accept: true,
                remember: true
            }
        );
    }
}

/// The key passphrase sheet in the real app tree: a path of any length stays
/// inside the card, and the secret is still filed under the daemon's own path.
#[cfg(test)]
mod passphrase_card_tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::session::Session;
    use gpui::{AppContext, TestAppContext, VisualTestContext};

    fn harness(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
            crate::ui::keymap::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| Tty59App::with_session(None, Some(Session::default()), window, cx));
            gpui_component::Root::new(app, window, cx)
        });
        cx.background_executor.run_until_parked();
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<Tty59App>()
                    .unwrap_or_else(|_| panic!("window root wraps a Tty59App"))
            })
            .unwrap();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (app, vcx)
    }

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        let _ = vcx.update(|window, cx| window.draw(cx));
        vcx.run_until_parked();
    }

    /// Raises the passphrase sheet for `key_path`. The returned pair keeps the
    /// routed attempt waiting: dropping it abandons the sheet.
    fn raise(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        key_path: &str,
    ) -> Box<dyn std::any::Any> {
        let (pending, rx, asker) = crate::ui::remote_connect::PendingAuth::for_test(
            tty59_core::host::HostId(0x15),
            AuthPromptKind::KeyPassphrase {
                key_path: key_path.into(),
                comment: String::new(),
                rejected: false,
            },
        );
        app.update_in(vcx, |app, window, cx| {
            let outcome = app.raise_routed_auth(pending, window, cx);
            assert!(matches!(
                outcome,
                crate::ui::remote_workspace::SheetOutcome::Raised
            ));
        });
        settle(vcx);
        Box::new((rx, asker))
    }

    /// (path line height, path line left, path line right, card left, card
    /// right), in logical pixels.
    fn measure(vcx: &mut VisualTestContext) -> (f32, f32, f32, f32, f32) {
        let path = vcx
            .debug_bounds("ssh-key-path")
            .expect("the passphrase sheet draws the key's path under the field");
        let card = vcx
            .debug_bounds("ssh-prompt-card")
            .expect("the passphrase sheet is drawn by the app's render");
        (
            f32::from(path.size.height),
            f32::from(path.left()),
            f32::from(path.right()),
            f32::from(card.left()),
            f32::from(card.right()),
        )
    }

    /// A bounding box cannot tell text that spills out of it from text that
    /// fits, so the rule is checked from both sides: the line is inside the
    /// card, *and* a path too long for one line takes more than one line
    /// instead of running on past the edge.
    #[gpui::test]
    fn a_long_key_path_wraps_inside_the_card_and_the_secret_keeps_the_real_path(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx) = harness(cx);
        let short = "/home/u/.ssh/id_deploy";
        let unbroken = format!("/srv/{}/id_deploy", "d".repeat(300));
        let segmented = format!("/home/u/{}id_deploy", "keys-and-more/".repeat(40));

        let _waiting = raise(&app, &mut vcx, short);
        let (one_line, ..) = measure(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.cancel_ssh_prompt(window, cx)
        });
        settle(&mut vcx);

        for path in [unbroken.as_str(), segmented.as_str()] {
            let _waiting = raise(&app, &mut vcx, path);
            let (height, left, right, card_left, card_right) = measure(&mut vcx);
            assert!(
                left >= card_left - 0.5 && right <= card_right + 0.5,
                "the path line ({left}..{right}) must stay inside the card ({card_left}..{card_right})"
            );
            assert!(
                height >= one_line * 1.5,
                "a {}-char path must wrap onto more lines than one ({height} vs {one_line}), \
                 not run past the card",
                path.len()
            );
            // The daemon's string is what the keychain is keyed by and what
            // goes back; the display strings never replace it.
            app.read_with(&vcx, |app, _| match &app.ssh_prompt.model {
                Some(PromptModel::KeyPassphrase {
                    key_path, display, ..
                }) => {
                    assert_eq!(key_path, path);
                    assert_eq!(&*display.path, path);
                }
                other => panic!("a passphrase sheet is up, got {other:?}"),
            });
            app.update_in(&mut vcx, |app, window, cx| {
                app.cancel_ssh_prompt(window, cx)
            });
            settle(&mut vcx);
        }
    }

    /// The remember toggle is the toolkit checkbox, reached the way a user
    /// reaches it: a click on its row toggles `remember`, and a second click
    /// toggles it back. The box's own size and border are not observable from
    /// here (the toolkit draws them inside); those are native-capture gates.
    #[gpui::test]
    fn clicking_the_save_to_keychain_row_toggles_remember(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        let _waiting = raise(&app, &mut vcx, "/home/u/.ssh/id_deploy");
        assert!(!app.read_with(&vcx, |app, _| app.ssh_prompt.remember));

        for expected in [true, false] {
            let row = vcx
                .debug_bounds("ssh-remember-row")
                .expect("the passphrase sheet draws the remember row");
            // Near the leading edge, where the box is, not the middle of the
            // label.
            let at = gpui::point(row.left() + gpui::px(8.), row.center().y);
            vcx.simulate_click(at, gpui::Modifiers::none());
            settle(&mut vcx);
            assert_eq!(
                app.read_with(&vcx, |app, _| app.ssh_prompt.remember),
                expected,
                "a click on the checkbox must flip remember to {expected}"
            );
        }
    }
}

/// The SSH sheet is a modal: real Tab / Shift+Tab stay inside it and reach
/// every enabled control, Escape from anywhere inside aborts it, and a
/// host-key sheet opens on its safe answer. Every key here goes through the
/// window as a platform event (a down and an up where a press is meant), in
/// the real app tree under `Root`, against the routed-auth channel the
/// backend answers on.
///
/// No key is ever pressed while focus is outside the sheet: a press that
/// would leave it is reported first, so a failing run cannot click anything
/// behind the scrim.
#[cfg(test)]
mod modal_keyboard_tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::daemon::protocol::KiPrompt;
    use gpui::{
        AppContext, KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext,
    };
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, TryRecvError};
    use tty59_core::host::HostId;

    type Rx = Receiver<AuthResponse>;

    const SENTINEL: &str = "sentinel-not-a-secret";

    fn decision(accept: bool) -> AuthResponse {
        AuthResponse::HostKeyDecision {
            accept,
            remember: accept,
        }
    }

    /// The prompt is still waiting: nothing has been sent and the channel is
    /// open, so the sheet's owner has not answered or dropped it.
    fn assert_pending(rx: &Rx, what: &str) {
        assert_eq!(
            rx.try_recv(),
            Err(TryRecvError::Empty),
            "{what}: the prompt must still be waiting, with nothing sent"
        );
    }

    /// After the one answer was taken: nothing further was sent. The sender
    /// goes with the answer, so a closed channel counts as nothing further.
    fn assert_no_more(rx: &Rx, what: &str) {
        assert!(
            matches!(
                rx.try_recv(),
                Err(TryRecvError::Empty | TryRecvError::Disconnected)
            ),
            "{what}: a second message was sent after the answer"
        );
    }

    fn password() -> AuthPromptKind {
        AuthPromptKind::Password {
            user: "me".into(),
            host: "example".into(),
        }
    }

    fn passphrase() -> AuthPromptKind {
        AuthPromptKind::KeyPassphrase {
            key_path: "~/.ssh/id_test".into(),
            comment: String::new(),
            rejected: false,
        }
    }

    fn ki(prompts: usize) -> AuthPromptKind {
        AuthPromptKind::KeyboardInteractive {
            name: "2FA".into(),
            instructions: String::new(),
            prompts: (0..prompts)
                .map(|i| KiPrompt {
                    text: format!("Code {i}:"),
                    echo: true,
                })
                .collect(),
            stored_rejected: false,
        }
    }

    fn unknown_host() -> AuthPromptKind {
        AuthPromptKind::HostKeyUnknown {
            host: "example".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint_sha256: "SHA256:zzz".into(),
            previously_known_as: None,
        }
    }

    fn changed_host() -> AuthPromptKind {
        AuthPromptKind::HostKeyChanged {
            host: "example".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint_sha256: "SHA256:new".into(),
            old_fingerprint_sha256: "SHA256:old".into(),
        }
    }

    /// Name, prompt, how many controls are eligible (the input or inputs,
    /// Remember where the sheet has one, Cancel and the submit button), and
    /// whether the sheet has a Remember checkbox.
    fn credential_cases() -> Vec<(&'static str, AuthPromptKind, usize, bool)> {
        vec![
            ("password", password(), 4, true),
            ("passphrase", passphrase(), 4, true),
            ("keyboard-interactive, 1 prompt", ki(1), 3, false),
            ("keyboard-interactive, 2 prompts", ki(2), 4, false),
        ]
    }

    /// What the changed-key sheet has eligible for each typed answer: the
    /// input and Abort, plus Override once the text is exactly what arms it.
    const ARMING: [(&str, usize); 5] = [("", 2), ("ye", 2), ("yes", 3), (" Yes ", 3), ("yes!", 2)];

    struct Rig {
        app: Entity<Tty59App>,
        vcx: VisualTestContext,
        next_host: u64,
        /// Somebody has to stay waiting on each raised sheet, or it is
        /// retracted as abandoned: the waiting counts live as long as the rig.
        askers: Vec<Arc<()>>,
    }

    impl Rig {
        fn new(cx: &mut TestAppContext) -> Self {
            crate::core::config::pin_test_config_dir();
            cx.executor().allow_parking();
            cx.update(|cx| {
                gpui_component::init(cx);
                cx.set_global(Config::default());
                crate::ui::keymap::init(cx);
            });
            let window = cx.add_window(|window, cx| {
                let app =
                    cx.new(|cx| Tty59App::with_session(None, Some(Session::default()), window, cx));
                gpui_component::Root::new(app, window, cx)
            });
            window
                .update(cx, |_, window, _| window.activate_window())
                .unwrap();
            cx.background_executor.run_until_parked();
            let app = window
                .update(cx, |root, _, _| {
                    root.view()
                        .clone()
                        .downcast::<Tty59App>()
                        .unwrap_or_else(|_| panic!("window root wraps a Tty59App"))
                })
                .unwrap();
            let vcx = VisualTestContext::from_window(window.into(), cx);
            Rig {
                app,
                vcx,
                next_host: 0x59d0,
                askers: Vec::new(),
            }
        }

        fn settle(&mut self) {
            self.vcx.run_until_parked();
            let _ = self.vcx.update(|window, cx| window.draw(cx));
            self.vcx.run_until_parked();
        }

        /// Raises a routed sheet and waits until it is drawn. The returned
        /// receiver is where the backend would read the answer.
        fn raise(&mut self, prompt: AuthPromptKind) -> Rx {
            self.next_host += 1;
            let (pending, rx, asker) =
                crate::ui::remote_connect::PendingAuth::for_test(HostId(self.next_host), prompt);
            self.askers.push(asker);
            self.app.update_in(&mut self.vcx, |app, window, cx| {
                let outcome = app.raise_routed_auth(pending, window, cx);
                assert!(
                    matches!(outcome, crate::ui::remote_workspace::SheetOutcome::Raised),
                    "setup: the sheet must be raised"
                );
            });
            self.settle();
            assert!(
                self.vcx.debug_bounds("ssh-prompt-card").is_some(),
                "setup: the raised sheet must be mounted and drawn"
            );
            rx
        }

        fn type_into(&mut self, ix: usize, text: &str) {
            let text = text.to_string();
            self.app.update_in(&mut self.vcx, |app, window, cx| {
                let input = app.ssh_prompt.inputs[ix].clone();
                input.update(cx, |s, cx| s.set_value(text, window, cx));
            });
            self.settle();
        }

        fn type_into_all(&mut self, text: &str) {
            let n = self
                .app
                .read_with(&self.vcx, |app, _| app.ssh_prompt.inputs.len());
            for ix in 0..n {
                self.type_into(ix, text);
            }
        }

        fn toggle_remember(&mut self) {
            self.app.update_in(&mut self.vcx, |app, _, cx| {
                app.toggle_ssh_remember(cx);
            });
            self.settle();
        }

        fn model_is_up(&mut self) -> bool {
            self.app
                .read_with(&self.vcx, |app, _| app.ssh_prompt.model.is_some())
        }

        fn card(&mut self) -> gpui::FocusHandle {
            self.app
                .read_with(&self.vcx, |app, _| app.ssh_prompt.focus_handle.clone())
        }

        fn focused(&mut self) -> Option<gpui::FocusHandle> {
            self.vcx.update(|window, cx| window.focused(cx))
        }

        fn in_card(&mut self) -> bool {
            let card = self.card();
            self.vcx
                .update(|window, cx| card.contains_focused(window, cx))
        }

        fn assert_in_card(&mut self, what: &str) {
            let at = self.focused();
            assert!(
                self.in_card(),
                "{what}: focus must be inside the SSH sheet, it is on {at:?}"
            );
        }

        fn key_down(&mut self, key: &str, held: bool) {
            self.assert_in_card(&format!("before {key} (down, held={held})"));
            self.vcx.simulate_event(KeyDownEvent {
                keystroke: Keystroke::parse(key).expect("a key"),
                is_held: held,
                prefer_character_input: false,
            });
        }

        /// The release is allowed to land anywhere: the press may have
        /// closed the sheet.
        fn key_up(&mut self, key: &str) {
            self.vcx.simulate_event(KeyUpEvent {
                keystroke: Keystroke::parse(key).expect("a key"),
            });
        }

        /// One whole press: down, a frame, up, a frame.
        fn press(&mut self, key: &str) {
            self.settle();
            self.key_down(key, false);
            self.settle();
            self.key_up(key);
            self.settle();
        }

        /// Presses `key` `n` times and returns where focus was after each,
        /// failing at the first press that leaves the sheet or lands on its
        /// container.
        fn walk(&mut self, key: &str, n: usize, what: &str) -> Vec<gpui::FocusHandle> {
            let card = self.card();
            let mut seen = Vec::new();
            for step in 1..=n {
                self.press(key);
                let at = self.focused();
                assert!(
                    self.in_card(),
                    "{what}: {key} #{step} of {n} left the SSH sheet (focus is on {at:?}); \
                     a modal must keep {key} inside itself"
                );
                assert!(
                    at.as_ref() != Some(&card),
                    "{what}: {key} #{step} of {n} stopped on the sheet's own container; \
                     the trap container must not be a tab stop"
                );
                seen.push(at.expect("focus is inside the sheet"));
            }
            seen
        }

        fn go_to(&mut self, key: &str, n: usize, what: &str) {
            let _ = self.walk(key, n, what);
        }

        /// `eligible` presses of `key` visit `eligible` different controls and
        /// come back to the one they started on.
        fn assert_cycle(&mut self, key: &str, eligible: usize, what: &str) {
            self.assert_in_card(&format!("{what}: the sheet must hold focus when up"));
            let start = self.focused().expect("focus is inside the sheet");
            let seen = self.walk(key, eligible, what);
            let mut distinct: Vec<gpui::FocusHandle> = Vec::new();
            for handle in &seen {
                if !distinct.contains(handle) {
                    distinct.push(handle.clone());
                }
            }
            assert_eq!(
                distinct.len(),
                eligible,
                "{what}: {eligible} presses of {key} must reach {eligible} different controls"
            );
            assert!(
                seen.last() == Some(&start),
                "{what}: a full {key} cycle must come back to the control it started on"
            );
        }

        /// Escape, and everything it must do: the one cancel answer, nothing
        /// more, the sheet gone, and focus handed back to the app.
        fn escape_expecting(&mut self, rx: &Rx, expected: AuthResponse, what: &str) {
            self.press("escape");
            assert_eq!(
                rx.try_recv().ok(),
                Some(expected),
                "{what}: Escape must answer with the sheet's cancel answer"
            );
            assert_no_more(rx, &format!("{what}: Escape"));
            assert!(
                !self.model_is_up(),
                "{what}: Escape must take the sheet down"
            );
            let card = self.card();
            let (inside, any) = self.vcx.update(|window, cx| {
                (
                    card.contains_focused(window, cx),
                    window.focused(cx).is_some(),
                )
            });
            assert!(!inside, "{what}: focus is still on the closed sheet");
            assert!(any, "{what}: Escape must hand focus back to the app");
        }

        fn answer_of(&mut self, rx: &Rx) -> Option<AuthResponse> {
            self.settle();
            rx.try_recv().ok()
        }

        /// A real left press and release at the middle of the button the
        /// drawn frame reports under `dialog-button-<id>`.
        fn click(&mut self, selector: &'static str, what: &str) {
            self.settle();
            let bounds = self
                .vcx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: the sheet drew no button {selector:?}"));
            self.vcx
                .simulate_click(bounds.center(), gpui::Modifiers::none());
            self.settle();
        }
    }

    /// Positive controls. If these fail the other tests prove nothing: the
    /// sheets mount, and the backend answers the way it always did through
    /// the existing entry points.
    #[gpui::test]
    fn the_sheets_mount_and_the_backend_still_answers(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);

        let rx = rig.raise(password());
        rig.type_into(0, SENTINEL);
        rig.app
            .update_in(&mut rig.vcx, |app, w, cx| app.submit_ssh_prompt(w, cx));
        assert_eq!(
            rx.try_recv().ok(),
            Some(AuthResponse::Secret(SENTINEL.into())),
            "control: submitting a password sheet sends the typed secret"
        );

        let rx = rig.raise(unknown_host());
        rig.app
            .update_in(&mut rig.vcx, |app, w, cx| app.trust_ssh_host_key(w, cx));
        assert_eq!(rx.try_recv().ok(), Some(decision(true)), "control: trust");

        let rx = rig.raise(unknown_host());
        rig.app
            .update_in(&mut rig.vcx, |app, w, cx| app.cancel_ssh_prompt(w, cx));
        assert_eq!(rx.try_recv().ok(), Some(decision(false)), "control: abort");

        let rx = rig.raise(changed_host());
        rig.type_into(0, "ye");
        rig.app
            .update_in(&mut rig.vcx, |app, w, cx| app.submit_ssh_prompt(w, cx));
        assert_pending(&rx, "control: \"ye\" on a changed-key sheet");
        assert!(
            rig.model_is_up(),
            "control: \"ye\" must leave the changed-key sheet up"
        );
        rig.type_into(0, "yes");
        rig.app
            .update_in(&mut rig.vcx, |app, w, cx| app.submit_ssh_prompt(w, cx));
        assert_eq!(
            rx.try_recv().ok(),
            Some(decision(true)),
            "control: \"yes\" accepts a changed key"
        );

        for (what, prompt, _, _) in credential_cases() {
            let rx = rig.raise(prompt);
            rig.app
                .update_in(&mut rig.vcx, |app, w, cx| app.cancel_ssh_prompt(w, cx));
            assert_eq!(
                rx.try_recv().ok(),
                Some(AuthResponse::Cancelled),
                "control: cancelling the {what} sheet"
            );
        }
    }

    /// Tab and Shift+Tab cycle inside a credential sheet over exactly its
    /// eligible controls. Observed before: Tab 3 left the sheet for the
    /// sidebar and Tab 4 for the workspace switcher behind it.
    #[gpui::test]
    fn credential_sheets_trap_tab_and_reach_every_control(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        for (what, prompt, eligible, _) in credential_cases() {
            let rx = rig.raise(prompt);
            rig.assert_cycle("tab", eligible, &format!("{what}, forward"));
            rig.assert_cycle("shift-tab", eligible, &format!("{what}, reverse"));
            rig.escape_expecting(&rx, AuthResponse::Cancelled, what);
        }
    }

    #[gpui::test]
    fn host_key_sheets_trap_tab_and_reach_every_control(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);

        let rx = rig.raise(unknown_host());
        rig.assert_cycle("tab", 2, "unknown host, forward");
        rig.assert_cycle("shift-tab", 2, "unknown host, reverse");
        rig.escape_expecting(&rx, decision(false), "unknown host");

        for (typed, eligible) in ARMING {
            let what = format!("changed key typed {typed:?}");
            let rx = rig.raise(changed_host());
            if !typed.is_empty() {
                rig.type_into(0, typed);
            }
            rig.assert_cycle("tab", eligible, &format!("{what}, forward"));
            rig.assert_cycle("shift-tab", eligible, &format!("{what}, reverse"));
            rig.escape_expecting(&rx, decision(false), &what);
        }
    }

    /// Escape from the input, from Remember and from each button answers
    /// Cancelled once, whatever was typed or ticked, and closes the sheet.
    #[gpui::test]
    fn escape_cancels_credential_sheets_from_every_position(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        for (what, prompt, eligible, remember) in credential_cases() {
            for position in 0..eligible {
                let label = format!("{what}, position {position}");
                let rx = rig.raise(prompt.clone());
                rig.type_into_all(SENTINEL);
                if remember {
                    rig.toggle_remember();
                }
                rig.go_to("tab", position, &label);
                rig.escape_expecting(&rx, AuthResponse::Cancelled, &label);
            }
        }
    }

    #[gpui::test]
    fn escape_aborts_host_key_sheets_from_every_position(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let cases = [
            ("unknown host", unknown_host(), "", 2),
            ("changed key", changed_host(), "", 2),
            ("changed key, armed", changed_host(), "yes", 3),
        ];
        for (what, prompt, typed, eligible) in cases {
            for position in 0..eligible {
                let label = format!("{what}, position {position}");
                let rx = rig.raise(prompt.clone());
                if !typed.is_empty() {
                    rig.type_into(0, typed);
                }
                rig.go_to("tab", position, &label);
                rig.escape_expecting(&rx, decision(false), &label);
            }
        }
    }

    /// Both host-key sheets open on Abort, so the Enter or Space that is
    /// already down when the sheet appears can only ever say no.
    #[gpui::test]
    fn host_key_sheets_open_on_abort_and_a_plain_press_never_trusts(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        for (what, prompt) in [
            ("unknown host", unknown_host()),
            ("changed key", changed_host()),
        ] {
            for key in ["enter", "space"] {
                let rx = rig.raise(prompt.clone());
                rig.assert_in_card(&format!("{what}: a fresh sheet must hold focus"));
                rig.press(key);
                assert_eq!(
                    rx.try_recv().ok(),
                    Some(decision(false)),
                    "{what}: {key} on a freshly opened sheet must take the safe Abort answer"
                );
                assert_no_more(&rx, &format!("{what}: {key}"));
                assert!(!rig.model_is_up(), "{what}: {key} must close the sheet");
            }
        }
    }

    /// Trust is reached only on purpose, by Tab or the arrows, and the arrows
    /// stop at the ends of the row.
    #[gpui::test]
    fn unknown_host_reaches_trust_only_by_deliberate_navigation(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let cases: [(&str, &[&str], &str, bool); 8] = [
            ("tab, enter", &["tab"], "enter", true),
            ("shift-tab wraps, enter", &["shift-tab"], "enter", true),
            ("right, enter", &["right"], "enter", true),
            ("right, space", &["right"], "space", true),
            ("right then left, enter", &["right", "left"], "enter", false),
            ("left stops at Abort", &["left"], "enter", false),
            ("right stops at Trust", &["right", "right"], "enter", true),
            ("tab twice wraps to Abort", &["tab", "tab"], "enter", false),
        ];
        for (what, moves, activate, trusts) in cases {
            let rx = rig.raise(unknown_host());
            for key in moves {
                rig.press(key);
                rig.assert_in_card(&format!("{what}: after {key}"));
            }
            rig.press(activate);
            assert_eq!(
                rx.try_recv().ok(),
                Some(decision(trusts)),
                "{what}: wrong answer"
            );
            assert_no_more(&rx, what);
            assert!(!rig.model_is_up(), "{what}: the sheet must close");
        }
    }

    /// A held Enter is one activation, and repeats with no fresh press are
    /// none at all.
    #[gpui::test]
    fn a_held_enter_activates_the_focused_button_once(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);

        let rx = rig.raise(unknown_host());
        rig.press("tab");
        rig.assert_in_card("Trust focused");
        rig.settle();
        rig.key_down("enter", false);
        rig.settle();
        for _ in 0..3 {
            // Past the answer the repeats would be the app's, not the
            // sheet's: stop feeding them once it is gone.
            if !rig.model_is_up() {
                break;
            }
            rig.key_down("enter", true);
            rig.settle();
        }
        rig.key_up("enter");
        rig.settle();
        assert_eq!(
            rx.try_recv().ok(),
            Some(decision(true)),
            "a complete Enter press on Trust must trust"
        );
        assert_no_more(&rx, "held repeats");

        let rx = rig.raise(unknown_host());
        rig.press("tab");
        for _ in 0..3 {
            // A repeat that did activate would close the sheet: stop there
            // so the assertion below names it.
            if !rig.model_is_up() {
                break;
            }
            rig.key_down("enter", true);
            rig.settle();
        }
        assert_pending(&rx, "repeats with no fresh press");
        assert!(
            rig.model_is_up(),
            "repeats with no fresh press must leave the sheet up"
        );
        rig.escape_expecting(&rx, decision(false), "after held repeats");
    }

    /// Override is armed by exactly the text that armed it before, and no
    /// key can reach it, or accept for it, until then.
    #[gpui::test]
    fn changed_key_override_follows_the_arming_policy(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        for (typed, eligible) in ARMING {
            let armed = eligible == 3;
            for key in ["enter", "space"] {
                let mut outcomes = Vec::new();
                for position in 0..eligible {
                    let label = format!("typed {typed:?}, {key}, position {position}");
                    let rx = rig.raise(changed_host());
                    if !typed.is_empty() {
                        rig.type_into(0, typed);
                    }
                    rig.go_to("tab", position, &label);
                    rig.press(key);
                    outcomes.push(rig.answer_of(&rx));
                    if rig.model_is_up() {
                        rig.escape_expecting(&rx, decision(false), &label);
                    }
                }
                let accepts = outcomes
                    .iter()
                    .filter(|o| **o == Some(decision(true)))
                    .count();
                let aborts = outcomes
                    .iter()
                    .filter(|o| **o == Some(decision(false)))
                    .count();
                // Enter in the input is the old submit path and so accepts
                // once armed; Space there types nothing. Override accepts
                // with either key. Abort always aborts.
                let expected = match (armed, key) {
                    (false, _) => 0,
                    (true, "enter") => 2,
                    (true, _) => 1,
                };
                assert_eq!(
                    accepts, expected,
                    "typed {typed:?}, {key}: {outcomes:?} accepted {accepts} times, expected {expected}"
                );
                assert_eq!(
                    aborts, 1,
                    "typed {typed:?}, {key}: exactly Abort aborts: {outcomes:?}"
                );
            }
        }
    }

    /// The Enter that submitted one sheet is released after the next prompt
    /// is already up. Whatever the buttons do on a press, that release is
    /// not a press on the new sheet. Green at the baseline, and must stay so.
    #[gpui::test]
    fn the_release_of_the_press_that_answered_a_sheet_cannot_answer_the_next(
        cx: &mut TestAppContext,
    ) {
        let mut rig = Rig::new(cx);
        let first = rig.raise(password());
        rig.type_into(0, SENTINEL);
        rig.settle();
        rig.key_down("enter", false);
        rig.settle();
        assert_eq!(
            first.try_recv().ok(),
            Some(AuthResponse::Secret(SENTINEL.into())),
            "setup: Enter in the input submits the password sheet"
        );
        assert!(!rig.model_is_up(), "setup: the first sheet is gone");

        let second = rig.raise(unknown_host());
        rig.key_up("enter");
        rig.settle();
        assert_pending(&second, "the release of the previous sheet's Enter");
        assert!(
            rig.model_is_up(),
            "the release of the previous sheet's Enter must not close the new sheet"
        );
        // Taken down through the backend entry point: Escape on this sheet is
        // covered by the tests of the keyboard path itself.
        rig.app
            .update_in(&mut rig.vcx, |app, w, cx| app.cancel_ssh_prompt(w, cx));
        assert_eq!(
            second.try_recv().ok(),
            Some(decision(false)),
            "cleanup: cancelling the new sheet rejects the host key"
        );
        assert_no_more(&second, "cleanup");
    }

    /// A key held through Escape repeats into whatever is raised next.
    #[gpui::test]
    fn a_held_escape_cannot_abort_the_next_prompt(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let first = rig.raise(password());
        rig.settle();
        rig.key_down("escape", false);
        rig.settle();
        assert_eq!(
            first.try_recv().ok(),
            Some(AuthResponse::Cancelled),
            "setup: Escape cancels the first sheet"
        );

        let second = rig.raise(password());
        rig.assert_in_card("the second sheet");
        for _ in 0..2 {
            // A repeat that did abort the sheet takes it down: stop there so
            // the assertion below names it.
            if !rig.model_is_up() {
                break;
            }
            rig.key_down("escape", true);
            rig.settle();
        }
        assert_pending(&second, "a held Escape repeating into the next prompt");
        assert!(
            rig.model_is_up(),
            "a held Escape repeating into the next prompt must not abort it"
        );
        rig.key_up("escape");
        rig.escape_expecting(&second, AuthResponse::Cancelled, "a fresh Escape press");
    }

    /// Dismissal leaves nothing behind: the next sheet opens on its own
    /// default, not on the control the last one was left on.
    #[gpui::test]
    fn a_reopened_sheet_starts_from_its_own_defaults(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);

        let rx = rig.raise(unknown_host());
        rig.press("tab");
        rig.escape_expecting(&rx, decision(false), "the first host-key sheet");
        let rx = rig.raise(unknown_host());
        rig.assert_in_card("the reopened host-key sheet");
        rig.press("enter");
        assert_eq!(
            rx.try_recv().ok(),
            Some(decision(false)),
            "a reopened host-key sheet must open on Abort, not where the last one ended"
        );

        let rx = rig.raise(passphrase());
        rig.toggle_remember();
        rig.type_into(0, SENTINEL);
        rig.go_to("tab", 2, "the first passphrase sheet");
        rig.escape_expecting(&rx, AuthResponse::Cancelled, "the first passphrase sheet");

        let rx = rig.raise(password());
        let (remember, typed) = rig.app.read_with(&rig.vcx, |app, cx| {
            (
                app.ssh_prompt.remember,
                app.ssh_prompt.inputs[0].read(cx).value().to_string(),
            )
        });
        assert!(!remember, "Remember must not carry over to the next sheet");
        assert!(typed.is_empty(), "typed text must not carry over");
        rig.assert_cycle("tab", 4, "the reopened password sheet");
        rig.escape_expecting(&rx, AuthResponse::Cancelled, "the reopened password sheet");
    }

    /// Reviewer-added after the implementation (not a pre-fix RED). The arrow
    /// keys on the changed-key sheet move only between enabled buttons: an
    /// unarmed Override is never a target, an armed one is, and the row stops
    /// at both ends.
    #[gpui::test]
    fn changed_key_arrows_skip_a_disabled_override_and_clamp_when_it_is_armed(
        cx: &mut TestAppContext,
    ) {
        let mut rig = Rig::new(cx);

        let unarmed: [&[&str]; 4] = [
            &["left"],
            &["right"],
            &["right", "left"],
            &["left", "right", "right"],
        ];
        for typed in ["", "ye"] {
            for moves in unarmed {
                let what = format!("typed {typed:?}, arrows {moves:?}");
                let rx = rig.raise(changed_host());
                if !typed.is_empty() {
                    rig.type_into(0, typed);
                }
                for key in moves {
                    rig.press(key);
                    rig.assert_in_card(&format!("{what}: after {key}"));
                }
                rig.press("enter");
                assert_eq!(
                    rx.try_recv().ok(),
                    Some(decision(false)),
                    "{what}: with Override disabled focus must have stayed on Abort"
                );
                assert_no_more(&rx, &what);
            }
        }

        let armed: [(&[&str], bool); 6] = [
            (&["left"], true),
            (&["right"], false),
            (&["left", "left"], true),
            (&["left", "right"], false),
            (&["right", "left"], true),
            (&["left", "right", "right"], false),
        ];
        for (moves, accepts) in armed {
            let what = format!("typed \"yes\", arrows {moves:?}");
            let rx = rig.raise(changed_host());
            rig.type_into(0, "yes");
            for key in moves {
                rig.press(key);
                rig.assert_in_card(&format!("{what}: after {key}"));
            }
            rig.press("enter");
            assert_eq!(
                rx.try_recv().ok(),
                Some(decision(accepts)),
                "{what}: Enter must act on the clamped arrow position"
            );
            assert_no_more(&rx, &what);
        }
    }

    /// Reviewer-added after the implementation (not a pre-fix RED). A real
    /// left press and release on each footer button takes the same callback
    /// as before, once; a disabled Override sends nothing, an armed one
    /// accepts.
    #[gpui::test]
    fn a_pointer_click_on_a_footer_button_runs_its_action_once(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);

        let cases = [
            (
                "unknown host Abort",
                unknown_host(),
                "dialog-button-ssh-hk-abort",
                decision(false),
            ),
            (
                "unknown host Trust",
                unknown_host(),
                "dialog-button-ssh-hk-trust",
                decision(true),
            ),
            (
                "changed key Abort",
                changed_host(),
                "dialog-button-ssh-hkc-abort",
                decision(false),
            ),
            (
                "password Cancel",
                password(),
                "dialog-button-ssh-cancel",
                AuthResponse::Cancelled,
            ),
            (
                "password Submit",
                password(),
                "dialog-button-ssh-submit",
                AuthResponse::Secret(SENTINEL.into()),
            ),
        ];
        for (what, prompt, selector, expected) in cases {
            let rx = rig.raise(prompt);
            rig.type_into_all(SENTINEL);
            rig.click(selector, what);
            assert_eq!(
                rx.try_recv().ok(),
                Some(expected),
                "{what}: a click must run the button's own action"
            );
            assert_no_more(&rx, what);
            assert!(!rig.model_is_up(), "{what}: the answered sheet must close");
        }

        for typed in ["", "ye"] {
            let what = format!("changed key typed {typed:?}, Override");
            let rx = rig.raise(changed_host());
            if !typed.is_empty() {
                rig.type_into(0, typed);
            }
            rig.click("dialog-button-ssh-hkc-override", &what);
            assert_pending(&rx, &what);
            assert!(
                rig.model_is_up(),
                "{what}: a disabled Override must leave the sheet up"
            );
            rig.app
                .update_in(&mut rig.vcx, |app, w, cx| app.cancel_ssh_prompt(w, cx));
            assert_eq!(rx.try_recv().ok(), Some(decision(false)), "cleanup: {what}");
        }

        let rx = rig.raise(changed_host());
        rig.type_into(0, "yes");
        rig.click("dialog-button-ssh-hkc-override", "armed Override");
        assert_eq!(
            rx.try_recv().ok(),
            Some(decision(true)),
            "armed Override: a click must accept the changed key"
        );
        assert_no_more(&rx, "armed Override");
        assert!(!rig.model_is_up(), "armed Override: the sheet must close");
    }
}
