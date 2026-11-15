//! The source control panel body: four groups of file rows over the working
//! tree status, in the order git itself talks about them.
//!
//! The panel is rendered from `WorkingTreeStatus`, which knows the difference
//! between the index and the working tree. That is the whole reason the old
//! flat list had to go: it ran one `git diff HEAD`, so it could not tell a
//! staged change from an unstaged one and showed every row the letter `M`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Focusable as _, SharedString, Window, div, prelude::*, px, relative, rems,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};

use tty59_core::core::git::diff::MAX_RENDERED_FILES;
use tty59_core::core::git::ops::GitOp;
use tty59_core::core::git::status::{
    ChangeCode, DecoStatus, FailureFact, HeadState, RepoOperation, RepoPath, StatusEntry,
    WorkingTreeStatus,
};

use crate::terminal::git_data::{StatusRead, index_of, status_of, status_read_of};
use crate::terminal::git_diff::DiffSource;
use crate::ui::app::{CONTENT_INSET, TILE_GLYPH_XS, TILE_SIZE_XS, Tty59App};
use crate::ui::host_ops::{HostId, SharedHost};
use crate::ui::i18n::{L10nKey, t, t_fmt, t_plural};
use crate::ui::right_panel::{
    BADGE_W, DISCLOSURE_CHEVRON, META, META_MONO, PANEL_ACTION_GAP, PanelAction, ROW_INSET,
    SEARCH_H, TEXT, TEXT_INSET, action_strip, action_strip_reserve, git_badge, info_chip,
};
use crate::ui::rounding::{HAIRLINE, ROW_RADIUS, RoundedCorners as _, segment_corners};
use crate::ui::scm::ScmIntent;
use crate::ui::scm::path::split_display_path;
use crate::ui::scm::rows_window::RowsWindow;
use crate::ui::scm::state::{RepoKey, ScmGroup};
use crate::ui::scm::status::{status_color, status_glyph};
use crate::ui::scm::tree::{TreeRow, normalize_query, path_matches, tree_rows};
use crate::ui::text_elision::{ElidedLabel, Mode as ElideMode};

/// Shared compact pitch for working-tree and commit-detail file rows.
///
/// A height, not a floor to pad past: the rows used to add 3px above and
/// below a 23px line box, so a file row stood 29 tall beside the 26px
/// directory rows of the same tree and the Files tab's 26px rows.
pub(super) const ROW_H: f32 = 26.;

/// The group header's disclosure chevron: the panel's one disclosure mark in a
/// box of its own size, flush with the status letters' column (`git_badge`'s
/// `BADGE_W` cell) and no wider, so the arrow stands over the letters — its
/// centre within a pixel of theirs — and the label after it lands
/// `10 + 6` = 16px past the text column.
pub(crate) const GROUP_CHEVRON: f32 = DISCLOSURE_CHEVRON;
const _: () = assert!(GROUP_CHEVRON <= BADGE_W && BADGE_W - GROUP_CHEVRON <= 2.);

/// One level of the tree view, the file tree's step: the Files panel and this
/// one draw the same directories, and the same depth should read the same.
const TREE_INDENT: f32 = 14.;

/// The key context the message box installs, and the one `ScmCommit` is
/// bound inside. The two are the same string on purpose: a binding whose
/// context nothing attaches is a binding that never fires.
pub(crate) const COMMIT_KEY_CONTEXT: &str = "ScmCommit";

/// Past this many, the branch switcher scrolls instead of growing.
const BRANCHES_IN_MENU: usize = 12;

/// Untracked files past this many start folded. A fresh clone of a repository
/// with a stale `.gitignore` can put thousands of them in front of the three
/// changes the user came to look at.
const UNTRACKED_AUTO_COLLAPSE: usize = 20;

/// The message box rests at 56px — room for a subject and the start of a body
/// — and grows with content from there.
const MSG_LINE: f32 = 20.;
const MSG_PAD_X: f32 = 10.;
const MSG_PAD_Y: f32 = 8.;
const MSG_MIN_H: f32 = 56.;
/// The ceiling, as `MSG_ROWS_MAX` bare lines. It is a rail, not a border-box
/// sum: the wrapper's own hairline and padding are not in it, so at the very
/// top of the box's growth the last row is clipped rather than framed.
const MSG_MAX_H: f32 = MSG_ROWS_MAX as f32 * MSG_LINE;

/// One line of input at rest inside the 56px box, growing into the message.
/// The box's resting height is the wrapper's floor, not the row count: a box
/// that stood six lines tall before anything was typed would push the file
/// list — the thing the panel is for — off the bottom of the panel.
const MSG_ROWS: usize = 1;
const MSG_ROWS_MAX: usize = 6;

/// The commit control: one frame, the label button and the chevron inside it,
/// and the glyph in the chevron.
///
/// A joined split control, sized like the panel's other controls rather than
/// stretched across it — the row it sits in reads "N staged" on the left and
/// offers this on the right, so it only has to be as wide as its label.
///
/// The frame follows [`ROW_H`], the panel's row pitch, so the control sits on the
/// same line grid as everything above and below it; it follows that constant
/// rather than restating it, because the two moving apart is the only way this
/// can go wrong.
///
/// The chevron half is a square cell of the same interior, so the divider falls
/// where the eye expects it rather than a couple of pixels early.
///
/// `COMMIT_GLYPH` is the split button's caret: a menu, not a fold, so a step
/// over the panel's disclosure chevrons rather than one of them. Marks are px
/// on purpose. The panel's *type* is on the interface font scale and grows
/// with it; a caret is chrome, sized against the box it sits in.
const COMMIT_H: f32 = ROW_H;
const COMMIT_CHEVRON_W: f32 = COMMIT_H - 2.;
const COMMIT_GLYPH: f32 = 11.;
const COMMIT_RADIUS: gpui::Pixels = px(6.);

/// The block pinned above the file list — branch, message, commit — reads as
/// one unit: 8px under the tab row, 10px between its three parts and 14px
/// before the first group.
///
/// On macOS the block starts flush under the tab row, the way Info does: the
/// branch is text centred in a 28px row, so it already sits ~7px down, level
/// with the top edge of the Files tab's search well (which does get an 8px
/// step, see `render_right_panel`). Elsewhere the tab row is inside the body
/// and the 8 is this block's.
const PINNED_TOP: f32 = if cfg!(target_os = "macos") { 0. } else { 8. };
const PINNED_GAP: f32 = 10.;
const PINNED_BOTTOM: f32 = 14.;

/// The pinned block's side inset: 2px further in than the lists' `CONTENT_INSET`, so
/// the message box and the commit control read as a panel of their own rather
/// than as rows of the list. The branch row pads a further `ROW_INSET`, which
/// puts its glyph at 22 — under the first tab label.
const PINNED_INSET: f32 = 14.;

/// The branch row's trailing refresh/sync tile, and the mark in it.
const BRANCH_TILE: f32 = 26.;
const BRANCH_TILE_GLYPH: f32 = 12.;

/// Space between two change groups, and the height of a group's header.
const GROUP_GAP: f32 = 16.;
const GROUP_HEADER_H: f32 = 22.;
/// Group headers are band labels: a half step under `META`, medium weight,
/// muted — the file names under them are what the panel is for.
const GROUP_HEADING: f32 = 11.5 / 16.;
/// A file row's name keeps at least this much before the directory beside it
/// has given up all of its width.
const NAME_FLOOR: f32 = 40.;

/// How long to wait before asking git again about a directory that answered
/// with nothing.
///
/// `scm_refresh` is safe to call every frame — it de-duplicates in-flight
/// probes and skips fresh ones. What it cannot do is notice that a probe came
/// back empty: a repository we never got a status for stays stale forever, so
/// without this the panel would start a new `git status` on every frame.
const PROBE_RETRY: Duration = Duration::from_secs(2);

/// How long "there is no repository here" is believed for. Long enough that
/// sitting in `/tmp` costs nothing, short enough that `git init` in the pane
/// below shows up without touching anything.
const NOT_A_REPO_RETRY: Duration = Duration::from_secs(10);

/// Debug selectors for the Changes tab's recovery buttons.
pub(crate) const SCM_STATUS_RETRY: &str = "panel-scm-status-retry";
pub(crate) const SCM_STATUS_COPY: &str = "panel-scm-status-copy";
pub(crate) const SCM_INIT_REPO: &str = "panel-scm-init-repo";

/// The words for a failure fact, in the language now. Only here, on the UI
/// thread, when something is drawn or reported: the core and the probe's pool
/// thread carry facts and never look a locale up.
///
/// `Raw` is somebody else's text — git's, the host's, the OS's — and comes
/// back exactly as it was given, whatever it says. The generated kinds are
/// told apart by their variant, never by comparing text.
pub(crate) fn failure_text(fact: &FailureFact) -> String {
    match fact {
        FailureFact::Raw(text) => text.clone(),
        FailureFact::ExitStatus(code) => t_fmt(
            L10nKey::PanelScmGitExitStatus,
            &[("code", code.to_string().as_str())],
        ),
        FailureFact::Signal => t(L10nKey::PanelScmGitSignal).to_string(),
        FailureFact::UnusableRoot => t(L10nKey::PanelScmGitUnusableRoot).to_string(),
        FailureFact::Panicked(Some(detail)) if !detail.trim().is_empty() => t_fmt(
            L10nKey::PanelScmGitPanickedWithReason,
            &[("reason", detail.as_str())],
        ),
        FailureFact::Panicked(_) => t(L10nKey::PanelScmGitPanicked).to_string(),
    }
}

/// Where `dir` is a repository on `host`, if it is: `Ok(Some(root))` for a
/// repository, `Ok(None)` for git's own "not a git repository", and `Err` with
/// what happened for anything else — a host that could not run git, a
/// repository git could not read — which is not an answer about the directory.
fn toplevel(host: &dyn tty59_core::host::Host, dir: &Path) -> Result<Option<String>, FailureFact> {
    let out = host
        .git(
            dir,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        )
        .map_err(|e| FailureFact::Raw(e.to_string()))?;
    if out.success() {
        return out
            .stdout_trimmed()
            .lines()
            .next()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| Some(l.to_string()))
            .ok_or(FailureFact::UnusableRoot);
    }
    if out
        .stderr_trimmed()
        .to_ascii_lowercase()
        .contains("not a git repository")
    {
        return Ok(None);
    }
    Err(tty59_core::core::git::status::failure_fact(&out))
}

/// `git init` in `dir`, through the host so a remote pane's folder is made a
/// repository on its own machine.
///
/// Asked first whether `dir` already is one: a directory git can read as a
/// repository is never initialised again (no nested repository), and one git
/// could not answer about is reported, not initialised over. `Ok` carries the
/// new root as the host spells it, or `None` when git initialised but would not
/// then say where. `Err` is always a fact: git's first line, its exit status
/// when it said nothing, or the transport's own error text; it is worded only
/// by the caller on the UI thread (`failure_text`).
pub(crate) fn init_repo(
    host: &dyn tty59_core::host::Host,
    dir: &Path,
) -> Result<Option<String>, FailureFact> {
    if let Some(root) = toplevel(host, dir)? {
        return Ok(Some(root));
    }
    let out = host
        .git(dir, &["init", "--quiet"])
        .map_err(|e| FailureFact::Raw(e.to_string()))?;
    if !out.success() {
        return Err(tty59_core::core::git::status::failure_fact(&out));
    }
    toplevel(host, dir)
}

/// What the panel knows about the directory the active pane is sitting in.
pub(crate) enum RepoLookup {
    /// Nothing has answered yet — the tab's own probe is still out.
    Pending,
    NotARepo,
    Root(PathBuf),
}

impl Tty59App {
    pub(crate) fn render_panel_scm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.scm_watch_status(cx);
        // An explicit repository pick should outlive a pane switch inside the
        // tab it was made on, and not a jump to a different tab.
        if self.scm.override_tab != Some(self.active) {
            self.scm.repo_override = None;
            self.scm.override_tab = None;
        }

        let Some((host, cwd)) = self.scm_pane_target(window, cx) else {
            let title = self.panel_title(t(L10nKey::PanelScmTitle), None, None, window, cx);
            let body = self.panel_empty(
                t(L10nKey::PanelNoWorkingDirectory),
                Some(t(L10nKey::PanelNoWorkingDirectoryHint)),
                cx,
            );
            return self.scm_shell(title, body);
        };

        let root = match self.scm_repo_root(&host, &cwd, cx) {
            RepoLookup::Pending => {
                let title = self.panel_title(t(L10nKey::PanelScmTitle), None, None, window, cx);
                let body = self.panel_empty(t(L10nKey::PanelLoading), None, cx);
                return self.scm_shell(title, body);
            }
            RepoLookup::NotARepo => {
                let title = self.panel_title(t(L10nKey::PanelScmTitle), None, None, window, cx);
                // Secondary: `cd` into an existing repository is as likely an
                // answer as making a new one here. An explicit click on the
                // pane's own host and directory, captured now.
                let init = PanelAction::secondary(
                    SCM_INIT_REPO,
                    t(L10nKey::PanelInitRepo),
                    cx.listener(move |this, _, window, cx| {
                        this.scm_init_repo(host.clone(), cwd.clone(), window, cx);
                    }),
                );
                // Plain words: the panel helper owns the hint's typography.
                let hint = t(L10nKey::PanelNotAGitRepoHint)
                    .to_string()
                    .into_any_element();
                let body =
                    self.panel_empty_with(t(L10nKey::PanelNotAGitRepo), Some(hint), Some(init), cx);
                return self.scm_shell(title, body);
            }
            RepoLookup::Root(root) => root,
        };

        self.scm.repo = Some(RepoKey {
            host: host.id(),
            root,
        });
        // An override whose host has left the registry is dropped, not worked
        // around: falling back to the *pane's* host while keeping the
        // override's root would probe the wrong machine for that path and
        // cache the answer under a mismatched key.
        if self
            .scm
            .repo_override
            .as_ref()
            .is_some_and(|k| crate::ui::host_registry::HostRegistry::get(cx, k.host).is_none())
        {
            self.scm.repo_override = None;
            self.scm.override_tab = None;
        }
        // An explicit pick from the switcher wins over the pane's own
        // repository, so everything below reads through `active_repo`.
        let repo = self
            .scm
            .active_repo()
            .cloned()
            .expect("the pane's repository was just recorded");
        let host = match crate::ui::host_registry::HostRegistry::get(cx, repo.host) {
            Some(host) => host,
            None => host,
        };
        self.scm_probe(&host, &repo.root, cx);
        let status = match self.scm_seen_status(repo.host, &repo.root, cx) {
            StatusRead::Ready(status) => status,
            StatusRead::Loading => {
                let title = self.panel_title(t(L10nKey::PanelScmTitle), None, None, window, cx);
                let body = self.panel_empty(t(L10nKey::PanelLoading), None, cx);
                return self.scm_shell(title, body);
            }
            StatusRead::Failed(reason) => {
                let title = self.panel_title(t(L10nKey::PanelScmTitle), None, None, window, cx);
                let body = self.scm_status_failed(host, repo.root.clone(), reason, cx);
                return self.scm_shell(title, body);
            }
        };

        // The count lives on the Changes tab now; the heading slot this used to
        // feed is the subtitle (`panel_title`'s second parameter) and was never
        // drawn for this panel.
        let title = self.panel_title(t(L10nKey::PanelScmTitle), None, None, window, cx);

        let branch = self.scm_branch_row(&repo, &status, cx);
        let naming = self.scm_new_branch_row(&repo, cx);
        let switching = self.scm_checkout_branch_row(&repo, cx);
        let commit = self.scm_commit_box(&repo, &status, window, cx);
        let buttons = self.scm_commit_buttons(&repo, &status, cx);
        let mut pinned = vec![branch];
        pinned.extend(naming);
        pinned.extend(switching);
        pinned.push(commit);
        pinned.push(buttons);
        // A commit's detail replaces the working tree's, so the two can never
        // be on screen claiming to be the same thing.
        let detail = self.scm.detail.clone();
        let body = match detail
            .as_ref()
            .and_then(|d| self.render_commit_detail(d, window, cx))
        {
            Some(body) => body,
            None if status.is_clean() => self.panel_empty(
                t(L10nKey::PanelNoChanges),
                Some(t(L10nKey::PanelNoChangesHint)),
                cx,
            ),
            None => {
                // Pinned with the rest, so the filter stays in reach however
                // far down the list has been scrolled. The blocks above carry
                // their own spacing and the list starts flush ("none above
                // the first" group), so the filter brings the pause under it
                // itself — the Files tab's 10 between its search and the tree.
                // Without it the first group header sat on the field's edge
                // while 14 stood above it.
                pinned.push(
                    div()
                        .pb(px(PINNED_GAP))
                        .child(self.scm_filter_row(window, cx))
                        .into_any_element(),
                );
                self.scm_groups(&repo, &status, window, cx)
            }
        };
        let history = self.render_graph_section(&repo, window, cx);
        self.scm_shell_full(title, pinned, body, history)
    }

    /// The repository line: which branch, how far from its upstream, and one
    /// button to close the gap.
    ///
    /// A row of its own rather than the title's trailing slot. Off macOS the
    /// tab tiles render *after* that slot, and a branch name is the elastic
    /// element here — it would be the first thing squeezed. `render_sftp_
    /// breadcrumb` sets the same precedent.
    fn scm_branch_row(
        &mut self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.scm_load_branches(repo, cx);
        let mono = cx.theme().mono_font_family.clone();
        let theme = cx.theme();
        let (warning, muted, fg) = (theme.warning, theme.muted_foreground, theme.foreground);
        let detached = matches!(status.head, HeadState::Detached { .. });
        // The same helper `scm_push`'s guard asks, so the tile and the toast
        // cannot answer differently about the same HEAD.
        let unpushable = pushable_branch(&status.head).err();
        let busy = self.scm_network_busy(repo, cx);
        let others = self.scm_other_repos(repo);

        let mut notes: Vec<AnyElement> = Vec::new();
        if let Some(count) = others {
            notes.push(branch_note(&format!("+{count}"), muted, &mono));
        }
        if detached {
            notes.push(info_chip(
                t(L10nKey::ScmDetached),
                warning.opacity(0.16),
                warning,
                &mono,
            ));
        }
        if let Some(op) = status.operation {
            notes.push(info_chip(
                t(operation_label(op)),
                warning.opacity(0.16),
                warning,
                &mono,
            ));
        }
        // Armed-amend is a mode you can forget you are in, which puts it in the
        // same class as `detached` and a rebase in progress above — so it gets
        // their tint, not a chip of its own invention.
        if self.scm.amend {
            notes.push(info_chip(
                t(L10nKey::ScmAmendBadge),
                warning.opacity(0.16),
                warning,
                &mono,
            ));
        }
        if let Some(text) = tracking_chip(
            status.upstream.as_deref(),
            status.ahead_behind,
            unpushable.is_none(),
        )
        .filter(|_| status.upstream.is_some())
        {
            notes.push(branch_note(&text, muted, &mono));
        }

        h_flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            // Taller than a file row, and what sets it is the `TILE_SIZE_SM`
            // sync tile at the end plus 2px of air. The branch trigger beside
            // it is the same 24, so the row has one interior height and the
            // type sits inside it rather than setting it.
            .min_h(rems(28. / 16.))
            .mt(px(PINNED_TOP))
            .pl(px(PINNED_INSET + ROW_INSET))
            .pr(px(PINNED_INSET))
            .child(
                Icon::empty()
                    .path("icons/git-branch.svg")
                    .xsmall()
                    .text_color(muted),
            )
            // The trigger is a `Button` because that is the one element the
            // dropdown trait is implemented for. `dropdown_caret` turns its
            // label row into `justify_between`, which is what puts the name on
            // the left and the chevron against the notes.
            //
            // The branch is the most important word on the row, and it earns
            // that by being in full-strength ink — not by being set larger
            // than the file names underneath it.
            //
            // `.small()` is how a `Button` is told to use `TEXT`; `.xsmall()`
            // is `META`, which is a step *under* the file names and would make
            // the branch quieter than the paths it scopes. Sans at `TEXT`
            // beside mono at `TEXT_MONO` is the ramp's own pairing, so the two
            // read as one size — which is the rule this comment is stating.
            // The height is spelled out because `.small()` also carries a 24px
            // box and the row's own air is budgeted below.
            // The wrapper is what carries `flex_1`, not the button.
            // `dropdown_menu_with_anchor` hands the button to a `Popover`,
            // which wraps it in a plain `div` of its own and drops the
            // trigger's style on the floor — so `flex_1` set on the button
            // lands *inside* a box that still measures its own content and
            // refuses to shrink. At the panel's 216px floor a 24-character
            // branch name is wider than the row, and what got pushed off the
            // end was the sync tile: gone entirely, with no way to reach it.
            .child(
                div().flex_1().min_w(rems(3.)).ml(px(-6.)).child(
                    Button::new("scm-branch")
                        .ghost()
                        .small()
                        .dropdown_caret(true)
                        // A child rather than `.label`, because a label is
                        // laid out `flex_none` and cannot be told to
                        // shrink. The label measures the width layout gives
                        // it and keeps both ends of the name, so it is the
                        // only cut here — an `elide_middle` character budget
                        // on top of it put two ellipses in a row
                        // (`fix/new-tab-……`). The menu this opens lists the
                        // whole name for a pointer that wants it.
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .line_height(relative(1.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(ElidedLabel::new(
                                    "scm-branch-label",
                                    head_label(&status.head),
                                    ElideMode::Middle,
                                )),
                        )
                        .w_full()
                        .h(rems(24. / 16.))
                        // The glyph's gap is the row's 6px. A small button's own
                        // padding stacked another 10 on it and set the name
                        // off the text column the file names below keep. It
                        // keeps 4 so the hover fill clears the text, and the
                        // wrapper takes 6 back, which lands the name on the
                        // file names' column.
                        .pl(px(4.))
                        .rounded(px(5.))
                        .text_color(fg)
                        .when(detached, |s| s.font_family(mono.clone()))
                        .dropdown_menu_with_anchor(
                            gpui::Anchor::TopLeft,
                            self.scm_branch_menu(repo, status, cx),
                        ),
                ),
            )
            // One box for every note and chip, and it is allowed to lose.
            // Each of them is `flex_none` on its own, so a detached HEAD
            // mid-rebase used to add up past the row and shove the sync tile
            // out of the panel — the same disappearance the branch name caused
            // above. The order of who gives way is now spelled out: the branch
            // name first (`flex_1`), this group second, and the tile never,
            // because a control you cannot reach is worse than a badge you
            // cannot finish reading.
            //
            // The box only exists when it holds something. An empty one still
            // counts as a flex item, and the row's `gap` would spend 6px on
            // either side of nothing — pushing the caret away from the tile on
            // the quiet branch that is most of what anyone looks at.
            .when(!notes.is_empty(), |row| {
                row.child(
                    h_flex()
                        .gap(px(6.))
                        .min_w(px(0.))
                        .overflow_hidden()
                        .children(notes),
                )
            })
            .child(
                crate::ui::tab_strip::chrome_tile_sized(
                    Button::new("scm-sync").icon(if busy {
                        Icon::new(IconName::LoaderCircle)
                    } else {
                        Icon::empty().path("icons/git-sync.svg")
                    }),
                    BRANCH_TILE,
                    BRANCH_TILE_GLYPH,
                    false,
                    cx,
                )
                .rounded(px(6.))
                // With no branch to move the tile could only ever lose (#545):
                // upstream is None at a detached or unborn HEAD by definition,
                // so sync degenerates into scm_push, which has nothing to
                // send. Say that on the tooltip instead of promising "Publish
                // Branch" — the one thing a detached HEAD cannot do — and let
                // the key binding, palette and compound-verb paths toast the
                // same reason from inside scm_push.
                .disabled(busy || unpushable.is_some())
                .tooltip(match unpushable {
                    Some(reason) => t(reason),
                    None if status.upstream.is_some() => t(L10nKey::ScmSync),
                    None => t(L10nKey::ScmPublishBranch),
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.run_scm_action(ScmIntent::Sync, window, cx);
                })),
            )
            .into_any_element()
    }

    /// Whether a network operation against this repository is still running.
    ///
    /// Asks the slot counter `run_git_op` claims from, which is the operation
    /// itself rather than a proxy for it: the epoch moves for a `.git` event
    /// or a file save too, so watching that would drop the spinner in the
    /// middle of a slow push.
    ///
    /// Read through `try_global`, never `default_global` — this runs from
    /// `render`, and taking the global mutably there queues a global-observer
    /// effect on every frame.
    fn scm_network_busy(&self, repo: &RepoKey, cx: &gpui::App) -> bool {
        cx.try_global::<crate::terminal::git_data::ScmData>()
            .is_some_and(|data| data.network_slots(repo.host, &repo.root) > 0)
    }

    /// How many repositories other than this one the panel could switch to.
    fn scm_other_repos(&self, current: &RepoKey) -> Option<usize> {
        let choices = self.scm_repo_choices();
        let count = choices.len().saturating_sub(1);
        (count > 0 && choices.contains(current)).then_some(count)
    }

    /// Read the local branch names, at most once per epoch.
    fn scm_load_branches(&mut self, repo: &RepoKey, cx: &mut Context<Self>) {
        let epoch = cx
            .default_global::<crate::terminal::git_data::ScmData>()
            .epoch(repo.host, &repo.root);
        if self
            .scm
            .branches
            .get(repo)
            .is_some_and(|(at, _)| *at == epoch)
            || self.scm.branches_loading.contains(repo)
        {
            return;
        }
        let Some(host) = crate::ui::host_registry::HostRegistry::get(cx, repo.host) else {
            return;
        };
        self.scm.branches_loading.insert(repo.clone());
        let root = repo.root.clone();
        let key = repo.clone();
        crate::ui::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                // `for-each-ref` rather than `branch`: no porcelain warnings,
                // no column layout, and one name per line whatever the config.
                tty59_core::core::git::git(
                    h,
                    &root,
                    &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
                )
            },
            move |this, out, cx| {
                this.scm.branches_loading.remove(&key);
                let names = out
                    .unwrap_or_default()
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect();
                this.scm.branches.insert(key, (epoch, names));
                cx.notify();
            },
        );
    }

    fn scm_branch_menu(
        &self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static + use<>
    {
        let app = cx.entity().downgrade();
        let repo = repo.clone();
        let current = match &status.head {
            HeadState::Branch { name, .. } | HeadState::Unborn { branch: name } => name.clone(),
            HeadState::Detached { .. } => String::new(),
        };
        let branches = self
            .scm
            .branches
            .get(&repo)
            .map(|(_, names)| names.clone())
            .unwrap_or_default();
        let others = self.scm_repo_choices();
        // Why Push is unavailable, in the words `scm_push`'s own guard uses.
        let push_blocked = pushable_branch(&status.head).err();
        // How far the branch is from its upstream, from the status snapshot.
        let (pull_count, push_count) = branch_verb_counts(status.ahead_behind);
        move |menu, _window, cx| {
            let mut menu = menu.min_w(px(200.));
            // Past a dozen the list stops being scannable, so it scrolls
            // rather than growing taller than the window.
            if branches.len() > BRANCHES_IN_MENU {
                menu = menu.scrollable(true).max_h(px(300.));
            }
            for name in &branches {
                let is_current = *name == current;
                menu = menu.item(
                    PopupMenuItem::new(name.clone())
                        .checked(is_current)
                        .disabled(is_current)
                        .on_click({
                            let app = app.clone();
                            let repo = repo.clone();
                            let name = name.clone();
                            move |_, window, cx| {
                                let _ = app.update(cx, |this, cx| {
                                    this.scm_op(
                                        repo.clone(),
                                        GitOp::CheckoutBranch { name: name.clone() },
                                        window,
                                        cx,
                                    )
                                });
                            }
                        }),
                );
            }
            menu = menu
                .separator()
                .item(PopupMenuItem::new(t(L10nKey::ScmCreateBranch)).on_click({
                    let app = app.clone();
                    move |_, window, cx| {
                        let _ = app.update(cx, |this, cx| this.scm_begin_create_branch(window, cx));
                    }
                }));
            for (label, intent) in [
                (L10nKey::ScmFetch, ScmIntent::Fetch),
                (L10nKey::ScmPull, ScmIntent::Pull),
                (L10nKey::ScmPush, ScmIntent::Push),
            ] {
                // Pull's number is what is waiting upstream, Push's what is
                // waiting here. A blocked Push says why in place of a number.
                let (count, reason) = match intent {
                    ScmIntent::Pull => (pull_count, None),
                    ScmIntent::Push => (
                        push_count.filter(|_| push_blocked.is_none()),
                        push_blocked.map(t),
                    ),
                    _ => (None, None),
                };
                let verb = t(label);
                menu = menu.item(
                    PopupMenuItem::element(move |_window, cx| verb_row(verb, count, reason, cx))
                        // Fetch works from any HEAD, and Pull fails loud out
                        // of git itself; a Push from a HEAD with no branch to
                        // move is the one item that could only die in
                        // scm_push's guard (#545).
                        .disabled(push_blocked.is_some() && matches!(intent, ScmIntent::Push))
                        .on_click({
                            let app = app.clone();
                            move |_, window, cx| {
                                let _ = app
                                    .update(cx, |this, cx| this.run_scm_action(intent, window, cx));
                            }
                        }),
                );
            }
            if others.len() > 1 {
                menu = menu
                    .separator()
                    .item(PopupMenuItem::label(t(L10nKey::ScmSwitchRepository)));
                for other in &others {
                    let (label, parent) = repo_choice_label(other, cx);
                    #[cfg(test)]
                    let key = other.root.display().to_string();
                    menu = menu.item(
                        PopupMenuItem::element(move |_window, cx| {
                            let row = div().w_full().child(crate::ui::tab_strip::menu_row(
                                label.clone(),
                                parent.clone(),
                                cx,
                            ));
                            #[cfg(test)]
                            let row = {
                                let key = key.clone();
                                row.debug_selector(move || format!("scm-repo-choice:{key}"))
                            };
                            row
                        })
                        .checked(*other == repo)
                        .on_click({
                            let app = app.clone();
                            let other = other.clone();
                            move |_, _window, cx| {
                                let _ = app.update(cx, |this, cx| {
                                    this.scm.repo_override = Some(other.clone());
                                    this.scm.override_tab = Some(this.active);
                                    cx.notify();
                                });
                            }
                        }),
                    );
                }
            }
            menu
        }
    }

    /// Every repository the panel has looked at this session.
    ///
    /// Built from the roots it resolved rather than from the open tabs,
    /// because only a resolved root is safe to act on: the cheap per-tab cache
    /// knows a repository's *home*, which is a different directory inside a
    /// linked worktree, and running an operation in the wrong tree is worse
    /// than not offering the switch.
    fn scm_repo_choices(&self) -> Vec<RepoKey> {
        let mut out: Vec<RepoKey> = Vec::new();
        for ((host, _cwd), (_, root)) in &self.scm.roots {
            let Some(root) = root else { continue };
            let key = RepoKey {
                host: *host,
                root: root.clone(),
            };
            if !out.contains(&key) {
                out.push(key);
            }
        }
        out.sort_by(|a, b| a.root.cmp(&b.root));
        out
    }

    pub(crate) fn scm_begin_create_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::ScmCreateBranch)));
        let handle = input.read(cx).focus_handle(cx);
        self.scm.checkout_branch = None;
        self.scm.new_branch = Some(input);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Open the inline "switch to which branch" input.
    ///
    /// The branch-name button already drops a menu of every local branch, and
    /// that is the better way to pick one. This is the other half: the command
    /// palette entry and its key binding, which have no button to hang a menu
    /// off. It used to be wired to nothing at all — the action was registered,
    /// listed and bindable, and invoking it silently did nothing.
    pub(crate) fn scm_begin_checkout_branch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::ScmCheckoutBranch)));
        let handle = input.read(cx).focus_handle(cx);
        self.scm.new_branch = None;
        self.scm.checkout_branch = Some(input);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// The inline "name your branch" row.
    ///
    /// A text input rather than a dialog: `window.prompt` can only offer
    /// buttons, and the project has no modal component to reach for. The file
    /// tree names new files the same way.
    fn scm_new_branch_row(&mut self, repo: &RepoKey, cx: &mut Context<Self>) -> Option<AnyElement> {
        let input = self.scm.new_branch.clone()?;
        let repo = repo.clone();
        Some(
            h_flex()
                .id("scm-new-branch")
                .flex_none()
                .items_center()
                // Every input row in the panel is 30px, and the field inside
                // it is the `.xsmall()` one that height was derived from.
                .h(px(SEARCH_H))
                .px(px(PINNED_INSET))
                .child(div().flex_1().min_w_0().child(Input::new(&input).xsmall()))
                .on_key_down(
                    cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            "escape" => {
                                this.scm.new_branch = None;
                                cx.notify();
                            }
                            "enter" => {
                                let Some(input) = this.scm.new_branch.take() else {
                                    return;
                                };
                                let name = input.read(cx).value().trim().to_string();
                                cx.notify();
                                if name.is_empty() {
                                    return;
                                }
                                this.scm_op(
                                    repo.clone(),
                                    GitOp::CreateBranch {
                                        name,
                                        start: None,
                                        checkout: true,
                                    },
                                    window,
                                    cx,
                                );
                            }
                            _ => {}
                        }
                    }),
                )
                .into_any_element(),
        )
    }

    /// The inline "switch to which branch" row, twin of [`Self::scm_new_branch_row`].
    fn scm_checkout_branch_row(
        &mut self,
        repo: &RepoKey,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let input = self.scm.checkout_branch.clone()?;
        let repo = repo.clone();
        Some(
            h_flex()
                .id("scm-checkout-branch")
                .flex_none()
                .items_center()
                .h(px(SEARCH_H))
                .px(px(PINNED_INSET))
                .child(div().flex_1().min_w_0().child(Input::new(&input).xsmall()))
                .on_key_down(
                    cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            "escape" => {
                                this.scm.checkout_branch = None;
                                cx.notify();
                            }
                            "enter" => {
                                let Some(input) = this.scm.checkout_branch.take() else {
                                    return;
                                };
                                let name = input.read(cx).value().trim().to_string();
                                cx.notify();
                                if name.is_empty() {
                                    return;
                                }
                                // A name that is not a branch is git's to
                                // reject — `scm_op` surfaces the failure the
                                // same way it does for every other operation,
                                // and second-guessing it here would also have
                                // to know about remote-tracking refs and tags.
                                this.scm_op(
                                    repo.clone(),
                                    GitOp::CheckoutBranch { name },
                                    window,
                                    cx,
                                );
                            }
                            _ => {}
                        }
                    }),
                )
                .into_any_element(),
        )
    }

    /// The message box.
    ///
    /// `key_context` rather than a focus trap: `secondary-enter` is
    /// `ToggleFullscreen` at the window level, and the two coexist because
    /// gpui resolves a keystroke by walking outwards from the focused node —
    /// the narrow context wins while the box has focus, and the window keeps
    /// the chord everywhere else.
    ///
    /// A hairline and no fill, like every other framed thing in the panel.
    /// Focus recolours the same hairline to `theme.ring` rather than adding
    /// anything: a border that appears on focus would move the text by a pixel
    /// every time the box was clicked, because gpui measures border-box.
    fn scm_commit_box(
        &mut self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.scm_commit_input(repo, status, window, cx);
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        let sf = cx.global::<crate::ui::presets::Surfaces>().sidebar;
        div()
            .key_context(COMMIT_KEY_CONTEXT)
            .flex_none()
            .px(px(PINNED_INSET))
            .pt(px(PINNED_GAP))
            .child(
                div()
                    // `MSG_MIN_H` is `panel_search`'s height and the paddings
                    // are what get it there; see the constants. The maximum is
                    // a rail rather than the truth — in auto-grow mode the
                    // input sizes itself off its row count, so `MSG_ROWS_MAX`
                    // is what actually stops it, and this holds the shape if it
                    // ever measures itself differently.
                    .min_h(px(MSG_MIN_H))
                    .max_h(px(MSG_MAX_H))
                    .rounded(ROW_RADIUS)
                    // Half a rung, not a whole one. The hover step is what a
                    // row wears when the pointer is on it — a transient state —
                    // and the message box wears its fill all the time, so at
                    // full strength it read as the loudest thing on an idle
                    // panel. Focus then takes the whole rung, which keeps the
                    // two apart without a ring: the caret already says where
                    // the keystrokes go, and the fill only has to say which
                    // shape owns them.
                    .bg(gpui::rgb(match focused {
                        true => sf.hover,
                        false => field_fill(sf),
                    }))
                    .px(px(MSG_PAD_X))
                    .py(px(MSG_PAD_Y))
                    // `.xsmall()` is also what sets the box's height, because
                    // it is the size whose `input_py` is zero — see the
                    // `MSG_*` constants.
                    .child(Input::new(&input).appearance(false).xsmall()),
            )
            .into_any_element()
    }

    /// Hand the box the draft belonging to the repository on screen, and keep
    /// whatever is in it under the repository it was typed for.
    ///
    /// Per repository rather than per tab or per pane: a working tree has one
    /// pending message however many panes are looking at it.
    fn scm_commit_input(
        &mut self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<InputState> {
        let input = match self.scm.commit_input.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .multi_line(true)
                        .auto_grow(MSG_ROWS, MSG_ROWS_MAX)
                        .placeholder(t(L10nKey::ScmCommitPlaceholder))
                });
                self.scm.commit_input = Some(input.clone());
                input
            }
        };
        let text = input.read(cx).value().to_string();

        if self.scm.commit_repo.as_ref() != Some(repo) {
            if let Some(previous) = self.scm.commit_repo.take() {
                self.scm.drafts.insert(previous, text);
            }
            let next = match self.scm.drafts.get(repo) {
                Some(draft) => draft.clone(),
                // A merge or a cherry-pick leaves git's own message in
                // `.git/MERGE_MSG`; starting from blank would throw away the
                // conflict summary the user is about to want.
                None => status.prefilled_message.clone().unwrap_or_default(),
            };
            input.update(cx, |state, cx| state.set_value(next, window, cx));
            self.scm.commit_repo = Some(repo.clone());
            return input;
        }

        if self.scm_commit_landed(repo, status, &text) {
            input.update(cx, |state, cx| state.set_value("", window, cx));
            return input;
        }
        if self.scm.drafts.get(repo).map(String::as_str) != Some(text.as_str()) {
            self.scm.drafts.insert(repo.clone(), text);
        }
        input
    }

    /// Whether the commit we dispatched actually happened, and so whether the
    /// message may be thrown away.
    ///
    /// Clearing the box the moment `git commit` is *dispatched* would lose a
    /// carefully written message to a pre-commit hook that rejects it. HEAD
    /// moving is the one signal that says the message is now in the
    /// repository; an edit made in the meantime keeps it too.
    fn scm_commit_landed(
        &mut self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        text: &str,
    ) -> bool {
        let Some((sent_repo, before, message)) = &self.scm.committing else {
            return false;
        };
        if sent_repo != repo || *before == status.head || message != text {
            return false;
        }
        self.scm.committing = None;
        self.scm.drafts.remove(repo);
        true
    }

    /// The line under the message box: what is staged on the left, what to do
    /// about it on the right.
    ///
    /// One split control, not a bar across the panel: the label button and the
    /// chevron share a single hairline frame with a 1px divider between them,
    /// and the pair is only as wide as the label. Right-aligned, with the
    /// count it acts on reading along the same line.
    ///
    /// An available commit is the panel's primary action. Empty/disabled
    /// controls stay neutral rather than advertising an action that cannot run.
    fn scm_commit_buttons(
        &self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The index's count is the refresh's own bookkeeping, so this frame
        // costs a hash probe rather than a walk over every entry. `None`
        // means the index has not landed — the button stays disabled rather
        // than guess zero conflicts.
        let plan = commit_plan_with_conflicts(
            status,
            self.scm.amend,
            self.scm.draft(repo),
            index_of(cx, repo.host, &repo.root).map(|index| index.conflict_count),
        );
        let repo_for_button = repo.clone();
        let live = plan.enabled;
        let staged = status.staged().count();
        let muted = cx.theme().muted_foreground;
        let paint = commit_paint(live, cx);
        h_flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(PINNED_INSET))
            .pt(px(PINNED_GAP))
            .pb(px(PINNED_BOTTOM))
            // The reading the button acts on, in the row it acts from. It
            // gives way first: a long count is still a count, and the control
            // beside it is the thing that has to keep its shape.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(META))
                    .text_color(muted)
                    // With `U` paths in the way, what the user needs counted
                    // is those — the staged tally comes back once they are
                    // resolved.
                    .child(if plan.conflicts > 0 {
                        t_plural(L10nKey::ScmConflictCount, plan.conflicts, &[])
                    } else {
                        t_plural(L10nKey::ScmStagedFileCount, staged, &[])
                    }),
            )
            .child(
                // Filled, not outlined, and for the same reason the message box
                // above it is: nothing else in this panel wears a border, and a
                // hairline box sitting on the panel's own fill reads as raised
                // — a shadow nobody drew.
                //
                // The frame owns the fill, the rounding *and* the hover, so the
                // control lights as one shape. Letting each half light itself
                // is what a split button conventionally does, but this one has
                // no outline around either half and a seam only a pixel wide:
                // half a lit pill reads as a paint bug rather than as "this is
                // the part you are on". The halves still carry
                // `segment_corners` so their own hit shapes stay inside the
                // frame's radius — `overflow_hidden` would do that too, and
                // would take the chevron's popup menu with it.
                //
                // Committable, the whole frame inverts — ink fill, surface
                // label — which is the one prominent shape on the panel. With
                // nothing to commit it sinks back to the message box's faint
                // fill, so it never advertises an action that cannot run.
                h_flex()
                    .flex_none()
                    .items_center()
                    .h(px(COMMIT_H))
                    .rounded(COMMIT_RADIUS)
                    .bg(paint.fill)
                    .hover(move |s| s.bg(paint.hover))
                    .child(
                        // `xsmall` is the token that sets a `Button`'s label to
                        // `META`, and a control's label is what `META` is for:
                        // the branch trigger above is `.small()` because it
                        // shows a *value*, and this one shows a verb. The
                        // padding and the height are set below, so the size
                        // token is doing nothing here but choosing the type.
                        Button::new("scm-commit")
                            // A custom variant, not `.ghost()`: the frame is
                            // already painted at the ramp's hover rung, and a
                            // ghost hovers to a colour close enough to it that
                            // the pointer would get no answer. This walks the
                            // same two rungs the file rows walk.
                            .custom(commit_half(paint.ink, cx))
                            .xsmall()
                            .label(t(plan.label))
                            .h_full()
                            .px(px(12.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .rounded_corners(segment_corners(0, 2, COMMIT_RADIUS, HAIRLINE))
                            // The ink is named here rather than left to the
                            // variant, so the disabled half greys out: it lands
                            // over the variant's own paint because
                            // `refine_style` runs after everything the variant
                            // does.
                            .text_color(if live { paint.ink } else { muted })
                            .disabled(!live)
                            .when(!live, |b| b.tooltip(plan.reason_text()))
                            // The button's own test handle, so a click test
                            // aims at the real control rather than a guess
                            // about where in the frame it ended up.
                            .when(cfg!(test), |b| b.debug_selector(|| "scm-commit".into()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.scm_commit(
                                    repo_for_button.clone(),
                                    this.scm.amend,
                                    None,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    // The seam is a short rule in the label's own ink, inset
                    // from both edges so it reads as a divider inside one shape
                    // rather than a cut through it. A div rather than
                    // `border_l_1` on the chevron, because a ghost `Button`
                    // repaints its border colour transparent in every state it
                    // has and the seam would vanish under the pointer.
                    .child(
                        div()
                            .flex_none()
                            .w(px(0.5))
                            .h(px(COMMIT_H - 12.))
                            .bg(paint.seam),
                    )
                    .child(self.scm_commit_menu(repo, paint.ink, cx)),
            )
            .into_any_element()
    }

    /// The chevron half of the split control.
    ///
    /// Never disabled, whatever the button beside it is doing: stash and amend
    /// still mean something with nothing staged, and this is the only way to
    /// reach them.
    fn scm_commit_menu(
        &self,
        repo: &RepoKey,
        ink: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let amend = self.scm.amend;
        Button::new("scm-commit-menu")
            .custom(commit_half(ink, cx))
            .icon(Icon::new(IconName::ChevronDown))
            // `Button` sizes an icon off its own `Size`, so the glyph is asked
            // for by way of the size that produces it — the same conversion
            // `chrome_tile_sized` does.
            .with_size(px(COMMIT_GLYPH / crate::ui::tab_strip::BUTTON_ICON_SCALE))
            .w(px(COMMIT_CHEVRON_W))
            .h_full()
            .rounded_corners(segment_corners(1, 2, COMMIT_RADIUS, HAIRLINE))
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, {
                let app = cx.entity().downgrade();
                let repo = repo.clone();
                move |menu, _window, _cx| {
                    let mut menu = menu.min_w(px(190.));
                    for (label, intent) in [
                        (L10nKey::ScmCommitButton, ScmIntent::Commit),
                        (L10nKey::ScmCommitAndPush, ScmIntent::CommitAndPush),
                        (L10nKey::ScmCommitAndSync, ScmIntent::CommitAndSync),
                    ] {
                        menu = menu.item(PopupMenuItem::new(t(label)).on_click({
                            let app = app.clone();
                            move |_, window, cx| {
                                let _ = app
                                    .update(cx, |this, cx| this.run_scm_action(intent, window, cx));
                            }
                        }));
                    }
                    menu = menu.separator().item(
                        // A menu item rather than a checkbox row: the panel is
                        // 260px wide, and the armed state already shows up in the
                        // button's label and in the chip on the branch row.
                        PopupMenuItem::new(t(L10nKey::ScmAmendLastCommit))
                            .checked(amend)
                            .on_click({
                                let app = app.clone();
                                move |_, _window, cx| {
                                    let _ = app.update(cx, |this, cx| {
                                        this.scm.amend = !this.scm.amend;
                                        cx.notify();
                                    });
                                }
                            }),
                    );
                    menu.separator()
                        .item(PopupMenuItem::new(t(L10nKey::ScmStashAll)).on_click({
                            let app = app.clone();
                            let repo = repo.clone();
                            move |_, window, cx| {
                                let _ = app.update(cx, |this, cx| {
                                    this.scm_stash_all(repo.clone(), window, cx)
                                });
                            }
                        }))
                }
            })
            .into_any_element()
    }

    /// Title over a scrolling body, with the panel's own scroll handle.
    ///
    /// Not `panel_scroll`: that one owns `right_panel.scroll`, and the rows
    /// that land between the title and the list in later steps have to stay
    /// pinned while the list moves under them.
    fn scm_shell(&self, title: AnyElement, body: AnyElement) -> AnyElement {
        self.scm_shell_with(title, Vec::new(), body)
    }

    /// `pinned` rows sit between the title and the list and do not scroll:
    /// the message box has to stay reachable however far down the files go.
    fn scm_shell_with(
        &self,
        title: AnyElement,
        pinned: Vec<AnyElement>,
        body: AnyElement,
    ) -> AnyElement {
        self.scm_shell_full(title, pinned, body, None)
    }

    /// …and `footer` is the history section, which scrolls on its own.
    ///
    /// The four bands sit flush on the panel with nothing between them — no
    /// gutter, no fill, no rounded blocks. What separates them is the same
    /// thing that separates every other stack of rows in the right panel: the
    /// rows' own insets and the pause between them. Each band keeps its own
    /// `CONTENT_INSET`, so the panel's text column stays one column from the
    /// title to the last commit in the history.
    fn scm_shell_full(
        &self,
        title: AnyElement,
        pinned: Vec<AnyElement>,
        body: AnyElement,
        footer: Option<AnyElement>,
    ) -> AnyElement {
        let scroller = div()
            .id("panel-scm-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scm.scroll)
            .child(body);
        v_flex()
            .flex_1()
            .min_h_0()
            .child(title)
            .children(pinned)
            .child(crate::ui::scrollbar::with_vertical_scrollbar(
                "panel-scm-scrollbar",
                scroller,
                &self.scm.scroll,
            ))
            .children(footer)
            .into_any_element()
    }

    /// The host and directory the panel is looking at.
    pub(crate) fn scm_pane_target(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(SharedHost, PathBuf)> {
        let leaf = self.tabs.get(self.active)?.detail_pane(window, cx)?;
        let view = leaf.read(cx);
        let cwd = view.effective_host_cwd()?;
        Some((view.host(cx)?, cwd))
    }

    /// Turn the pane's directory into the repository root every write has to
    /// run from.
    ///
    /// Pathspecs out of `status --porcelain=v2` are relative to the root, so
    /// running `git add` from a subdirectory would name the wrong files. The
    /// root is also the cache key, which is what lets two panes in two
    /// subdirectories of one repository share a single status.
    ///
    /// Resolved with its own `rev-parse` rather than borrowed from the cheap
    /// per-tab cache. That cache holds a repository's *home*, which is a
    /// different directory inside a linked worktree, and it is only filled in
    /// for panes whose shell reports a cwd — a pane without shell integration
    /// would leave the panel loading forever.
    pub(crate) fn scm_repo_root(
        &mut self,
        host: &SharedHost,
        cwd: &Path,
        cx: &mut Context<Self>,
    ) -> RepoLookup {
        let id = host.id();
        let key = (id, cwd.to_path_buf());
        match self.scm.roots.get(&key) {
            Some((_, Some(root))) => return RepoLookup::Root(root.clone()),
            // "Not a repository" is re-asked now and then, because `git init`
            // in the pane below has to start showing up without a restart.
            Some((at, None)) if at.elapsed() < NOT_A_REPO_RETRY => return RepoLookup::NotARepo,
            _ => {}
        }
        if !self.scm.root_lookups.insert(key.clone()) {
            return RepoLookup::Pending;
        }
        let dir = cwd.to_path_buf();
        crate::ui::host_ops::HostOps::run(
            host.clone(),
            cx,
            move |h| {
                // Asked separately from the status probe, and asked first: the
                // answer is what everything else is keyed by, it is the same
                // for every pane in the tree, and it is a ref lookup rather
                // than a walk of the working tree.
                tty59_core::core::git::git(
                    h,
                    &dir,
                    &["rev-parse", "--path-format=absolute", "--show-toplevel"],
                )
            },
            move |this, out, cx| {
                this.scm.root_lookups.remove(&key);
                // In the spelling everything else keys by: Git for Windows
                // answers `C:/Users/…` and this root is what the panel, the
                // commit detail and `ScmData` all compare against a path the
                // OS spelled. Only for a repository on this machine — a remote
                // root is native over there and every write below runs `git`
                // from it on that box. See `tty59_core::core::path_spelling`.
                let root = out
                    .as_deref()
                    .and_then(|s| s.lines().next())
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| tty59_core::core::path_spelling::spelling_on_buf(id, l));
                this.scm.roots.insert(key, (Instant::now(), root));
                cx.notify();
            },
        );
        RepoLookup::Pending
    }

    /// `scm_refresh` with a floor under how often a fruitless probe repeats.
    fn scm_probe(&mut self, host: &SharedHost, root: &Path, cx: &mut Context<Self>) {
        let key = (host.id(), root.to_path_buf());
        if status_of(cx, host.id(), root).is_none() {
            let now = Instant::now();
            match self.scm.probe_attempt.get(&key) {
                Some(at) if now.duration_since(*at) < PROBE_RETRY => return,
                _ => {
                    self.scm.probe_attempt.insert(key, now);
                }
            }
        }
        self.scm_refresh(host.clone(), root.to_path_buf(), cx);
    }

    /// Read the status and record which one this frame drew, so the watcher
    /// below can tell a real change from its own noise.
    fn scm_seen_status(&mut self, host: HostId, root: &Path, cx: &mut Context<Self>) -> StatusRead {
        let read = status_read_of(cx, host, root);
        self.scm.seen = Some(((host, root.to_path_buf()), read.clone()));
        read
    }

    /// The Changes tab when git could not be asked and nothing is cached: the
    /// failure in the danger tone, git's own words under it, and the way out.
    ///
    /// Inks: the dot is a mark, `theme.danger` (3:1); the heading is the body
    /// ink; the reason is 12px text in the cached `ChromeInk.tab_count` role,
    /// derived over the sidebar's own fills. Nothing is measured here and
    /// nothing is read from disk: the reason arrived with the failure.
    pub(crate) fn scm_status_failed(
        &self,
        host: SharedHost,
        root: PathBuf,
        fact: Arc<FailureFact>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (danger, muted, fg) = (theme.danger, theme.muted_foreground, theme.foreground);
        let reason_ink: gpui::Hsla = gpui::rgb(crate::ui::theme::chrome_ink(cx).tab_count).into();
        let retry = PanelAction::primary(
            SCM_STATUS_RETRY,
            t(L10nKey::PanelScmStatusRetry),
            cx.listener(move |this, _, _, cx| {
                this.scm_retry_status(host.clone(), root.clone(), cx);
            }),
        );
        // The words are made here, once, in the language now: the line shown
        // and the line Copy error writes are this same string. A failure that
        // said nothing has nothing to show or copy.
        let text = failure_text(&fact);
        let reason: Option<Arc<str>> = (!text.trim().is_empty()).then(|| Arc::from(text));
        let copy = reason.clone().map(|reason| {
            PanelAction::secondary(
                SCM_STATUS_COPY,
                t(L10nKey::PanelCopyError),
                move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(reason.to_string()));
                },
            )
        });
        v_flex()
            .px(px(TEXT_INSET))
            .py(px(4.))
            .gap(px(3.))
            .text_size(rems(TEXT))
            .text_color(muted)
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(danger))
                    .child(
                        div()
                            // Wraps inside the column instead of running past
                            // it: a long translation still shows in full.
                            .flex_1()
                            .min_w_0()
                            .text_color(fg)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(t(L10nKey::PanelScmStatusFailed).to_string()),
                    ),
            )
            .children(reason.map(|reason| {
                div()
                    .text_size(rems(META))
                    .text_color(reason_ink)
                    .child(reason.to_string())
            }))
            .child(
                h_flex()
                    .pt(px(PANEL_ACTION_GAP))
                    .gap(px(8.))
                    // Retry and Copy error stack when the panel is too narrow
                    // for both labels side by side, rather than clipping.
                    .flex_wrap()
                    .child(retry.render(cx))
                    .children(copy.map(|copy| copy.render(cx))),
            )
            .into_any_element()
    }

    /// Retry: end this repository's failure and its rest and the panel's own
    /// probe floor, then ask git again through the ordinary off-thread
    /// refresh. The panel reads Loading until the answer lands, and a late
    /// answer still answers to the host, epoch and subscription checks.
    pub(crate) fn scm_retry_status(
        &mut self,
        host: SharedHost,
        root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let id = host.id();
        cx.default_global::<crate::terminal::git_data::ScmData>()
            .forget_failure(id, &root);
        self.scm.probe_attempt.remove(&(id, root.clone()));
        self.scm_refresh(host, root, cx);
        cx.notify();
    }

    /// `git init` in `dir` on the host the pane is on, off the UI thread.
    ///
    /// Only an explicit click gets here. The landing acts on the host and
    /// directory captured at the click and on nothing the panel shows now, so
    /// a late answer after the pane moved elsewhere changes nothing about the
    /// new target. A refusal is reported and leaves every answer as it was.
    pub(crate) fn scm_init_repo(
        &mut self,
        host: SharedHost,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = dir.clone();
        let captured = host.clone();
        crate::ui::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |h| init_repo(h, &dir),
            move |this, result, window, cx| match result {
                Ok(root) => this.scm_initialized(&captured, &target, root, cx),
                Err(fact) => {
                    // Git's own words keep the toast policy every git failure
                    // has (severity prefix dropped, sentence case); the
                    // generated kinds are worded by the catalog as they are.
                    let text = match &fact {
                        FailureFact::Raw(raw) => crate::terminal::git_data::git_reason(raw),
                        generated => failure_text(generated),
                    };
                    let note = gpui_component::notification::Notification::error(text)
                        .title(t_fmt(L10nKey::GitOpFailed, &[("op", "init")]));
                    gpui_component::WindowExt::push_notification(window, note, cx);
                    cx.notify();
                }
            },
        );
    }

    /// A repository now exists in `dir` on the host that ran `git init`:
    /// forget the negative answers about exactly that directory — the panel's
    /// root lookup and the cheap per-tab cache's, including a probe still in
    /// flight — and mark the new root changed, so the panel, the Changes count
    /// and the cache each resolve it through their own producers. Nothing
    /// else is erased.
    ///
    /// Only for the same, still connected host: a `HostId` is stable across
    /// reconnects, so an entry for the id proves nothing, and being registered
    /// is not being connected. The registry must hold the very connection the
    /// click captured (`Arc::ptr_eq`) and that connection must still report
    /// `Host::is_connected`, or the caches would take answers about a machine
    /// that is not the one that was initialised.
    fn scm_initialized(
        &mut self,
        captured: &SharedHost,
        dir: &Path,
        root: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let id = captured.id();
        let still_here = crate::ui::host_registry::HostRegistry::get(cx, id)
            .is_some_and(|now| Arc::ptr_eq(&now, captured) && now.is_connected());
        if !still_here {
            return;
        }
        self.scm.roots.remove(&(id, dir.to_path_buf()));
        cx.default_global::<crate::terminal::git_status::GitStatusCache>()
            .forget_cwd(id, dir);
        if let Some(root) = root {
            let root = tty59_core::core::path_spelling::spelling_on_buf(id, &root);
            self.scm.probe_attempt.remove(&(id, root.clone()));
            self.scm_invalidate(id, &root, cx);
        }
        cx.notify();
    }

    /// Re-render when a probe lands.
    ///
    /// The subscription has to compare before it notifies. `scm_refresh`
    /// reaches for `ScmData` through `default_global`, which fires the global
    /// observers whether or not anything changed — and it is called from
    /// `render`. An unconditional `cx.notify()` here would therefore ask for a
    /// frame from inside a frame, forever.
    fn scm_watch_status(&mut self, cx: &mut Context<Self>) {
        if self.scm.watch.is_some() {
            return;
        }
        self.scm.watch = Some(cx.observe_global::<crate::terminal::git_data::ScmData>(
            |this, cx| {
                let Some((key, seen)) = this.scm.seen.clone() else {
                    return;
                };
                let now = status_read_of(cx, key.0, &key.1);
                if !now.same_identity(&seen) {
                    this.scm.seen = Some((key, now));
                    cx.notify();
                }
            },
        ));
    }

    /// Record the row a context menu just opened on, until it is dismissed.
    fn scm_menu_opened(
        &mut self,
        group: ScmGroup,
        path: String,
        menu: &gpui::Entity<PopupMenu>,
        cx: &mut Context<Self>,
    ) {
        self.scm.menu_row = Some((group, path));
        // Replacing the subscription drops the last menu's: only the menu
        // that is open now can clear the pin.
        self.scm.menu_sub = Some(
            cx.subscribe(menu, |this, _menu, _: &gpui::DismissEvent, cx| {
                this.scm.menu_row = None;
                cx.notify();
            }),
        );
    }

    fn scm_menu_row_is(&self, group: ScmGroup, entry: &StatusEntry) -> bool {
        self.scm
            .menu_row
            .as_ref()
            .is_some_and(|(g, path)| *g == group && path == entry.path.as_str())
    }

    /// A group's run of `len` rows, windowed to the panel's scroller at the
    /// rem this frame lays out against.
    fn scm_rows_window(
        &self,
        group: ScmGroup,
        len: usize,
        window: &Window,
        cx: &Context<Self>,
    ) -> RowsWindow {
        let row_h = rems(ROW_H / 16.).to_pixels(window.rem_size()).as_f32();
        RowsWindow::new(
            group,
            len,
            row_h,
            &self.scm.run_tops,
            &self.scm.scroll,
            cx.entity_id(),
            window,
        )
    }

    fn scm_groups(
        &mut self,
        repo: &RepoKey,
        status: &Arc<WorkingTreeStatus>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Groups are told apart by the pause between them, not by a rule:
        // 16px, and none above the first. Inside a group every row — header
        // included — sits 1px from the next, so two hovered neighbours read
        // as two fills rather than one long one.
        let query = self.scm_filter_query(cx);
        let tree = cx.global::<crate::core::config::Config>().scm_changes_tree;
        let mut matched = 0;
        let mut list = v_flex()
            .px(px(CONTENT_INSET))
            .pb(px(12.))
            .gap(px(GROUP_GAP));
        for group in ScmGroup::ORDER {
            // Filtered before anything else reads it, so the header's count
            // and its stage/discard-all act on what is on screen — a
            // "discard all" that also reached the files the filter hid would
            // be the one surprise this panel cannot take back.
            let entries: Vec<&StatusEntry> = status
                .entries
                .iter()
                .filter(|e| in_group(e, group))
                .filter(|e| {
                    query
                        .as_deref()
                        .is_none_or(|q| path_matches(e.path.as_str(), q))
                })
                .collect();
            if entries.is_empty() {
                continue;
            }
            matched += entries.len();
            let collapsed = self.scm.group_collapsed(group, entries.len());
            let mut block = v_flex()
                .gap(px(1.))
                .child(self.scm_group_header(repo, group, &entries, collapsed, cx));
            if !collapsed {
                let shown = entries.len().min(MAX_RENDERED_FILES);
                // Only the rows the viewport reaches are built; see
                // `rows_window`. Which rows exist is decided here as before.
                let run = if tree {
                    // A filter opens every directory: a match folded away
                    // under a closed row is a match the filter failed to show.
                    let filtering = query.is_some();
                    let folded = &self.scm.folded_dirs;
                    let rows = tree_rows(entries[..shown].iter().map(|e| e.path.as_str()), |key| {
                        !filtering && folded.contains(&(group, key.to_string()))
                    });
                    let pin = rows.iter().position(|row| {
                        matches!(row, TreeRow::File { index, .. }
                            if self.scm_menu_row_is(group, entries[*index]))
                    });
                    let run = self.scm_rows_window(group, rows.len(), window, cx).pin(pin);
                    let built = rows
                        .into_iter()
                        .enumerate()
                        .filter(|(i, _)| run.builds(*i))
                        .map(|(_, row)| match row {
                            TreeRow::Dir {
                                key,
                                label,
                                depth,
                                files,
                                collapsed,
                            } => self.scm_dir_row(
                                group, key, label, depth, files, collapsed, !filtering, cx,
                            ),
                            TreeRow::File { index, depth } => self.scm_file_row(
                                repo,
                                group,
                                entries[index],
                                Some(depth),
                                None,
                                cx,
                            ),
                        })
                        .collect();
                    run.finish(built)
                } else {
                    let pin = entries[..shown]
                        .iter()
                        .position(|e| self.scm_menu_row_is(group, e));
                    let run = self.scm_rows_window(group, shown, window, cx).pin(pin);
                    let built = run
                        .indices()
                        .map(|i| {
                            self.scm_file_row(repo, group, entries[i], None, Some((i, shown)), cx)
                        })
                        .collect();
                    run.finish(built)
                };
                block = block.child(run);
                if entries.len() > shown {
                    block = block.child(self.scm_note(
                        t_plural(L10nKey::PanelMoreChangedFiles, entries.len() - shown, &[]),
                        cx,
                    ));
                }
            }
            list = list.child(block);
        }
        if query.is_some() && matched == 0 {
            list = list.child(self.scm_note(t(L10nKey::ScmNoMatchingChanges).to_string(), cx));
        }
        if status.truncated {
            list = list.child(self.scm_note(
                t_fmt(
                    L10nKey::ScmTooManyChanges,
                    &[
                        ("shown", &status.entries.len().to_string()),
                        ("total", &status.total_entries.to_string()),
                    ],
                ),
                cx,
            ));
        }
        list.into_any_element()
    }

    /// The filter over the changed files, with the list/tree switch at its
    /// end.
    ///
    /// A standing field, like the Files panel's, rather than one behind a
    /// tile like the history's: the list is what this panel is for, and a
    /// run that touched sixty files is exactly when nobody wants to go
    /// looking for how to narrow it.
    fn scm_filter_row(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let input = match self.scm.filter.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| {
                    InputState::new(window, cx).placeholder(t(L10nKey::ScmFilterChanges))
                });
                // An `InputState` is its own entity; without this the list
                // would never hear about the text typed into it.
                self.scm.filter_sub =
                    Some(
                        cx.subscribe_in(&input, window, |_this, _input, ev, _window, cx| {
                            if matches!(ev, gpui_component::input::InputEvent::Change) {
                                cx.notify();
                            }
                        }),
                    );
                self.scm.filter = Some(input.clone());
                input
            }
        };
        let tree = cx.global::<crate::core::config::Config>().scm_changes_tree;
        // The tile shows where it goes, not where you are — the rows under it
        // already say which view this is.
        let (glyph, tip) = match tree {
            true => ("icons/list-flat.svg", t(L10nKey::ScmViewAsList)),
            false => ("icons/list-tree.svg", t(L10nKey::ScmViewAsTree)),
        };
        let toggle = crate::ui::tab_strip::chrome_tile_sized(
            Button::new("scm-changes-view").icon(Icon::empty().path(glyph)),
            crate::ui::app::TILE_SIZE_SM,
            crate::ui::app::TILE_GLYPH_SM,
            false,
            cx,
        )
        .rounded_md()
        .tooltip(tip)
        .on_click(cx.listener(|this, _, _window, cx| this.scm_toggle_changes_tree(cx)))
        .into_any_element();
        self.panel_search_with(&input, Some(toggle), cx)
    }

    /// The filter box's text, if it has any.
    fn scm_filter_query(&self, cx: &gpui::App) -> Option<String> {
        normalize_query(&self.scm.filter.as_ref()?.read(cx).value())
    }

    /// A directory in the tree view: a chevron in the status letters'
    /// column, the (possibly compacted) name, and how many changed files
    /// are under it.
    #[allow(clippy::too_many_arguments)]
    fn scm_dir_row(
        &self,
        group: ScmGroup,
        key: String,
        label: String,
        depth: usize,
        files: usize,
        collapsed: bool,
        foldable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<crate::ui::presets::Surfaces>().sidebar;
        let mono = cx.theme().mono_font_family.clone();
        let muted = cx.theme().muted_foreground;
        let id = SharedString::from(format!("scm-dir-{group:?}-{key}"));
        let selector = id.clone();
        #[cfg(test)]
        let chevron_selector = format!("scm-dir-chevron-{group:?}-{key}");
        let toggle = cx.listener({
            let key = key.clone();
            move |this, _, _window, cx| {
                this.scm.toggle_dir(group, key.clone());
                cx.notify();
            }
        });
        h_flex()
            .id(id)
            .role(gpui::Role::TreeItem)
            .aria_label(SharedString::from(label.clone()))
            .aria_expanded(!collapsed)
            .debug_selector(move || selector.to_string())
            .items_center()
            .gap(px(8.))
            .min_h(rems(ROW_H / 16.))
            .w_full()
            .min_w_0()
            .px(px(ROW_INSET))
            .pl(px(ROW_INSET + depth as f32 * TREE_INDENT))
            .rounded(px(6.))
            .when(foldable, |row| {
                row.cursor_pointer()
                    .hover(|s| s.bg(gpui::rgb(sf.hover)))
                    .on_click(toggle)
            })
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(key.clone()).build(window, cx)
            })
            .child({
                let cell = div()
                    .flex_none()
                    .w(px(BADGE_W))
                    .flex()
                    .justify_center()
                    .text_color(muted)
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .size(px(DISCLOSURE_CHEVRON)),
                    );
                #[cfg(test)]
                let cell = cell.debug_selector(move || chevron_selector);
                cell
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(crate::ui::right_panel::TEXT))
                    .text_color(gpui::rgb(sf.text_resting))
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(rems(META_MONO))
                    .font_family(mono)
                    .text_color(muted)
                    .child(files.to_string()),
            )
            .into_any_element()
    }

    /// "…and 40 more", and the note about a status git had to truncate.
    ///
    /// Secondary: it is prose about the list rather than a row of it, and the
    /// one thing it must not do is read as another file.
    fn scm_note(&self, text: String, cx: &mut Context<Self>) -> AnyElement {
        div()
            .px(px(ROW_INSET))
            .py(px(3.))
            .text_size(rems(META))
            .text_color(cx.theme().muted_foreground)
            .child(text)
            .into_any_element()
    }

    fn scm_group_header(
        &self,
        repo: &RepoKey,
        group: ScmGroup,
        entries: &[&StatusEntry],
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = entries.len();
        let sf = cx.global::<crate::ui::presets::Surfaces>().sidebar;
        let id = SharedString::from(format!("scm-group-{group:?}"));
        let actions = self.scm_group_actions(&id, repo, group, entries, sf.hover, cx);
        let selector = id.clone();
        h_flex()
            .id(id.clone())
            .debug_selector(move || selector.to_string())
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!(
                "{} {count}",
                t(group_label(group))
            )))
            .aria_expanded(!collapsed)
            .group(id)
            .relative()
            .items_center()
            .gap(px(6.))
            .min_h(rems(GROUP_HEADER_H / 16.))
            .px(px(ROW_INSET))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.scm_toggle_group(group, count, cx);
            }))
            // The chevron starts on the text column like the status letters
            // below it, in a box 2px narrower than their cell, so each arrow
            // stands over the `M`s and `U`s with its centre a pixel short of
            // theirs.
            .child({
                let cell = div()
                    .flex_none()
                    .w(px(GROUP_CHEVRON))
                    .flex()
                    .justify_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .size(px(GROUP_CHEVRON)),
                    );
                #[cfg(test)]
                let cell = cell.debug_selector(move || format!("scm-group-chevron-{group:?}"));
                cell
            })
            // Sentence case, medium, muted: a band label over the rows rather
            // than a heading competing with them.
            .child({
                let label = div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(GROUP_HEADING))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child(t(group_label(group)));
                #[cfg(test)]
                let label = label.debug_selector(move || format!("scm-group-label-{group:?}"));
                label
            })
            .child(
                div()
                    .flex_none()
                    .text_size(rems(GROUP_HEADING))
                    .text_color(cx.theme().muted_foreground)
                    .child(count.to_string()),
            )
            .child(actions)
            .into_any_element()
    }

    /// One changed file. `depth` is its level in the tree view, and `None` in
    /// the flat list — where the row carries its directory beside the name,
    /// since there is no directory row above it to say it.
    ///
    /// `set` is the row's index in the flat list and the list's length, so a
    /// screen reader can say "12 of 300" although only the rows on screen are
    /// built.
    fn scm_file_row(
        &self,
        repo: &RepoKey,
        group: ScmGroup,
        entry: &StatusEntry,
        depth: Option<usize>,
        set: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<crate::ui::presets::Surfaces>().sidebar;
        let mono = cx.theme().mono_font_family.clone();
        let path = entry.path.as_str().to_string();
        let (name, dir) = split_display_path(&path);
        let (letter, deco) = row_status(entry, group);
        let source = group_diff_source(group);
        let selected =
            self.diff_overlay_focus(repo.host, &repo.root, &source) == Some(path.as_str());
        let id = SharedString::from(format!("scm-row-{group:?}-{path}"));
        let actions = self.scm_row_actions(
            &id,
            repo,
            group,
            entry,
            if selected { sf.selected } else { sf.hover },
            cx,
        );

        let selector = id.clone();
        let hover_group = id.clone();
        #[cfg(test)]
        let dir_selector = id.clone();
        #[cfg(test)]
        let name_selector = format!("{id}-name");
        let strip_reserve = action_strip_reserve(row_verbs(group).len());
        h_flex()
            .id(id.clone())
            // The whole path and which list it is on: the letter drawn beside
            // it is a glyph, and the directory may be elided or not drawn.
            .role(gpui::Role::TreeItem)
            .aria_label(SharedString::from(format!(
                "{path}, {}",
                t(group_label(group))
            )))
            .aria_selected(selected)
            .when_some(set, |row, (index, size)| {
                row.aria_position_in_set(index + 1).aria_size_of_set(size)
            })
            .debug_selector(move || selector.to_string())
            .group(id)
            .relative()
            .items_center()
            .gap(px(8.))
            .min_h(rems(ROW_H / 16.))
            .w_full()
            .min_w_0()
            .px(px(ROW_INSET))
            .when_some(depth, |row, depth| {
                row.pl(px(ROW_INSET + depth as f32 * TREE_INDENT))
            })
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .when(selected, |s| s.bg(gpui::rgb(sf.selected)))
            .on_click({
                let repo = repo.clone();
                let path = path.clone();
                cx.listener(move |this, _, window, cx| {
                    this.open_diff_overlay(
                        repo.host,
                        repo.root.clone(),
                        source.clone(),
                        Some(path.clone()),
                        window,
                        cx,
                    );
                })
            })
            .tooltip({
                let full_path = path.clone();
                move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(full_path.clone()).build(window, cx)
                }
            })
            .context_menu({
                let app = cx.entity().downgrade();
                let repo = repo.clone();
                let entry = entry.clone();
                move |menu, _window, cx| {
                    // Built on open, outside render: the row is kept built
                    // while its menu is up, however far the list scrolls.
                    let popup = cx.entity();
                    let path = entry.path.as_str().to_string();
                    let _ =
                        app.update(cx, |this, cx| this.scm_menu_opened(group, path, &popup, cx));
                    Self::scm_row_context_menu(menu, &app, &repo, group, &entry, cx)
                }
            })
            .child(git_badge(letter, status_color(deco, cx), &mono))
            // Names use the same resting/selected hierarchy as sidebar rows.
            // The name takes its own width first and only then shrinks, down
            // to a floor; the directory beside it gets whatever is left.
            .child({
                let name = div()
                    .flex_shrink(1.)
                    .min_w(px(NAME_FLOOR))
                    .truncate()
                    .text_size(rems(crate::ui::right_panel::TEXT))
                    .text_color(if deco == DecoStatus::Conflict {
                        status_color(deco, cx)
                    } else if selected {
                        gpui::rgb(sf.text_selected).into()
                    } else {
                        gpui::rgb(sf.text_resting).into()
                    })
                    .when(selected, |s| s.font_weight(gpui::FontWeight::MEDIUM))
                    .when(deco == DecoStatus::Deleted, |s| s.line_through())
                    .child(name.to_string());
                #[cfg(test)]
                let name = name.debug_selector(move || name_selector);
                name
            })
            // The directory is flush right and gives way from its *start*:
            // the folder nearest the file is the part that says where it is,
            // and `src/ui/…` cut from the end keeps the part that says least.
            // The row tooltip has the whole path either way. A tree row has
            // its directory above it, so it carries none.
            .when(depth.is_none(), |this| {
                // Stateful, so the cell's hover state exists when layout asks
                // for the margin below: a bare `div` only refines paint style.
                // The id need only be unique under the row's own.
                let cell = div()
                    .id("dir")
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .text_right()
                    .text_size(rems(META))
                    .text_color(cx.theme().muted_foreground)
                    // The strip is drawn over the row's end, so on hover the
                    // cell gives up the strip's width and the ellipsis lands
                    // before it, not under it. A margin rather than padding:
                    // the cell's own box is what narrows.
                    .group_hover(hover_group, move |s| s.mr(px(strip_reserve)));
                #[cfg(test)]
                let cell = cell.debug_selector(move || format!("{dir_selector}-dir"));
                this.child(cell.child(dir.to_string()))
            })
            .child(actions)
            .into_any_element()
    }

    /// The buttons that appear over a hovered row.
    fn scm_row_actions(
        &self,
        row: &SharedString,
        repo: &RepoKey,
        group: ScmGroup,
        entry: &StatusEntry,
        backing: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A path git cannot be given is a path nothing can be done to. The
        // row stays readable and the buttons say why they are dead.
        let writable = entry.path.pathspec().is_some();
        let path = entry.path.as_str();
        let mut actions = action_strip(row, backing);

        for &(verb, ref icon) in row_verbs(group) {
            let id = SharedString::from(format!("scm-{verb:?}-{group:?}-{path}"));
            let repo = repo.clone();
            let entry = entry.clone();
            actions = actions.child(
                self.scm_tile(id, icon.clone(), verb_tooltip(verb), writable, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.scm_row_verb(verb, &repo, group, &entry, window, cx);
                    })),
            );
        }
        actions.into_any_element()
    }

    fn scm_group_actions(
        &self,
        row: &SharedString,
        repo: &RepoKey,
        group: ScmGroup,
        entries: &[&StatusEntry],
        backing: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let paths = writable_paths(entries);
        let mut actions = action_strip(row, backing);

        for &(verb, ref icon) in group_verbs(group) {
            let id = SharedString::from(format!("scm-all-{verb:?}-{group:?}"));
            let repo = repo.clone();
            let paths = paths.clone();
            actions = actions.child(
                self.scm_tile(
                    id,
                    icon.clone(),
                    verb_all_tooltip(verb),
                    !paths.is_empty(),
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    let Some(op) = verb_op(verb, group, paths.clone()) else {
                        return;
                    };
                    this.scm_op(repo.clone(), op, window, cx);
                })),
            );
        }
        actions.into_any_element()
    }

    /// A [`TILE_SIZE_XS`] tile. Smaller than `TILE_SIZE_SM`, because three of
    /// those on a row would eat 72 of the 236px a file name has to live in.
    fn scm_tile(
        &self,
        id: SharedString,
        icon: IconName,
        tooltip: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        crate::ui::tab_strip::chrome_tile_sized(
            Button::new(id).icon(Icon::new(icon.clone())),
            TILE_SIZE_XS,
            TILE_GLYPH_XS,
            false,
            cx,
        )
        .rounded(px(4.))
        .disabled(!enabled)
        .tooltip(if enabled {
            tooltip
        } else {
            t(L10nKey::ScmUnrepresentablePath)
        })
    }

    fn scm_row_verb(
        &mut self,
        verb: RowVerb,
        repo: &RepoKey,
        group: ScmGroup,
        entry: &StatusEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if verb == RowVerb::OpenConflict {
            self.open_diff_overlay(
                repo.host,
                repo.root.clone(),
                group_diff_source(group),
                Some(entry.path.as_str().to_string()),
                window,
                cx,
            );
            return;
        }
        let Some(op) = verb_op(verb, group, vec![entry.path.clone()]) else {
            return;
        };
        self.scm_op(repo.clone(), op, window, cx);
    }

    fn scm_row_context_menu(
        menu: PopupMenu,
        app: &gpui::WeakEntity<Self>,
        repo: &RepoKey,
        group: ScmGroup,
        entry: &StatusEntry,
        cx: &gpui::App,
    ) -> PopupMenu {
        let danger = cx.theme().danger;
        let rel = entry.path.as_str().to_string();
        let absolute = repo.root.join(&rel);
        let source = group_diff_source(group);
        let staged = group == ScmGroup::Staged;

        let mut menu = menu
            .min_w(px(200.))
            .item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextOpen)).on_click({
                    let app = app.clone();
                    let absolute = absolute.clone();
                    move |_, window, cx| {
                        let _ = app.update(cx, |this, cx| {
                            this.open_file_in_editor(&absolute, window, cx)
                        });
                    }
                }),
            )
            .item(PopupMenuItem::new(t(L10nKey::ScmOpenChanges)).on_click({
                let app = app.clone();
                let repo = repo.clone();
                let rel = rel.clone();
                move |_, window, cx| {
                    let _ = app.update(cx, |this, cx| {
                        this.open_diff_overlay(
                            repo.host,
                            repo.root.clone(),
                            source.clone(),
                            Some(rel.clone()),
                            window,
                            cx,
                        );
                    });
                }
            }))
            .separator()
            .item(
                PopupMenuItem::new(if staged {
                    t(L10nKey::ScmUnstage)
                } else {
                    t(L10nKey::ScmStage)
                })
                .on_click({
                    let app = app.clone();
                    let repo = repo.clone();
                    let paths = vec![entry.path.clone()];
                    move |_, window, cx| {
                        let op = if staged {
                            GitOp::Unstage {
                                paths: paths.clone(),
                            }
                        } else {
                            GitOp::Stage {
                                paths: paths.clone(),
                            }
                        };
                        let _ =
                            app.update(cx, |this, cx| this.scm_op(repo.clone(), op, window, cx));
                    }
                }),
            )
            .separator()
            .item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextCopyPath)).on_click({
                    // Same locality rule as Reveal below: a remote
                    // repository's paths belong to its own filesystem and
                    // are left spelled the way that machine spells them.
                    let text = match repo.host == HostId::LOCAL {
                        true => crate::ui::path_display::native_separators(&absolute)
                            .display()
                            .to_string(),
                        false => absolute.display().to_string(),
                    };
                    move |_, _window, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                    }
                }),
            );

        // Revealing a path only means anything on the machine the window is
        // running on; a remote repository's paths are not this filesystem's.
        if repo.host == HostId::LOCAL {
            menu = menu.item(
                PopupMenuItem::new(crate::ui::right_panel::reveal_label()).on_click({
                    let absolute = absolute.clone();
                    move |_, _window, cx| {
                        cx.reveal_path(&crate::ui::path_display::native_separators(&absolute))
                    }
                }),
            );
        }

        if let Some(op) = verb_op(RowVerb::Discard, group, vec![entry.path.clone()]) {
            menu = menu.separator().item(
                PopupMenuItem::element(move |_window, _cx| {
                    div().text_color(danger).child(t(L10nKey::ScmDiscard))
                })
                .on_click({
                    let app = app.clone();
                    let repo = repo.clone();
                    move |_, window, cx| {
                        let op = op.clone();
                        let _ =
                            app.update(cx, |this, cx| this.scm_op(repo.clone(), op, window, cx));
                    }
                }),
            );
        }
        menu
    }

    /// What `app.rs`'s `GitStatusCache` observer calls when the cheap
    /// per-tab probe lands.
    ///
    /// That probe runs at every command boundary, which makes it the best
    /// signal there is that the working tree moved — far better than a timer.
    /// The comparison in front of the bump is what keeps it from turning every
    /// notification (including the one the probe's *start* fires) into another
    /// `git status`.
    pub(crate) fn right_panel_refresh_changes(&mut self, cx: &mut Context<Self>) {
        // The count's repository is the current pane's, so it wins over the
        // panel's own, which is only as fresh as the last Changes render. With
        // the right panel closed there is no count target and the old answer
        // stands in: the epoch bump below is what lets the panel reopen on a
        // status that is not older than the last command.
        let (repo, cwd) = match self.scm.count_repo.clone() {
            Some(repo) => (repo, self.scm.count_cwd.clone()),
            None => match self.scm.repo.clone() {
                Some(repo) => (repo, None),
                None => return,
            },
        };
        // The cheap per-tab probe is keyed by the pane's directory, which is
        // not always the repository root.
        let Some(seen) = cx
            .try_global::<crate::terminal::git_status::GitStatusCache>()
            .and_then(|cache| cache.status_for(repo.host, cwd.as_deref().unwrap_or(&repo.root)))
        else {
            return;
        };
        // Compared as a pair: the same status under another repository is a
        // pane that moved, not a command that touched this one.
        if self
            .scm
            .last_tab_status
            .as_ref()
            .is_some_and(|(held, status)| *held == repo && *status == seen)
        {
            return;
        }
        self.scm.last_tab_status = Some((repo.clone(), seen));
        self.scm_invalidate(repo.host, &repo.root, cx);
        cx.notify();
    }

    /// What the Changes tab counts, for the repository of the current pane.
    ///
    /// Reads the status the watcher already keeps current (see
    /// `ScmWatcher::ChangesCount`): two hash lookups and an `Arc` clone, no
    /// scan of the entries and no git. `None` until that status has arrived,
    /// and for a clean tree.
    pub(crate) fn scm_changes_count(&self, cx: &gpui::App) -> Option<ChangesCount> {
        let repo = self.scm.count_repo.as_ref()?;
        let status = status_of(cx, repo.host, &repo.root)?;
        changes_count(status.as_ref())
    }

    fn scm_toggle_group(&mut self, group: ScmGroup, count: usize, cx: &mut Context<Self>) {
        let collapsed = self.scm.group_collapsed(group, count);
        self.scm.set_group_collapsed(group, !collapsed);
        cx.notify();
    }
}

/// The Changes tab's number, before it is worded.
///
/// It counts *files*: one per path git reports, which is
/// `WorkingTreeStatus::total_entries`. A partly staged file is listed under
/// both Staged and Changes but is one file here, and a conflict is one. The
/// list below can therefore show more rows than the tab says, and the tab's
/// tooltip says why. `total_entries` also keeps counting past the status
/// cap, so the figure is not the length of whatever subset was loaded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ChangesCount {
    /// An exact number of changed files, at least one.
    Files(usize),
    /// The status hit its record cap, so any figure would look more precise
    /// than it is.
    Capped,
}

impl ChangesCount {
    /// The largest number the tab draws; past it the tab reads `99+`.
    pub(crate) const SHOWN_MAX: usize = 99;
    /// Drawn in place of a number past [`Self::SHOWN_MAX`]. Not a word, so not
    /// in the catalog; the full figure is in [`Self::description`].
    pub(crate) const OVERFLOW: &'static str = "99+";

    /// The text beside the tab's label.
    pub(crate) fn label(self) -> String {
        match self {
            ChangesCount::Files(n) if n <= Self::SHOWN_MAX => n.to_string(),
            _ => Self::OVERFLOW.to_string(),
        }
    }

    /// The tab's tooltip and accessible name: the exact count, or that the
    /// cap was reached, and how partly staged files are counted.
    pub(crate) fn description(self) -> String {
        match self {
            ChangesCount::Files(n) => t_plural(L10nKey::PanelChangesTabCount, n, &[]),
            ChangesCount::Capped => t(L10nKey::PanelChangesTabCapped).to_string(),
        }
    }
}

/// The count a status gives the Changes tab; `None` for a clean tree.
pub(crate) fn changes_count(status: &WorkingTreeStatus) -> Option<ChangesCount> {
    use tty59_core::core::git::status::MAX_STATUS_ENTRIES;
    match status.total_entries {
        0 => None,
        // `truncated` is "more than the cap"; a status of exactly the cap is
        // just as likely to be the first `MAX_STATUS_ENTRIES` of more.
        n if status.truncated || n >= MAX_STATUS_ENTRIES => Some(ChangesCount::Capped),
        n => Some(ChangesCount::Files(n)),
    }
}

/// Which sections an entry shows up in.
///
/// A file can be staged and unstaged at once (`XY == "MM"`), and then it
/// belongs in both — the same thing VS Code shows, and the honest reading of
/// what a commit right now would contain.
pub(crate) fn in_group(entry: &StatusEntry, group: ScmGroup) -> bool {
    match group {
        ScmGroup::Merge => entry.is_conflicted(),
        ScmGroup::Untracked => entry.is_untracked(),
        ScmGroup::Staged => entry.is_staged(),
        ScmGroup::Changes => entry.is_unstaged() && !entry.is_untracked(),
    }
}

/// The letter and colour a row wears, decided by the half of `XY` its group
/// is about — a file staged as added and then modified is `A` under Staged
/// and `M` under Changes, which is what `git status` itself says.
pub(crate) fn row_status(entry: &StatusEntry, group: ScmGroup) -> (&'static str, DecoStatus) {
    let deco = match group {
        ScmGroup::Merge => DecoStatus::Conflict,
        ScmGroup::Untracked => DecoStatus::Untracked,
        ScmGroup::Staged => code_deco(entry.index),
        ScmGroup::Changes => code_deco(entry.worktree),
    };
    let letter = match group {
        // The two groups whose letter is fixed use the shared glyph table, so
        // a conflict is `U` here and in the file tree alike.
        ScmGroup::Merge | ScmGroup::Untracked => status_glyph(deco),
        ScmGroup::Staged => letter_of(entry.index),
        ScmGroup::Changes => letter_of(entry.worktree),
    };
    (letter, deco)
}

/// `ChangeCode::letter` returns a `char`; rows want a `&'static str` so the
/// badge never allocates. `T` and `C` keep their own letters rather than being
/// folded into `M` and `R` — git shows them, and they mean different things.
fn letter_of(code: ChangeCode) -> &'static str {
    match code {
        ChangeCode::None => " ",
        ChangeCode::Modified => "M",
        ChangeCode::TypeChanged => "T",
        ChangeCode::Added => "A",
        ChangeCode::Deleted => "D",
        ChangeCode::Renamed => "R",
        ChangeCode::Copied => "C",
        ChangeCode::Unmerged => "U",
    }
}

fn code_deco(code: ChangeCode) -> DecoStatus {
    match code {
        ChangeCode::Deleted => DecoStatus::Deleted,
        ChangeCode::Added => DecoStatus::Added,
        ChangeCode::Renamed | ChangeCode::Copied => DecoStatus::Renamed,
        ChangeCode::Unmerged => DecoStatus::Conflict,
        _ => DecoStatus::Modified,
    }
}

/// Which patch a row's click opens.
///
/// Staged rows show `git diff --cached`; everything else shows the working
/// tree. Getting this wrong is not cosmetic — the file name would be right and
/// the hunks underneath it would be someone else's.
pub(crate) fn group_diff_source(group: ScmGroup) -> DiffSource {
    match group {
        ScmGroup::Staged => DiffSource::Staged,
        ScmGroup::Merge | ScmGroup::Changes | ScmGroup::Untracked => DiffSource::Worktree,
    }
}

fn group_label(group: ScmGroup) -> L10nKey {
    match group {
        ScmGroup::Merge => L10nKey::ScmGroupMerge,
        ScmGroup::Staged => L10nKey::ScmGroupStaged,
        ScmGroup::Changes => L10nKey::ScmGroupChanges,
        ScmGroup::Untracked => L10nKey::ScmGroupUntracked,
    }
}

/// Whether a group nobody has touched starts folded.
pub(crate) fn starts_collapsed(group: ScmGroup, count: usize) -> bool {
    group == ScmGroup::Untracked && count > UNTRACKED_AUTO_COLLAPSE
}

/// What the branch row says where the branch name goes.
pub(crate) fn head_label(head: &HeadState) -> String {
    match head {
        // A detached HEAD has no name, so it wears its sha — shortened to the
        // seven characters git itself abbreviates to.
        HeadState::Detached { oid } => oid.chars().take(7).collect(),
        _ => head.label(),
    }
}

/// The numbers beside Pull and Push, as `(pull, push)`, from the status
/// snapshot's `(ahead, behind)`.
///
/// Pull is what is waiting upstream — behind — and Push what is waiting here —
/// ahead. `None` is unknown and zero is level, and neither earns a number.
fn branch_verb_counts(ahead_behind: Option<(u32, u32)>) -> (Option<u32>, Option<u32>) {
    match ahead_behind {
        Some((ahead, behind)) => (
            Some(behind).filter(|n| *n > 0),
            Some(ahead).filter(|n| *n > 0),
        ),
        None => (None, None),
    }
}

/// A branch-menu verb: its name, then — trailing, muted, in tabular figures —
/// how many commits it would move, or, under the name, why it is unavailable.
///
/// The number is a reading of the status snapshot, not a promise about the
/// network, so it sits in the menu's secondary ink beside the verb instead of
/// in its label.
fn verb_row(
    verb: &'static str,
    count: Option<u32>,
    reason: Option<&'static str>,
    cx: &gpui::App,
) -> impl IntoElement + use<> {
    let muted: gpui::Hsla = gpui::rgb(crate::ui::theme::chrome_ink(cx).unpicked_meta).into();
    let top = h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_3()
        .child(div().child(verb))
        .when_some(count, |row, n| {
            let count_el = div()
                .text_color(muted)
                .font_features(crate::ui::theme::tabular_figures())
                .child(n.to_string());
            #[cfg(test)]
            let count_el = count_el.debug_selector(move || format!("scm-menu-count:{verb}:{n}"));
            row.child(count_el)
        });
    v_flex()
        .w_full()
        .child(top)
        .when_some(reason, |column, why| {
            let note = div().text_size(rems(META)).text_color(muted).child(why);
            #[cfg(test)]
            let note = note.debug_selector(move || format!("scm-menu-reason:{why}"));
            column.child(note)
        })
}

/// The folder a repository lives in, as the menu spells it: from `~` when
/// that is inside `home`, and empty for a root that has no parent.
fn parent_label(root: &Path, home: Option<&Path>) -> String {
    root.parent()
        .map(|parent| {
            crate::ui::path_display::abbreviate_home(&parent.to_string_lossy(), home).into_owned()
        })
        .unwrap_or_default()
}

/// A repository choice's name and, beside it, the folder it lives in.
///
/// Two checkouts of one project share a name, so the name alone cannot tell
/// them apart. The folder is spelled from the stored root — nothing is
/// canonicalised or read from disk — and starts from `~` where that host's
/// home is known. Only what is drawn is made of this; which repository a row
/// selects is its `RepoKey`.
fn repo_choice_label(repo: &RepoKey, cx: &gpui::App) -> (SharedString, SharedString) {
    let name = repo
        .root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| repo.root.display().to_string());
    let home = crate::ui::path_display::home_for_host(cx, repo.host);
    let parent = parent_label(&repo.root, home.as_deref());
    (name.into(), parent.into())
}

/// The `↑2 ↓0` reading, or nothing.
///
/// A branch that is level with its upstream says nothing at all: the quiet
/// state is the common one, and a token that is always there stops being read.
/// A branch with no upstream offers to publish instead — but only if there is
/// a branch to publish. A detached or unborn HEAD has no upstream *by
/// definition*, and the offer there is one the sync tile already refuses
/// (#545); it was the widest token on the row while naming the one thing that
/// could not happen.
///
/// Once there *is* something to say, both halves are said, zero included. The
/// pair is one reading of one distance, and dropping the empty half makes it
/// change shape as the numbers move — `↑2` becoming `↑2 ↓1` on a fetch is a
/// different-looking thing in the corner of the eye, where `↑2 ↓0` becoming
/// `↑2 ↓1` is the same thing with a digit changed.
pub(crate) fn tracking_chip(
    upstream: Option<&str>,
    ahead_behind: Option<(u32, u32)>,
    pushable: bool,
) -> Option<String> {
    if upstream.is_none() {
        return pushable.then(|| t(L10nKey::ScmPublishBranch).to_string());
    }
    match ahead_behind? {
        (0, 0) => None,
        (ahead, behind) => Some(format!("↑{ahead} ↓{behind}")),
    }
}

/// A quiet token on the branch row: the tracking distance, or the count of
/// other repositories.
///
/// Not `info_chip`. A chip's fill is what earns it the right to shout, and the
/// only two things on this row worth shouting are a state you could forget you
/// are in — detached, mid-rebase, armed to amend — and they already wear the
/// warning tint. `↑2 ↓0` is a reading, so it reads as text; the row's own 6px
/// gap is enough to keep it off the branch name without a box around it.
///
/// The same quiet mono step the panel's badges and chips are set on, minus the
/// fill, so a note and a chip still line up as one row of tokens.
/// The fill under the commit area's two shapes — the message box and the split
/// button's frame.
///
/// Half of the ramp's first rung, not the rung itself. `hover` is what a row
/// wears while the pointer is on it, which is a state that lasts a moment;
/// these two wear their fill permanently, and at full strength they were the
/// loudest thing on an idle panel. Both read from here so they cannot drift
/// apart: the commit area is one block, and two greys a shade off each other
/// look like a mistake rather than a hierarchy.
fn field_fill(sf: crate::ui::presets::Surface) -> u32 {
    crate::ui::presets::mix(sf.base, sf.hover, 0.5)
}

/// One half of the split commit control: paints nothing, ever.
///
/// Every state is transparent so the frame around both halves is the only
/// thing that fills, and the control lights as one shape rather than one end.
/// Two of those states are worth naming, because a stock variant gets them
/// wrong here in opposite directions:
///
/// * `.ghost()` hovers to `sidebar_accent`, which sits close enough to the
///   frame's own fill that the half would light almost invisibly — the worst of
///   both, a highlight you can see is uneven but cannot read.
/// * gpui-component resolves a `Custom` variant's *selected* paint from its
///   `active` slot, and a dropdown marks its trigger selected for as long as
///   the menu is open. Anything but transparent there parks a block on the
///   chevron for the whole time the user is reading the menu.
fn commit_half(ink: gpui::Hsla, cx: &gpui::App) -> ButtonCustomVariant {
    let clear = gpui::transparent_black();
    ButtonCustomVariant::new(cx)
        .color(clear)
        .foreground(ink)
        .hover(clear)
        .active(clear)
}

/// How the split commit control is painted.
#[derive(Clone, Copy)]
struct CommitPaint {
    fill: gpui::Hsla,
    hover: gpui::Hsla,
    ink: gpui::Hsla,
    seam: gpui::Hsla,
}

/// Committable: an inverted neutral — the panel's ink as the fill and its own
/// surface as the label, so it is the one solid shape in a panel of rows, and
/// it stays neutral rather than borrowing the accent the sidebar and focus
/// rings already spend. Not committable: the message box's faint fill and the
/// panel's ink, a control that is there but is not asking for anything.
///
/// The label ink is the opaque surface rather than `theme.background`, which
/// carries the window's transparency when one is configured.
fn commit_paint(live: bool, cx: &gpui::App) -> CommitPaint {
    let sf = cx.global::<crate::ui::presets::Surfaces>().sidebar;
    let fg = cx.theme().foreground;
    match live {
        true => {
            let ink: gpui::Hsla = gpui::rgb(sf.base).into();
            CommitPaint {
                fill: fg,
                hover: fg.blend(ink.opacity(0.14)),
                ink,
                seam: ink.opacity(0.24),
            }
        }
        false => CommitPaint {
            fill: gpui::rgb(field_fill(sf)).into(),
            hover: gpui::rgb(sf.hover).into(),
            ink: fg,
            seam: fg.opacity(0.18),
        },
    }
}

fn branch_note(text: &str, ink: gpui::Hsla, mono: &SharedString) -> AnyElement {
    div()
        .flex_none()
        .text_size(rems(META_MONO))
        .font_family(mono.clone())
        .text_color(ink)
        .child(text.to_string())
        .into_any_element()
}

/// Which sequencer operation is parked in the repository.
///
/// `RebaseInteractive` reads as "rebasing" on purpose: modern git writes
/// `rebase-merge/interactive` for every rebase, so the distinction the variant
/// name suggests is not one the repository on disk can actually make — and
/// `git status` does not draw it either.
pub(crate) fn operation_label(op: RepoOperation) -> L10nKey {
    match op {
        RepoOperation::Merge => L10nKey::ScmOpMerge,
        RepoOperation::Rebase | RepoOperation::RebaseInteractive => L10nKey::ScmOpRebase,
        RepoOperation::CherryPick => L10nKey::ScmOpCherryPick,
        RepoOperation::Revert => L10nKey::ScmOpRevert,
        RepoOperation::Bisect => L10nKey::ScmOpBisect,
        RepoOperation::Am => L10nKey::ScmOpAm,
    }
}

/// What the commit button says, and whether it can be pressed at all.
pub(crate) struct CommitPlan {
    pub(crate) label: L10nKey,
    pub(crate) enabled: bool,
    /// Why the button is disabled, when it is. "Nothing to commit" and "write
    /// a message" call for opposite actions, and one tooltip for both sends
    /// the user staging files they already staged.
    pub(crate) reason: L10nKey,
    /// The unresolved `U` paths — the blocker the conflict reason names and
    /// the count line under the box reports, so the two never disagree.
    pub(crate) conflicts: usize,
}

impl CommitPlan {
    /// The reason with its count filled in where it carries one.
    pub(crate) fn reason_text(&self) -> String {
        if self.conflicts > 0 {
            t_plural(self.reason, self.conflicts, &[])
        } else {
            t(self.reason).to_string()
        }
    }
}

/// Decide both from the state of the index.
///
/// "Commit All" rather than a silent `-a`: with nothing staged, `git commit`
/// would commit nothing, and the honest thing is to say on the button that
/// every tracked change is about to go in. An armed amend needs no message —
/// `--no-edit` keeps the one that is already on HEAD.
pub(crate) fn commit_plan(status: &WorkingTreeStatus, amend: bool, message: &str) -> CommitPlan {
    // Given only the status there is no cheaper answer than walking the
    // entries — fine for a click or a test, and the panel's per-frame render
    // goes through `commit_plan_with_conflicts` with the refresh's own count
    // instead.
    commit_plan_with_conflicts(status, amend, message, Some(status.conflicts().count()))
}

/// The decision itself, with the conflict count supplied so a per-frame
/// caller can hand over the number the refresh already counted. `None` means
/// that count does not exist yet (the index has not landed): the button
/// stays disabled with "loading" rather than guess zero.
fn commit_plan_with_conflicts(
    status: &WorkingTreeStatus,
    amend: bool,
    message: &str,
    conflicts: Option<usize>,
) -> CommitPlan {
    let staged = status.staged().next().is_some();
    let tracked_edits = status.unstaged().next().is_some();
    // `git commit` refuses outright while any path is still unmerged, whatever
    // the message, the index or the merge's own `--allow-empty-message` say —
    // so the button and the action paths gate on this before anything else.
    let conflicts_known = conflicts.is_some();
    let conflicts = conflicts.unwrap_or(0);
    let label = if amend {
        L10nKey::ScmCommitAmendButton
    } else if staged {
        L10nKey::ScmCommitButton
    } else {
        L10nKey::ScmCommitAllButton
    };
    let has_message = !message.trim().is_empty();
    // Mid-merge, an empty message is still committable: `ops` sends
    // `--allow-empty-message` for exactly this, because refusing would strand
    // the merge behind a message the user deliberately cleared.
    let merging = matches!(status.operation, Some(RepoOperation::Merge));
    let something = staged || tracked_edits || amend;
    CommitPlan {
        label,
        enabled: conflicts_known
            && something
            && (has_message || amend || merging)
            && conflicts == 0,
        // The conflicts outrank the other excuses: there is work, and a
        // written message does not help — the `U` paths are what git refuses on.
        reason: if !conflicts_known {
            L10nKey::PanelLoading
        } else if conflicts > 0 {
            L10nKey::ScmCommitHasConflicts
        } else if something {
            L10nKey::ScmCommitNeedsMessage
        } else {
            L10nKey::ScmNothingToCommit
        },
        conflicts,
    }
}

/// Whether a commit has to stage everything tracked first (`-a`).
pub(crate) fn commit_stages_everything(status: &WorkingTreeStatus, amend: bool) -> bool {
    // Amending with nothing staged means "fix the message", not "sweep the
    // working tree into the commit I already made".
    !amend && status.staged().next().is_none()
}

/// The branch a push would move, or why there is no push to make.
///
/// Both dead ends used to die in the same silent `let-else` in `scm_push`
/// (#545): a detached HEAD has no branch to push — and pushing a bare sha
/// needs a refspec the panel has no way to ask for — while an unborn branch
/// has a name but no commits to send yet. The key binding, the palette and
/// the follow-up half of "Commit and Push" all land in that guard, so the
/// reason is a key the caller can toast, not a log line.
pub(crate) fn pushable_branch(head: &HeadState) -> Result<&str, L10nKey> {
    match head {
        HeadState::Branch { name, .. } => Ok(name),
        HeadState::Detached { .. } => Err(L10nKey::ScmPushDetached),
        HeadState::Unborn { .. } => Err(L10nKey::ScmPushNoCommits),
    }
}

/// What a row's buttons do. Named rather than inlined because the same verb
/// appears on the row, on its group header and in its context menu, and the
/// three must not drift into meaning different things.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RowVerb {
    Discard,
    Stage,
    Unstage,
    OpenConflict,
    MarkResolved,
}

/// Right to left, most-used last: the pointer travels to the right edge, so
/// the button under it should be the one nine hovers out of ten want.
pub(crate) fn row_verbs(group: ScmGroup) -> &'static [(RowVerb, IconName)] {
    match group {
        ScmGroup::Merge => &[
            (RowVerb::OpenConflict, IconName::Eye),
            (RowVerb::MarkResolved, IconName::Check),
        ],
        ScmGroup::Staged => &[(RowVerb::Unstage, IconName::Minus)],
        ScmGroup::Changes | ScmGroup::Untracked => &[
            (RowVerb::Discard, IconName::Undo2),
            (RowVerb::Stage, IconName::Plus),
        ],
    }
}

/// The header's buttons are the row's, minus the ones that only make sense
/// for one file: there is no group-wide "open the conflict".
pub(crate) fn group_verbs(group: ScmGroup) -> &'static [(RowVerb, IconName)] {
    match group {
        ScmGroup::Merge => &[(RowVerb::MarkResolved, IconName::Check)],
        _ => row_verbs(group),
    }
}

/// The operation a verb runs over one or many paths. `None` for the verbs
/// that change no state.
pub(crate) fn verb_op(verb: RowVerb, group: ScmGroup, paths: Vec<RepoPath>) -> Option<GitOp> {
    if paths.is_empty() {
        return None;
    }
    Some(match verb {
        // Resolving a conflict is `git add`, exactly as it is on the command
        // line — there is no separate "resolve" verb in git.
        RowVerb::Stage | RowVerb::MarkResolved => GitOp::Stage { paths },
        RowVerb::Unstage => GitOp::Unstage { paths },
        RowVerb::Discard if group == ScmGroup::Untracked => {
            let directories = paths.iter().any(|p| p.as_str().ends_with('/'));
            GitOp::DiscardUntracked { paths, directories }
        }
        RowVerb::Discard => GitOp::DiscardWorktree { paths },
        RowVerb::OpenConflict => return None,
    })
}

/// The paths in a group git can actually be told about.
///
/// A path that is not valid UTF-8 cannot be sent as a pathspec at all, and
/// `GitOp::validate` rejects the whole operation over one of them — so a group
/// action has to leave them out rather than fail for everyone. Their own rows
/// are greyed out and say why.
pub(crate) fn writable_paths(entries: &[&StatusEntry]) -> Vec<RepoPath> {
    entries
        .iter()
        .filter(|e| e.path.pathspec().is_some())
        .map(|e| e.path.clone())
        .collect()
}

fn verb_tooltip(verb: RowVerb) -> &'static str {
    t(match verb {
        RowVerb::Discard => L10nKey::ScmDiscard,
        RowVerb::Stage => L10nKey::ScmStage,
        RowVerb::Unstage => L10nKey::ScmUnstage,
        RowVerb::OpenConflict => L10nKey::ScmOpenConflict,
        RowVerb::MarkResolved => L10nKey::ScmMarkResolved,
    })
}

fn verb_all_tooltip(verb: RowVerb) -> &'static str {
    t(match verb {
        RowVerb::Discard => L10nKey::ScmDiscardAll,
        RowVerb::Stage => L10nKey::ScmStageAll,
        RowVerb::Unstage => L10nKey::ScmUnstageAll,
        RowVerb::OpenConflict => L10nKey::ScmOpenConflict,
        RowVerb::MarkResolved => L10nKey::ScmMarkResolved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::actions::{ScmCommit, ToggleFullscreen};
    use crate::core::config::{CoreConfig, DiffViewMode, RightPanelTab};
    use crate::ui::app::test_window::harness;
    use gpui::TestAppContext;
    use tty59_core::core::git::status::{ConflictKind, EntryKind, RepoPath};

    fn entry(path: &str, index: ChangeCode, worktree: ChangeCode, kind: EntryKind) -> StatusEntry {
        StatusEntry {
            path: RepoPath::from_bytes(path.as_bytes()),
            orig_path: None,
            index,
            worktree,
            kind,
            submodule: None,
            rename_score: None,
            conflict: (kind == EntryKind::Unmerged).then_some(ConflictKind::BothModified),
        }
    }

    fn groups_of(entry: &StatusEntry) -> Vec<ScmGroup> {
        ScmGroup::ORDER
            .into_iter()
            .filter(|g| in_group(entry, *g))
            .collect()
    }

    #[test]
    fn a_file_staged_and_edited_again_lands_in_both_groups() {
        let e = entry(
            "a.rs",
            ChangeCode::Modified,
            ChangeCode::Modified,
            EntryKind::Tracked,
        );
        assert_eq!(groups_of(&e), vec![ScmGroup::Staged, ScmGroup::Changes]);
    }

    #[test]
    fn each_other_kind_of_entry_lands_in_exactly_one_group() {
        let staged = entry(
            "a.rs",
            ChangeCode::Added,
            ChangeCode::None,
            EntryKind::Tracked,
        );
        assert_eq!(groups_of(&staged), vec![ScmGroup::Staged]);

        let unstaged = entry(
            "b.rs",
            ChangeCode::None,
            ChangeCode::Modified,
            EntryKind::Tracked,
        );
        assert_eq!(groups_of(&unstaged), vec![ScmGroup::Changes]);

        let untracked = entry(
            "c.rs",
            ChangeCode::None,
            ChangeCode::None,
            EntryKind::Untracked,
        );
        assert_eq!(groups_of(&untracked), vec![ScmGroup::Untracked]);

        // A conflict is only ever a conflict: it must not also show up under
        // Changes, or resolving it would look like two separate jobs.
        let conflict = entry(
            "d.rs",
            ChangeCode::Unmerged,
            ChangeCode::Unmerged,
            EntryKind::Unmerged,
        );
        assert_eq!(groups_of(&conflict), vec![ScmGroup::Merge]);
    }

    #[test]
    fn a_row_wears_the_letter_of_the_half_its_group_is_about() {
        // Added to the index, then edited again in the working tree.
        let e = entry(
            "a.rs",
            ChangeCode::Added,
            ChangeCode::Modified,
            EntryKind::Tracked,
        );
        assert_eq!(row_status(&e, ScmGroup::Staged), ("A", DecoStatus::Added));
        assert_eq!(
            row_status(&e, ScmGroup::Changes),
            ("M", DecoStatus::Modified)
        );

        let untracked = entry(
            "c.rs",
            ChangeCode::None,
            ChangeCode::None,
            EntryKind::Untracked,
        );
        assert_eq!(
            row_status(&untracked, ScmGroup::Untracked),
            ("?", DecoStatus::Untracked)
        );
        let conflict = entry(
            "d.rs",
            ChangeCode::Unmerged,
            ChangeCode::Unmerged,
            EntryKind::Unmerged,
        );
        assert_eq!(
            row_status(&conflict, ScmGroup::Merge),
            ("U", DecoStatus::Conflict)
        );
    }

    #[test]
    fn staged_rows_open_the_cached_diff_and_the_rest_the_working_tree() {
        assert_eq!(group_diff_source(ScmGroup::Staged), DiffSource::Staged);
        for group in [ScmGroup::Merge, ScmGroup::Changes, ScmGroup::Untracked] {
            assert_eq!(group_diff_source(group), DiffSource::Worktree);
        }
    }

    #[test]
    fn only_a_long_untracked_list_starts_folded() {
        assert!(!starts_collapsed(
            ScmGroup::Untracked,
            UNTRACKED_AUTO_COLLAPSE
        ));
        assert!(starts_collapsed(
            ScmGroup::Untracked,
            UNTRACKED_AUTO_COLLAPSE + 1
        ));
        for group in [ScmGroup::Merge, ScmGroup::Staged, ScmGroup::Changes] {
            assert!(!starts_collapsed(group, 1_000));
        }
    }

    #[test]
    fn every_button_a_row_offers_maps_to_the_verb_it_is_named_for() {
        let file = vec![RepoPath::from_bytes(b"a.rs")];
        assert!(matches!(
            verb_op(RowVerb::Stage, ScmGroup::Changes, file.clone()),
            Some(GitOp::Stage { .. })
        ));
        assert!(matches!(
            verb_op(RowVerb::Unstage, ScmGroup::Staged, file.clone()),
            Some(GitOp::Unstage { .. })
        ));
        // Resolving a conflict is `git add`; git has no other verb for it.
        assert!(matches!(
            verb_op(RowVerb::MarkResolved, ScmGroup::Merge, file.clone()),
            Some(GitOp::Stage { .. })
        ));
        assert!(matches!(
            verb_op(RowVerb::Discard, ScmGroup::Changes, file.clone()),
            Some(GitOp::DiscardWorktree { .. })
        ));
        // `checkout --` cannot restore a file git has never heard of.
        assert!(matches!(
            verb_op(RowVerb::Discard, ScmGroup::Untracked, file.clone()),
            Some(GitOp::DiscardUntracked {
                directories: false,
                ..
            })
        ));
        assert!(matches!(
            verb_op(
                RowVerb::Discard,
                ScmGroup::Untracked,
                vec![RepoPath::from_bytes(b"vendor/")]
            ),
            Some(GitOp::DiscardUntracked {
                directories: true,
                ..
            })
        ));
        assert!(verb_op(RowVerb::OpenConflict, ScmGroup::Merge, file).is_none());
        assert!(verb_op(RowVerb::Stage, ScmGroup::Changes, Vec::new()).is_none());
    }

    #[test]
    fn everything_that_can_lose_work_says_so_before_it_runs() {
        // The gate in `scm_op` keys off `destructive()`. If one of these ever
        // stopped reporting, the panel would throw the work away in silence.
        let paths = vec![RepoPath::from_bytes(b"a.rs")];
        for group in [ScmGroup::Changes, ScmGroup::Untracked] {
            let op = verb_op(RowVerb::Discard, group, paths.clone()).expect("discard has an op");
            assert!(op.destructive().is_some(), "{group:?} discard");
        }
        // Staging and unstaging are reversible, so they must not stop to ask.
        for verb in [RowVerb::Stage, RowVerb::Unstage, RowVerb::MarkResolved] {
            let op = verb_op(verb, ScmGroup::Changes, paths.clone()).expect("verb has an op");
            assert!(op.destructive().is_none(), "{verb:?}");
        }
    }

    #[test]
    fn a_group_action_leaves_out_the_paths_git_cannot_be_told_about() {
        let good = entry(
            "a.rs",
            ChangeCode::None,
            ChangeCode::Modified,
            EntryKind::Tracked,
        );
        let mut bad = good.clone();
        bad.path = RepoPath::from_bytes(&[0xff, 0xfe, b'.', b'r', b's']);
        assert!(bad.path.pathspec().is_none(), "the fixture must be lossy");

        let paths = writable_paths(&[&good, &bad]);
        assert_eq!(paths.len(), 1, "the lossy path is dropped, not carried");
        // One unrepresentable path would otherwise fail the operation for the
        // whole group.
        let op = verb_op(RowVerb::Stage, ScmGroup::Changes, paths).expect("still has work to do");
        assert!(op.validate().is_ok());

        let all_bad = verb_op(RowVerb::Stage, ScmGroup::Changes, writable_paths(&[&bad]));
        assert!(all_bad.is_none(), "nothing to do means no operation at all");
    }

    #[test]
    fn a_group_header_offers_no_button_that_only_makes_sense_for_one_file() {
        let verbs: Vec<RowVerb> = group_verbs(ScmGroup::Merge)
            .iter()
            .map(|(v, _)| *v)
            .collect();
        assert_eq!(verbs, vec![RowVerb::MarkResolved]);
        for group in [ScmGroup::Staged, ScmGroup::Changes, ScmGroup::Untracked] {
            for (verb, _) in group_verbs(group) {
                assert_ne!(*verb, RowVerb::OpenConflict);
            }
        }
    }

    /// No type size in this panel is a number.
    ///
    /// The Source Control tab spent its whole life one step under the rest of
    /// the right panel, and every part of how that happened was quiet. It was
    /// branched from main hours before the interface font scale landed and
    /// copied the ramp as it stood; the commit that moved every other file onto
    /// rems could not touch these, because they were not on main yet; and the
    /// merge that brought the two together had nothing to conflict over,
    /// because the two sides had edited different files. Three sizes frozen at
    /// 12/11/10.5 while their neighbours went to 14/13/12/11 and started
    /// tracking `ui_font_size`, and the only thing that could have caught it
    /// was somebody looking at two tabs and noticing.
    ///
    /// So: a size the reader can *read* is `rems(TEXT)` and its four siblings,
    /// and a `text_size(px(…))` anywhere under `scm/` is either a mistake or a
    /// value that should have been one of them. `px` still belongs on the
    /// things type sits inside — row heights, tiles, the marks — and this only
    /// looks at `text_size`.
    ///
    /// Reading the source is the point. Any assertion over the constants would
    /// only prove the constants agree with themselves; what went wrong was a
    /// literal at a call site, and a literal at a call site is what this reads.
    #[test]
    fn the_panels_type_is_named_rather_than_numbered() {
        // Assembled rather than written, because this test reads the file it is
        // written in and a spelled-out needle would find itself.
        let needle = concat!("text_size", "(px(");
        for (file, src) in [
            ("panel.rs", include_str!("panel.rs")),
            ("graph.rs", include_str!("graph.rs")),
            ("detail.rs", include_str!("detail.rs")),
        ] {
            let offenders: Vec<&str> = src
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with("//"))
                .filter(|line| line.contains(needle))
                .collect();
            assert!(
                offenders.is_empty(),
                "{file} sets type in pixels, which is how the panel fell a step \
                 behind the rest of the window last time: {offenders:?}"
            );
        }
    }

    fn repo(root: &str) -> RepoKey {
        RepoKey {
            host: HostId::LOCAL,
            root: PathBuf::from(root),
        }
    }

    fn status_of_repo(root: &str, entries: Vec<StatusEntry>) -> WorkingTreeStatus {
        WorkingTreeStatus {
            root: PathBuf::from(root),
            home: PathBuf::from(root),
            head: HeadState::Branch {
                name: "main".into(),
                oid: "1111111".into(),
            },
            upstream: None,
            ahead_behind: None,
            total_entries: entries.len(),
            entries,
            truncated: false,
            stash_count: 0,
            operation: None,
            prefilled_message: None,
        }
    }

    /// The tab counts files; the list counts rows. A partly staged file is
    /// two rows (Staged and Changes) and one file, a conflict is one of each.
    /// The oracle for the rows is the grouping the panel itself draws.
    #[test]
    fn the_changes_count_is_files_and_a_partly_staged_file_counts_once() {
        let entries = vec![
            // Staged, then edited again: under both Staged and Changes.
            entry(
                "both.rs",
                ChangeCode::Modified,
                ChangeCode::Modified,
                EntryKind::Tracked,
            ),
            entry(
                "staged.rs",
                ChangeCode::Added,
                ChangeCode::None,
                EntryKind::Tracked,
            ),
            entry(
                "new.rs",
                ChangeCode::None,
                ChangeCode::None,
                EntryKind::Untracked,
            ),
            entry(
                "clash.rs",
                ChangeCode::Unmerged,
                ChangeCode::Unmerged,
                EntryKind::Unmerged,
            ),
        ];
        let rows: usize = entries.iter().map(|e| groups_of(e).len()).sum();
        assert_eq!(rows, 5, "both.rs is listed twice, every other path once");
        assert!(
            groups_of(&entries[3]) == vec![ScmGroup::Merge],
            "a conflict is in the merge group only, never in a second one"
        );

        let status = status_of_repo("/a", entries);
        assert_eq!(changes_count(&status), Some(ChangesCount::Files(4)));
    }

    #[test]
    fn a_clean_tree_has_no_count() {
        assert_eq!(changes_count(&status_of_repo("/a", Vec::new())), None);
    }

    /// The count is `total_entries`, which keeps counting past the cap, so
    /// the loaded subset is never mistaken for the total; and at or over the
    /// cap it stops pretending to be exact.
    #[test]
    fn the_count_stops_being_exact_at_the_status_cap() {
        use tty59_core::core::git::status::MAX_STATUS_ENTRIES;
        let mut status = status_of_repo("/a", Vec::new());

        status.total_entries = MAX_STATUS_ENTRIES - 1;
        assert_eq!(
            changes_count(&status),
            Some(ChangesCount::Files(MAX_STATUS_ENTRIES - 1))
        );
        // Exactly the cap could be the first `MAX_STATUS_ENTRIES` of more.
        status.total_entries = MAX_STATUS_ENTRIES;
        assert_eq!(changes_count(&status), Some(ChangesCount::Capped));
        // Past it the entries are dropped but the total is not.
        status.total_entries = MAX_STATUS_ENTRIES + 4_000;
        status.truncated = true;
        assert_eq!(changes_count(&status), Some(ChangesCount::Capped));
    }

    #[test]
    fn the_tab_reads_99_plus_past_99_and_the_tooltip_keeps_the_figure() {
        assert_eq!(ChangesCount::Files(1).label(), "1");
        assert_eq!(ChangesCount::Files(99).label(), "99");
        assert_eq!(ChangesCount::Files(100).label(), "99+");
        assert_eq!(ChangesCount::Capped.label(), "99+");

        crate::ui::i18n::set_locale("en");
        let one = ChangesCount::Files(1).description();
        assert!(
            one.contains("1 changed file") && !one.contains("1 changed files"),
            "{one}"
        );
        let many = ChangesCount::Files(250).description();
        assert!(many.contains("250 changed files"), "{many}");
        assert!(
            ChangesCount::Capped.description().contains("10,000+"),
            "the cap is named, not a precise-looking total"
        );
        for text in [&one, &many, &ChangesCount::Capped.description()] {
            assert!(!text.contains("{count}"), "{text}");
        }
    }

    /// Pull is what is behind, Push what is ahead. 3 ahead and 1 behind is the
    /// audit's own example, so a swapped pair cannot pass.
    #[test]
    fn pull_counts_what_is_behind_and_push_what_is_ahead() {
        assert_eq!(branch_verb_counts(Some((3, 1))), (Some(1), Some(3)));
        assert_eq!(branch_verb_counts(Some((0, 2))), (Some(2), None));
        assert_eq!(branch_verb_counts(Some((5, 0))), (None, Some(5)));
        assert_eq!(branch_verb_counts(Some((0, 0))), (None, None), "level");
        assert_eq!(
            branch_verb_counts(None),
            (None, None),
            "unknown is not level"
        );
    }

    /// Two checkouts of one project are told apart by where they live, from
    /// the stored path alone.
    #[test]
    fn same_name_repositories_are_told_apart_by_their_folder() {
        let home = Path::new("/home/demo");
        let a = parent_label(Path::new("/home/demo/acme-web"), Some(home));
        let b = parent_label(Path::new("/home/demo/work/acme-web"), Some(home));
        let c = parent_label(Path::new("/srv/acme-web"), Some(home));
        assert_eq!(a, "~");
        assert_eq!(b, "~/work");
        assert_eq!(c, "/srv", "outside home, spelled whole");
        assert_eq!(
            parent_label(Path::new("/home/demo/work/acme-web"), None),
            "/home/demo/work",
            "with no home to measure against the path is shown whole"
        );
        assert_eq!(parent_label(Path::new("/"), Some(home)), "");
    }

    #[test]
    fn the_commit_button_says_what_pressing_it_would_actually_do() {
        let staged = status_of_repo(
            "/a",
            vec![entry(
                "a.rs",
                ChangeCode::Modified,
                ChangeCode::None,
                EntryKind::Tracked,
            )],
        );
        let unstaged = status_of_repo(
            "/a",
            vec![entry(
                "a.rs",
                ChangeCode::None,
                ChangeCode::Modified,
                EntryKind::Tracked,
            )],
        );
        let clean = status_of_repo("/a", Vec::new());

        assert_eq!(
            commit_plan(&staged, false, "msg").label,
            L10nKey::ScmCommitButton
        );
        // Nothing staged: the button says so rather than quietly running -a.
        assert_eq!(
            commit_plan(&unstaged, false, "msg").label,
            L10nKey::ScmCommitAllButton
        );
        assert_eq!(
            commit_plan(&staged, true, "").label,
            L10nKey::ScmCommitAmendButton
        );

        assert!(commit_plan(&staged, false, "msg").enabled);
        assert!(
            !commit_plan(&staged, false, "   ").enabled,
            "an all-whitespace message is no message"
        );
        // ...and the reason must say so: staged work with a blank message is
        // not "nothing to commit", which is what the palette path used to
        // answer (#546).
        assert_eq!(
            commit_plan(&staged, false, "  ").reason,
            L10nKey::ScmCommitNeedsMessage
        );
        assert_eq!(
            commit_plan(&clean, false, "msg").reason,
            L10nKey::ScmNothingToCommit
        );
        assert!(
            commit_plan(&clean, true, "").enabled,
            "amending with no message keeps HEAD's own with --no-edit"
        );
        assert!(!commit_plan(&clean, false, "msg").enabled);
    }

    fn conflict(path: &str) -> StatusEntry {
        entry(
            path,
            ChangeCode::Unmerged,
            ChangeCode::Unmerged,
            EntryKind::Unmerged,
        )
    }

    fn staged(path: &str) -> StatusEntry {
        entry(
            path,
            ChangeCode::Modified,
            ChangeCode::None,
            EntryKind::Tracked,
        )
    }

    #[test]
    fn unresolved_conflicts_block_every_kind_of_commit() {
        // `git commit` refuses outright while any path is still unmerged — a
        // message, a staged file and an in-progress merge change nothing —
        // so the button must stop saying "go" however it is armed.
        let mixed = || status_of_repo("/a", vec![conflict("hit.rs"), staged("staged.rs")]);
        assert_eq!(
            mixed().conflicts().count(),
            1,
            "the fixture must hold a real conflict or nothing below tests anything"
        );
        assert!(
            !commit_plan(&mixed(), false, "msg").enabled,
            "a message does not lift a conflict"
        );
        let mut merging = mixed();
        merging.operation = Some(RepoOperation::Merge);
        assert!(
            !commit_plan(&merging, false, "").enabled,
            "--allow-empty-message is for a resolved merge, not a live one"
        );
        assert!(
            !commit_plan(&mixed(), true, "").enabled,
            "amend cannot conclude an unresolved merge either"
        );

        // The control on the same gate: resolving the last `U` must hand the
        // button back. Mid-merge is the one state an empty message commits,
        // because `ops` sends --allow-empty-message for exactly it.
        let mut resolved = status_of_repo("/a", vec![staged("a.rs")]);
        resolved.operation = Some(RepoOperation::Merge);
        let plan = commit_plan(&resolved, false, "");
        assert!(
            plan.enabled,
            "a resolved merge still commits on --allow-empty-message"
        );
        assert_eq!(
            plan.label,
            L10nKey::ScmCommitButton,
            "the merge commit is still an ordinary commit, not Commit All"
        );
    }

    #[test]
    fn a_conflicted_plan_does_not_blame_the_message_or_the_index() {
        // Both stock answers lie here: a message is written, and there is
        // work — it just cannot be committed yet. The reason must name the
        // conflicts instead; the conflict-specific key lands with the fix, so
        // this only pins what the answer must not be.
        let mixed = status_of_repo("/a", vec![conflict("a.rs"), staged("staged.rs")]);
        let plan = commit_plan(&mixed, false, "msg");
        assert!(!plan.enabled);
        assert_ne!(
            plan.reason,
            L10nKey::ScmCommitNeedsMessage,
            "a written message is not the blocker"
        );
        let only_conflicts = status_of_repo("/a", vec![conflict("a.rs")]);
        assert_eq!(only_conflicts.conflicts().count(), 1);
        assert_ne!(
            commit_plan(&only_conflicts, false, "msg").reason,
            L10nKey::ScmNothingToCommit,
            "there is work; it cannot be committed yet"
        );
    }

    #[test]
    fn a_head_without_a_pushable_branch_says_why() {
        let branch = HeadState::Branch {
            name: "main".into(),
            oid: "1111111".into(),
        };
        assert_eq!(pushable_branch(&branch), Ok("main"));
        // The two states the old let-else in scm_push swallowed without a
        // word (#545): the toast now names each one.
        assert_eq!(
            pushable_branch(&HeadState::Detached {
                oid: "1111111".into()
            }),
            Err(L10nKey::ScmPushDetached)
        );
        assert_eq!(
            pushable_branch(&HeadState::Unborn {
                branch: "main".into()
            }),
            Err(L10nKey::ScmPushNoCommits)
        );
    }

    #[test]
    fn only_a_commit_with_an_empty_index_sweeps_the_working_tree() {
        let staged = status_of_repo(
            "/a",
            vec![entry(
                "a.rs",
                ChangeCode::Modified,
                ChangeCode::None,
                EntryKind::Tracked,
            )],
        );
        let unstaged = status_of_repo(
            "/a",
            vec![entry(
                "a.rs",
                ChangeCode::None,
                ChangeCode::Modified,
                EntryKind::Tracked,
            )],
        );
        assert!(commit_stages_everything(&unstaged, false));
        assert!(!commit_stages_everything(&staged, false));
        // Amending is "fix the last commit", not "add everything to it".
        assert!(!commit_stages_everything(&unstaged, true));
    }

    #[test]
    fn a_branch_level_with_its_upstream_says_nothing() {
        assert_eq!(tracking_chip(Some("origin/main"), Some((0, 0)), true), None);
        // Both halves once there is anything to say, so the reading keeps its
        // shape as the numbers move.
        assert_eq!(
            tracking_chip(Some("origin/main"), Some((2, 0)), true).as_deref(),
            Some("↑2 ↓0")
        );
        assert_eq!(
            tracking_chip(Some("origin/main"), Some((0, 1)), true).as_deref(),
            Some("↑0 ↓1")
        );
        assert_eq!(
            tracking_chip(Some("origin/main"), Some((2, 1)), true).as_deref(),
            Some("↑2 ↓1")
        );
        // Nothing to compare against is not the same as being level: the chip
        // becomes the offer to publish.
        assert_eq!(
            tracking_chip(None, None, true).as_deref(),
            Some(t(L10nKey::ScmPublishBranch))
        );
        // Unless there is no branch to publish. A detached HEAD has no
        // upstream by definition, and the tile beside this token already
        // refuses the operation (#545) — offering it here only made the row
        // wider while naming the one thing that cannot happen.
        assert_eq!(tracking_chip(None, None, false), None);
        // An upstream git could not count against says nothing rather than
        // claiming zero.
        assert_eq!(tracking_chip(Some("origin/main"), None, true), None);
    }

    #[test]
    fn a_detached_head_shows_the_sha_git_would_print() {
        assert_eq!(
            head_label(&HeadState::Detached {
                oid: "0123456789abcdef".into()
            }),
            "0123456"
        );
        assert_eq!(
            head_label(&HeadState::Branch {
                name: "main".into(),
                oid: "0123456".into()
            }),
            "main"
        );
    }

    #[test]
    fn every_parked_operation_has_something_to_say() {
        // Interactive and plain rebase read the same on purpose: git writes
        // `rebase-merge/interactive` for both, so the distinction is not one
        // the repository on disk can make.
        assert_eq!(
            operation_label(RepoOperation::RebaseInteractive),
            operation_label(RepoOperation::Rebase)
        );
        for op in [
            RepoOperation::Merge,
            RepoOperation::Rebase,
            RepoOperation::CherryPick,
            RepoOperation::Revert,
            RepoOperation::Bisect,
            RepoOperation::Am,
        ] {
            assert!(!t(operation_label(op)).is_empty(), "{op:?}");
        }
    }

    #[gpui::test]
    fn commit_action_is_scoped_to_the_message_box(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (_app, mut vcx) = harness(cx);

        let (in_box, outside, window_wide) = vcx.update(|window, _cx| {
            let scoped = gpui::KeyContext::parse(COMMIT_KEY_CONTEXT)
                .expect("the context the panel installs parses");
            (
                window.bindings_for_action_in_context(&ScmCommit, scoped),
                window.bindings_for_action_in_context(
                    &ScmCommit,
                    gpui::KeyContext::new_with_defaults(),
                ),
                window.bindings_for_action_in_context(
                    &ToggleFullscreen,
                    gpui::KeyContext::new_with_defaults(),
                ),
            )
        });

        assert!(
            !in_box.is_empty(),
            "the message box installs {COMMIT_KEY_CONTEXT}, and the binding has to live in it"
        );
        assert!(
            outside.is_empty(),
            "outside the box the chord must not commit anything"
        );
        assert!(
            !window_wide.is_empty(),
            "the window keeps its own binding for the same chord"
        );
        if cfg!(target_os = "macos") {
            // The whole point: one chord, two meanings, separated only by the
            // context the box installs. If they ever stopped colliding this
            // test would still pass for the wrong reason, so assert they do.
            assert_eq!(
                in_box[0].keystrokes(),
                window_wide[0].keystrokes(),
                "secondary-enter is shared, and scope is what tells them apart"
            );
        }
    }

    #[gpui::test]
    fn a_commit_draft_follows_its_repository(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);
        let (a, b) = (repo("/a"), repo("/b"));
        let status_a = status_of_repo("/a", Vec::new());
        let status_b = status_of_repo("/b", Vec::new());

        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.scm_commit_input(&a, &status_a, window, cx);
            input.update(cx, |state, cx| state.set_value("wip: a", window, cx));

            let input = app.scm_commit_input(&b, &status_b, window, cx);
            assert_eq!(
                input.read(cx).value(),
                "",
                "another working tree is another message"
            );
            input.update(cx, |state, cx| state.set_value("wip: b", window, cx));

            let input = app.scm_commit_input(&a, &status_a, window, cx);
            assert_eq!(
                input.read(cx).value(),
                "wip: a",
                "coming back has to bring the draft with it"
            );
            assert_eq!(app.scm.draft(&b), "wip: b");
        });
    }

    #[gpui::test]
    fn a_merge_prefills_the_box_with_gits_own_message(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);
        let key = repo("/a");
        let mut status = status_of_repo("/a", Vec::new());
        status.prefilled_message = Some("Merge branch 'topic'".into());

        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.scm_commit_input(&key, &status, window, cx);
            assert_eq!(input.read(cx).value(), "Merge branch 'topic'");
        });
    }

    #[gpui::test]
    fn a_rejected_commit_keeps_the_message_it_was_given(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);
        let key = repo("/a");
        let before = status_of_repo("/a", Vec::new());

        app.update(&mut vcx, |app, _cx| {
            app.scm.committing = Some((key.clone(), before.head.clone(), "wip".into()));

            // A pre-commit hook said no: HEAD has not moved, so the message
            // the user wrote is still the only copy of it there is.
            assert!(!app.scm_commit_landed(&key, &before, "wip"));

            let mut after = before.clone();
            after.head = HeadState::Branch {
                name: "main".into(),
                oid: "2222222".into(),
            };
            assert!(app.scm_commit_landed(&key, &after, "wip"));
            assert_eq!(app.scm.draft(&key), "");
        });
    }

    fn tab_from(json: &str) -> RightPanelTab {
        serde_json::from_str::<CoreConfig>(json)
            .expect("config deserializes")
            .right_panel_tab
    }

    #[gpui::test]
    fn scm_tab_opens_and_config_round_trips(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);

        app.update(&mut vcx, |app, cx| {
            app.set_right_panel_tab(RightPanelTab::Scm, cx)
        });
        let (visible, tab) = app.read_with(&vcx, |app, _| {
            (app.right_panel_visible, app.right_panel_tab)
        });
        assert!(visible);
        assert_eq!(tab, RightPanelTab::Scm);

        // The whole reason the variant was renamed in place: what lands on
        // disk is still `"changes"`, so a build from before this change reads
        // its own config back and stays on the panel the user left open.
        let cfg = CoreConfig {
            right_panel_tab: RightPanelTab::Scm,
            ..Default::default()
        };
        let json = serde_json::to_value(&cfg).expect("config serializes");
        assert_eq!(json["right_panel_tab"], serde_json::json!("changes"));

        assert_eq!(
            tab_from(r#"{"right_panel_tab":"changes"}"#),
            RightPanelTab::Scm
        );
        assert_eq!(tab_from(r#"{"right_panel_tab":"scm"}"#), RightPanelTab::Scm);
        assert_eq!(tab_from(r#"{"right_panel_tab":"git"}"#), RightPanelTab::Scm);
        // Anything unrecognised falls back through `de_lenient` rather than
        // failing the whole file.
        assert_eq!(
            tab_from(r#"{"right_panel_tab":"nonsense"}"#),
            RightPanelTab::Info
        );
    }

    #[gpui::test]
    fn the_new_config_fields_default_to_todays_behaviour(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);

        let cfg = CoreConfig::default();
        assert_eq!(cfg.diff_view, DiffViewMode::Split);
        assert!(!cfg.scm_graph_expanded);

        // Toggling has to survive the round trip through the config, since the
        // panel reads it back on the next launch.
        app.update(&mut vcx, |app, cx| app.scm_toggle_graph(cx));
        assert!(app.read_with(&vcx, |app, _| app.scm.graph.expanded));
        assert!(vcx.update(|_, cx| {
            cx.global::<crate::core::config::Config>()
                .scm_graph_expanded
        }));

        app.update(&mut vcx, |app, cx| app.toggle_diff_view_mode(cx));
        assert_eq!(
            vcx.update(|_, cx| cx.global::<crate::core::config::Config>().diff_view),
            DiffViewMode::Unified
        );
    }

    #[gpui::test]
    fn a_folded_group_stays_folded_across_rerenders(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);

        app.update(&mut vcx, |app, cx| {
            app.set_right_panel_tab(RightPanelTab::Scm, cx);
            app.scm_toggle_group(ScmGroup::Staged, 3, cx);
        });
        vcx.background_executor.run_until_parked();
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.scm.group_collapsed(ScmGroup::Staged, 3)));

        // And a long untracked list that the user opened by hand stays open,
        // rather than snapping shut again on the count.
        app.update(&mut vcx, |app, cx| {
            app.scm_toggle_group(ScmGroup::Untracked, 500, cx)
        });
        vcx.run_until_parked();
        assert!(!app.read_with(&vcx, |app, _| {
            app.scm.group_collapsed(ScmGroup::Untracked, 500)
        }));
    }
}

/// The panel asks git for a lot, from inside `render`. These hold it to
/// asking once and then going quiet.
///
/// The hazard is specific: `scm_refresh` reaches for its cache through
/// `default_global`, which fires the global observers whether or not anything
/// changed, and it is called every frame. A watcher that notified on every one
/// of those would request a frame from inside a frame and never stop.
#[cfg(test)]
mod render_idle_gpui_tests {
    use super::*;
    use crate::daemon::protocol::DaemonMsg;
    use crate::ui::app::{render_probe, test_window};
    use crate::ui::host_ops::HostId;
    use gpui::{
        Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
        PlatformInput, Point, TestAppContext, VisualTestContext,
    };
    use gpui_component::WindowExt as _;
    use tty59_core::core::config::RightPanelTab;

    const BUDGET: u64 = 200;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tty59-scm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed");
    }

    /// `git` for the answers, not the side effects: what `rev-parse` says.
    fn git_out(root: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// `scm_panel_on`'s own wait, for the states the fixture reaches after the
    /// panel is already up: a landed probe is one debounce plus a read.
    fn wait_until(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        until: impl Fn(&Tty59App, &gpui::App) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            app.update_in(vcx, |_, _, cx| cx.notify());
            vcx.background_executor.run_until_parked();
            if app.update_in(vcx, |app, _, cx| until(app, cx)) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the panel never reached the wanted state"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The pointer ritual of the strip tests, on the elements this window
    /// really draws.
    fn click(vcx: &mut VisualTestContext, at: Point<Pixels>) {
        let move_event = |pressed| {
            PlatformInput::MouseMove(MouseMoveEvent {
                position: at,
                pressed_button: pressed,
                modifiers: Modifiers::none(),
            })
        };
        let button_event = |down: bool| {
            if down {
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                    first_mouse: false,
                })
            } else {
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                })
            }
        };
        vcx.update(|window, cx| {
            window.dispatch_event(move_event(None), cx);
            window.dispatch_event(button_event(true), cx);
            window.dispatch_event(button_event(false), cx);
            window.refresh();
        });
        vcx.run_until_parked();
    }

    fn scm_panel_on(
        cx: &mut TestAppContext,
        root: &Path,
        until: impl Fn(&Tty59App, &gpui::App) -> bool,
    ) -> (
        Entity<Tty59App>,
        VisualTestContext,
        crate::daemon::transport::Stream,
    ) {
        let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
        DaemonMsg::Cwd(root.to_path_buf())
            .encode(&mut pane)
            .expect("the pane's socket takes the cwd");
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_visible = true;
            app.right_panel_tab = RightPanelTab::Scm;
            cx.notify();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            app.update_in(&mut vcx, |_, _, cx| cx.notify());
            vcx.background_executor.run_until_parked();
            if app.update_in(&mut vcx, |app, _, cx| until(app, cx)) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the panel never settled on the directory"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        test_window::quiesce(&mut vcx, Some(root));
        (app, vcx, pane)
    }

    fn draws_while_idle(vcx: &mut VisualTestContext) -> u64 {
        test_window::quiesce(vcx, None);
        render_probe::arm(BUDGET);
        vcx.background_executor.run_until_parked();
        vcx.executor()
            .advance_clock(std::time::Duration::from_secs(3));
        vcx.background_executor.run_until_parked();
        render_probe::arm(BUDGET);
        // No real-time exposure in the counted window, deliberately. The file
        // tree holds a real filesystem watch, so real time is a channel input
        // arrives on — and a test that spends it here is asking to be handed
        // some. What has to be waited out is waited out in `quiesce` above,
        // where a frame costs nothing.
        vcx.executor()
            .advance_clock(std::time::Duration::from_secs(9));
        vcx.background_executor.run_until_parked();
        render_probe::draws()
    }

    /// The only test in this module that waits on `repo.root`, and so the only
    /// one Windows cannot run: since #796 that root is keyed by one spelling
    /// and carries a `Disk` prefix, while the pane's cwd came out of `scratch`
    /// above — `std::fs::canonicalize`, so `\\?\C:\Users\—` and a
    /// `VerbatimDisk` prefix — and the equality below never holds between the
    /// two. The slashes are the red herring; `Path` compares by component, so
    /// `C:/x` and `C:\x` are equal. Spelling `scratch`'s answer the way
    /// `tty59_core::core::path_spelling` does should lift this, in a change
    /// that can show it green. Its sibling keys off the pane's cwd instead,
    /// and runs everywhere.
    #[cfg(unix)]
    #[gpui::test]
    fn a_settled_source_control_panel_reaches_render_idle(cx: &mut TestAppContext) {
        let _serial = serial();
        let root = scratch("settled");
        git(&root, &["init", "--quiet"]);
        std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
        git(&root, &["add", "a.rs"]);
        std::fs::write(root.join("b.rs"), "// untracked\n").unwrap();

        let want = root.clone();
        let (app, mut vcx, _pane) = scm_panel_on(cx, &root, move |app, cx| {
            app.scm.repo.as_ref().is_some_and(|r| r.root == want)
                && crate::terminal::git_data::status_of(cx, HostId::LOCAL, &want).is_some()
        });

        let status = app.update_in(&mut vcx, |app, _, cx| {
            crate::terminal::git_data::status_of(
                cx,
                HostId::LOCAL,
                &app.scm.repo.clone().unwrap().root,
            )
        });
        let status = status.expect("the panel read a status");
        assert_eq!(status.staged().count(), 1, "a.rs is in the index");
        assert_eq!(status.untracked().count(), 1, "b.rs is not");

        assert_eq!(draws_while_idle(&mut vcx), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[gpui::test]
    fn a_directory_with_no_repository_reaches_render_idle(cx: &mut TestAppContext) {
        let _serial = serial();
        let root = scratch("bare");
        std::fs::write(root.join("notes.txt"), "").unwrap();

        // Nothing here is a repository, so `git status` must never be reached
        // — and the panel must not spin looking for one that is not coming.
        let want = root.clone();
        let (app, mut vcx, _pane) = scm_panel_on(cx, &root, move |app, _cx| {
            app.scm.roots.contains_key(&(HostId::LOCAL, want.clone()))
        });
        assert!(
            app.update_in(&mut vcx, |app, _, _| app
                .scm
                .roots
                .values()
                .all(|(_, r)| r.is_none())),
            "no root was resolved for a directory that is not a repository"
        );
        assert!(
            app.update_in(&mut vcx, |app, _, _| app.scm.repo.is_none()),
            "and no status was ever asked for"
        );
        assert_eq!(draws_while_idle(&mut vcx), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The tree view and the filter, end to end over a real repository: the
    /// rows that come out are the ones the pure helpers promise, drawn.
    #[gpui::test]
    fn the_changes_list_folds_into_a_tree_and_filters_by_path(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let root = scratch("tree");
        git(&root, &["init", "--quiet"]);
        std::fs::create_dir_all(root.join("src/ui")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("src/ui/a.rs"), "a\n").unwrap();
        std::fs::write(root.join("src/ui/b.rs"), "b\n").unwrap();
        std::fs::write(root.join("docs/readme.md"), "r\n").unwrap();

        // Keyed off the root the panel resolved, not off `root`: on Windows
        // `scratch` hands back a `\\?\` verbatim path that never compares
        // equal to git's spelling of the same directory — see the note on
        // `a_settled_source_control_panel_reaches_render_idle`.
        let (app, mut vcx, _pane) = scm_panel_on(cx, &root, |app, cx| {
            app.scm.repo.as_ref().is_some_and(|r| {
                crate::terminal::git_data::status_of(cx, r.host, &r.root)
                    .is_some_and(|s| s.untracked().count() == 3)
            })
        });
        let drawn = |vcx: &mut VisualTestContext, what: &'static str| {
            app.update_in(vcx, |_, _, cx| cx.notify());
            vcx.run_until_parked();
            vcx.debug_bounds(what).is_some()
        };
        let a = "scm-row-Untracked-src/ui/a.rs";
        let readme = "scm-row-Untracked-docs/readme.md";
        let src_ui = "scm-dir-Untracked-src/ui";

        // The flat list is the default, and it has no directory rows.
        assert!(drawn(&mut vcx, a));
        assert!(!drawn(&mut vcx, src_ui));

        // `src` holds nothing but `ui`, so the two share one row.
        app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
        assert!(vcx.update(|_, cx| cx.global::<crate::core::config::Config>().scm_changes_tree));
        assert!(drawn(&mut vcx, src_ui));
        assert!(!drawn(&mut vcx, "scm-dir-Untracked-src"));
        assert!(drawn(&mut vcx, "scm-dir-Untracked-docs"));
        assert!(drawn(&mut vcx, a));

        // Folding the directory keeps its row and hides what is under it.
        app.update(&mut vcx, |app, cx| {
            app.scm.toggle_dir(ScmGroup::Untracked, "src/ui".into());
            cx.notify();
        });
        assert!(drawn(&mut vcx, src_ui));
        assert!(!drawn(&mut vcx, a));
        assert!(drawn(&mut vcx, readme));

        // A filter drops what does not match — and opens the folded
        // directory the match is in.
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.scm.filter.clone().expect("the filter row was drawn");
            input.update(cx, |state, cx| state.set_value("UI A", window, cx));
        });
        assert!(drawn(&mut vcx, a));
        assert!(!drawn(&mut vcx, "scm-row-Untracked-src/ui/b.rs"));
        assert!(!drawn(&mut vcx, readme));
        assert!(!drawn(&mut vcx, "scm-dir-Untracked-docs"));

        // The same filter narrows the flat list too.
        app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
        assert!(drawn(&mut vcx, a));
        assert!(!drawn(&mut vcx, readme));
        assert!(!drawn(&mut vcx, src_ui));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Panels F20, measured on the drawn panel over a real repository: every
    /// fold in the Changes list — the group headers, open and folded, and the
    /// tree view's directories — is a 10px chevron, the status letters stand
    /// in a 12px column, and each group arrow sits over those letters. The
    /// expected offsets are written out from the finding (8px row inset, 12px
    /// letter cell, 8px gap; 10px chevron, 6px gap), not read back from the
    /// constants the panel draws with.
    #[gpui::test]
    fn disclosure_chevrons_are_10px_over_a_12px_status_column(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let root = scratch("chevrons");
        git(&root, &["init", "--quiet"]);
        std::fs::create_dir_all(root.join("src/ui")).unwrap();
        std::fs::write(root.join("src/ui/a.rs"), "a\n").unwrap();
        std::fs::write(root.join("notes.md"), "n\n").unwrap();
        std::fs::write(root.join("staged.rs"), "s\n").unwrap();
        git(&root, &["add", "staged.rs"]);

        let (app, mut vcx, _pane) = scm_panel_on(cx, &root, |app, cx| {
            app.scm.repo.as_ref().is_some_and(|r| {
                crate::terminal::git_data::status_of(cx, r.host, &r.root)
                    .is_some_and(|s| s.untracked().count() == 2 && s.staged().count() == 1)
            })
        });
        // `debug_bounds` takes `&'static str`, so every selector below is a literal.
        let bounds = |vcx: &mut VisualTestContext, what: &'static str| {
            app.update_in(vcx, |_, _, cx| cx.notify());
            vcx.run_until_parked();
            vcx.debug_bounds(what)
                .unwrap_or_else(|| panic!("{what} was not drawn"))
        };
        let eps = 0.01;
        let ten = gpui::size(gpui::px(10.), gpui::px(10.));

        let row = bounds(&mut vcx, "scm-row-Untracked-notes.md");
        let name = bounds(&mut vcx, "scm-row-Untracked-notes.md-name");
        assert!(
            ((name.left() - row.left()).as_f32() - 28.).abs() < eps,
            "a flat row's name starts 8 + 12 + 8 = 28px past its fill edge: row {row:?}, name {name:?}"
        );
        let staged = bounds(&mut vcx, "scm-row-Staged-staged.rs");
        let staged_name = bounds(&mut vcx, "scm-row-Staged-staged.rs-name");
        assert!(((staged_name.left() - staged.left()).as_f32() - 28.).abs() < eps);

        for collapse in [false, true] {
            if collapse {
                app.update(&mut vcx, |app, cx| {
                    app.scm_toggle_group(ScmGroup::Untracked, 2, cx);
                });
            }
            for (group, chevron, label) in [
                (
                    "Staged",
                    "scm-group-chevron-Staged",
                    "scm-group-label-Staged",
                ),
                (
                    "Untracked",
                    "scm-group-chevron-Untracked",
                    "scm-group-label-Untracked",
                ),
            ] {
                let chevron = bounds(&mut vcx, chevron);
                let label = bounds(&mut vcx, label);
                assert_eq!(
                    chevron.size, ten,
                    "{group} chevron (collapsed={collapse}) is 10px"
                );
                assert!(
                    ((chevron.left() - staged.left()).as_f32() - 8.).abs() < eps,
                    "{group} chevron starts on the letters' column: {chevron:?} vs row {staged:?}"
                );
                let letter_centre = staged.left().as_f32() + 8. + 6.;
                let arrow_centre = chevron.left().as_f32() + chevron.size.width.as_f32() / 2.;
                assert!(
                    (arrow_centre - letter_centre).abs() <= 1. + eps,
                    "{group} arrow centre {arrow_centre} stands over the letters' {letter_centre}"
                );
                assert!(
                    ((label.left() - chevron.left()).as_f32() - 16.).abs() < eps,
                    "{group} label lands 10 + 6 = 16px past the chevron: {label:?}"
                );
            }
        }
        app.update(&mut vcx, |app, cx| {
            app.scm_toggle_group(ScmGroup::Untracked, 2, cx);
        });

        // The tree view: a directory row's chevron is the same 10px mark,
        // centred in the letters' 12px cell, open and folded.
        app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
        let dir = "scm-dir-Untracked-src/ui";
        let chevron = "scm-dir-chevron-Untracked-src/ui";
        for fold in [false, true] {
            if fold {
                app.update(&mut vcx, |app, cx| {
                    app.scm.toggle_dir(ScmGroup::Untracked, "src/ui".into());
                    cx.notify();
                });
            }
            let dir_row = bounds(&mut vcx, dir);
            let cell = bounds(&mut vcx, chevron);
            assert_eq!(
                cell.size,
                gpui::size(gpui::px(12.), gpui::px(10.)),
                "a 10px chevron in the 12px letter cell (folded={fold})"
            );
            assert!(((cell.left() - dir_row.left()).as_f32() - 8.).abs() < eps);
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The whole bug on a real repository: while a path sits unmerged the
    /// commit must be dead — the button that offers it and the action the key
    /// binding reaches — until the path is marked resolved.
    #[gpui::test]
    fn a_conflicted_merge_keeps_commit_dead_until_the_paths_clear(cx: &mut TestAppContext) {
        let _serial = serial();
        let root = scratch("conflict");
        git(&root, &["init", "--quiet", "-b", "base"]);
        git(&root, &["config", "user.email", "test@tty59"]);
        git(&root, &["config", "user.name", "tty59"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("hit.rs"), "fn hit() {}\n").unwrap();
        git(&root, &["add", "hit.rs"]);
        git(&root, &["commit", "--quiet", "-m", "base"]);
        git(&root, &["checkout", "--quiet", "-b", "side"]);
        std::fs::write(root.join("hit.rs"), "fn hit() { side() }\n").unwrap();
        git(&root, &["commit", "--quiet", "-am", "side"]);
        git(&root, &["checkout", "--quiet", "base"]);
        std::fs::write(root.join("hit.rs"), "fn hit() { base() }\n").unwrap();
        git(&root, &["commit", "--quiet", "-am", "base2"]);
        // The merge has to refuse here — that refusal is the fixture.
        let merged = std::process::Command::new("git")
            .args(["merge", "--quiet", "side"])
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(!merged.status.success(), "the merge must stop on hit.rs");
        std::fs::write(root.join("other.rs"), "fn other() {}\n").unwrap();
        git(&root, &["add", "other.rs"]);

        let (app, mut vcx, _pane) = scm_panel_on(cx, &root, |app, cx| {
            app.scm.repo.as_ref().is_some_and(|r| {
                crate::terminal::git_data::status_of(cx, r.host, &r.root)
                    .is_some_and(|s| s.conflicts().count() == 1)
            })
        });
        let press_commit = |vcx: &mut VisualTestContext| {
            let at = vcx
                .debug_bounds("scm-commit")
                .expect("the commit button is drawn")
                .center();
            click(vcx, at);
        };
        let evidence = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
            app.update_in(vcx, |app, window, cx| {
                (app.scm.committing.is_some(), window.notifications(cx).len())
            })
        };

        // The press the panel shows must die on the button: no op dispatched,
        // no toast. Lit pre-fix, this click put `git commit` straight into the
        // `U` path and let git be the one to say no.
        press_commit(&mut vcx);
        let (armed, toasts) = evidence(&app, &mut vcx);
        assert!(!armed, "a dead button still dispatched the commit");
        assert_eq!(toasts, 0, "a swallowed press should say nothing");

        // The palette has no button to grey out, so the action carries the
        // reason — and still dispatches nothing.
        app.update_in(&mut vcx, |app, window, cx| {
            app.run_scm_action(crate::ui::scm::actions::ScmIntent::Commit, window, cx);
        });
        let (armed, toasts) = evidence(&app, &mut vcx);
        assert!(!armed, "the action path dispatched the same commit");
        assert_eq!(toasts, 1, "the refused action should say why");

        // The count the render reads comes from the status index; lose it
        // while the status stays and the answer is still no — a missing count
        // is not a clean one. The toast count is checked too, so a
        // dispatched-and-failed commit cannot hide behind `committing`
        // clearing itself on the way back.
        app.update_in(&mut vcx, |app, _, cx| {
            let repo = app.scm.repo.clone().expect("a repo is on screen");
            cx.default_global::<crate::terminal::git_data::ScmData>()
                .forget_index(repo.host, &repo.root);
            cx.notify();
        });
        press_commit(&mut vcx);
        let (armed, toasts_after) = evidence(&app, &mut vcx);
        assert!(!armed, "no index is not 'no conflicts'");
        assert_eq!(
            toasts_after, toasts,
            "no index is still a refusal, not a dispatch"
        );

        // Resolve the path; the watcher brings the new status and a fresh
        // index in together, and only then does the same press mean something.
        git(&root, &["add", "hit.rs"]);
        let before = git_out(&root, &["rev-parse", "HEAD"]);
        wait_until(&app, &mut vcx, |app, cx| {
            app.scm.repo.as_ref().is_some_and(|r| {
                crate::terminal::git_data::index_of(cx, r.host, &r.root).is_some()
                    && crate::terminal::git_data::status_of(cx, r.host, &r.root).is_some_and(|s| {
                        s.conflicts().next().is_none()
                            && matches!(s.operation, Some(RepoOperation::Merge))
                    })
            })
        });
        press_commit(&mut vcx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            vcx.background_executor.run_until_parked();
            if git_out(&root, &["rev-parse", "HEAD"]) != before {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the live button never made the merge commit"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The real Changes row in the real panel: hovering it paints the strip
    /// over the row's end, and the directory text must end before it. Drawn
    /// through `scm_file_row`, so what is under test is the row's own wiring.
    #[gpui::test]
    fn hovering_a_changed_file_moves_its_directory_out_from_under_the_strip(
        cx: &mut TestAppContext,
    ) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let root = scratch("strip");
        git(&root, &["init", "--quiet"]);
        std::fs::create_dir_all(root.join("src/ui")).unwrap();
        std::fs::write(root.join("src/ui/a.rs"), "a\n").unwrap();
        let (app, mut vcx, _pane) = scm_panel_on(cx, &root, |app, cx| {
            app.scm.repo.as_ref().is_some_and(|r| {
                crate::terminal::git_data::status_of(cx, r.host, &r.root)
                    .is_some_and(|s| s.untracked().count() == 1)
            })
        });
        // `debug_bounds` keys are `&'static str`, so the three selectors are
        // spelled as literals built from one row id.
        macro_rules! row_id {
            () => {
                "scm-row-Untracked-src/ui/a.rs"
            };
        }
        const ROW: &str = row_id!();
        const DIR: &str = concat!(row_id!(), "-dir");
        const STRIP: &str = concat!(row_id!(), "-strip");
        let draw = |vcx: &mut VisualTestContext| {
            app.update_in(vcx, |_, _, cx| cx.notify());
            vcx.run_until_parked();
        };
        draw(&mut vcx);
        let row = vcx.debug_bounds(ROW).expect("the row is drawn");
        let resting = vcx
            .debug_bounds(DIR)
            .expect("the flat list draws the directory");

        vcx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: gpui::point(row.left() + gpui::px(12.), row.center().y),
                    pressed_button: None,
                    modifiers: gpui::Modifiers::none(),
                }),
                cx,
            );
            window.refresh();
        });
        draw(&mut vcx);
        let strip = vcx
            .debug_bounds(STRIP)
            .expect("hovering the row paints its strip");
        let hovered = vcx.debug_bounds(DIR).expect("the directory is still drawn");

        assert!(
            hovered.right() <= strip.left(),
            "the directory ends at {:?}, under the strip that starts at {:?}",
            hovered.right(),
            strip.left()
        );
        assert_eq!(
            hovered.left(),
            resting.left(),
            "only the end of the cell gives way"
        );
        assert!(
            hovered.right() < resting.right(),
            "hovering gave no width up: {:?}",
            hovered.right()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    const LONG_BRANCH: &str = "feature/checkout-flow-with-saved-payment-methods";

    /// A repository on [`LONG_BRANCH`] with one commit and one untracked file.
    fn repo_on_long_branch(name: &str) -> PathBuf {
        let root = scratch(name);
        git(&root, &["init", "--quiet", "--initial-branch", LONG_BRANCH]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "-A"]);
        git(
            &root,
            &[
                "-c",
                "user.name=tty59 test",
                "-c",
                "user.email=test@tty59.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "feat: first",
            ],
        );
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        root
    }

    /// What `shown` kept of `full` when it was cut in the middle: a source
    /// prefix and a source suffix around exactly one inserted ellipsis. The
    /// fixture names contain none, so any `…` is the label's. Returns how many
    /// characters survived, so a wider box can be shown to keep more.
    fn kept_around_one_ellipsis(full: &str, shown: &str) -> usize {
        assert_eq!(
            shown.matches('…').count(),
            1,
            "{shown:?} should carry exactly one inserted ellipsis"
        );
        let (head, tail) = shown.split_once('…').unwrap();
        assert!(!head.is_empty(), "{shown:?} lost the start of {full:?}");
        assert!(!tail.is_empty(), "{shown:?} lost the end of {full:?}");
        assert!(
            full.starts_with(head),
            "{shown:?} head is not from {full:?}"
        );
        assert!(full.ends_with(tail), "{shown:?} tail is not from {full:?}");
        assert!(
            head.len() + tail.len() < full.len(),
            "{shown:?} cut nothing from {full:?}"
        );
        head.chars().count() + tail.chars().count()
    }

    /// Draws a frame and answers what the label with this id prepared in it.
    /// `None` only when it was not drawn at all.
    fn observed(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        id: &'static str,
    ) -> Option<crate::ui::text_elision::observe::Label> {
        crate::ui::text_elision::observe::clear();
        app.update_in(vcx, |_, _, cx| cx.notify());
        vcx.run_until_parked();
        crate::ui::text_elision::observe::label(&gpui::ElementId::from(id))
    }

    /// Frames until the label with this id is drawn, or panics with `root`.
    fn wait_observed(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        id: &'static str,
        root: &Path,
    ) -> crate::ui::text_elision::observe::Label {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(label) = observed(app, vcx, id) {
                return label;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "label {id:?} was never drawn for {}",
                root.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn set_panel_width(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, width: f32) -> f32 {
        app.update_in(vcx, |app, window, cx| {
            app.right_panel_width.set(width);
            cx.notify();
            app.right_panel_px(window, cx)
        })
    }

    /// The Changes header, end to end over a real repository: a long branch
    /// is cut in the middle at the width the panel actually has, a wider panel
    /// shows more of it.
    #[gpui::test]
    fn the_changes_header_keeps_both_ends_of_a_long_branch(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let root = repo_on_long_branch("header-branch");
        let (app, mut vcx, _pane) = scm_panel_on(cx, &root, |app, cx| {
            app.scm.repo.as_ref().is_some_and(|r| {
                crate::terminal::git_data::status_of(cx, r.host, &r.root)
                    .is_some_and(|s| s.untracked().count() == 1)
            })
        });
        vcx.simulate_resize(gpui::size(px(1800.), px(1000.)));

        let narrow = set_panel_width(&app, &mut vcx, 280.);
        let at_narrow = wait_observed(&app, &mut vcx, "scm-branch-label", &root);
        assert!(
            at_narrow.painted,
            "the label was prepared but never painted"
        );
        let kept_narrow = kept_around_one_ellipsis(LONG_BRANCH, &at_narrow.text);

        let wide = set_panel_width(&app, &mut vcx, 420.);
        assert!(wide > narrow, "the panel did not widen: {narrow} -> {wide}");
        let at_wide = observed(&app, &mut vcx, "scm-branch-label")
            .expect("the header is still drawn when the panel widens");
        let kept_wide = if at_wide.text.as_ref() == LONG_BRANCH {
            LONG_BRANCH.chars().count()
        } else {
            kept_around_one_ellipsis(LONG_BRANCH, &at_wide.text)
        };
        assert!(
            kept_wide > kept_narrow,
            "a wider panel kept {kept_wide} characters against {kept_narrow}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The Info tab's Branch row: cut in the middle at the live panel width,
    /// and the copy tile still puts the whole name on the clipboard.
    #[gpui::test]
    fn the_info_branch_row_keeps_both_ends_and_copies_the_whole_name(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let root = repo_on_long_branch("info-branch");
        let (app, mut vcx, _pane) = scm_panel_on(cx, &root, |app, cx| {
            app.scm.repo.as_ref().is_some_and(|r| {
                crate::terminal::git_data::status_of(cx, r.host, &r.root)
                    .is_some_and(|s| s.untracked().count() == 1)
            })
        });
        vcx.simulate_resize(gpui::size(px(1800.), px(1000.)));
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_tab = RightPanelTab::Info;
            cx.notify();
        });
        set_panel_width(&app, &mut vcx, 240.);
        let shown = wait_observed(&app, &mut vcx, "branch", &root);
        assert!(shown.painted, "the label was prepared but never painted");
        kept_around_one_ellipsis(LONG_BRANCH, &shown.text);

        // Hover the real row, then click the one tile its strip holds.
        let row = vcx
            .debug_bounds("panel-info-branch-row")
            .expect("the branch row is drawn");
        let pointer = |vcx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>, press: bool| {
            vcx.update(|window, cx| {
                let event = match press {
                    false => gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: gpui::Modifiers::none(),
                    }),
                    true => gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        button: gpui::MouseButton::Left,
                        position: at,
                        modifiers: gpui::Modifiers::none(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                };
                window.dispatch_event(event, cx);
                if press {
                    window.dispatch_event(
                        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                            button: gpui::MouseButton::Left,
                            position: at,
                            modifiers: gpui::Modifiers::none(),
                            click_count: 1,
                        }),
                        cx,
                    );
                }
                window.refresh();
            });
            vcx.run_until_parked();
        };
        pointer(
            &mut vcx,
            row.origin + gpui::point(px(4.), row.size.height / 2.),
            false,
        );
        app.update_in(&mut vcx, |_, _, cx| cx.notify());
        vcx.run_until_parked();
        let strip = vcx
            .debug_bounds("panel-info-branch-strip")
            .expect("hovering the row reveals its copy tile");
        pointer(&mut vcx, strip.center(), true);
        let copied = vcx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(
            copied.as_deref(),
            Some(LONG_BRANCH),
            "the copy tile must give the whole name, not the cut one"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Is the repository's full status (`ScmData`, what `status_of` reads) there
/// when the right panel is open on Info and the Changes tab has never been
/// rendered? Real repository, the pane's real directory, the ordinary cheap
/// per-tab resolution; nothing planted, no new producer, no accessor beyond
/// what the application already has.
#[cfg(all(test, unix))]
mod first_visit_backing_status_gpui_tests {
    use super::*;
    use crate::daemon::protocol::DaemonMsg;
    use crate::terminal::git_data::ScmData;
    use crate::terminal::git_status::GitStatusCache;
    use crate::ui::app::test_window;
    use gpui::{Entity, TestAppContext, VisualTestContext};
    use tty59_core::core::config::RightPanelTab;

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed");
    }

    /// A repository with one edited file and two new ones: three changes.
    fn repository() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("tty59-first-visit-")
            .tempdir()
            .expect("a scratch directory");
        let root = std::fs::canonicalize(dir.path()).expect("it resolves");
        git(&root, &["init", "--quiet"]);
        git(&root, &["config", "user.email", "test@tty59"]);
        git(&root, &["config", "user.name", "tty59"]);
        std::fs::write(root.join("base.txt"), "0\n").unwrap();
        git(&root, &["add", "base.txt"]);
        git(
            &root,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "base",
            ],
        );
        std::fs::write(root.join("base.txt"), "1\n").unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        (dir, root)
    }

    /// Where things stand, for the message of an assertion.
    fn dump(app: &Tty59App, cx: &gpui::App, root: &Path) -> String {
        let cache = cx.global::<GitStatusCache>();
        let resolved = cache
            .repo_root_for(HostId::LOCAL, root)
            .map(Path::to_path_buf);
        let (subscribed, epoch) = match &resolved {
            Some(resolved) => (
                cx.try_global::<ScmData>()
                    .is_some_and(|data| data.is_subscribed(HostId::LOCAL, resolved)),
                cx.try_global::<ScmData>()
                    .map(|data| data.epoch(HostId::LOCAL, resolved)),
            ),
            None => (false, None),
        };
        format!(
            "panel visible={} tab={:?} | cheap status={:?} resolved root={resolved:?} | \
             scm.repo={:?} subscribed={subscribed} epoch={epoch:?} | full status present={}",
            app.right_panel_visible,
            app.right_panel_tab,
            cache.status_for(HostId::LOCAL, root),
            app.scm.repo,
            resolved
                .as_deref()
                .is_some_and(|r| status_of(cx, HostId::LOCAL, r).is_some()),
        )
    }

    /// The pane really is in the repository (as `scm_panel_on` puts it), the
    /// right panel is open on `tab`, and the ordinary cheap per-tab probe has
    /// resolved the repository: waited for, not assumed, on the clock the
    /// pane polls on (300 ms rounds) and the real one (20 ms sleeps).
    fn open_and_resolve(
        cx: &mut TestAppContext,
        root: &Path,
        tab: RightPanelTab,
    ) -> (
        Entity<Tty59App>,
        VisualTestContext,
        crate::daemon::transport::Stream,
        PathBuf,
    ) {
        let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
        DaemonMsg::Cwd(root.to_path_buf())
            .encode(&mut pane)
            .expect("the pane's socket takes the cwd");
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_visible = true;
            app.right_panel_tab = tab;
            cx.notify();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let resolved = loop {
            app.update_in(&mut vcx, |_, _, cx| cx.notify());
            vcx.executor()
                .advance_clock(std::time::Duration::from_millis(300));
            vcx.background_executor.run_until_parked();
            let resolved = vcx.update(|_, cx| {
                let cache = cx.global::<GitStatusCache>();
                cache
                    .status_for(HostId::LOCAL, root)
                    .and_then(|_| cache.repo_root_for(HostId::LOCAL, root))
                    .map(Path::to_path_buf)
            });
            if let Some(resolved) = resolved {
                break resolved;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the cheap per-tab probe never resolved the repository: {}",
                app.update_in(&mut vcx, |app, _, cx| dump(app, cx, root))
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        (app, vcx, pane, resolved)
    }

    /// Whether the full status shows up within `seconds` of real time, pumping
    /// frames and the executor between looks.
    fn full_status_within(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        resolved: &Path,
        seconds: u64,
    ) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        loop {
            app.update_in(vcx, |_, _, cx| cx.notify());
            vcx.executor()
                .advance_clock(std::time::Duration::from_millis(300));
            vcx.background_executor.run_until_parked();
            if vcx.update(|_, cx| status_of(cx, HostId::LOCAL, resolved).is_some()) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// THE CLAIM. The repository is resolved by the ordinary cheap probe, the
    /// panel is open on Info and the Changes tab was never rendered: the full
    /// status is already there. The window keeps redrawing for its own reasons
    /// (caret, the pane's poll), which is none of this test's business, so it
    /// does not wait for the window to go quiet: the observation below pumps
    /// frames and the executor itself.
    #[gpui::test]
    fn the_full_status_exists_before_the_changes_tab_was_ever_rendered(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repository();
        let (app, mut vcx, _pane, resolved) = open_and_resolve(cx, &root, RightPanelTab::Info);

        app.update_in(&mut vcx, |app, _, _| {
            assert!(
                app.scm.repo.is_none(),
                "precondition: the Changes tab was never rendered"
            );
            assert_eq!(app.right_panel_tab, RightPanelTab::Info);
        });
        let present = full_status_within(&app, &mut vcx, &resolved, 10);
        assert!(
            present,
            "full backing status absent before Changes: the repository is resolved and the panel \
             is open on Info, and nothing has read its status in 10 s. {}",
            app.update_in(&mut vcx, |app, _, cx| dump(app, cx, &root))
        );
    }

    /// The control: the same fixture, and the Changes tab rendered. The status
    /// machinery, the repository path and the fixture all work, so an absence
    /// in the test above is the application's and not the fixture's.
    #[gpui::test]
    fn the_changes_tab_render_produces_the_status_in_the_same_fixture(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repository();
        let (app, mut vcx, _pane, resolved) = open_and_resolve(cx, &root, RightPanelTab::Info);
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_tab = RightPanelTab::Scm;
            cx.notify();
        });
        let present = full_status_within(&app, &mut vcx, &resolved, 30);
        assert!(
            present,
            "control failed: the Changes tab rendered and the status never arrived. {}",
            app.update_in(&mut vcx, |app, _, cx| dump(app, cx, &root))
        );
        app.update_in(&mut vcx, |app, _, _| {
            assert!(app.scm.repo.is_some(), "the Changes tab did render");
        });
    }
}

/// The action strip is the one place in the panel where a hover reveal sits
/// under the pointer it is reacting to, so it is the one place where "is the
/// row hovered" and "can the row's hitbox be seen from here" can disagree.
#[cfg(test)]
mod action_strip_gpui_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;
    use gpui::{
        Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
        PlatformInput, Point, Render, TestAppContext, VisualTestContext, point,
    };

    /// A file row in miniature: a group, something to hover on the left, the
    /// real strip on the right with one button-sized child in it, and the
    /// row's own click handler — the one that opens the diff overlay, and the
    /// one a click on the strip must never reach.
    #[derive(Default)]
    struct Row {
        row_clicks: Rc<Cell<usize>>,
    }

    impl Render for Row {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let id = SharedString::from("row");
            let clicks = self.row_clicks.clone();
            // Off the origin, because a fresh test window's pointer sits at
            // (0, 0) and a row under it would start out hovered.
            div().pt(px(50.)).pl(px(50.)).child(
                div()
                    .id(id.clone())
                    .group(id.clone())
                    .relative()
                    .w(px(300.))
                    .h(px(ROW_H))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                    .child(
                        action_strip(&id, 0x00EEEEEE).child(
                            div()
                                .w(px(TILE_SIZE_XS))
                                .h(px(TILE_SIZE_XS))
                                .debug_selector(|| "scm-action".into()),
                        ),
                    ),
            )
        }
    }

    fn hover(vcx: &mut VisualTestContext, at: Point<Pixels>) {
        vcx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at,
                    pressed_button: None,
                    modifiers: Modifiers::none(),
                }),
                cx,
            );
            window.refresh();
        });
        vcx.run_until_parked();
    }

    fn click(vcx: &mut VisualTestContext, at: Point<Pixels>) {
        hover(vcx, at);
        vcx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                }),
                cx,
            );
            window.refresh();
        });
        vcx.run_until_parked();
    }

    #[gpui::test]
    fn the_strip_survives_the_pointer_reaching_it(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Row::default());
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("scm-action").is_none(),
            "an unhovered row draws no buttons"
        );

        hover(&mut vcx, point(px(70.), px(62.)));
        let strip = vcx
            .debug_bounds("scm-action")
            .expect("hovering the row reveals the buttons");

        // The whole point of the strip: the pointer travels right, onto it.
        hover(&mut vcx, strip.center());
        assert!(
            vcx.debug_bounds("scm-action").is_some(),
            "the buttons are still drawn with the pointer on top of them"
        );
    }

    /// What `occlude()` used to buy, and what replaced it.
    #[gpui::test]
    fn a_click_on_the_strip_never_reaches_the_row(cx: &mut TestAppContext) {
        let row = Row::default();
        let clicks = row.row_clicks.clone();
        let window = cx.add_window(|_, _| row);
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();

        click(&mut vcx, point(px(70.), px(62.)));
        assert_eq!(clicks.get(), 1, "clicking the row opens the diff");

        let strip = vcx
            .debug_bounds("scm-action")
            .expect("the row is hovered, so the strip is up");
        click(&mut vcx, strip.center());
        assert_eq!(
            clicks.get(),
            1,
            "but a click on the strip belongs to the strip"
        );
    }
}

/// The Changes tab's count, drawn from a real repository and a real status
/// probe, on a tab that is not the Changes tab.
#[cfg(all(test, unix))]
mod changes_count_gpui_tests {
    use super::*;
    use crate::daemon::protocol::DaemonMsg;
    use crate::terminal::git_data::ScmData;
    use crate::terminal::git_status::{GitStatusCache, RepoSnapshot};
    use crate::ui::app::test_window;
    use gpui::{Entity, TestAppContext, VisualTestContext};
    use tty59_core::core::config::RightPanelTab;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed");
    }

    /// A repository with `untracked` new files and one file that is staged and
    /// then edited again: `1 + untracked` files, and one row more than that in
    /// the list, because the edited file is under Staged and under Changes.
    fn repo_with(untracked: usize) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("tty59-scm-count-")
            .tempdir()
            .expect("a scratch directory");
        let root = std::fs::canonicalize(dir.path()).expect("it resolves");
        git(&root, &["init", "--quiet"]);
        git(&root, &["config", "user.email", "test@tty59"]);
        git(&root, &["config", "user.name", "tty59"]);
        std::fs::write(root.join("base.txt"), "0\n").unwrap();
        git(&root, &["add", "base.txt"]);
        git(
            &root,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "base",
            ],
        );
        std::fs::write(root.join("base.txt"), "1\n").unwrap();
        git(&root, &["add", "base.txt"]);
        std::fs::write(root.join("base.txt"), "2\n").unwrap();
        for i in 0..untracked {
            std::fs::write(root.join(format!("u{i}.txt")), "x\n").unwrap();
        }
        // The directory goes when the caller lets go of it, whatever the
        // test's outcome.
        (dir, root)
    }

    /// Where the count's inputs stand, for the message of a wait that gave up:
    /// the panel, the pane's own directories, the mirror, what the status
    /// cache holds and who holds the repository.
    fn state_dump(app: &Tty59App, cx: &gpui::App) -> String {
        let (effective, git_cwd) = app
            .tabs
            .first()
            .and_then(|tab| tab.pane.first_leaf())
            .and_then(|leaf| leaf.terminal().cloned())
            .map(|pane| {
                let view = pane.read(cx);
                (
                    view.effective_cwd(),
                    view.git_status_cwd().map(Path::to_path_buf),
                )
            })
            .unwrap_or_default();
        let target = app.scm.count_repo.as_ref();
        let (loaded, holders, epoch) = match target {
            Some(repo) => (
                crate::terminal::git_data::status_of(cx, repo.host, &repo.root).is_some(),
                cx.try_global::<ScmData>()
                    .is_some_and(|data| data.is_subscribed(repo.host, &repo.root)),
                cx.try_global::<ScmData>()
                    .map(|data| data.epoch(repo.host, &repo.root)),
            ),
            None => (false, false, None),
        };
        format!(
            "panel visible={} tab={:?} | pane effective_cwd={effective:?} git_status_cwd={git_cwd:?} \
             | count_repo={:?} count_cwd={:?} scm.repo={:?} | count={:?} status_loaded={loaded} \
             held={holders} epoch={epoch:?} | last_tab_status_repo={:?}",
            app.right_panel_visible,
            app.right_panel_tab,
            app.scm.count_repo,
            app.scm.count_cwd,
            app.scm.repo,
            app.scm_changes_count(cx),
            app.scm.last_tab_status.as_ref().map(|(repo, _)| repo),
        )
    }

    /// Wait for `until`, for as long as 30 seconds, on the real clock alone.
    /// Every fixture that plants the pane's directory uses this one: the
    /// pane's own poll runs on the executor clock and would put back the
    /// directory the pane really has, so that clock is left where it is.
    fn wait_until(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        until: impl Fn(&Tty59App, &gpui::App) -> bool,
    ) {
        wait(app, vcx, false, until)
    }

    /// [`wait_until`] that also moves the executor clock 300 ms a round, as
    /// `test_window::quiesce` does: the repository's debounce waits on that
    /// clock (`scm_debounce`) and compares against the real one. Only for a
    /// pane whose directory is its real one (`DaemonMsg::Cwd`), since the pane
    /// polls on that clock too.
    fn wait_advancing(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        until: impl Fn(&Tty59App, &gpui::App) -> bool,
    ) {
        wait(app, vcx, true, until)
    }

    fn wait(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        advance: bool,
        until: impl Fn(&Tty59App, &gpui::App) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            app.update_in(vcx, |_, _, cx| cx.notify());
            if advance {
                vcx.executor()
                    .advance_clock(std::time::Duration::from_millis(300));
            }
            vcx.background_executor.run_until_parked();
            if app.update_in(vcx, |app, _, cx| until(app, cx)) {
                return;
            }
            if std::time::Instant::now() >= deadline {
                let state = app.update_in(vcx, |app, _, cx| state_dump(app, cx));
                panic!(
                    "the wanted state never arrived (executor clock advanced: {advance}): {state}"
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Put the first tab's pane in `cwd`, and tell the cheap per-tab cache that
    /// directory is in the repository `root` — which is how the app learns
    /// where a pane is, with no Changes panel involved.
    fn plant(app: &Tty59App, cwd: &Path, root: &Path, cx: &mut gpui::App) {
        plant_with(app, cwd, root, (0, 0), cx)
    }

    /// [`plant`] with the cheap status's diff counts, which is what a command
    /// that touched the repository changes.
    fn plant_with(app: &Tty59App, cwd: &Path, root: &Path, counts: (u32, u32), cx: &mut gpui::App) {
        let leaf = app.tabs[0].pane.first_leaf().expect("test tab has a pane");
        leaf.terminal()
            .expect("test pane is a terminal")
            .update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.to_path_buf()))
            });
        cx.update_global::<GitStatusCache, _>(|cache, _| {
            cache.finish_probe(
                HostId::LOCAL,
                cwd,
                Some(RepoSnapshot {
                    root: root.to_path_buf(),
                    home: root.to_path_buf(),
                    branch: "main".into(),
                    counts: Some(counts),
                }),
            );
        });
    }

    fn show(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        visible: bool,
        tab: RightPanelTab,
    ) {
        app.update_in(vcx, |app, _, cx| {
            app.right_panel_visible = visible;
            app.right_panel_tab = tab;
            cx.notify();
        });
    }

    fn selector(tab: RightPanelTab) -> &'static str {
        Box::leak(format!("right-panel-tab-{}", tab as usize).into_boxed_str())
    }

    /// The whole of the first look: no Changes render has happened, so the
    /// panel's own `repo` is `None`, and the count is there anyway — resolved
    /// from the pane, probed off-thread, drawn on the tab.
    #[gpui::test]
    fn the_count_is_there_before_the_changes_tab_was_ever_opened(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repo_with(3);
        let (app, mut vcx, _pane) = test_window::harness_with_pane(cx);
        app.update_in(&mut vcx, |app, _, cx| plant(app, &root, &root, cx));
        show(&app, &mut vcx, true, RightPanelTab::Info);

        // 1 staged-then-edited file and 3 untracked: 4 files, 5 rows.
        wait_until(&app, &mut vcx, |app, cx| {
            app.scm_changes_count(cx) == Some(ChangesCount::Files(4))
        });
        app.update_in(&mut vcx, |app, _, _| {
            assert!(
                app.scm.repo.is_none(),
                "the Changes tab was never rendered, so it recorded no repository"
            );
        });
        vcx.run_until_parked();
        let count = vcx
            .debug_bounds("right-panel-tab-count")
            .expect("the Changes tab draws its count while another tab is current");
        let label = vcx
            .debug_bounds("right-panel-tab-label")
            .expect("and its label");
        let gap = (count.left() - label.right()).as_f32();
        assert!(
            (gap - crate::ui::tab_strip::TAB_COUNT_GAP).abs() < 0.5,
            "the count sits one gap after the label, not {gap}px"
        );
    }

    /// The four tabs' drawn width and the width the panel's minimum was built
    /// from, with `untracked + 1` files on the Changes tab.
    fn tab_row_widths(cx: &mut TestAppContext, untracked: usize) -> (f32, f32) {
        let (_dir, root) = repo_with(untracked);
        let (app, mut vcx, _pane) = test_window::harness_with_pane(cx);
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_width.set(crate::ui::right_panel::MIN_WIDTH);
            plant(app, &root, &root, cx)
        });
        show(&app, &mut vcx, true, RightPanelTab::Info);
        wait_until(&app, &mut vcx, |app, cx| {
            app.scm_changes_count(cx).is_some()
        });
        vcx.run_until_parked();

        let first = vcx
            .debug_bounds(selector(RightPanelTab::Info))
            .expect("first tab is drawn");
        let last = vcx
            .debug_bounds(selector(RightPanelTab::GitHub))
            .expect("last tab is drawn");
        let span = (last.right() - first.left()).as_f32();
        let reserved = app.update_in(&mut vcx, |app, _, _| app.right_tab_metrics.tabs_w());
        (span, reserved)
    }

    /// What the painter draws and what the floor reserves are one geometry: a
    /// two-digit count (the audit's 11) takes no more than the floor holds.
    #[gpui::test]
    fn eleven_changes_fit_in_what_the_floor_reserves(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (span, reserved) = tab_row_widths(cx, 10);
        assert!(
            span <= reserved + 0.5,
            "the tabs take {span}px but the floor reserves {reserved}px"
        );
    }

    /// With the widest count on the tab (`99+`, whatever the real number is)
    /// the tabs take exactly what the floor reserves: the reservation is that
    /// count's, held when there is none too.
    #[gpui::test]
    fn the_widest_count_takes_exactly_the_reserved_width(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (span, reserved) = tab_row_widths(cx, 120);
        assert!(
            (span - reserved).abs() < 1.0,
            "the painter and the floor measure the same row: {span}px against {reserved}px"
        );
    }

    /// The count follows the pane the user is in now. The panel's own `repo`
    /// still names the repository of the last Changes visit, and must not win.
    #[gpui::test]
    fn the_count_follows_the_current_pane_not_the_last_changes_visit(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let ((_dir_a, a), (_dir_b, b)) = (repo_with(1), repo_with(5));
        let (app, mut vcx, _pane) = test_window::harness_with_pane(cx);

        app.update_in(&mut vcx, |app, _, cx| plant(app, &a, &a, cx));
        show(&app, &mut vcx, true, RightPanelTab::Scm);
        wait_until(&app, &mut vcx, |app, cx| {
            app.scm.repo.is_some() && app.scm_changes_count(cx) == Some(ChangesCount::Files(2))
        });

        // Off the Changes tab, into the other repository.
        app.update_in(&mut vcx, |app, _, cx| plant(app, &b, &b, cx));
        show(&app, &mut vcx, true, RightPanelTab::Info);
        wait_until(&app, &mut vcx, |app, cx| {
            app.scm_changes_count(cx) == Some(ChangesCount::Files(6))
        });
        app.update_in(&mut vcx, |app, _, _| {
            assert_eq!(
                app.scm.repo.as_ref().map(|r| &r.root),
                Some(&a),
                "the panel's own repository is the stale one"
            );
            assert_eq!(app.scm.count_repo.as_ref().map(|r| &r.root), Some(&b));
        });
    }

    /// A commit made behind the tab's back reaches the count through the
    /// repository's own watch, with the Changes tab never opened.
    #[gpui::test]
    fn a_commit_clears_the_count_on_a_tab_that_never_showed_changes(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repo_with(2);
        let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
        // The pane really is in the repository, as `scm_panel_on` puts it, so
        // its poll (on the executor clock moved below) keeps that directory
        // and reads the cheap status itself: nothing here is planted.
        DaemonMsg::Cwd(root.clone())
            .encode(&mut pane)
            .expect("the pane's socket takes the cwd");
        show(&app, &mut vcx, true, RightPanelTab::Info);
        wait_advancing(&app, &mut vcx, |app, cx| {
            app.scm_changes_count(cx) == Some(ChangesCount::Files(3))
        });

        git(&root, &["add", "-A"]);
        git(
            &root,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "all",
            ],
        );
        wait_advancing(&app, &mut vcx, |app, cx| {
            app.scm_changes_count(cx).is_none()
        });
        assert!(app.update_in(&mut vcx, |app, _, _| app.scm.repo.is_none()));
    }

    /// Who holds the repository open: the Changes panel and the count are two
    /// holders of one repository, the count alone holds it on another tab, and
    /// with the panel shut nothing does and nothing was read.
    #[gpui::test]
    fn the_count_holds_the_repository_only_while_the_panel_is_open(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repo_with(1);
        let (app, mut vcx, _pane) = test_window::harness_with_pane(cx);
        app.update_in(&mut vcx, |app, _, cx| plant(app, &root, &root, cx));

        let holders = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| {
                cx.default_global::<ScmData>()
                    .subscriptions()
                    .count(HostId::LOCAL, &root)
            })
        };

        // Shut: frames go by, and nothing is held or read.
        show(&app, &mut vcx, false, RightPanelTab::Info);
        for _ in 0..10 {
            app.update_in(&mut vcx, |_, _, cx| cx.notify());
            vcx.background_executor.run_until_parked();
        }
        assert_eq!(holders(&mut vcx), 0);
        assert!(app.update_in(&mut vcx, |app, _, _| app.scm.count_repo.is_none()));
        assert!(
            vcx.update(|_, cx| crate::terminal::git_data::status_of(cx, HostId::LOCAL, &root))
                .is_none()
        );

        // The Changes tab: the panel and the count, one hold each.
        show(&app, &mut vcx, true, RightPanelTab::Scm);
        wait_until(&app, &mut vcx, |app, cx| {
            app.scm_changes_count(cx).is_some()
        });
        assert_eq!(holders(&mut vcx), 2);

        // Another tab: the count alone keeps it.
        show(&app, &mut vcx, true, RightPanelTab::Info);
        wait_until(&app, &mut vcx, |app, cx| {
            app.scm_changes_count(cx).is_some()
        });
        assert_eq!(holders(&mut vcx), 1);

        // Shut again: both let go, and the target is cleared with them.
        show(&app, &mut vcx, false, RightPanelTab::Info);
        wait_until(&app, &mut vcx, |app, _| app.scm.count_repo.is_none());
        assert_eq!(holders(&mut vcx), 0);
    }

    /// A command finishing refreshes the repository the count is for, even
    /// while the panel's own `repo` still names another from an earlier visit —
    /// and the comparison is per repository, so the same status under the other
    /// repository is a pane that moved, not a command.
    #[gpui::test]
    fn a_command_boundary_refreshes_the_count_target_and_not_the_last_visit(
        cx: &mut TestAppContext,
    ) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = test_window::harness(cx);
        let (stale, current) = (
            RepoKey {
                host: HostId::LOCAL,
                root: PathBuf::from("/stale"),
            },
            RepoKey {
                host: HostId::LOCAL,
                root: PathBuf::from("/current"),
            },
        );
        app.update_in(&mut vcx, |app, _, cx| {
            cx.update_global::<GitStatusCache, _>(|cache, _| {
                cache.finish_probe(
                    HostId::LOCAL,
                    Path::new("/current/src"),
                    Some(RepoSnapshot {
                        root: PathBuf::from("/current"),
                        home: PathBuf::from("/current"),
                        branch: "main".into(),
                        counts: Some((0, 0)),
                    }),
                );
            });
            app.scm.repo = Some(stale.clone());
            app.scm.count_repo = Some(current.clone());
            app.scm.count_cwd = Some(PathBuf::from("/current/src"));

            // The comparison is per repository: what was last seen is the SAME
            // cheap status as the count target's own, but filed under the other
            // repository (the pane moved from it). Equal statuses alone would
            // read as "nothing happened"; the repository is what says otherwise.
            let same = cx
                .global::<GitStatusCache>()
                .status_for(HostId::LOCAL, Path::new("/current/src"))
                .expect("the cache answers for the count target's directory");
            app.scm.last_tab_status = Some((stale.clone(), same.clone()));
            assert!(
                app.scm.last_tab_status == Some((stale.clone(), same)),
                "precondition: the last status seen is the target's own, under the other repository"
            );
            let epoch = |cx: &mut gpui::App, key: &RepoKey| {
                cx.default_global::<ScmData>().epoch(key.host, &key.root)
            };
            let (current_before, stale_before) = (epoch(cx, &current), epoch(cx, &stale));

            app.right_panel_refresh_changes(cx);
            assert_eq!(
                app.scm.last_tab_status.as_ref().map(|(k, _)| k),
                Some(&current),
                "the status read is the count target's, from the pane's directory"
            );
            assert!(
                epoch(cx, &current) > current_before,
                "the count target's repository was marked changed"
            );
            assert_eq!(
                epoch(cx, &stale),
                stale_before,
                "the repository of the last visit was not"
            );
        });
    }

    /// A directory that is not a repository. The pane is really in it and the
    /// ordinary cheap per-tab probe has answered "no repository" (waited for,
    /// not assumed), the right panel is open on Info, and an earlier Changes
    /// visit has left its own repository in `scm.repo`. Over bounded rounds of
    /// frames and executor turns the count has no target, no count, and the
    /// status store has no subscription, no status and no epoch movement for
    /// either the stale repository or this directory.
    ///
    /// The claim is exactly that: NO ADDED full-status git work, repository
    /// resolution or polling. The pane's own cheap cwd detection (its poll and
    /// `probe_repo`) is the baseline's and keeps running.
    #[gpui::test]
    fn a_directory_that_is_not_a_repository_adds_no_status_work_and_no_count(
        cx: &mut TestAppContext,
    ) {
        crate::core::config::pin_test_config_dir();
        let dir = tempfile::Builder::new()
            .prefix("tty59-no-repo-")
            .tempdir()
            .expect("a scratch directory");
        let plain = std::fs::canonicalize(dir.path()).expect("it resolves");
        std::fs::write(plain.join("note.txt"), "x\n").unwrap();
        let stale = repo("/stale/previous-visit");
        let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
        DaemonMsg::Cwd(plain.clone())
            .encode(&mut pane)
            .expect("the pane's socket takes the cwd");
        app.update_in(&mut vcx, |app, _, cx| {
            // What an earlier visit to the Changes tab left behind.
            app.scm.repo = Some(stale.clone());
            app.right_panel_visible = true;
            app.right_panel_tab = RightPanelTab::Info;
            cx.notify();
        });

        // The ordinary cheap probe answers "not a repository": `Some(None)`.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            app.update_in(&mut vcx, |_, _, cx| cx.notify());
            vcx.executor()
                .advance_clock(std::time::Duration::from_millis(300));
            vcx.background_executor.run_until_parked();
            let answered = vcx.update(|_, cx| {
                cx.global::<GitStatusCache>()
                    .known_repo_for(HostId::LOCAL, &plain)
                    == Some(None)
            });
            if answered {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the cheap probe never answered 'not a repository' for the pane's directory: {}",
                app.update_in(&mut vcx, |app, _, cx| state_dump(app, cx))
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.update_in(&mut vcx, |app, _, _| {
            assert!(
                app.scm.repo_override.is_none() && app.scm.override_tab.is_none(),
                "precondition: no explicit repository pick"
            );
            assert!(
                app.tabs.first().is_some_and(|tab| tab.code.is_none()),
                "precondition: no editor"
            );
            assert_eq!(app.right_panel_tab, RightPanelTab::Info);
        });

        for round in 0..20 {
            app.update_in(&mut vcx, |_, _, cx| cx.notify());
            vcx.executor()
                .advance_clock(std::time::Duration::from_millis(300));
            vcx.background_executor.run_until_parked();
            app.update_in(&mut vcx, |app, _, cx| {
                assert!(
                    app.scm.count_repo.is_none() && app.scm.count_cwd.is_none(),
                    "round {round}: the count has no target: {}",
                    state_dump(app, cx)
                );
                assert!(
                    app.scm_changes_count(cx).is_none(),
                    "round {round}: no count: {}",
                    state_dump(app, cx)
                );
            });
        }

        vcx.update(|_, cx| {
            let data = cx.try_global::<ScmData>();
            assert!(
                data.is_none_or(ScmData::is_quiet),
                "the status store holds no subscription and no watch"
            );
            for root in [&stale.root, &plain] {
                assert!(
                    !data.is_some_and(|d| d.is_subscribed(HostId::LOCAL, root)),
                    "{root:?} is not held"
                );
                assert!(
                    crate::terminal::git_data::status_of(cx, HostId::LOCAL, root).is_none(),
                    "{root:?} has no full status"
                );
                assert_eq!(
                    data.map_or(0, |d| d.epoch(HostId::LOCAL, root)),
                    0,
                    "{root:?} was never marked changed"
                );
            }
        });
    }

    fn set_ui_font_size(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, size: f32) {
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<crate::core::config::Config>().clone();
            cfg.ui_font_size = size;
            cx.set_global(cfg);
        });
        app.update_in(vcx, |_, _, cx| cx.notify());
        vcx.run_until_parked();
    }

    /// The probe the audit asked for, as far as a test window can make it: with
    /// the widest count on the tab, in every shipped language at 14 and 16px,
    /// all four tabs sit inside the panel at its minimum width, and what the
    /// floor comes to is printed (`--nocapture`).
    ///
    /// A test window shapes through a text system that gives every character
    /// the same advance, so the *numbers* printed here are not the real
    /// widths — those come from a native run — but containment holding in each
    /// of these says the painter and the floor are one geometry whatever the
    /// label lengths and the size are.
    #[gpui::test]
    fn all_four_tabs_stay_inside_the_panel_at_its_minimum(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repo_with(120);
        let (app, mut vcx, _pane) = test_window::harness_with_pane(cx);
        app.update_in(&mut vcx, |app, _, cx| {
            // Whatever width was saved, the panel is at its floor.
            app.right_panel_width.set(0.);
            plant(app, &root, &root, cx)
        });
        show(&app, &mut vcx, true, RightPanelTab::Info);
        wait_until(&app, &mut vcx, |app, cx| {
            app.scm_changes_count(cx).is_some()
        });

        for locale in ["en", "ru-RU", "zh-CN", "ja-JP"] {
            // `set_locale` falls back to English for a code it does not know.
            assert!(crate::ui::i18n::find_language(locale).is_some(), "{locale}");
            crate::ui::i18n::set_locale(locale);
            for size in [14., 16.] {
                set_ui_font_size(&app, &mut vcx, size);
                let floor = app.update_in(&mut vcx, |app, window, cx| {
                    app.right_panel_min_px(window, cx)
                });
                let bounds = |vcx: &mut VisualTestContext, selector: &'static str| {
                    vcx.debug_bounds(selector)
                        .unwrap_or_else(|| panic!("{locale} {size}px: {selector} was not drawn"))
                };
                let panel = bounds(&mut vcx, "right-panel");
                let first = bounds(&mut vcx, selector(RightPanelTab::Info));
                let last = bounds(&mut vcx, selector(RightPanelTab::GitHub));
                eprintln!(
                    "B3-P floor {locale} {size}px: min {floor}px (240 default), panel {}px, \
                     tabs {}px",
                    panel.size.width.as_f32(),
                    (last.right() - first.left()).as_f32()
                );
                assert!(
                    panel.size.width.as_f32() + 0.5 >= floor,
                    "{locale} {size}px: the panel is narrower than its own floor"
                );
                assert!(
                    (last.right().as_f32() + crate::ui::right_panel::tabs_chrome_w())
                        <= panel.right().as_f32() + 0.5,
                    "{locale} {size}px: the last tab runs past the panel's trailing chrome"
                );
            }
        }
        crate::ui::i18n::set_locale("en");
    }

    /// The pane moves to another directory of the same repository, and the
    /// cheap status for that directory differs (a command changed something).
    /// Whatever order the cache's landing and the frame that moves the count's
    /// mirror arrive in, the outcome has to be the same: the status last seen
    /// is the new directory's, and the repository has been marked changed.
    ///
    /// The order itself is deliberately not asserted. A first version of this
    /// test required the cache to land BEFORE the mirror moved, in order to
    /// witness a gap where the new status is read against the old directory
    /// and missed; on the first Boat run the observer had already recorded the
    /// new status, so that interleaving was not what happens here and the gap
    /// is an unproven hypothesis, not a defect.
    #[gpui::test]
    fn a_new_directory_in_the_same_repository_marks_it_changed_when_its_status_differs(
        cx: &mut TestAppContext,
    ) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx, _pane) = test_window::harness_with_pane(cx);
        // Not a repository on disk: nothing here may be read, only the cheap
        // cache's answers matter, and a real `git status` would write `.git`
        // and bump the epoch on its own.
        let root = PathBuf::from("/ordering/repo");
        let (src, docs) = (root.join("src"), root.join("docs"));

        app.update_in(&mut vcx, |app, _, cx| plant(app, &src, &root, cx));
        show(&app, &mut vcx, true, RightPanelTab::Info);
        wait_until(&app, &mut vcx, |app, _| {
            app.scm.count_cwd.as_deref() == Some(src.as_path())
        });
        app.update_in(&mut vcx, |app, _, cx| app.right_panel_refresh_changes(cx));
        vcx.run_until_parked();
        let epoch = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| cx.default_global::<ScmData>().epoch(HostId::LOCAL, &root))
        };
        let before = epoch(&mut vcx);
        let baseline = app.update_in(&mut vcx, |app, _, _| app.scm.last_tab_status.clone());
        assert!(
            baseline.is_some(),
            "the baseline status was recorded against src"
        );

        app.update_in(&mut vcx, |app, _, cx| {
            plant_with(app, &docs, &root, (2, 0), cx)
        });
        vcx.run_until_parked();

        assert_eq!(
            app.update_in(&mut vcx, |app, _, _| app.scm.count_cwd.clone()),
            Some(docs.clone()),
            "the mirror is on the pane's new directory"
        );
        let landed = vcx.update(|_, cx| {
            cx.global::<GitStatusCache>()
                .status_for(HostId::LOCAL, &docs)
        });
        assert!(
            landed.is_some() && landed != baseline.as_ref().map(|(_, status)| status.clone()),
            "the cache holds a status for the new directory that differs from the last one seen"
        );
        assert!(
            app.update_in(&mut vcx, |app, _, _| app.scm.last_tab_status.clone())
                == landed.map(|status| (repo(root.to_str().unwrap()), status)),
            "the status last seen is the new directory's, for this repository"
        );
        assert!(
            epoch(&mut vcx) > before,
            "the new directory's status differs from the one last seen, and nothing marked the \
             repository changed"
        );
    }

    fn repo(root: &str) -> RepoKey {
        RepoKey {
            host: HostId::LOCAL,
            root: PathBuf::from(root),
        }
    }

    fn leak(selector: String) -> &'static str {
        Box::leak(selector.into_boxed_str())
    }

    fn status_with(
        root: &str,
        head: HeadState,
        ahead_behind: Option<(u32, u32)>,
    ) -> WorkingTreeStatus {
        WorkingTreeStatus {
            root: PathBuf::from(root),
            home: PathBuf::from(root),
            head,
            upstream: Some("origin/main".into()),
            ahead_behind,
            entries: Vec::new(),
            total_entries: 0,
            truncated: false,
            stash_count: 0,
            operation: None,
            prefilled_message: None,
        }
    }

    fn branch(name: &str) -> HeadState {
        HeadState::Branch {
            name: name.into(),
            oid: "1111111".into(),
        }
    }

    /// The branch menu as the panel builds it, drawn. The entity is handed
    /// back because it is the menu: its item listeners hold it weakly, so once
    /// the caller lets go of it a click on the drawn frame does nothing.
    fn draw_branch_menu(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
    ) -> Entity<PopupMenu> {
        let build = app.update_in(vcx, |app, _, cx| app.scm_branch_menu(repo, status, cx));
        let menu = vcx.update(|window, cx| {
            PopupMenu::build(window, cx, |menu, window, cx| build(menu, window, cx))
        });
        vcx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(420.), gpui::px(700.)),
            |_, _| menu.clone().into_any_element(),
        );
        menu
    }

    fn click(vcx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
        use gpui::{
            Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput,
        };
        vcx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at,
                    pressed_button: None,
                    modifiers: Modifiers::none(),
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                }),
                cx,
            );
            window.refresh();
        });
        vcx.run_until_parked();
    }

    /// The opened menu, not a label builder: Pull and Push carry the numbers
    /// the snapshot says (3 ahead, 1 behind), a blocked Push says why in place
    /// of a number, two checkouts of one name show where they live, and
    /// clicking a repository selects exactly that repository.
    #[gpui::test]
    fn the_branch_menu_shows_counts_reasons_and_parents_and_keeps_identities(
        cx: &mut TestAppContext,
    ) {
        crate::core::config::pin_test_config_dir();
        crate::ui::i18n::set_locale("en");
        let (app, mut vcx) = test_window::harness(cx);
        let (one, two) = (repo("/work/one/acme-web"), repo("/work/two/acme-web"));
        app.update_in(&mut vcx, |app, _, _| {
            for (at, key) in [&one, &two].into_iter().enumerate() {
                app.scm.roots.insert(
                    (HostId::LOCAL, PathBuf::from(format!("/cwd/{at}"))),
                    (Instant::now(), Some(key.root.clone())),
                );
            }
        });

        // 3 ahead, 1 behind: Pull says 1, Push says 3, never the other way.
        let level = status_with("/work/one/acme-web", branch("main"), Some((3, 1)));
        let menu = draw_branch_menu(&app, &mut vcx, &one, &level);
        let shown = |vcx: &mut VisualTestContext, selector: String| {
            vcx.debug_bounds(leak(selector)).is_some()
        };
        assert!(shown(&mut vcx, "scm-menu-count:Pull:1".into()));
        assert!(shown(&mut vcx, "scm-menu-count:Push:3".into()));
        assert!(!shown(&mut vcx, "scm-menu-count:Pull:3".into()));
        assert!(!shown(&mut vcx, "scm-menu-count:Push:1".into()));
        assert!(
            !shown(&mut vcx, "scm-menu-count:Fetch:3".into())
                && !shown(&mut vcx, "scm-menu-count:Fetch:1".into()),
            "Fetch carries no number"
        );

        // The two same-named repositories are told apart by their folders.
        assert!(shown(&mut vcx, "menu-row-name:acme-web".into()));
        assert!(shown(&mut vcx, "menu-row-note:/work/one".into()));
        assert!(shown(&mut vcx, "menu-row-note:/work/two".into()));

        // Each row selects its own repository, whatever it is called.
        let center = |vcx: &mut VisualTestContext, key: &RepoKey| {
            vcx.debug_bounds(leak(format!("scm-repo-choice:{}", key.root.display())))
                .expect("the repository row is drawn")
                .center()
        };
        let at = center(&mut vcx, &two);
        click(&mut vcx, at);
        app.update_in(&mut vcx, |app, _, _| {
            assert_eq!(app.scm.repo_override.as_ref(), Some(&two));
            assert_eq!(app.scm.override_tab, Some(app.active));
        });
        // The first click dismissed that menu, as a click does. A second one
        // is a fresh menu, opened the way the panel opens it.
        drop(menu);
        let menu = draw_branch_menu(&app, &mut vcx, &one, &level);
        let at = center(&mut vcx, &one);
        click(&mut vcx, at);
        app.update_in(&mut vcx, |app, _, _| {
            assert_eq!(app.scm.repo_override.as_ref(), Some(&one));
        });

        // A detached HEAD cannot push: no number, and the reason is on screen.
        drop(menu);
        let _menu = draw_branch_menu(
            &app,
            &mut vcx,
            &one,
            &status_with(
                "/work/one/acme-web",
                HeadState::Detached {
                    oid: "2222222".into(),
                },
                Some((3, 1)),
            ),
        );
        assert!(
            shown(
                &mut vcx,
                format!("scm-menu-reason:{}", t(L10nKey::ScmPushDetached))
            ),
            "a blocked Push says why"
        );
        assert!(!shown(&mut vcx, "scm-menu-count:Push:3".into()));
        assert!(
            shown(&mut vcx, "scm-menu-count:Pull:1".into()),
            "Pull is not blocked by a detached HEAD"
        );
    }
}

/// B3b witnesses, written against c70043c7 with literal selectors only: a
/// repository whose full status cannot be read must reach a Retry on the real
/// Changes render, and a folder that is not a repository must offer
/// Initialize repository through its real body. Each fails on c700 because the
/// button is absent, not because an API is.
#[cfg(all(test, unix))]
mod status_lifecycle_witness_gpui_tests {
    use super::*;
    use crate::daemon::protocol::DaemonMsg;
    use crate::terminal::git_data::ScmData;
    use crate::terminal::git_status::GitStatusCache;
    use crate::ui::app::test_window;
    use gpui::{
        Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        PlatformInput, TestAppContext, VisualTestContext,
    };
    use gpui_component::WindowExt as _;
    use tty59_core::core::config::RightPanelTab;

    const RETRY: &str = "panel-scm-status-retry";
    const COPY: &str = "panel-scm-status-copy";
    const INIT: &str = "panel-scm-init-repo";

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed");
    }

    fn scratch(prefix: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("a scratch directory");
        let root = std::fs::canonicalize(dir.path()).expect("it resolves");
        (dir, root)
    }

    /// A real repository that `git rev-parse` answers for and `git status`
    /// cannot: its index is not an index. Nothing here depends on how the
    /// failure is worded, only that the first full read cannot succeed and
    /// that deleting the file mends it.
    fn repository_with_a_broken_index() -> (tempfile::TempDir, PathBuf) {
        let (dir, root) = scratch("tty59-status-failed-");
        git(&root, &["init", "--quiet"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        std::fs::write(root.join(".git/index"), b"not an index").unwrap();
        (dir, root)
    }

    fn mend(root: &Path) {
        std::fs::remove_file(root.join(".git/index")).expect("the broken index is there");
    }

    /// The pane is really in `root` and the Changes tab is open. With `hold`,
    /// the `.git` watch of `root` is kept closed so nothing but the code under
    /// test can re-ask after the fixture mends the repository.
    fn open_changes(
        cx: &mut TestAppContext,
        root: &Path,
        hold: bool,
    ) -> (
        Entity<Tty59App>,
        VisualTestContext,
        crate::daemon::transport::Stream,
    ) {
        let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
        if hold {
            vcx.update(|_, cx| {
                cx.default_global::<ScmData>()
                    .hold_watch_closed(HostId::LOCAL, root)
            });
        }
        DaemonMsg::Cwd(root.to_path_buf())
            .encode(&mut pane)
            .expect("the pane's socket takes the cwd");
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_visible = true;
            app.right_panel_tab = RightPanelTab::Scm;
            cx.notify();
        });
        (app, vcx, pane)
    }

    /// Frames and the executor, on the real clock, until `until` holds or
    /// `secs` have passed. `advance` also moves the executor clock, which the
    /// pane's own poll and the repository's debounce run on.
    fn wait(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        secs: u64,
        advance: bool,
        until: impl Fn(&Tty59App, &gpui::App) -> bool,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            app.update_in(vcx, |_, _, cx| cx.notify());
            if advance {
                vcx.executor().advance_clock(Duration::from_millis(300));
            }
            vcx.run_until_parked();
            if app.update_in(vcx, |app, _, cx| until(app, cx)) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether `id` shows up with nothing asking for a frame but the code
    /// under test: only the executor is pumped, the app is never notified.
    fn drawn_unprompted(vcx: &mut VisualTestContext, id: &'static str, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            vcx.run_until_parked();
            if vcx.debug_bounds(id).is_some() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Wait for `id` to be drawn, pumping frames as the panel's own tests do.
    fn appears(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        id: &'static str,
        secs: u64,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            app.update_in(vcx, |_, _, cx| cx.notify());
            vcx.run_until_parked();
            if vcx.debug_bounds(id).is_some() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn click(vcx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
        vcx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at,
                    pressed_button: None,
                    modifiers: Modifiers::none(),
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                }),
                cx,
            );
            window.refresh();
        });
        vcx.run_until_parked();
    }

    fn press(vcx: &mut VisualTestContext, id: &'static str) {
        let at = vcx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} is not drawn"))
            .center();
        click(vcx, at);
    }

    fn full_status(vcx: &mut VisualTestContext, root: &Path) -> Option<Arc<WorkingTreeStatus>> {
        vcx.update(|_, cx| crate::terminal::git_data::status_of(cx, HostId::LOCAL, root))
    }

    /// The first full read of a known repository cannot succeed. The real
    /// Changes render must say so with a Retry instead of "Loading…" for ever,
    /// the failure rests rather than being re-asked every frame, and Retry
    /// reads the mended repository again at once, well inside the failure's
    /// ten-second rest.
    #[gpui::test]
    fn a_status_that_cannot_be_read_offers_retry_and_retry_reads_it_again(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repository_with_a_broken_index();
        let (app, mut vcx, _pane) = open_changes(cx, &root, true);

        assert!(
            appears(&app, &mut vcx, RETRY, 20),
            "the first full status failed and the Changes tab never offered Retry \
             (it is still on Loading)"
        );
        assert!(
            full_status(&mut vcx, &root).is_none(),
            "nothing was ever read, so nothing is cached"
        );
        assert!(
            vcx.debug_bounds(COPY).is_some(),
            "the failure carried a reason, so it can be copied"
        );

        mend(&root);
        // Frames keep coming while the user looks at the error; none of them
        // may re-ask before Retry is pressed.
        wait(&app, &mut vcx, 2, false, |_, _| false);
        assert!(
            full_status(&mut vcx, &root).is_none(),
            "a failed probe rests instead of being re-asked at frame rate"
        );
        assert!(
            vcx.debug_bounds(RETRY).is_some(),
            "the error is still what the panel shows"
        );

        press(&mut vcx, RETRY);
        let read = wait(&app, &mut vcx, 6, false, |_, cx| {
            crate::terminal::git_data::status_of(cx, HostId::LOCAL, &root).is_some()
        });
        assert!(
            read,
            "Retry did not read the mended repository within the failure's rest"
        );
        let status = full_status(&mut vcx, &root).expect("read");
        assert_eq!(status.untracked().count(), 1, "a.txt is untracked");
        app.update_in(&mut vcx, |_, _, cx| cx.notify());
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds(RETRY).is_none(),
            "a repository that reads is no longer drawn as an error"
        );
    }

    /// Nothing but the landing itself asks for the frame that turns Loading
    /// into the error: no unrelated input, no notify from the test.
    #[gpui::test]
    fn the_first_failure_repaints_by_itself(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repository_with_a_broken_index();
        let (_app, mut vcx, _pane) = open_changes(cx, &root, true);
        // One frame so the panel starts and asks; after that nobody notifies.
        vcx.run_until_parked();
        assert!(
            drawn_unprompted(&mut vcx, RETRY, 20),
            "the probe failed and the panel stayed on Loading until something else repainted"
        );
    }

    /// Copy error puts the reason the host or git gave on the clipboard, as
    /// one line, and nothing synthetic: not a status code made up for it.
    #[gpui::test]
    fn copy_error_puts_the_reason_on_the_clipboard(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repository_with_a_broken_index();
        let (app, mut vcx, _pane) = open_changes(cx, &root, true);
        assert!(
            appears(&app, &mut vcx, COPY, 20),
            "no Copy error: the failure never reached the panel"
        );
        press(&mut vcx, COPY);
        let copied = vcx
            .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .expect("Copy error wrote to the clipboard");
        assert!(!copied.trim().is_empty(), "an empty reason was copied");
        assert!(
            !copied.contains('\n'),
            "the reason is git's first line, not its whole stderr: {copied:?}"
        );
        assert!(
            copied.to_lowercase().contains("index"),
            "git said what is wrong with the index, and that is what was copied: {copied:?}"
        );
    }

    /// A folder that is not a repository: its real body offers Initialize
    /// repository, and pressing it makes the folder a repository that the
    /// panel reads, the Changes count counts and the cheap cache knows,
    /// promptly, while an unrelated cached root survives.
    #[gpui::test]
    fn initializing_a_repository_converges_the_panel_and_the_count(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, plain) = scratch("tty59-init-");
        std::fs::write(plain.join("note.txt"), "x\n").unwrap();
        let (app, mut vcx, _pane) = open_changes(cx, &plain, false);

        let negative = wait(&app, &mut vcx, 30, true, |_, cx| {
            cx.global::<GitStatusCache>()
                .known_repo_for(HostId::LOCAL, &plain)
                == Some(None)
        });
        assert!(
            negative,
            "precondition: the cheap probe answered 'not a repository' for the pane's directory"
        );
        assert!(
            appears(&app, &mut vcx, INIT, 20),
            "a folder that is not a repository offers no way to make it one"
        );

        let unrelated = (HostId::LOCAL, PathBuf::from("/unrelated/cwd"));
        app.update_in(&mut vcx, |app, _, _| {
            app.scm.roots.insert(
                unrelated.clone(),
                (Instant::now(), Some(PathBuf::from("/unrelated/root"))),
            );
        });

        press(&mut vcx, INIT);
        let panel = wait(&app, &mut vcx, 4, true, |app, cx| {
            app.scm.repo.as_ref().is_some_and(|r| r.root == plain)
                && crate::terminal::git_data::status_of(cx, HostId::LOCAL, &plain).is_some()
        });
        assert!(plain.join(".git").is_dir(), "git init ran in the folder");
        assert!(
            panel,
            "the Changes panel did not pick up the new repository promptly"
        );
        let counted = wait(&app, &mut vcx, 4, true, |app, cx| {
            app.scm_changes_count(cx) == Some(ChangesCount::Files(1))
        });
        assert!(
            counted,
            "the Changes count did not converge: the cheap negative answer for the folder \
             outlived the initialization"
        );
        assert!(
            app.update_in(&mut vcx, |app, _, _| app.scm.roots.contains_key(&unrelated)),
            "an unrelated cached root was erased"
        );
    }

    /// A refused initialization says so and creates nothing: the folder is
    /// removed under the panel after it drew its button, so git cannot run
    /// there.
    #[gpui::test]
    fn a_refused_initialization_is_reported_and_leaves_the_state_alone(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, plain) = scratch("tty59-init-refused-");
        let (app, mut vcx, _pane) = open_changes(cx, &plain, false);
        assert!(
            appears(&app, &mut vcx, INIT, 30),
            "a folder that is not a repository offers no way to make it one"
        );
        let toasts = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
            app.update_in(vcx, |_, window, cx| window.notifications(cx).len())
        };
        assert_eq!(toasts(&app, &mut vcx), 0, "nothing was reported yet");

        std::fs::remove_dir_all(&plain).expect("the folder goes");
        press(&mut vcx, INIT);
        let deadline = Instant::now() + Duration::from_secs(10);
        while toasts(&app, &mut vcx) == 0 && Instant::now() < deadline {
            app.update_in(&mut vcx, |_, _, cx| cx.notify());
            vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            toasts(&app, &mut vcx),
            1,
            "a refused git init has to be reported, once"
        );
        assert!(
            !plain.exists(),
            "nothing was created where git could not run"
        );
        assert!(
            app.update_in(&mut vcx, |app, _, _| {
                app.scm
                    .roots
                    .get(&(HostId::LOCAL, plain.clone()))
                    .is_some_and(|(_, root)| root.is_none())
            }),
            "a failed initialization leaves the not-a-repository answer in place"
        );
    }

    // ---- added with the production change: these use the new API ----

    fn local_host(cx: &gpui::App) -> SharedHost {
        crate::ui::host_registry::HostRegistry::lookup(cx, HostId::LOCAL)
            .expect("the local host is registered")
    }

    /// Pump until `ScmData` says the repository's last probe left a failure to
    /// rest on (`is_stale` is false only then, for a repository never read).
    fn failure_rests(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, root: &Path) -> bool {
        wait(app, vcx, 20, false, |_, cx| {
            cx.try_global::<ScmData>()
                .is_some_and(|data| !data.is_stale(HostId::LOCAL, root))
        })
    }

    /// The probe of a host that was wiped while it ran answers to nobody: its
    /// failure is not recorded, and the next ask is a fresh one that is.
    #[gpui::test]
    fn a_failure_that_lands_after_the_host_was_wiped_is_not_recorded(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repository_with_a_broken_index();
        let (app, mut vcx) = test_window::harness(cx);
        app.update(&mut vcx, |app, cx| {
            let host = local_host(cx);
            app.scm_refresh(host, root.clone(), cx);
            cx.default_global::<ScmData>().clear_host(HostId::LOCAL);
        });
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
        vcx.update(|_, cx| {
            let data = cx.default_global::<ScmData>();
            assert!(
                data.is_stale(HostId::LOCAL, &root),
                "a stale landing left a failure to rest on"
            );
            assert!(matches!(
                data.status_read(HostId::LOCAL, &root),
                StatusRead::Loading
            ));
        });

        // The same fixture does land a failure when nothing wiped it.
        app.update(&mut vcx, |app, cx| {
            let host = local_host(cx);
            app.scm_refresh(host, root.clone(), cx);
        });
        assert!(
            failure_rests(&app, &mut vcx, &root),
            "control: no failure landed"
        );
        assert!(matches!(
            vcx.update(|_, cx| status_read_of(cx, HostId::LOCAL, &root)),
            StatusRead::Failed(fact) if !failure_text(&fact).trim().is_empty()
        ));
    }

    /// A failure that lands after something marked the repository changed is
    /// still recorded, and the invalidation the landing then asks for ends its
    /// rest through the existing bump. This documents that existing policy: a
    /// late failure is followed by an immediate re-ask, not a ten-second rest.
    #[gpui::test]
    fn a_failure_from_a_superseded_epoch_is_followed_by_a_fresh_ask(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repository_with_a_broken_index();
        let (app, mut vcx) = test_window::harness(cx);
        app.update(&mut vcx, |app, cx| {
            let host = local_host(cx);
            app.scm_refresh(host, root.clone(), cx);
            cx.default_global::<ScmData>().bump(HostId::LOCAL, &root);
        });
        // Our bump is epoch 1; the landing's own invalidation is epoch 2.
        let landed = wait(&app, &mut vcx, 20, false, |_, cx| {
            cx.try_global::<ScmData>()
                .is_some_and(|data| data.epoch(HostId::LOCAL, &root) >= 2)
        });
        assert!(landed, "the superseded probe never landed");
        vcx.update(|_, cx| {
            let data = cx.default_global::<ScmData>();
            assert!(
                data.is_stale(HostId::LOCAL, &root),
                "the bump the landing asked for ended the failure's rest"
            );
            assert!(matches!(
                data.status_read(HostId::LOCAL, &root),
                StatusRead::Loading
            ));
        });
    }

    /// A held status wins over a failed later probe: the panel keeps the very
    /// status it had, with no error drawn over it.
    #[gpui::test]
    fn a_failed_later_probe_leaves_the_held_status_on_screen(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = scratch("tty59-status-retained-");
        git(&root, &["init", "--quiet"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        let (app, mut vcx, _pane) = open_changes(cx, &root, true);
        assert!(
            wait(&app, &mut vcx, 20, false, |_, cx| {
                crate::terminal::git_data::status_of(cx, HostId::LOCAL, &root).is_some()
            }),
            "the first read never landed"
        );
        let held = full_status(&mut vcx, &root).expect("read");

        std::fs::write(root.join(".git/index"), b"not an index").unwrap();
        app.update_in(&mut vcx, |app, _, cx| {
            let host = local_host(cx);
            cx.default_global::<ScmData>().bump(HostId::LOCAL, &root);
            app.scm_refresh(host, root.clone(), cx);
        });
        assert!(
            failure_rests(&app, &mut vcx, &root),
            "the failure never landed"
        );
        match vcx.update(|_, cx| status_read_of(cx, HostId::LOCAL, &root)) {
            StatusRead::Ready(status) => assert!(
                Arc::ptr_eq(&status, &held),
                "the held status is the one still drawn"
            ),
            other => panic!("a failed later probe replaced the held status with {other:?}"),
        }
        app.update_in(&mut vcx, |_, _, cx| cx.notify());
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds(RETRY).is_none(),
            "no error is drawn over a status the panel holds"
        );
    }

    /// Retry's own effects, checked without waiting on a clock: the failure is
    /// gone at once (so the next look is not resting) and so is the panel's own
    /// probe floor, which the failed first look had set.
    #[gpui::test]
    fn retry_ends_the_failure_and_the_panels_probe_floor_at_once(cx: &mut TestAppContext) {
        let _serial = serial();
        crate::core::config::pin_test_config_dir();
        let (_dir, root) = repository_with_a_broken_index();
        let (app, mut vcx, _pane) = open_changes(cx, &root, true);
        assert!(appears(&app, &mut vcx, RETRY, 20), "no Retry to press");
        let key = (HostId::LOCAL, root.clone());
        app.update_in(&mut vcx, |app, _, cx| {
            assert!(
                matches!(
                    status_read_of(cx, HostId::LOCAL, &root),
                    StatusRead::Failed(_)
                ),
                "the panel is showing a failure"
            );
            assert!(
                app.scm.probe_attempt.contains_key(&key),
                "the failed first look set the panel's probe floor"
            );
            let host = local_host(cx);
            app.scm_retry_status(host, root.clone(), cx);
            assert!(
                !app.scm.probe_attempt.contains_key(&key),
                "Retry left the panel's probe floor in place"
            );
            assert!(
                matches!(
                    status_read_of(cx, HostId::LOCAL, &root),
                    StatusRead::Loading
                ),
                "Retry left the failure and its rest in place"
            );
        });
    }

    /// The init landing, driven directly. With the host the click captured
    /// still registered, the directory's negative answers go and its new root
    /// is marked changed. With another host registered under the same id (a
    /// reconnect) the caches, the panel's root answers and the epoch are left
    /// exactly as they were. A host that is gone from the registry takes the
    /// same branch: nothing registered under the id is not the captured host
    /// (the local host cannot be unregistered, so only the replacement is
    /// driven here).
    #[gpui::test]
    fn an_init_landing_invalidates_only_for_the_host_that_was_captured(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = test_window::harness(cx);
        let dir = PathBuf::from("/init-landing/plain");
        let root = dir.clone();
        let key = (HostId::LOCAL, dir.clone());

        let plant = |app: &mut Tty59App, cx: &mut gpui::App| {
            app.scm.roots.insert(key.clone(), (Instant::now(), None));
            cx.update_global::<GitStatusCache, _>(|cache, _| {
                cache.finish_probe(HostId::LOCAL, &dir, None);
            });
        };
        let state = |app: &Tty59App, cx: &gpui::App| {
            (
                app.scm.roots.contains_key(&key),
                cx.global::<GitStatusCache>()
                    .known_repo_for(HostId::LOCAL, &dir),
                cx.try_global::<ScmData>()
                    .map_or(0, |data| data.epoch(HostId::LOCAL, &root)),
            )
        };
        let landing =
            |app: &mut Tty59App, captured: &SharedHost, cx: &mut gpui::Context<Tty59App>| {
                app.scm_initialized(captured, &dir, Some(root.display().to_string()), cx)
            };

        // Control: the captured host is the registered one.
        app.update(&mut vcx, |app, cx| {
            plant(app, cx);
            let (had_root, known, epoch) = state(app, cx);
            assert!(had_root && known == Some(None), "precondition");
            let captured = local_host(cx);
            landing(app, &captured, cx);
            let (has_root, known, after) = state(app, cx);
            assert!(!has_root, "the panel's negative root answer went");
            assert_eq!(known, None, "so did the cheap cache's");
            assert_eq!(after, epoch + 1, "and the new root was marked changed");
        });

        // A reconnect: another host answers to the same id.
        app.update(&mut vcx, |app, cx| {
            plant(app, cx);
            let before = state(app, cx);
            let captured = local_host(cx);
            crate::ui::host_registry::HostRegistry::insert(
                cx,
                tty59_core::host::local::LocalHost::new(),
            );
            let now = local_host(cx);
            assert!(
                !Arc::ptr_eq(&captured, &now),
                "precondition: a different host"
            );
            landing(app, &captured, cx);
            assert_eq!(
                state(app, cx),
                before,
                "a landing for a replaced host touched the caches"
            );
            crate::ui::host_registry::HostRegistry::insert(cx, captured);
        });
    }

    #[test]
    fn init_repo_makes_the_folder_a_repository_and_names_its_root() {
        let (_dir, dir) = scratch("tty59-init-repo-");
        let host = tty59_core::host::local::LocalHost::new();
        let root = init_repo(&*host, &dir)
            .expect("git init in an empty folder succeeds")
            .expect("and says where the repository is");
        assert!(dir.join(".git").is_dir());
        assert_eq!(
            std::fs::canonicalize(&root).expect("the root exists"),
            dir,
            "the repository is the folder itself, not one above it"
        );
    }

    /// A folder inside a repository is already one: it is never initialised
    /// again, so no repository is nested in another.
    #[test]
    fn init_repo_never_nests_a_repository() {
        let (_dir, outer) = scratch("tty59-init-nested-");
        git(&outer, &["init", "--quiet"]);
        let inner = outer.join("sub");
        std::fs::create_dir_all(&inner).unwrap();
        let host = tty59_core::host::local::LocalHost::new();
        let root = init_repo(&*host, &inner)
            .expect("an existing repository is not an error")
            .expect("it names the repository that holds the folder");
        assert_eq!(std::fs::canonicalize(&root).expect("it exists"), outer);
        assert!(
            !inner.join(".git").exists(),
            "a repository was nested inside another"
        );
    }

    /// A folder git cannot run in is an error with a reason, and nothing is
    /// created where it could not run.
    #[test]
    fn init_repo_reports_a_folder_git_cannot_run_in() {
        let (_dir, dir) = scratch("tty59-init-gone-");
        let gone = dir.join("gone");
        let host = tty59_core::host::local::LocalHost::new();
        let fact = init_repo(&*host, &gone).expect_err("there is nowhere to initialise");
        assert!(
            matches!(&fact, FailureFact::Raw(text) if !text.trim().is_empty()),
            "the refusal carries the host's own words: {fact:?}"
        );
        assert!(!gone.exists());
    }

    /// The failed body for one retained fact, in a window of its own. Helper
    /// coverage for the words and the Copy click: that the real Changes render
    /// routes Failed here is what the witnesses above prove.
    struct FailedBody {
        app: Entity<Tty59App>,
        fact: Arc<FailureFact>,
    }

    impl gpui::Render for FailedBody {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let fact = self.fact.clone();
            let body = self.app.update(cx, |app, cx| {
                let host = local_host(cx);
                app.scm_status_failed(host, PathBuf::from("/failed-body"), fact, cx)
            });
            div().w(px(320.)).h(px(600.)).child(body)
        }
    }

    fn failed_body(
        cx: &mut TestAppContext,
        fact: FailureFact,
    ) -> (Arc<FailureFact>, VisualTestContext) {
        let (app, _main) = test_window::harness(cx);
        let fact = Arc::new(fact);
        let window = cx.add_window({
            let (app, fact) = (app.clone(), fact.clone());
            move |_, _| FailedBody { app, fact }
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();
        // Drawn explicitly, as the bounds tests do, so the buttons' bounds
        // never depend on a frame the platform happens to deliver.
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (fact, vcx)
    }

    /// Re-render the body after something it reads changed (the language) and
    /// draw it, so the new frame's bounds are the ones a click lands on.
    fn redraw(vcx: &mut VisualTestContext) {
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn copied(vcx: &mut VisualTestContext) -> Option<String> {
        vcx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
    }

    /// One retained fact, worded in the language of the moment: Copy error
    /// writes the words that are drawn. The expected strings are literals
    /// written here, in English and in Russian, not the formatter's output.
    /// Switching language re-words the same retained fact; nothing re-probes
    /// (the fact is held by the view, no `ScmData` is involved).
    #[gpui::test]
    fn copy_error_writes_a_generated_fact_in_the_language_of_the_moment(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        crate::ui::i18n::set_locale("en");
        let (_fact, mut vcx) = failed_body(cx, FailureFact::ExitStatus(3));
        press(&mut vcx, COPY);
        assert_eq!(
            copied(&mut vcx).as_deref(),
            Some("git exited with status 3")
        );

        crate::ui::i18n::set_locale("ru-RU");
        redraw(&mut vcx);
        press(&mut vcx, COPY);
        assert_eq!(
            copied(&mut vcx).as_deref(),
            Some("git завершился со статусом 3"),
            "the same fact, in Russian, on the clipboard"
        );
        crate::ui::i18n::set_locale("en");
    }

    /// Git's own words are copied exactly, even when they read like a sentence
    /// the app would have generated, and even in another language.
    #[gpui::test]
    fn copy_error_writes_raw_text_that_looks_like_a_fallback_unchanged(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        crate::ui::i18n::set_locale("ru-RU");
        let (_fact, mut vcx) =
            failed_body(cx, FailureFact::Raw("git exited with status 3".to_string()));
        press(&mut vcx, COPY);
        assert_eq!(
            copied(&mut vcx).as_deref(),
            Some("git exited with status 3"),
            "raw text is not translated"
        );
        crate::ui::i18n::set_locale("en");
    }

    /// Every generated kind, in all four languages. The Russian expectations
    /// are written out here as literals, not read back from the formatter or
    /// the catalog; the others are held to what must hold of any translation:
    /// the payload survives verbatim, no placeholder is left unreplaced, and
    /// it is not the English sentence.
    #[test]
    fn generated_failure_facts_are_worded_in_every_language() {
        let facts = [
            FailureFact::ExitStatus(7),
            FailureFact::Signal,
            FailureFact::UnusableRoot,
            FailureFact::Panicked(None),
            FailureFact::Panicked(Some("boom".into())),
        ];
        crate::ui::i18n::set_locale("en");
        let english = [
            "git exited with status 7",
            "git was stopped by a signal",
            "git rev-parse said nothing usable",
            "git status panicked",
            "git status panicked: boom",
        ];
        for (fact, want) in facts.iter().zip(english) {
            assert_eq!(failure_text(fact), want, "en {fact:?}");
        }

        crate::ui::i18n::set_locale("ru-RU");
        let russian = [
            "git завершился со статусом 7",
            "git остановлен сигналом",
            "git rev-parse не вернул пригодных данных",
            "git status аварийно завершился",
            "git status аварийно завершился: boom",
        ];
        for (fact, want) in facts.iter().zip(russian) {
            assert_eq!(failure_text(fact), want, "ru {fact:?}");
        }

        for locale in ["ru-RU", "zh-CN", "ja-JP"] {
            crate::ui::i18n::set_locale(locale);
            for (fact, en) in facts.iter().zip(english) {
                let text = failure_text(fact);
                assert!(!text.trim().is_empty(), "{locale} {fact:?}");
                assert!(
                    !text.contains('{') && !text.contains('}'),
                    "{locale} {fact:?} left a placeholder: {text:?}"
                );
                assert_ne!(text, en, "{locale} {fact:?} is still the English sentence");
            }
            assert!(
                failure_text(&facts[0]).contains('7'),
                "{locale} keeps the code"
            );
            assert!(
                failure_text(&facts[4]).contains("boom"),
                "{locale} keeps the panic text"
            );
        }
        crate::ui::i18n::set_locale("en");
    }

    /// Somebody else's words are never reworded: raw text that reads exactly
    /// like a sentence the app generates stays exact in every language, and so
    /// does raw text in another script.
    #[test]
    fn raw_failure_text_is_exact_in_every_language() {
        for raw in [
            "git exited with status 3",
            "git was stopped by a signal",
            "git rev-parse said nothing usable",
            "git status panicked: boom",
            "fatal: index file smaller than expected",
            "{code} {reason}",
        ] {
            for locale in ["en", "ru-RU", "zh-CN", "ja-JP"] {
                crate::ui::i18n::set_locale(locale);
                assert_eq!(
                    failure_text(&FailureFact::Raw(raw.to_string())),
                    raw,
                    "{locale}: {raw:?}"
                );
            }
        }
        crate::ui::i18n::set_locale("en");
    }
}
