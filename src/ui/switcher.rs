use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, MouseButton, MouseDownEvent, Subscription,
    Window, div, prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use tty59_core::core::machine::TabId;
use tty59_core::core::session::{RemoteTarget, RouteSnapshot, WorkspaceId};

use crate::core::actions::{SwitcherAcross, SwitcherAcrossBack};
use crate::core::session::WorkspaceStore;
use crate::daemon::install::InstallPhase;
use crate::terminal::pane_liveness::Liveness;
use crate::ui::app::Tty59App;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::remote_connect::{self, HostChoice, RemoteWorkspaceRow};
use crate::ui::remote_workspace::{ConnectFlow, MachineStatus, RemoteLinks};

mod machine_scope;
pub(crate) use machine_scope::MachineScope;
use machine_scope::RailEntry;

const CARD_W: f32 = 760.0;

/// The create form's card. Narrower than the list — it is a form, not a
/// browser.
const FORM_W: f32 = 480.0;

/// The host dropdown shows about eight rows before it scrolls.
const FORM_LIST_H: f32 = 8.5 * (ROW_H + 8.0);

const LEFT_W: f32 = 340.0;

pub(crate) const CARD_TOP: f32 = 112.0;

/// Breathing room the card keeps from the window edge, and the height its own
/// search row, machine rail and footer take on top of the body.
const CARD_MARGIN: f32 = 24.0;
const SEARCH_H: f32 = 48.0;
const FOOTER_H: f32 = 40.0;

/// The machine rail's strip: tall enough for its chips to breathe, short
/// enough that they stay within a glance of the search box. Every height the
/// card spends on the rail hangs off this one constant.
const RAIL_H: f32 = 40.0;
const RAIL_CHIP_H: f32 = 26.0;

/// The chrome's own borders: the search row, the rail and the footer each
/// draw one.
const CARD_CHROME_H: f32 = SEARCH_H + RAIL_H + FOOTER_H + 3.0;

const BODY_H: f32 = 420.0;

/// The card's corner — a notch rounder than an ordinary popover: it is the
/// one overlay that fills most of the window.
const CARD_RADIUS: f32 = 12.0;

/// Padding around each column's list.
const COLUMN_PAD: f32 = 8.0;

/// A workspace on the left: a name line over a machine · path · time line,
/// beside a disc carrying the machine's link state.
const WS_ROW_H: f32 = 52.0;
const WS_AVATAR: f32 = 26.0;
const WS_DOT: f32 = 8.0;

/// A tab in the preview column, with the same brand disc the sidebar uses.
const TAB_ROW_H: f32 = 44.0;
const TAB_AVATAR: f32 = 18.0;
const RUN_DOT: f32 = 5.0;

/// The list rows' corner.
const LIST_RADIUS: f32 = 8.0;

/// The create form's rows and the rename field keep the compact height.
const ROW_H: f32 = 34.0;
const HOST_H: f32 = 28.0;

const ICON: f32 = 14.0;

const ROW_PAD: f32 = 10.0;

/// `Failed` stays a unit variant so `Link` can be `Copy` and travel by value in
/// `GroupRef`; what went wrong rides in `Group::error` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    Local,
    Connected,
    Connecting,
    /// The supervisor is retrying this machine on its backoff. Distinct from
    /// `Offline` because the group still has rows worth showing and Disconnect
    /// still has something to call off.
    Reconnecting {
        attempt: u32,
    },
    Failed,
    Offline,
}

/// Whether Disconnect has anything to stop. A machine the supervisor is still
/// retrying has no live link, but calling the retry off is exactly what the
/// verb is for, so it has to stay enabled.
fn link_is_engaged(link: Link) -> bool {
    matches!(link, Link::Connected | Link::Reconnecting { .. })
}

/// What a machine chip's dot says about its link state. Pure over `Link`, so
/// an ordinary test pins every variant without a window; the inks are the
/// renderer's business.
///
/// The rail is the machine list, so unlike the row avatars — where the quiet
/// states say nothing and only the exceptions speak — every machine answers
/// "is it reachable?" at a glance: reachable is a solid success dot, and a
/// configured machine with no link is a hollow ring — the chip is there,
/// nothing answers behind it yet. The states that want attention keep the
/// rows' inks. The row's takeover case (`Connected` on a preempted machine)
/// stays a row matter: `RailEntry` carries the link alone, and which
/// workspace was taken over is not the machine's news.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailDotStyle {
    /// Reachable: this computer, or a link that is up.
    Solid,
    /// On the rail, configured, and not connected.
    Hollow,
    /// A connect in flight, or a retry on its backoff.
    Warn,
    /// The last attempt failed.
    Danger,
}

/// The dot style one link state puts on its machine chip.
fn rail_dot_style(link: Link) -> RailDotStyle {
    match link {
        Link::Local | Link::Connected => RailDotStyle::Solid,
        Link::Offline => RailDotStyle::Hollow,
        Link::Connecting | Link::Reconnecting { .. } => RailDotStyle::Warn,
        Link::Failed => RailDotStyle::Danger,
    }
}

/// The dot itself: the style above dressed in the theme's inks. The hollow
/// ring is a muted border and no fill, so it reads as an outline at the same
/// size its solid neighbours fill.
fn rail_dot(style: RailDotStyle, theme: &gpui_component::Theme) -> gpui::Div {
    let dot = div().size(px(6.)).flex_shrink_0().rounded_full();
    match style {
        RailDotStyle::Solid => dot.bg(theme.success),
        RailDotStyle::Hollow => dot.border(px(1.5)).border_color(theme.muted_foreground),
        RailDotStyle::Warn => dot.bg(theme.warning),
        RailDotStyle::Danger => dot.bg(theme.danger),
    }
}

/// How one machine's row is drawn, from the two things that know.
///
/// This window's own `connect` still comes first — it is the attempt the user
/// is watching, and it is the only one that can be Failed with a reason on
/// screen. Everything after it is the supervisor's, which is the only view that
/// survives the window that started the connect. Reading the `HostLinks` table
/// alone, as this used to, cannot tell a machine that is being retried right
/// now from one nobody has ever connected to: the pump drops the entry the
/// moment the link dies, and the group then collapses as if it were empty.
fn link_from(
    connect: Option<&ConnectFlow>,
    target: &RemoteTarget,
    supervised: Option<&MachineStatus>,
    has_link: bool,
) -> Link {
    match connect {
        Some(ConnectFlow::Connecting { choice }) if &choice.target == target => {
            return Link::Connecting;
        }
        Some(ConnectFlow::Failed { choice, .. }) if &choice.target == target => {
            return Link::Failed;
        }
        _ => {}
    }
    match supervised {
        Some(MachineStatus::Connecting) => Link::Connecting,
        Some(MachineStatus::Attached) => Link::Connected,
        Some(MachineStatus::Reconnecting { attempt, .. }) => {
            Link::Reconnecting { attempt: *attempt }
        }
        Some(MachineStatus::Failed(_)) => Link::Failed,
        None if has_link => Link::Connected,
        None => Link::Offline,
    }
}

struct Group {
    key: String,
    label: String,
    endpoint: String,
    target: Option<RemoteTarget>,
    link: Link,
    error: Option<String>,
    installing: Option<InstallPhase>,
    /// Another client is holding at least one workspace of this machine. The
    /// link itself is fine, so nothing in `link` would ever say so.
    preempted: bool,
    /// The route behind this group no longer resolves — its profile was
    /// deleted or its alias left the ssh config (#485). Parked groups are
    /// not retried; the group shows the snapshot label and offers to forget
    /// its entries instead of a retry button that could never succeed.
    parked: bool,
    rows: Vec<Row>,
}

struct Row {
    id: WorkspaceId,
    name: String,
    path: String,
    when: String,
    /// Raw timestamp behind `when` — what the flat list sorts by.
    last_active: u64,
    live: Liveness,
    open: bool,
    current: bool,
    preempted: bool,
    adopt: Option<Box<RemoteWorkspaceRow>>,
    remote_id: Option<WorkspaceId>,
    /// Which `SelectWorkspaceN` reaches this workspace, zero-based — the
    /// stable number the Workspaces menu shows too (#760). The list itself
    /// stays most-recently-used first; the number is what does not move.
    slot: Option<usize>,
    tabs: Vec<TabRow>,
}

/// One tab in the right-hand column. Built once per frame for every workspace
/// on the left, so the search can match tab names and the column can render
/// without a second pass over the machine tree. Search Everywhere's Terminals
/// tab is built from the same rows.
#[derive(Clone)]
pub(crate) struct TabRow {
    pub(crate) id: TabId,
    /// Position in the owning workspace's tab order — what `activate` wants.
    pub(crate) index: usize,
    pub(crate) label: String,
    pub(crate) path: String,
    /// Whether `label` is a name someone gave the tab. When it is not, the
    /// label is already derived from the working directory and showing `path`
    /// next to it just prints the same place twice.
    pub(crate) named: bool,
    pub(crate) agent: Option<crate::core::cli_agent::CLIAgent>,
    pub(crate) status: Option<crate::core::cli_agent::AgentStatus>,
    pub(crate) unread: usize,
    /// "waiting 3m" while an agent in the tab is blocked on the user, counted
    /// from the longest wait. Only this window's own tabs know it.
    pub(crate) waited: Option<String>,
    pub(crate) ssh: Option<u32>,
    pub(crate) active: bool,
    /// Branch and diff counts, the same line the tab sidebar shows. Only this
    /// window's own tabs have it — the machine tree carries no git state.
    pub(crate) git: Option<tty59_core::core::git::GitStatus>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Column {
    Left,
    Right,
}

/// A selectable line in the left column: `(group, row)` into `Layout::groups`.
/// The list is flat — one workspace per line, machines told apart by the badge
/// on the row itself — and rendering and keyboard navigation walk the same
/// list so an arrow key can never land somewhere the eye cannot see.
type Nav = (usize, usize);

pub(crate) struct HostSnapshot {
    pub target: RemoteTarget,
    pub rows: Vec<RemoteWorkspaceRow>,
}

/// A pane the daemon runs that no workspace holds — what an interrupted
/// `tty59 run` leaves behind, and what `tty59 pane ls --all` points the CLI's
/// reaper at. The switcher is where a GUI user finds and closes one (#596).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OrphanPane {
    pub pane_id: u64,
    pub title: String,
    pub cwd: Option<String>,
    pub owner: Option<String>,
}

/// Every pane id the local machine's workspaces hold — what the registry
/// listing is measured against to find the orphans (#596).
fn held_local_pane_ids(cx: &App) -> HashSet<u64> {
    crate::ui::machine_mirror::MachineMirrors::machine(cx, crate::core::session::HostId::LOCAL)
        .map(|machine| {
            machine
                .workspaces
                .iter()
                .flat_map(|ws| ws.tabs.iter())
                .flat_map(|tab| tab.root.pane_ids())
                .collect()
        })
        .unwrap_or_default()
}

/// The registry's live panes minus the ones a workspace holds. Dead entries
/// drop out too: a corpse the daemon has not reaped yet is not something the
/// user can act on.
pub(crate) fn orphan_panes_of(
    listed: Vec<tty59_core::daemon::protocol::PaneInfo>,
    held: &HashSet<u64>,
) -> Vec<OrphanPane> {
    listed
        .into_iter()
        .filter(|info| info.alive && !held.contains(&info.pane_id))
        .map(|info| OrphanPane {
            pane_id: info.pane_id,
            title: info.title,
            cwd: info.cwd.map(|p| p.display().to_string()),
            owner: info.owner,
        })
        .collect()
}

/// The name this machine shows for the workspace an orphan still claims —
/// `None` once that workspace is gone, which is the ordinary case for a pane
/// nobody holds.
fn workspace_name_of(owner: &str, cx: &App) -> Option<String> {
    let id = owner.parse::<WorkspaceId>().ok()?;
    let machine = crate::ui::machine_mirror::MachineMirrors::machine(
        cx,
        crate::core::session::HostId::LOCAL,
    )?;
    let ws = machine.workspaces.iter().find(|ws| ws.id == id)?;
    Some(crate::ui::machine_mirror::display_name_of(
        ws,
        &machine.panes,
    ))
}

/// What to print where an orphan names its owner. The owner it carries is a
/// `WorkspaceId` — 36 characters of UUID that say nothing on screen and, left
/// whole, push the row's Close button clean out of the card. So: name the
/// workspace when this machine still has one, and otherwise keep the first 8
/// characters, which is what `tty59 pane ls --all` prints and enough to line
/// the two listings up. An owner that is no workspace id at all is an older
/// client's own label (`tty59-cli`) and already reads fine.
fn owner_label(owner: &str, workspace_name: Option<String>) -> String {
    workspace_name.unwrap_or_else(|| match owner.parse::<WorkspaceId>() {
        Ok(_) => owner.chars().take(8).collect(),
        Err(_) => owner.to_string(),
    })
}

/// Which face the card is showing: the workspace list, or the create form.
pub(crate) enum Page {
    List,
    Create(CreateForm),
}

/// The "New Workspace" form: a name the user has to give, and a host picked
/// from a combobox that folds however many machines are configured into one
/// row.
pub(crate) struct CreateForm {
    name: Entity<InputState>,
    /// The combobox's filter text. Only meaningful while `open`.
    host: Entity<InputState>,
    /// Whether the host dropdown is unfolded.
    open: bool,
    /// Cursor into the dropdown's item list.
    sel: usize,
    /// The picked host. `None` is this computer.
    chosen: Option<HostChoice>,
}

impl CreateForm {
    /// The typed name, trimmed; `None` while the box is blank, which is also
    /// when Create stays disabled.
    fn name(&self, cx: &App) -> Option<String> {
        let name = self.name.read(cx).value().trim().to_string();
        (!name.is_empty()).then_some(name)
    }
}

/// A create the user asked for on a machine that was not connected yet: the
/// connect has to land first, because only a live link knows the home
/// directory a fresh workspace is rooted at. `finish_connect` consumes it.
pub(crate) struct PendingCreate {
    pub target: RemoteTarget,
    pub name: Option<String>,
}

pub(crate) struct Switcher {
    pub query: Entity<InputState>,
    page: Page,
    renaming: Option<(WorkspaceId, Entity<InputState>)>,
    /// Panes the local daemon runs that no workspace holds (#596). Filled
    /// asynchronously after the panel opens; empty both while the listing is
    /// in flight and when there is nothing to reap.
    orphans: Vec<OrphanPane>,
    /// Why the last Close on an orphan did not take, by pane id. Said on the
    /// orphan's own line: a Close that fails in silence reads as a dead
    /// button.
    orphan_failures: HashMap<u64, String>,
    column: Column,
    /// The machine the left column is narrowed to. Starts at All on every
    /// open; a leading `@token` in the query previews another machine on top
    /// of it (see `machine_scope::effective`).
    scope: MachineScope,
    /// Set while a committed `@token` rewrites the query, so the write is not
    /// mistaken for fresh typing if the input ever emits for it.
    committing_token: bool,
    left_sel: usize,
    right_sel: usize,
    /// Order the tab column most-recently-used first. Set when Ctrl+Tab opened
    /// the panel; the plain Cmd+Shift+O panel keeps strip order.
    mru: bool,
    /// The modifiers held down when Ctrl+Tab opened the panel. Releasing them
    /// commits the highlighted tab, IDEA-style.
    hold: Option<gpui::Modifiers>,
    /// Where the pointer is: inside the card at all, and inside the tab column
    /// specifically. Both are set by hover listeners, so they only mean
    /// anything once the mouse has moved since the panel came up.
    hover_card: bool,
    hover_tabs: bool,
    left_scroll: gpui::ScrollHandle,
    right_scroll: gpui::ScrollHandle,
    /// Anchors on the two scrolls, worn by whichever row is selected. Both
    /// columns hold their rows inside one child element, and `scroll_to_item`
    /// indexes a scroll's *direct* children — so it could only ever find item
    /// 0, and walking the list with the arrows quietly left the selection off
    /// the bottom of the column.
    left_anchor: gpui::ScrollAnchor,
    right_anchor: gpui::ScrollAnchor,
    /// The machine rail's own scroll, horizontal. The chips are its direct
    /// children, so `scroll_to_item` can find the effective scope's chip:
    /// `render_rail` asks for it whenever that chip moves — a click,
    /// Alt+←/→, or a `@token` being typed into the box — and gpui carries
    /// the rail along by the minimal amount, which is why a hand browsing
    /// the rail is never fought on a steady frame.
    rail_scroll: gpui::ScrollHandle,
    /// The index of the chip the rail last asked to show. Interior because
    /// `render_rail` renders from `&self`, and it is what tells a move from
    /// a steady frame.
    rail_followed: std::cell::Cell<Option<usize>>,
    _subs: Vec<Subscription>,
}

impl Switcher {
    fn text(&self, cx: &App) -> String {
        self.query.read(cx).value().trim().to_lowercase()
    }

    /// The pointer is parked in the card but off the tab column — on a
    /// workspace row, the search box, a banner. Letting go of Ctrl there is
    /// not a commit: the user is reaching for the mouse, and closing the panel
    /// out from under them makes the workspace list unreachable by hand.
    fn hover_keeps_open(&self) -> bool {
        self.hover_card && !self.hover_tabs
    }
}

/// Everything the panel needs for one frame: the groups (one per machine,
/// still the unit that carries link state and errors), and the flat,
/// most-recently-used-first left column the arrow keys walk.
struct Layout {
    groups: Vec<Group>,
    nav: Vec<Nav>,
    /// The machine rail's chips, left to right.
    rail: Vec<RailEntry>,
    /// What the list is narrowed to this frame: the stored scope, or the
    /// machine a half-typed `@token` previews.
    scope: MachineScope,
}

impl Layout {
    /// Which workspace row the tab column is showing.
    fn subject(&self, sel: usize) -> Option<(usize, usize)> {
        self.nav.get(sel).copied()
    }

    fn subject_row(&self, sel: usize) -> Option<&Row> {
        let (g, r) = self.subject(sel)?;
        self.groups[g].rows.get(r)
    }
}

/// The flat left column: every row the query leaves visible, one line per
/// workspace, most recently used first with this window's own workspace on
/// top. A query matching a machine's name keeps all of that machine's rows —
/// searching "devbox" is how the old per-machine grouping is asked for now.
/// Narrowed to one machine, open workspaces come first so the list can wear
/// its two headings; All keeps the single blended order.
fn flatten(groups: &[Group], query: &str, scope: &MachineScope) -> Vec<Nav> {
    let mut nav: Vec<Nav> = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        if !scope.admits(group) {
            continue;
        }
        let matched_host = group.label.to_lowercase().contains(query);
        for (r, row) in group.rows.iter().enumerate() {
            if query.is_empty() || matched_host || row.matches(query) {
                nav.push((g, r));
            }
        }
    }
    // Narrowed to one machine the list has two headings, so its rows have to
    // come out in two runs or a heading would have to repeat. All keeps the
    // single blended order it has always had.
    let scoped = !matches!(scope, MachineScope::All);
    nav.sort_by(|&(ga, ra), &(gb, rb)| {
        row_order(query, scoped, &groups[ga].rows[ra], &groups[gb].rows[rb])
    });
    nav
}

/// One left-column row against another. Open rows lead when the list is
/// narrowed to a machine — that is what separates "Open in a window" from the
/// rest of the list. Everything after that is the order the flat list already
/// had: how well the name matched, this window, recency, then name.
fn row_order(query: &str, scoped: bool, a: &Row, b: &Row) -> std::cmp::Ordering {
    let open = if scoped {
        b.open.cmp(&a.open)
    } else {
        std::cmp::Ordering::Equal
    };
    // How a row matched comes before which one is current: "qa" is the
    // workspace named qa, not this window's one whose tabs sit in
    // `/tmp/t7qa`, and Enter takes the top row.
    open.then_with(|| a.name_match(query).cmp(&b.name_match(query)))
        .then_with(|| b.current.cmp(&a.current))
        .then_with(|| b.last_active.cmp(&a.last_active))
        .then_with(|| a.name.cmp(&b.name))
}

/// Which heading a row of a scoped list falls under. A change between
/// neighbouring rows is where the heading is drawn; nothing is added to `nav`,
/// so the arrow keys still walk exactly the rows on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Open,
    Closed,
    /// The rows of a machine that is not connected are remembered ones; a
    /// single heading says so instead of splitting them by open state.
    LastKnown,
}

impl Section {
    fn key(self) -> L10nKey {
        match self {
            Section::Open => L10nKey::SwitcherOpenInWindow,
            Section::Closed => L10nKey::SwitcherOnThisMachine,
            Section::LastKnown => L10nKey::SwitcherLastKnownWorkspaces,
        }
    }
}

/// The heading under which each navigation slot falls, in nav order. `None`
/// for the unscoped list, which has no headings.
fn nav_sections(
    nav: &[Nav],
    groups: &[Group],
    scope: &MachineScope,
    last_known: bool,
) -> Vec<Option<Section>> {
    if matches!(scope, MachineScope::All) {
        return vec![None; nav.len()];
    }
    nav.iter()
        .map(|&(g, r)| {
            Some(match (last_known, groups[g].rows[r].open) {
                (true, _) => Section::LastKnown,
                (false, true) => Section::Open,
                (false, false) => Section::Closed,
            })
        })
        .collect()
}

/// One line of the create form's host dropdown.
enum HostItem {
    /// This computer — first, while it matches the filter.
    Local,
    Host(HostChoice),
    /// Pinned last, filter or no filter: the way out when the machine wanted
    /// is not configured yet.
    AddHost,
}

fn host_items(hosts: Vec<HostChoice>, query: &str, local_label: &str) -> Vec<HostItem> {
    let query = query.trim();
    let mut items: Vec<HostItem> = Vec::new();
    if query.is_empty() || crate::ui::search::fuzzy_score(query, local_label).is_some() {
        items.push(HostItem::Local);
    }
    items.extend(
        remote_connect::filter_hosts(&hosts, query)
            .into_iter()
            .map(HostItem::Host),
    );
    items.push(HostItem::AddHost);
    items
}

/// The scoped machine that is waiting on a connect, if any: remote, not
/// parked, and neither connected nor in flight. A parked route can never be
/// connected (#485), and an attempt already on the wire has its own banner.
fn offline_connect_group<'a>(scope: &MachineScope, groups: &'a [Group]) -> Option<&'a Group> {
    let group = scope.group(groups)?;
    group.target.as_ref()?;
    (!group.parked && matches!(group.link, Link::Offline | Link::Failed)).then_some(group)
}

/// The endpoint line the offline card prints: the configured route's detail,
/// or the target's own spelling when the machine was never configured.
fn group_endpoint(group: &Group) -> String {
    match group.endpoint.trim() {
        "" => group
            .target
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        endpoint => endpoint.to_string(),
    }
}

/// The connect the offline card's button asks for. `None` only for a group
/// with no target, which the card never shows.
fn connect_choice(group: &Group) -> Option<HostChoice> {
    Some(HostChoice {
        target: group.target.clone()?,
        label: group.label.clone(),
        detail: group_endpoint(group),
    })
}

/// What the offline card's second line has to say: the failure that took the
/// machine down (the row truncates it) and the endpoint it would dial. A group
/// with no failure behind it only has the endpoint to show.
fn offline_card_detail(group: &Group) -> (Option<String>, String) {
    (group.error.clone(), group_endpoint(group))
}

/// Orphan panes are this computer's; under a remote scope they are not
/// something the list is about.
fn orphans_visible(scope: &MachineScope) -> bool {
    matches!(scope, MachineScope::All) || scope == &MachineScope::local()
}

/// What the empty list says. Unscoped keeps the ordinary no-match wording; a
/// machine names itself, since "no match" is not what an empty machine is.
fn empty_list_text(scope: &MachineScope, groups: &[Group]) -> String {
    match scope.group(groups) {
        Some(group) => t_fmt(
            L10nKey::SwitcherNoWorkspacesOnMachine,
            &[("machine", &group.label)],
        ),
        None => t(L10nKey::SwitcherNoMatch).to_string(),
    }
}

/// The sub-line under a workspace's name: its path, led by the machine only in
/// the blended All list. A scoped row lives on the one machine the list is
/// about, so repeating its name there is noise.
fn row_meta(group: &Group, row: &Row, show_machine: bool) -> Vec<String> {
    let mut meta: Vec<String> = Vec::new();
    if show_machine {
        meta.push(if group.target.is_some() {
            group.label.clone()
        } else {
            t(L10nKey::SwitcherLocalHost).to_string()
        });
    }
    if !row.path.is_empty() {
        meta.push(row.path.clone());
    }
    meta
}

/// One of the scoped list's section headings.
fn section_heading(section: Section, cx: &App) -> AnyElement {
    div()
        .px(px(ROW_PAD))
        .pt(px(8.))
        .pb(px(2.))
        .text_size(gpui::rems(10.5 / 16.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(t(section.key()))
        .into_any_element()
}

impl Tty59App {
    pub(crate) fn toggle_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_some() {
            self.close_switcher(window, cx);
        } else {
            self.open_switcher(window, cx);
        }
    }

    pub(crate) fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_switcher_in(Column::Left, false, None, window, cx);
    }

    /// The palette's Open Workspace on Machine: an ordinary picker with a bare
    /// `@` already in the box, so the next keystrokes narrow the rail while
    /// the list still previews. Nothing is scoped until a name matches.
    pub(crate) fn open_switcher_on_machine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_switcher(window, cx);
        let Some(query) = self.switcher.as_ref().map(|sw| sw.query.clone()) else {
            return;
        };
        query.update(cx, |state, cx| state.set_value("@", window, cx));
        cx.notify();
    }

    /// Opens a fresh picker already narrowed to `scope`, for the places that
    /// know the machine before the picker exists. Opening first means the
    /// ordinary All reset happens, then the scope is a real change.
    #[allow(dead_code)] // the status-strip entry point is deferred
    pub(crate) fn open_switcher_scoped(
        &mut self,
        scope: MachineScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_switcher(window, cx);
        self.select_switcher_machine(scope, window, cx);
    }

    fn open_switcher_in(
        &mut self,
        column: Column,
        mru: bool,
        hold: Option<gpui::Modifiers>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        remote_connect::register(cx);
        remote_connect::sweep_wsl(cx);
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::ui::i18n::t(
                    crate::ui::i18n::L10nKey::SearchWorkspacesAndMachines,
                ))
                // On macOS a held Ctrl turns every click into a right click,
                // and the input answers a right click with Cut/Copy/Paste —
                // so reaching for this box mid-Ctrl+Tab popped a menu instead
                // of placing a caret. The rows already dodge this by dropping
                // their own menus while the gesture is on; this box has no
                // menu worth keeping either, and Cmd+V still pastes.
                .context_menu(false)
        });
        query.update(cx, |state, cx| state.focus(window, cx));
        let subs = vec![cx.subscribe_in(
            &query,
            window,
            |this, input, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.commit_query_machine_token(input, window, cx);
                    // A narrower list can strand the cursor past its end. Land
                    // it on the first hit so the tab column shows the hits
                    // straight away.
                    if let Some(sw) = this.switcher.as_mut() {
                        sw.left_sel = 0;
                        sw.right_sel = 0;
                    }
                    cx.notify();
                }
            },
        )];
        let (left_scroll, right_scroll, rail_scroll) = (
            gpui::ScrollHandle::new(),
            gpui::ScrollHandle::new(),
            gpui::ScrollHandle::new(),
        );
        self.switcher = Some(Switcher {
            query,
            page: Page::List,
            renaming: None,
            orphans: Vec::new(),
            orphan_failures: HashMap::new(),
            column,
            scope: MachineScope::All,
            committing_token: false,
            left_sel: 0,
            right_sel: 0,
            mru,
            hold,
            hover_card: false,
            hover_tabs: false,
            left_scroll: left_scroll.clone(),
            right_scroll: right_scroll.clone(),
            left_anchor: gpui::ScrollAnchor::for_handle(left_scroll),
            right_anchor: gpui::ScrollAnchor::for_handle(right_scroll),
            rail_scroll,
            rail_followed: std::cell::Cell::new(None),
            _subs: subs,
        });
        // Park the left cursor on this window's own workspace so the tab column
        // opens on something useful.
        let layout = self.switcher_layout(cx);
        let here = self.workspace;
        if let Some(at) = layout
            .nav
            .iter()
            .position(|&(g, r)| layout.groups[g].rows[r].id == here)
            && let Some(sw) = self.switcher.as_mut()
        {
            sw.left_sel = at;
        }
        self.refresh_orphan_panes(cx);
        cx.notify();
    }

    /// List the local daemon's panes and keep the ones no workspace holds.
    /// The query is blocking daemon I/O, so it runs off the UI thread; the
    /// filter runs back on it, where the machine mirror lives.
    ///
    /// Local on purpose: a remote machine's orphans belong to its own daemon,
    /// and routing a listing per host is what the CLI's reaper already does.
    fn refresh_orphan_panes(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_spawn(async move { tty59_core::client::PaneClient::local().list() })
                .await;
            let listed = match listed {
                Ok(listed) => listed,
                Err(e) => {
                    log::warn!(target: "tty59::switcher", "orphan pane listing failed: {e}");
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| {
                let held = held_local_pane_ids(cx);
                if let Some(sw) = this.switcher.as_mut() {
                    sw.orphans = orphan_panes_of(listed, &held);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Hang up one orphan pane and show what is left. The kill is
    /// fire-and-forget, so the refresh that follows is also the confirmation:
    /// a pane that survived it simply stays on the list.
    fn close_orphan_pane(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_spawn(async move {
                    let client = tty59_core::client::PaneClient::local();
                    client.kill(pane_id)?;
                    client.list()
                })
                .await;
            let listed = match listed {
                Ok(listed) => listed,
                Err(e) => {
                    log::warn!(target: "tty59::switcher", "closing orphan %{pane_id} failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        if let Some(sw) = this.switcher.as_mut() {
                            sw.orphan_failures.insert(pane_id, e.to_string());
                        }
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| {
                let held = held_local_pane_ids(cx);
                if let Some(sw) = this.switcher.as_mut() {
                    sw.orphan_failures.remove(&pane_id);
                    sw.orphans = orphan_panes_of(listed, &held);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ctrl+Tab. The first press raises the panel on the tab column with the
    /// previously used tab already highlighted; further presses walk it. Holding
    /// the modifier keeps the panel up, releasing it commits — IDEA's gesture.
    ///
    /// With fewer than two tabs there is nothing to cycle, so the panel opens on
    /// the workspace column and *stays* — no hold, no commit-on-release. The
    /// gesture degrades into "open the switcher" rather than doing nothing.
    pub(crate) fn tab_switch(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.switcher.is_some() {
            let layout = self.switcher_layout(cx);
            self.switcher_step_right(&layout, forward, window, cx);
            return;
        }
        let n = self.tabs.len();
        let cycling = n >= 2;
        let held = window.modifiers();
        self.open_switcher_in(
            match cycling {
                true => Column::Right,
                false => Column::Left,
            },
            cycling,
            (cycling && held.modified()).then_some(held),
            window,
            cx,
        );
        if cycling && let Some(sw) = self.switcher.as_mut() {
            sw.right_sel = match forward {
                true => 1,
                false => n - 1,
            };
        }
        cx.notify();
    }

    /// Watches the modifiers while a Ctrl+Tab panel is up. Letting go of any
    /// part of the combination that raised it is the commit gesture.
    pub(crate) fn switcher_hold_changed(
        &mut self,
        now: &gpui::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hold) = self.switcher.as_ref().and_then(|sw| sw.hold) else {
            return;
        };
        if now.modified() && hold.is_subset_of(now) {
            return;
        }
        // The pointer is already on the workspace list or the search box, so
        // the release is the user's hand leaving the keyboard, not a pick.
        // Drop the hold and leave the panel up for the mouse to finish in.
        if self
            .switcher
            .as_ref()
            .is_some_and(Switcher::hover_keeps_open)
        {
            self.switcher_release_hold(cx);
            return;
        }
        self.switcher_commit_hold(window, cx);
    }

    /// Called when the modifier that raised the panel comes back up.
    pub(crate) fn switcher_commit_hold(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.as_ref().and_then(|sw| sw.hold).is_none() {
            return;
        }
        let layout = self.switcher_layout(cx);
        self.switcher_confirm(&layout, false, window, cx);
        // Confirming a tab already closed the panel; anything else (an empty
        // column, a workspace with nothing in it) still has to come down.
        if self.switcher.is_some() {
            self.close_switcher(window, cx);
        }
    }

    /// Drops the hold without acting on it. The modifier release will never
    /// arrive at a window that is no longer focused, so the panel would
    /// otherwise sit there waiting forever.
    pub(crate) fn switcher_release_hold(&mut self, cx: &mut Context<Self>) {
        if let Some(sw) = self.switcher.as_mut()
            && sw.hold.take().is_some()
        {
            cx.notify();
        }
    }

    pub(crate) fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.take().is_some() {
            if matches!(self.connect, Some(ConnectFlow::Failed { .. })) {
                self.connect = None;
            }
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn switcher_groups(&self, cx: &mut Context<Self>) -> Vec<Group> {
        let now = crate::ui::home::now_secs();
        let current = self.workspace;
        crate::terminal::pane_liveness::sweep(cx);

        let mut groups: Vec<Group> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let slots = crate::ui::windows::menu_order(cx);
        let slot_of = |id: WorkspaceId| slots.iter().position(|(slot, _)| *slot == id);
        {
            let app: &App = cx;
            let store = WorkspaceStore::all(app);
            for w in &store.views {
                let (key, label, endpoint, target) = match w.host.as_ref() {
                    None => (
                        String::new(),
                        t(L10nKey::SwitcherThisComputer).to_string(),
                        String::new(),
                        None,
                    ),
                    Some(r) => {
                        let key = r.target.to_string();
                        // The live profile's own name overwrites this below
                        // while the route still resolves; when it does not,
                        // the snapshot — or, for entries saved before
                        // snapshots existed, a placeholder — is all the entry
                        // has. Never the bare profile UUID (#485).
                        let label = r.route_label(t(L10nKey::RemoteProfileGone));
                        let endpoint = r
                            .via
                            .as_ref()
                            .map(RouteSnapshot::endpoint)
                            .filter(|e| e != &label)
                            .unwrap_or_default();
                        (key, label, endpoint, Some(r.target.clone()))
                    }
                };
                let slot = *index.entry(key.clone()).or_insert_with(|| {
                    groups.push(Group {
                        key,
                        label,
                        endpoint,
                        target,
                        link: Link::Offline,
                        error: None,
                        installing: None,
                        preempted: false,
                        parked: false,
                        rows: Vec::new(),
                    });
                    groups.len() - 1
                });
                // A row's path is on the workspace's own machine, so that is
                // the machine whose home may shorten it (#580).
                let home = crate::ui::path_display::home_for_host(app, w.host_id());
                groups[slot].rows.push(Row {
                    id: w.id,
                    name: crate::ui::machine_mirror::display_name(app, w)
                        .unwrap_or_else(|| t(L10nKey::WindowUntitled).to_string()),
                    path: crate::ui::machine_mirror::subject_path(app, w)
                        .map(|p| {
                            crate::ui::home::display_path(std::path::Path::new(&p), home.as_deref())
                        })
                        .unwrap_or_default(),
                    when: crate::ui::home::relative_time(now, w.last_active),
                    last_active: w.last_active,
                    live: crate::terminal::pane_liveness::liveness_of(app, w),
                    open: w.open,
                    current: w.id == current,
                    preempted: false,
                    adopt: None,
                    remote_id: w.host.as_ref().map(|r| r.workspace),
                    slot: slot_of(w.id),
                    tabs: self.tab_rows_for(w.id, self.switcher_mru(), app),
                });
            }
        }

        // Listed once for the whole frame: the pending groups below name
        // themselves from it, and so does the pass that settles every group's
        // link state further down.
        let configured = remote_connect::available_hosts(cx);

        for target in self.pending_machines() {
            let key = target.to_string();
            if index.contains_key(&key) {
                continue;
            }
            index.insert(key.clone(), groups.len());
            groups.push(Group {
                // Not `key`: a `Profile` target spells itself as its config
                // UUID, so a machine whose profile has been deleted would
                // announce itself to the banners below by a raw UUID (#485).
                // The pass below overwrites this while the profile is still
                // configured; this is what is left when it is not.
                label: remote_connect::label_from_hosts(&configured, &target),
                key,
                endpoint: String::new(),
                target: Some(target),
                link: Link::Offline,
                error: None,
                installing: None,
                preempted: false,
                parked: false,
                rows: Vec::new(),
            });
        }

        // Every machine the user has configured is on the rail, connected or
        // not, so a machine never visited can still be picked and connected
        // from here. Empty until a connect lists its workspaces.
        for host in &configured {
            let key = host.target.to_string();
            if index.contains_key(&key) {
                continue;
            }
            index.insert(key.clone(), groups.len());
            groups.push(Group {
                key,
                label: host.label.clone(),
                endpoint: String::new(),
                target: Some(host.target.clone()),
                link: Link::Offline,
                error: None,
                installing: None,
                preempted: false,
                parked: false,
                rows: Vec::new(),
            });
        }

        if !index.contains_key("") {
            groups.insert(
                0,
                Group {
                    key: String::new(),
                    label: t(L10nKey::SwitcherThisComputer).to_string(),
                    endpoint: String::new(),
                    target: None,
                    link: Link::Offline,
                    error: None,
                    installing: None,
                    preempted: false,
                    parked: false,
                    rows: Vec::new(),
                },
            );
        }

        // This window's own workspace has to be in the list even when the store
        // has not caught up with it: Ctrl+Tab reaches its tabs through the same
        // left column, and a missing row would leave the panel with nothing to
        // switch between.
        if !groups
            .iter()
            .any(|g| g.rows.iter().any(|r| r.id == current))
            && let Some(slot) = groups.iter().position(|g| g.key.is_empty())
        {
            let app: &App = cx;
            groups[slot].rows.insert(
                0,
                Row {
                    id: current,
                    name: crate::ui::machine_mirror::display_name_for(app, current)
                        .unwrap_or_else(|| t(L10nKey::WindowUntitled).to_string()),
                    path: String::new(),
                    when: crate::ui::home::relative_time(now, now),
                    last_active: now,
                    live: Liveness::Alive,
                    open: true,
                    current: true,
                    preempted: false,
                    adopt: None,
                    remote_id: None,
                    slot: slot_of(current),
                    tabs: self.tab_rows_for(current, self.switcher_mru(), app),
                },
            );
        }

        // Workspaces this machine holds that the store has never heard of: the
        // CLI makes them too, and one that never appears here looks to the
        // person who ran `tty59 new` like nothing happened at all. They open
        // like any other row — the id in the tree is the id a window claims.
        if let Some(slot) = groups.iter().position(|g| g.key.is_empty()) {
            // Measured against the rows already listed rather than against the
            // store, which is what put them there: the block above lists this
            // window's own workspace before the store has caught up with it,
            // and two rows under one id would be two ways into one window.
            let listed: Vec<WorkspaceId> = groups
                .iter()
                .flat_map(|g| g.rows.iter().map(|r| r.id))
                .collect();
            let app: &App = cx;
            // Unclaimed *local* workspaces: their paths are on this machine,
            // so this machine's home is the right one to measure them by.
            let local_home = crate::ui::path_display::local_home();
            let rows: Vec<Row> = crate::ui::machine_mirror::unclaimed_local_workspaces(app)
                .into_iter()
                .filter(|ws| !listed.contains(&ws.id))
                .map(|ws| Row {
                    id: ws.id,
                    name: ws.name,
                    path: ws
                        .path
                        .map(|p| {
                            crate::ui::home::display_path(
                                std::path::Path::new(&p),
                                local_home.as_deref(),
                            )
                        })
                        .unwrap_or_default(),
                    when: crate::ui::home::relative_time(now, ws.last_active),
                    last_active: ws.last_active,
                    live: match ws.live {
                        true => Liveness::Alive,
                        false => Liveness::Stopped,
                    },
                    open: false,
                    current: false,
                    preempted: false,
                    adopt: None,
                    remote_id: None,
                    slot: None,
                    tabs: self.tab_rows_for(ws.id, self.switcher_mru(), app),
                })
                .collect();
            groups[slot].rows.extend(rows);
        }

        // Row order is `flatten`'s business now — the left column is one flat
        // most-recently-used list. Groups keep local-first order only so the
        // trouble banners under the list come out in a stable order.
        groups.sort_by(|a, b| a.key.is_empty().cmp(&b.key.is_empty()).reverse());

        for group in &mut groups {
            let Some(target) = group.target.clone() else {
                group.link = Link::Local;
                // A remote client can take one of this computer's workspaces
                // too, and the row says so the same way.
                let taken: HashSet<WorkspaceId> =
                    RemoteLinks::preempted_on(cx, crate::core::session::HostId::LOCAL)
                        .into_iter()
                        .collect();
                for row in &mut group.rows {
                    row.preempted = taken.contains(&row.id);
                }
                continue;
            };
            if let Some(known) = configured.iter().find(|h| h.target == target) {
                group.label = known.label.clone();
                if known.detail != known.label {
                    group.endpoint = known.detail.clone();
                }
            }
            let id = target.host_id();
            let supervised = RemoteLinks::machine_status(cx, id);
            group.link = self.link_state(&target, supervised.as_ref(), cx);
            // A route that no longer resolves parks the whole group (#485):
            // not retried, labeled from the snapshot, forget-not-retry for
            // actions. A live link is exempt — its panes keep working
            // regardless of what happened to the profile that made them.
            group.parked =
                group.link != Link::Connected && !remote_connect::route_resolvable(cx, &target);
            if let Some(ConnectFlow::Failed { choice, error }) = &self.connect
                && choice.target == target
            {
                group.error = Some(error.clone());
            }
            if group.error.is_none() {
                if let Some(error) = self.remote_host_errors.get(&target.to_string()) {
                    group.error = Some(error.clone());
                }
            }
            // The supervisor's own attempts never touch this window's `connect`,
            // so without this last fallback the route it could not build, and
            // the reconnect that has been failing for an hour, have nowhere at
            // all to be said.
            if group.error.is_none() {
                group.error = RemoteLinks::machine_error(cx, id);
            }
            // A connect of this window's own is on the wire: whatever failed
            // before it is the question it is answering, and the band has to
            // say Connecting — an old reason left up reads as a button that
            // did nothing.
            if matches!(group.link, Link::Connecting) {
                group.error = None;
            }
            let taken: HashSet<WorkspaceId> =
                RemoteLinks::preempted_on(cx, id).into_iter().collect();
            group.preempted = !taken.is_empty();
            for row in &mut group.rows {
                row.preempted = taken.contains(&row.id);
            }
            let reported = remote_connect::install_progress_for(id);
            if group.link == Link::Connecting
                || matches!(group.link, Link::Reconnecting { .. })
                || group.error.is_some()
                || matches!(reported, Some(InstallPhase::Restarting))
            {
                group.installing = reported;
            }
            if let Some(snapshot) = self.host_snapshots.get(&id) {
                group.merge(&snapshot.rows, now);
            }
        }
        groups
    }

    /// Whether the open switcher lists this window's tabs most recently used
    /// first — the Ctrl-Tab gesture — rather than in strip order.
    fn switcher_mru(&self) -> bool {
        self.switcher.as_ref().is_some_and(|sw| sw.mru)
    }

    /// The tab column's rows for one workspace. This window's own workspace has
    /// live in-memory tabs (agent status, unread counts, MRU order); every other
    /// workspace comes out of the machine mirror, which is the only view this
    /// process has of windows it does not own.
    ///
    /// `mru` orders this window's own tabs most recently used first instead of
    /// in strip order; other workspaces' tabs come in the order they have.
    pub(crate) fn tab_rows_for(&self, id: WorkspaceId, mru: bool, cx: &App) -> Vec<TabRow> {
        if id == self.workspace {
            let now_ms = crate::core::config::unix_now().saturating_mul(1000);
            let order = match mru {
                true => self.tabs_by_mru(),
                false => (0..self.tabs.len()).collect(),
            };
            return order
                .into_iter()
                .map(|i| {
                    let tab = &self.tabs[i];
                    TabRow {
                        id: tab.tree_id.get(),
                        index: i,
                        label: self.tab_label(tab, i, None, cx),
                        named: tab.name.as_deref().is_some_and(|n| !n.trim().is_empty())
                            || tab.agent(cx).is_some(),
                        path: tab
                            .pane
                            .terminals()
                            .first()
                            .and_then(|leaf| {
                                let leaf = leaf.read(cx);
                                Some((leaf.cwd()?, leaf.display_home(cx)))
                            })
                            .map(|(p, home)| crate::ui::home::display_path(&p, home.as_deref()))
                            .unwrap_or_default(),
                        agent: tab.agent(cx),
                        status: tab.agent_status(cx),
                        unread: tab.agent_unread_count(cx),
                        waited: tab.agent_waited(now_ms, cx),
                        ssh: self.tab_ssh_dot(tab, cx),
                        active: i == self.active,
                        git: tab.git_status(None, cx),
                    }
                })
                .collect();
        }

        let Some((views, active)) = crate::ui::machine_mirror::tab_views_for(cx, id) else {
            return Vec::new();
        };
        // Git state is cached globally per (host, cwd) and outlives the panes
        // that filled it, so a workspace this window recently left still has
        // its branches on hand. Read-only on purpose: probing every cwd of
        // every workspace to populate a panel that closes in a second would
        // cost a git invocation each, and a round trip each when the host is
        // remote.
        let host = WorkspaceStore::all(cx).get(id).map(|w| w.host_id());
        let git = |cwd: Option<&str>| -> Option<tty59_core::core::git::GitStatus> {
            let (host, cwd) = (host?, cwd?);
            cx.try_global::<crate::terminal::git_status::GitStatusCache>()?
                .status_for(host, std::path::Path::new(cwd))
        };
        // These rows describe a workspace on `host`, and the cwds they carry
        // are that machine's. Only its home may shorten them (#580).
        let home = host.and_then(|host| crate::ui::path_display::home_for_host(cx, host));
        views
            .into_iter()
            .enumerate()
            .map(|(i, v)| TabRow {
                label: tab_view_label(&v, i, home.as_deref()),
                // The label only stands in for the path when it came *from* the
                // path; a name or an agent leaves the location still worth
                // printing.
                named: v.name.as_deref().is_some_and(|n| !n.trim().is_empty()) || v.agent.is_some(),
                path: v
                    .cwd
                    .as_deref()
                    .map(|p| {
                        crate::ui::home::display_path(std::path::Path::new(p), home.as_deref())
                    })
                    .unwrap_or_default(),
                agent: v.agent,
                status: v.status,
                unread: 0,
                waited: None,
                ssh: None,
                active: Some(v.id) == active,
                git: git(v.cwd.as_deref()),
                index: i,
                id: v.id,
            })
            .collect()
    }

    /// Builds one frame's worth of panel: the groups, and the flat left
    /// column the arrow keys walk.
    fn switcher_layout(&self, cx: &mut Context<Self>) -> Layout {
        let groups = self.switcher_groups(cx);
        let query = self
            .switcher
            .as_ref()
            .map(|sw| sw.text(cx))
            .unwrap_or_default();
        let rail = machine_scope::rail_entries(&groups, t(L10nKey::SwitcherMachineAll));
        let stored = self
            .switcher
            .as_ref()
            .map(|sw| sw.scope.clone())
            .unwrap_or_default();
        let (scope, rest) = machine_scope::effective(&stored, &query, &rail);
        let nav = flatten(&groups, rest, &scope);
        Layout {
            groups,
            nav,
            rail,
            scope,
        }
    }

    fn pending_machines(&self) -> Vec<RemoteTarget> {
        let mut out: Vec<RemoteTarget> = self
            .host_snapshots
            .values()
            .map(|s| s.target.clone())
            .collect();
        if let Some(choice) = self.connect.as_ref().and_then(ConnectFlow::choice) {
            out.push(choice.target.clone());
        }
        out
    }

    fn link_state(
        &self,
        target: &RemoteTarget,
        supervised: Option<&MachineStatus>,
        cx: &mut Context<Self>,
    ) -> Link {
        let has_link = remote_connect::HostLinks::get(cx, target.host_id()).is_some();
        link_from(self.connect.as_ref(), target, supervised, has_link)
    }

    fn switcher_open(
        &mut self,
        row: RowRef,
        new_window: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_switcher(window, cx);
        match row.adopt {
            Some((target, remote)) => self.open_remote_workspace(target, *remote, window, cx),
            None if new_window => crate::ui::windows::open(cx, Some(row.id)),
            None => self.reveal_workspace(row.id, window, cx),
        }
    }

    fn switcher_rename(&mut self, id: WorkspaceId, window: &mut Window, cx: &mut Context<Self>) {
        let current = crate::ui::machine_mirror::display_name_for(cx, id).unwrap_or_default();
        let input = Self::rename_box(current, window, cx);
        let sub = cx.subscribe_in(
            &input,
            window,
            move |this, _input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.switcher_commit_rename(window, cx)
                }
                _ => {}
            },
        );
        if let Some(sw) = self.switcher.as_mut() {
            sw.renaming = Some((id, input));
            sw._subs.push(sub);
        }
        cx.notify();
    }

    fn switcher_commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, input)) = self.switcher.as_mut().and_then(|sw| sw.renaming.take()) else {
            return;
        };
        let value = input.read(cx).value().trim().to_string();
        crate::ui::tree_sync::rename_workspace(cx, id, (!value.is_empty()).then_some(value));
        crate::ui::windows::refresh_menu(cx);
        if id == self.workspace {
            self.sync_window_title(window, cx);
        }
        if let Some(sw) = self.switcher.as_ref() {
            sw.query.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    fn switcher_disconnect(&mut self, target: &RemoteTarget, cx: &mut Context<Self>) {
        // Deferred: the disconnect detaches the panes of every window on that
        // machine, and reads each window's app to find them — this one among
        // them, which is mid-update right here. Reading it now panics inside
        // the menu's mouse-up handler, and that panic cannot unwind out of
        // the platform callback, so the whole process aborts.
        let host = target.host_id();
        cx.defer(move |cx| crate::ui::remote_workspace::RemoteLinks::disconnect(cx, host));
        if self
            .connect
            .as_ref()
            .and_then(ConnectFlow::choice)
            .is_some_and(|c| &c.target == target)
        {
            self.connect = None;
        }
        // A create still waiting on this machine's link dies with the link:
        // calling the connect off is calling the create off, or the next
        // successful connect would grow a workspace nobody asked it for.
        // One parked on a dialect refusal is waiting all the same, so it
        // dies here too.
        if self
            .pending_create
            .as_ref()
            .is_some_and(|p| &p.target == target)
        {
            self.pending_create = None;
        }
        if self
            .parked_create
            .as_ref()
            .is_some_and(|p| &p.target == target)
        {
            self.parked_create = None;
        }
        cx.notify();
    }

    /// The footer's New Workspace wording and the host the create form starts
    /// on, both from the effective scope: a machine picked by a chip, Alt+←/→
    /// or a half-typed `@token` names itself, and All means this computer's
    /// own form (no host).
    fn create_preset(&self, cx: &mut Context<Self>) -> (String, Option<HostChoice>) {
        let groups = self.switcher_groups(cx);
        let rail = machine_scope::rail_entries(&groups, t(L10nKey::SwitcherMachineAll));
        let stored = self
            .switcher
            .as_ref()
            .map(|sw| sw.scope.clone())
            .unwrap_or_default();
        let query = self
            .switcher
            .as_ref()
            .map(|sw| sw.text(cx))
            .unwrap_or_default();
        let (scope, _) = machine_scope::effective(&stored, &query, &rail);
        let MachineScope::Machine(key) = &scope else {
            return (t(L10nKey::AppMenuNewWorkspace).to_string(), None);
        };
        let Some(group) = groups.iter().find(|g| &g.key == key) else {
            return (t(L10nKey::AppMenuNewWorkspace).to_string(), None);
        };
        (
            t_fmt(
                L10nKey::SwitcherNewWorkspaceOnMachine,
                &[("machine", group.label.as_str())],
            ),
            group.target.clone().map(|target| HostChoice {
                target,
                label: group.label.clone(),
                detail: group.endpoint.clone(),
            }),
        )
    }

    /// `⌘⇧N` and the footer button. Raises the switcher if it is down and
    /// flips the card to the create form. A fresh panel is All, so the usual
    /// `⌘⇧N`, a name, then Enter is a local workspace; a panel that is already
    /// scoped hands the form that machine instead.
    pub(crate) fn open_workspace_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_none() {
            self.open_switcher(window, cx);
        }
        let (_, chosen) = self.create_preset(cx);
        self.switcher_to_form(chosen, window, cx);
    }

    fn switcher_to_form(
        &mut self,
        chosen: Option<HostChoice>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(L10nKey::SwitcherFormNamePlaceholder))
        });
        // Create is enabled by what the box says, so the card redraws as it
        // goes from blank to named and back.
        let name_sub = cx.subscribe_in(
            &name,
            window,
            |_this, _input, ev: &InputEvent, _window, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            },
        );
        let host = cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::FilterHosts)));
        // Retyping the filter moves the dropdown cursor back onto the first hit.
        let sub = cx.subscribe_in(
            &host,
            window,
            |this, _input, ev: &InputEvent, _window, cx| {
                if matches!(ev, InputEvent::Change)
                    && let Some(sw) = this.switcher.as_mut()
                    && let Page::Create(form) = &mut sw.page
                {
                    form.sel = 0;
                    cx.notify();
                }
            },
        );
        name.update(cx, |state, cx| state.focus(window, cx));
        if let Some(sw) = self.switcher.as_mut() {
            sw.page = Page::Create(CreateForm {
                name,
                host,
                open: false,
                sel: 0,
                chosen,
            });
            sw._subs.push(sub);
            sw._subs.push(name_sub);
        }
        cx.notify();
    }

    fn switcher_back_to_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sw) = self.switcher.as_mut() {
            sw.page = Page::List;
            let query = sw.query.clone();
            query.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    /// The host dropdown's lines for the filter text currently in the box.
    fn form_items(&self, form: &CreateForm, cx: &App) -> Vec<HostItem> {
        let query = form.host.read(cx).value().trim().to_string();
        host_items(
            remote_connect::available_hosts(cx),
            &query,
            t(L10nKey::SwitcherThisComputer),
        )
    }

    fn switcher_form_open_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sw) = self.switcher.as_mut()
            && let Page::Create(form) = &mut sw.page
        {
            form.open = true;
            form.sel = 0;
            let host = form.host.clone();
            host.update(cx, |state, cx| {
                state.set_value("", window, cx);
                state.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn switcher_form_close_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sw) = self.switcher.as_mut()
            && let Page::Create(form) = &mut sw.page
        {
            form.open = false;
            let name = form.name.clone();
            name.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    fn switcher_form_pick(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let item = {
            let Some(sw) = self.switcher.as_ref() else {
                return;
            };
            let Page::Create(form) = &sw.page else {
                return;
            };
            let mut items = self.form_items(form, cx);
            if at >= items.len() {
                return;
            }
            items.swap_remove(at)
        };
        match item {
            HostItem::Local => self.switcher_form_choose(None, window, cx),
            HostItem::Host(choice) => self.switcher_form_choose(Some(choice), window, cx),
            HostItem::AddHost => {
                self.close_switcher(window, cx);
                self.open_new_ssh_host(window, cx);
            }
        }
    }

    fn switcher_form_choose(
        &mut self,
        chosen: Option<HostChoice>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(sw) = self.switcher.as_mut()
            && let Page::Create(form) = &mut sw.page
        {
            form.chosen = chosen;
            form.open = false;
            form.sel = 0;
            let (host, name) = (form.host.clone(), form.name.clone());
            host.update(cx, |state, cx| state.set_value("", window, cx));
            name.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    /// Enter or Create on the form. Nothing happens until the name box says
    /// something. Local and already-connected machines create on the spot; a
    /// machine with no live link has to connect first — the create is parked
    /// on the app and `finish_connect` completes it, because only a live link
    /// knows the home directory a fresh workspace is rooted at.
    fn switcher_form_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (name, chosen) = {
            let Some(sw) = self.switcher.as_ref() else {
                return;
            };
            let Page::Create(form) = &sw.page else {
                return;
            };
            let Some(name) = form.name(cx) else {
                return;
            };
            (Some(name), form.chosen.clone())
        };
        match chosen {
            None => {
                self.close_switcher(window, cx);
                self.switch_workspace(None, window, cx);
                self.name_fresh_workspace(name, window, cx);
            }
            Some(choice) => match remote_connect::HostLinks::home(cx, choice.target.host_id()) {
                Some(home) => {
                    self.close_switcher(window, cx);
                    self.create_remote_workspace(choice.target.clone(), home, window, cx);
                    self.name_fresh_workspace(name, window, cx);
                }
                None => {
                    self.pending_create = Some(PendingCreate {
                        target: choice.target.clone(),
                        name,
                    });
                    // Back to the list, where the connect has somewhere to be
                    // seen while it works.
                    self.switcher_back_to_list(window, cx);
                    self.connect_to_host(choice, cx);
                }
            },
        }
    }

    /// Names the workspace this window just created and switched to.
    pub(crate) fn name_fresh_workspace(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = name else {
            return;
        };
        crate::ui::tree_sync::name_new_workspace(cx, self.workspace, name);
        crate::ui::windows::refresh_menu(cx);
        self.sync_window_title(window, cx);
    }

    /// Arrow keys and Enter for the panel. These run ahead of the text input's
    /// own `MoveUp`/`MoveDown` bindings, so anything handled here must stop
    /// propagating or the cursor jumps inside the search box instead.
    fn on_switcher_key(
        &mut self,
        ev: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sw) = self.switcher.as_ref() else {
            return;
        };
        if matches!(sw.page, Page::Create(_)) {
            self.on_form_key(ev, window, cx);
            return;
        }
        let (key, mods) = (ev.keystroke.key.as_str(), ev.keystroke.modifiers);
        if key == "escape" {
            cx.stop_propagation();
            // A rename in progress is the smaller thing to back out of; the
            // panel itself only closes on a second press.
            if let Some(sw) = self.switcher.as_mut()
                && sw.renaming.take().is_some()
            {
                let query = sw.query.clone();
                query.update(cx, |state, cx| state.focus(window, cx));
                cx.notify();
                return;
            }
            self.close_switcher(window, cx);
            return;
        }
        // A rename box owns every key while it is up.
        if sw.renaming.is_some() {
            return;
        }
        let column = sw.column;
        // Backspace on an empty search box steps back out of a machine. Shift
        // keeps the same deletion meaning (the input binds shift-backspace to
        // Backspace), while Alt and Ctrl turn it into word deletion, so they
        // stay the input's.
        let leave_machine = key == "backspace"
            && !mods.alt
            && !mods.function
            && !mods.control
            && !mods.secondary()
            && sw.scope != MachineScope::All
            && sw.query.read(cx).value().is_empty();
        if leave_machine {
            cx.stop_propagation();
            self.set_switcher_scope(MachineScope::All, cx);
            return;
        }
        let layout = self.switcher_layout(cx);
        match key_intent(key, mods) {
            // Escape already returned above; closing again is harmless and
            // beats a panic in the render path if that ever stops being true.
            Key::Close => self.close_switcher(window, cx),
            Key::Pass => {}
            Key::Machine(delta) => {
                cx.stop_propagation();
                // Step from the chip the list is effectively showing — a
                // half-typed `@token`'s preview included — so "next" is
                // where the user is looking, and let the pick drop the token
                // so its preview cannot undo it. The rail carries itself to
                // the new chip through `render_rail`, like every scope move.
                let scope = machine_scope::step(&layout.rail, &layout.scope, delta);
                self.select_switcher_machine(scope, window, cx);
            }
            Key::Step(forward) => {
                cx.stop_propagation();
                match column {
                    Column::Left => self.switcher_step_left(&layout, forward, window, cx),
                    Column::Right => self.switcher_step_right(&layout, forward, window, cx),
                }
            }
            // Once there is a query, left and right belong to the caret in the
            // search box; Tab is then the way across.
            Key::ToColumn(Column::Left) if column == Column::Right && sw.text(cx).is_empty() => {
                cx.stop_propagation();
                self.switcher_focus(Column::Left, cx);
            }
            Key::ToColumn(Column::Right) if column == Column::Left && sw.text(cx).is_empty() => {
                let has_tabs = layout
                    .subject_row(sw.left_sel)
                    .is_some_and(|r| !r.tabs.is_empty());
                if has_tabs {
                    cx.stop_propagation();
                    self.switcher_focus(Column::Right, cx);
                }
            }
            Key::ToColumn(_) => {}
            Key::Tab(forward) => {
                cx.stop_propagation();
                self.switcher_step_right(&layout, forward, window, cx);
            }
            Key::Confirm(new_window) => {
                cx.stop_propagation();
                self.switcher_confirm(&layout, new_window, window, cx);
            }
        }
    }

    /// The create form's keys. Characters fall through to whichever input is
    /// focused; everything structural is decided here, ahead of the inputs'
    /// own bindings, so it has to stop propagating whatever it handles.
    fn on_form_key(
        &mut self,
        ev: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (open, sel, n) = {
            let Some(sw) = self.switcher.as_ref() else {
                return;
            };
            let Page::Create(form) = &sw.page else {
                return;
            };
            (form.open, form.sel, self.form_items(form, cx).len())
        };
        let (key, mods) = (ev.keystroke.key.as_str(), ev.keystroke.modifiers);
        let bare = !mods.alt && !mods.function && !mods.control && !mods.secondary();
        match key {
            // The dropdown is the smaller thing to back out of; the form only
            // folds back into the list on a second press.
            "escape" => {
                cx.stop_propagation();
                match open {
                    true => self.switcher_form_close_hosts(window, cx),
                    false => self.switcher_back_to_list(window, cx),
                }
            }
            "tab" if bare || mods.shift => {
                cx.stop_propagation();
                match open {
                    true => self.switcher_form_close_hosts(window, cx),
                    false => self.switcher_form_open_hosts(window, cx),
                }
            }
            "up" | "down" if bare => {
                cx.stop_propagation();
                match open {
                    true if n > 0 => {
                        if let Some(sw) = self.switcher.as_mut()
                            && let Page::Create(form) = &mut sw.page
                        {
                            form.sel = step(sel.min(n - 1), n, key == "down");
                        }
                        cx.notify();
                    }
                    false if key == "down" => self.switcher_form_open_hosts(window, cx),
                    _ => {}
                }
            }
            "enter" if bare => {
                cx.stop_propagation();
                match open {
                    true => self.switcher_form_pick(sel, window, cx),
                    false => self.switcher_form_create(window, cx),
                }
            }
            _ => {}
        }
    }

    /// The anchor a row wears while it is the selected one, so stepping the
    /// cursor with the keyboard carries the column to it.
    fn switcher_anchor(&self, column: Column, picked: bool) -> Option<gpui::ScrollAnchor> {
        let sw = self.switcher.as_ref()?;
        picked.then(|| match column {
            Column::Left => sw.left_anchor.clone(),
            Column::Right => sw.right_anchor.clone(),
        })
    }

    /// Moves the left cursor to a clicked row so the tab column follows it.
    /// Deliberately not wired to hover — the tab column swapping out from under
    /// the pointer on the way to somewhere else is noise, not a preview.
    fn switcher_point_at(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some(sw) = self.switcher.as_mut() else {
            return;
        };
        if sw.left_sel == at && sw.column == Column::Left {
            return;
        }
        sw.left_sel = at;
        sw.right_sel = 0;
        sw.column = Column::Left;
        cx.notify();
    }

    /// Aims the tab cursor at one row, without acting on it.
    fn switcher_point_tab(&mut self, nth: usize, cx: &mut Context<Self>) {
        if let Some(sw) = self.switcher.as_mut() {
            sw.column = Column::Right;
            sw.right_sel = nth;
            cx.notify();
        }
    }

    fn switcher_focus(&mut self, column: Column, cx: &mut Context<Self>) {
        if let Some(sw) = self.switcher.as_mut() {
            sw.column = column;
        }
        cx.notify();
    }

    fn switcher_step_left(
        &mut self,
        layout: &Layout,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = layout.nav.len();
        let Some(sw) = self.switcher.as_mut() else {
            return;
        };
        if n == 0 {
            return;
        }
        sw.column = Column::Left;
        sw.left_sel = step(sw.left_sel.min(n - 1), n, forward);
        // A different workspace means a different tab column.
        sw.right_sel = 0;
        let anchor = sw.left_anchor.clone();
        anchor.scroll_to(window, cx);
        cx.notify();
    }

    fn switcher_step_right(
        &mut self,
        layout: &Layout,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sel = self.switcher.as_ref().map(|sw| sw.left_sel).unwrap_or(0);
        let query = self
            .switcher
            .as_ref()
            .map(|sw| sw.text(cx))
            .unwrap_or_default();
        let n = layout
            .subject_row(sel)
            .map(|row| visible_tabs(row, &query).len())
            .unwrap_or(0);
        let Some(sw) = self.switcher.as_mut() else {
            return;
        };
        if n == 0 {
            return;
        }
        sw.column = Column::Right;
        sw.right_sel = step(sw.right_sel.min(n - 1), n, forward);
        let anchor = sw.right_anchor.clone();
        anchor.scroll_to(window, cx);
        cx.notify();
    }

    fn switcher_confirm(
        &mut self,
        layout: &Layout,
        new_window: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sw) = self.switcher.as_ref() else {
            return;
        };
        let (sel, column, right_sel) = (sw.left_sel, sw.column, sw.right_sel);
        let query = sw.text(cx);
        // Nothing under the cursor — an empty list, or a filter that left no
        // row to stand on. This sits ahead of the column split so the tab
        // column cannot swallow it: it can stay the active column after a
        // query narrowed the list away. On a machine that is not connected,
        // Enter is the obvious way to ask for the connection; a selected row
        // keeps its own open/reconnect path below.
        let Some(&(g, r)) = layout.nav.get(sel) else {
            if new_window {
                return;
            }
            if let Some(group) = offline_connect_group(&layout.scope, &layout.groups)
                && let Some(choice) = connect_choice(group)
            {
                self.connect_to_host(choice, cx);
            }
            return;
        };
        if column == Column::Right {
            let row = &layout.groups[g].rows[r];
            let Some(tab) = visible_tabs(row, &query)
                .get(right_sel)
                .and_then(|i| row.tabs.get(*i))
            else {
                return;
            };
            let (ws, id) = (row.id, tab.id);
            self.switcher_open_tab(ws, id, new_window, window, cx);
            return;
        }
        let group = &layout.groups[g];
        let row = RowRef::of(group, &group.rows[r]);
        self.switcher_open(row, new_window, window, cx);
    }

    fn switcher_open_tab(
        &mut self,
        ws: WorkspaceId,
        tab: TabId,
        new_window: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_switcher(window, cx);
        self.go_to_tab(ws, tab, new_window, window, cx);
    }

    /// Activates one tab of `ws`, wherever that workspace happens to live.
    pub(crate) fn go_to_tab(
        &mut self,
        ws: WorkspaceId,
        tab: TabId,
        new_window: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if new_window {
            crate::ui::windows::open_at_tab(cx, ws, tab);
            return;
        }
        if ws == self.workspace {
            self.activate_tree_tab(tab, window, cx);
            return;
        }
        // A workspace that already has a window belongs to that window; anything
        // else comes here, the same as picking the workspace itself.
        if crate::ui::windows::WindowRegistry::window_for(cx, ws).is_some() {
            crate::ui::windows::open_at_tab(cx, ws, tab);
            return;
        }
        self.switch_workspace(Some(ws), window, cx);
        self.activate_tree_tab(tab, window, cx);
    }

    pub(crate) fn render_switcher(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let sw = self.switcher.as_ref()?;
        let scrim = crate::ui::presets::scrim_fill(cx);
        let card = match &sw.page {
            Page::Create(form) => self.render_create_card(form, window, cx),
            Page::List => self.render_list_card(window, cx),
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(CARD_TOP))
                .bg(scrim)
                .key_context("Switcher")
                .on_action(cx.listener(|this, _: &SwitcherAcross, window, cx| {
                    let layout = this.switcher_layout(cx);
                    this.switcher_step_right(&layout, true, window, cx);
                }))
                .on_action(cx.listener(|this, _: &SwitcherAcrossBack, window, cx| {
                    let layout = this.switcher_layout(cx);
                    this.switcher_step_right(&layout, false, window, cx);
                }))
                .on_key_down(cx.listener(Self::on_switcher_key))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        // The scrim covers the whole window, the tile that
                        // opens the switcher included. Without this the press
                        // dismissed here and then carried on down to that
                        // tile, whose click toggled the switcher straight back
                        // open — so clicking it a second time looked like it
                        // did nothing. A dismissing click is spent on the
                        // dismissal and reaches nothing beneath it.
                        cx.stop_propagation();
                        this.close_switcher(window, cx)
                    }),
                )
                .child(
                    div()
                        .id("switcher-card")
                        .occlude()
                        .on_hover(cx.listener(|this, hovered: &bool, _window, _cx| {
                            if let Some(sw) = this.switcher.as_mut() {
                                sw.hover_card = *hovered;
                            }
                        }))
                        .child(card),
                )
                .into_any_element(),
        )
    }

    fn render_list_card(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let sw = self.switcher.as_ref().expect("only rendered while up");
        let (sel, column) = (sw.left_sel, sw.column);
        let (left_scroll, right_scroll) = (sw.left_scroll.clone(), sw.right_scroll.clone());
        let layout = self.switcher_layout(cx);

        let theme = cx.theme();
        let border = theme.border;

        let mut list = v_flex().gap(px(1.));
        // A machine that is not connected says so above its remembered rows,
        // and the memory is not split by open state: none of it can be opened
        // until the connect lands.
        let offline = offline_connect_group(&layout.scope, &layout.groups);
        let last_known = offline.is_some();
        if let Some(group) = offline {
            list = list.child(self.render_offline_card(group, cx));
        }
        let sections = nav_sections(&layout.nav, &layout.groups, &layout.scope, last_known);
        let mut heading: Option<Section> = None;
        for (at, (&(g, r), section)) in layout.nav.iter().zip(sections).enumerate() {
            if let Some(section) = section
                && heading != Some(section)
            {
                list = list.child(section_heading(section, cx));
                heading = Some(section);
            }
            let picked = sel == at && column == Column::Left;
            let group = &layout.groups[g];
            // Scoped rows live on the one machine the list is about; repeating
            // its name on every sub-line is noise.
            let show_machine = matches!(layout.scope, MachineScope::All);
            list = list.child(self.render_row(
                group,
                &group.rows[r],
                picked,
                Some(at),
                show_machine,
                cx,
            ));
        }
        if layout.nav.is_empty() {
            list = list.child(
                div()
                    .px(px(ROW_PAD))
                    .py(px(14.))
                    .text_size(gpui::rems(13. / 16.))
                    .text_color(cx.theme().muted_foreground)
                    .child(empty_list_text(&layout.scope, &layout.groups)),
            );
        }
        // Orphan panes belong to the machine, not to a workspace, so they are
        // not rows and join no navigation — the bottom of the workspace list
        // is simply where a user looking for them finds them (#596). A search
        // narrows the panel to workspaces, and the box steps out of the way
        // for one. A remote scope is about another computer's workspaces, so
        // this machine's leaks are not on offer there.
        if !sw.orphans.is_empty() && sw.text(cx).is_empty() && orphans_visible(&layout.scope) {
            list = list.child(self.render_orphan_panes(cx));
        }

        // Fixed height, not fit-to-content: the tab column changes length every
        // time the left cursor moves, and a card that resizes under the pointer
        // is unusable.
        // The card is fixed-size by design — a panel that resizes under the
        // pointer is unusable — but it still has to fit the window it floats
        // over. Below its natural size it takes what there is.
        let viewport = window.viewport_size();
        let card_w = CARD_W
            .min(viewport.width.as_f32() - 2. * CARD_MARGIN)
            .max(320.);
        let body_h = BODY_H
            .min(viewport.height.as_f32() - CARD_TOP - CARD_CHROME_H - CARD_MARGIN)
            .max(120.);
        let body = div()
            .flex()
            .flex_row()
            .items_stretch()
            .h(px(body_h))
            // Both columns scroll once the lists outrun the card, and neither
            // said so. The border moves out to the column so it stays put
            // while the rows underneath it move.
            .child(
                v_flex()
                    .w(px(LEFT_W.min(card_w * 0.5)))
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(border)
                    .child(crate::ui::scrollbar::with_vertical_scrollbar(
                        "switcher-workspaces-scrollbar",
                        div()
                            .id("switcher-workspaces")
                            .track_scroll(&left_scroll)
                            .size_full()
                            .overflow_y_scroll()
                            .p(px(COLUMN_PAD))
                            .child(list),
                        &left_scroll,
                    )),
            )
            .child(
                v_flex()
                    .id("switcher-tab-column")
                    .flex_1()
                    .min_w_0()
                    .on_hover(cx.listener(|this, hovered: &bool, _window, _cx| {
                        if let Some(sw) = this.switcher.as_mut() {
                            sw.hover_tabs = *hovered;
                        }
                    }))
                    .child(crate::ui::scrollbar::with_vertical_scrollbar(
                        "switcher-tabs-scrollbar",
                        div()
                            .id("switcher-tabs")
                            .track_scroll(&right_scroll)
                            .size_full()
                            .overflow_y_scroll()
                            .p(px(COLUMN_PAD))
                            .child(self.render_tabs(&layout, sel, column, cx)),
                        &right_scroll,
                    )),
            );

        v_flex()
            .w(px(card_w))
            .map(|panel| crate::ui::theme::floating_surface(panel, cx))
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .child(self.render_search(cx))
            .child(self.render_rail(&layout, cx))
            .child(body)
            .children(self.render_banners(&layout, cx))
            .child(self.render_footer(cx))
            .into_any_element()
    }

    /// The machine rail between the search box and the columns: one chip per
    /// `layout.rail` entry — All, this computer, then every configured host —
    /// plus "Add machine" at the end. The chip the list is effectively scoped
    /// to is marked, a half-typed `@token`'s preview included, and picking a
    /// chip (click or Alt+←/→) goes through `select_switcher_machine` so the
    /// stored scope wins over any token left in the box.
    fn render_rail(&self, layout: &Layout, cx: &mut Context<Self>) -> AnyElement {
        let sw = self.switcher.as_ref().expect("only rendered while up");
        let theme = cx.theme();
        let (fg, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let sf = rungs(cx);
        let (hover, picked_bg) = (gpui::rgb(sf.hover), gpui::rgb(sf.selected));
        let rail_scroll = sw.rail_scroll.clone();
        // The rail follows the effective scope wherever the move came from —
        // a click, Alt+←/→, or a `@token` typed into the box — but only when
        // the chip it points at actually moved, and by the minimal amount: a
        // rail that asked on every frame would fight a hand that is only
        // browsing along it. What is followed is the chip's *index*, not the
        // scope: a configured host regrouping from empty to populated keeps
        // its scope but changes places on the rail, and the rail should
        // carry to the new place. The chips are the scroller's direct
        // children, which is the index `scroll_to_item` speaks.
        let picked_ix = layout.rail.iter().position(|e| e.scope == layout.scope);
        if sw.rail_followed.replace(picked_ix) != picked_ix
            && let Some(ix) = picked_ix
        {
            rail_scroll.scroll_to_item(ix);
        }

        let mut chips: Vec<AnyElement> = Vec::with_capacity(layout.rail.len() + 1);
        for (ix, entry) in layout.rail.iter().enumerate() {
            let picked = entry.scope == layout.scope;
            let scope = entry.scope.clone();
            chips.push(
                h_flex()
                    .id(("switcher-rail-chip", ix))
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(6.))
                    .h(px(RAIL_CHIP_H))
                    .px(px(10.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(gpui::rems(12.5 / 16.))
                    .text_color(fg)
                    .when(picked, |chip| chip.bg(picked_bg))
                    .hover(move |chip| chip.bg(if picked { picked_bg } else { hover }))
                    .children(
                        entry
                            .link
                            .map(rail_dot_style)
                            .map(|style| rail_dot(style, theme)),
                    )
                    .child(entry.label.clone())
                    // Zero says nothing: an empty machine is exactly what its
                    // chip is there to name, and the count would only add
                    // noise to the machines that matter.
                    .when(entry.count > 0, |chip| {
                        chip.child(
                            div()
                                .flex_shrink_0()
                                .text_color(muted)
                                .child(entry.count.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // The listener is an Fn that outlives the click, so
                        // the captured scope travels by clone.
                        this.select_switcher_machine(scope.clone(), window, cx);
                    }))
                    .into_any_element(),
            );
        }
        // The way out when the machine wanted is not configured yet — the
        // same exit the create form's host dropdown offers, reached one step
        // earlier.
        chips.push(
            h_flex()
                .id("switcher-rail-add")
                .flex_shrink_0()
                .items_center()
                .gap(px(6.))
                .h(px(RAIL_CHIP_H))
                .px(px(10.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(gpui::rems(12.5 / 16.))
                .text_color(muted)
                .hover(move |chip| chip.bg(hover))
                .child(Icon::new(IconName::Plus).size(px(10.)).text_color(muted))
                .child(t(L10nKey::SwitcherAddMachine))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_switcher(window, cx);
                    this.open_new_ssh_host(window, cx);
                }))
                .into_any_element(),
        );

        h_flex()
            .items_center()
            .gap(px(8.))
            .h(px(RAIL_H))
            .px(px(COLUMN_PAD))
            .border_b_1()
            .border_color(border)
            .child(
                // The chips scroll once the configured machines outrun the
                // card; the hint stays pinned past them so it is always where
                // the eye left it.
                h_flex()
                    .id("switcher-rail")
                    .track_scroll(&rail_scroll)
                    .min_w_0()
                    .flex_1()
                    .items_center()
                    .gap(px(6.))
                    .overflow_x_scroll()
                    .children(chips),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(gpui::rems(12.5 / 16.))
                    .text_color(muted)
                    // The modifier names itself the way the footer's
                    // shortcuts do — a glyph on macOS, a word elsewhere —
                    // and the localized template places it.
                    .child(t_fmt(
                        L10nKey::SwitcherMachineHint,
                        &[(
                            "modifier",
                            if cfg!(target_os = "macos") {
                                "⌥"
                            } else {
                                "Alt"
                            },
                        )],
                    )),
            )
            .into_any_element()
    }

    /// Narrows the left column to `scope` — the one way the rail, Alt+←/→ and
    /// a committed `@token` change it. The cursor goes back to the top so the
    /// tab column shows the new list's first workspace, even when the scope
    /// is the one already stored: clicking the selected chip is how the
    /// keyboard user gets back to the top.
    pub(crate) fn set_switcher_scope(&mut self, scope: MachineScope, cx: &mut Context<Self>) {
        let Some(sw) = self.switcher.as_mut() else {
            return;
        };
        sw.scope = scope;
        sw.left_sel = 0;
        sw.right_sel = 0;
        sw.column = Column::Left;
        cx.notify();
    }

    /// Turns a committed `@machine` token into the stored scope, the way the
    /// space after it is meant to: the token leaves the box and the rest of
    /// the query keeps searching. A half-typed token still previews, and an
    /// unresolved one is left for the search to find nothing with.
    fn commit_query_machine_token(
        &mut self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.switcher.as_ref().is_none_or(|sw| sw.committing_token) {
            return;
        }
        let text = input.read(cx).value().to_string();
        let Some(token) = machine_scope::parse_at_token(&text) else {
            return;
        };
        if !token.committed {
            return;
        }
        let (needle, rest) = (token.needle.to_string(), token.rest.to_string());
        let entries =
            machine_scope::rail_entries(&self.switcher_groups(cx), t(L10nKey::SwitcherMachineAll));
        let Some(scope) = machine_scope::resolve_needle(&entries, &needle) else {
            return;
        };
        self.set_switcher_scope(scope, cx);
        if let Some(sw) = self.switcher.as_mut() {
            sw.committing_token = true;
        }
        input.update(cx, |state, cx| state.set_value(rest, window, cx));
        if let Some(sw) = self.switcher.as_mut() {
            sw.committing_token = false;
        }
    }

    /// Picks a machine from the rail — a chip click or an Alt+←/→ step — by
    /// setting the stored scope and dropping a leading `@token` from the
    /// query. The token previews its own machine every frame, so leaving it
    /// in place would undo the pick; whatever was typed after it is the
    /// search that stays.
    pub(crate) fn select_switcher_machine(
        &mut self,
        scope: MachineScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sw) = self.switcher.as_ref() else {
            return;
        };
        let query = sw.query.clone();
        let text = query.read(cx).value().to_string();
        let rest = machine_scope::parse_at_token(&text).map(|token| token.rest.to_string());
        self.set_switcher_scope(scope, cx);
        if let Some(rest) = rest {
            query.update(cx, |state, cx| state.set_value(rest, window, cx));
        }
    }

    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        h_flex()
            .items_center()
            .gap(px(10.))
            .pl(px(18.))
            .pr(px(14.))
            .h(px(SEARCH_H))
            .border_b_1()
            .border_color(border)
            .child(Icon::new(IconName::Search).size(px(12.)).text_color(muted))
            .child(
                div().flex_1().min_w_0().children(
                    self.switcher
                        .as_ref()
                        .map(|sw| Input::new(&sw.query).appearance(false).pl_0()),
                ),
            )
            .child(keycap("esc", cx))
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (fg, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let hover = hover_fill(cx);
        let holding = self.switcher.as_ref().is_some_and(|sw| sw.hold.is_some());
        // With a query in the box, ← and → belong to the caret and Tab becomes
        // the way across. That remap is deliberate, and until now it was also
        // silent: arrows simply stopped working with nothing said.
        let filtering = self
            .switcher
            .as_ref()
            .is_some_and(|sw| !sw.text(cx).trim().is_empty());
        let (create_label, _) = self.create_preset(cx);
        let hint = |keys: Vec<AnyElement>, label: String| {
            h_flex()
                .items_center()
                .gap(px(6.))
                .children(keys)
                .child(label)
                .into_any_element()
        };
        let mut hints: Vec<AnyElement> = Vec::new();
        if holding {
            hints.push(
                div()
                    .child(t(L10nKey::SwitcherHoldToSwitch))
                    .into_any_element(),
            );
        } else {
            if filtering {
                hints.push(
                    div()
                        .child(t(L10nKey::SwitcherTabToCrossColumns))
                        .into_any_element(),
                );
            }
            hints.push(hint(
                vec![keycap("↑", cx), keycap("↓", cx)],
                t(L10nKey::SwitcherHintNavigate).to_string(),
            ));
            hints.push(hint(
                vec![keycap("↵", cx)],
                t(L10nKey::SwitcherHintOpen).to_string(),
            ));
            hints.push(hint(
                vec![
                    keycap(crate::ui::keymap::secondary_glyph(), cx),
                    keycap("↵", cx),
                ],
                t(L10nKey::SwitcherHintNewWindow).to_string(),
            ));
        }
        h_flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .border_t_1()
            .border_color(border)
            .h(px(FOOTER_H))
            .pl(px(8.))
            .pr(px(10.))
            .child(
                h_flex()
                    .id("switcher-new-workspace")
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(move |r| r.bg(hover))
                    .text_size(gpui::rems(12.5 / 16.))
                    .text_color(fg)
                    .child(Icon::new(IconName::Plus).size(px(10.)).text_color(muted))
                    .child(create_label)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let (_, chosen) = this.create_preset(cx);
                        this.switcher_to_form(chosen, window, cx);
                    })),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .overflow_hidden()
                    .items_center()
                    .gap(px(16.))
                    .text_size(gpui::rems(11.5 / 16.))
                    .text_color(muted)
                    .children(hints),
            )
    }

    /// The machine-trouble bands between the list and the footer: install
    /// progress, a failed connect with its retry, a parked route (#485), or a
    /// connect still in flight. With the per-machine headers gone this is
    /// where a machine's state gets to speak; a healthy machine says nothing
    /// here — its state dot rides on its workspace rows.
    fn render_banners(&self, layout: &Layout, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        // The connect card already says what went wrong and offers the way
        // back; the error band under it would be the same sentence twice.
        // Only the carded group's band steps aside — a parked or in-flight
        // machine has no card and keeps its own banner.
        let carded = offline_connect_group(&layout.scope, &layout.groups).map(|g| g.key.as_str());
        for group in layout.groups.iter().filter(|g| layout.scope.admits(g)) {
            if group.target.is_none() {
                continue;
            }
            if let Some(phase) = group.installing {
                out.push(
                    self.render_install_progress(&group.label, phase, cx)
                        .into_any_element(),
                );
            } else if group.parked && !group.rows.is_empty() {
                // Dismissed means quiet. Falling through would put the
                // failure that came with the parking straight back up as the
                // error band, under a route nothing can retry.
                if !self.parked_dismissed.contains(&group.key) {
                    out.push(self.render_parked_notice(group, cx).into_any_element());
                }
            } else if let Some(error) = group.error.clone()
                && carded != Some(group.key.as_str())
            {
                out.push(self.render_error_band(group, &error, cx));
            } else if matches!(group.link, Link::Connecting | Link::Reconnecting { .. }) {
                let theme = cx.theme();
                out.push(
                    h_flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(12.))
                        .py(px(6.))
                        .border_t_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex_shrink_0()
                                .size(px(6.))
                                .rounded_full()
                                .bg(theme.warning),
                        )
                        .child(
                            div()
                                .text_size(gpui::rems(11. / 16.))
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(t_fmt(
                                    L10nKey::SwitcherConnectingTo,
                                    &[("machine", &group.label)],
                                )),
                        )
                        .into_any_element(),
                );
            }
        }
        out
    }

    fn render_error_band(&self, group: &Group, error: &str, cx: &mut Context<Self>) -> AnyElement {
        let retry = GroupRef::of(group);
        let replace = retry.clone();
        let retry_key = group.key.clone();
        let replace_key = group.key.clone();
        let dismiss_key = group.key.clone();
        let dismiss_target = group.target.clone();
        let dismiss_error = error.to_string();
        // The band no longer sits under a machine header, so plain errors
        // carry the machine's name themselves; the dialect restatement
        // already names it.
        let shown = remote_connect::dialect_complaint(error, &group.label)
            .unwrap_or_else(|| format!("{}: {error}", group.label));
        let replace_action = crate::ui::remote_workspace::mismatch_action_key(error);
        let theme = cx.theme();
        v_flex()
            .gap(px(4.))
            .px(px(12.))
            .py(px(8.))
            .border_t_1()
            .border_color(theme.danger.opacity(0.4))
            .child(
                div()
                    .text_size(gpui::rems(11. / 16.))
                    .text_color(theme.muted_foreground)
                    .child(shown),
            )
            .child(
                h_flex()
                    .gap(px(4.))
                    // A route that no longer resolves cannot be retried into
                    // working (#485); a button that can only fail again is
                    // worse than none.
                    .when(!group.parked, |row| {
                        row.child(
                            Button::new(gpui::SharedString::from(format!(
                                "switcher-retry:{}",
                                group.key
                            )))
                            .label(t(L10nKey::TryAgain))
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(
                                move |this, _, _window, cx| {
                                    let ours = this.remote_host_errors.remove(&retry_key).is_some()
                                        || matches!(
                                            &this.connect,
                                            Some(ConnectFlow::Failed { choice, .. })
                                                if Some(&choice.target) == retry.target.as_ref()
                                        );
                                    if let Some(target) = retry.target.clone() {
                                        let host = target.host_id();
                                        if !ours && RemoteLinks::machine_error(cx, host).is_some() {
                                            RemoteLinks::retry_machine_now(cx, host);
                                            cx.notify();
                                            return;
                                        }
                                        this.connect_to_host(
                                            HostChoice {
                                                target,
                                                label: retry.label.clone(),
                                                detail: String::new(),
                                            },
                                            cx,
                                        );
                                    }
                                },
                            )),
                        )
                    })
                    .when(
                        crate::daemon::control::is_dialect_refusal(error)
                            // Same gate as the workspace strip's: a machine
                            // whose server is not ours to install cannot be
                            // helped by this button, and a click that can only
                            // fail is worse than no button.
                            && replace
                                .target
                                .as_ref()
                                .is_some_and(|t| t.hosts_our_server()),
                        |row| {
                            row.child(
                                Button::new(gpui::SharedString::from(format!(
                                    "switcher-replace:{}",
                                    group.key
                                )))
                                .label(t(replace_action))
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.remote_host_errors.remove(&replace_key);
                                        if let Some(target) = replace.target.clone() {
                                            this.confirm_replace_remote_server(
                                                target,
                                                replace.label.clone(),
                                                replace_action,
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                        },
                    )
                    .child(
                        Button::new(gpui::SharedString::from(format!(
                            "switcher-dismiss:{}",
                            group.key
                        )))
                        .label(t(L10nKey::Dismiss))
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(
                            move |this, _, _window, cx| {
                                this.remote_host_errors.remove(&dismiss_key);
                                // The supervisor keeps its own copy of the
                                // reason, and it is the one that outlives every
                                // other source: without this the band is back
                                // on the very next frame.
                                if let Some(target) = &dismiss_target {
                                    let host = target.host_id();
                                    if RemoteLinks::machine_error(cx, host).as_ref()
                                        == Some(&dismiss_error)
                                    {
                                        RemoteLinks::dismiss_machine_error(
                                            cx,
                                            host,
                                            &dismiss_error,
                                        );
                                    }
                                }
                                // The other half of this block can come from a
                                // failed connect. Retire that too, but only when
                                // it is this host's failure — a connect to
                                // anywhere else is still in flight.
                                if let Some(ConnectFlow::Failed { choice, .. }) = &this.connect
                                    && Some(&choice.target) == dismiss_target.as_ref()
                                {
                                    this.connect = None;
                                }
                                // Dismissing the refusal is giving up on the
                                // machine, and the create parked on it goes
                                // with it — an update run weeks later must
                                // not revive a workspace nobody remembers
                                // asking for.
                                if this
                                    .parked_create
                                    .as_ref()
                                    .is_some_and(|p| Some(&p.target) == dismiss_target.as_ref())
                                {
                                    this.parked_create = None;
                                }
                                cx.notify();
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    /// The card a scoped machine gets while it is not connected: why the rows
    /// below are only remembered ones, where the connect would dial, and the
    /// button that starts it. Parked machines never get this card — their
    /// route cannot be rebuilt (#485).
    fn render_offline_card(&self, group: &Group, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        let Some(choice) = connect_choice(group) else {
            return div().into_any_element();
        };
        let muted_text = gpui::rems(11. / 16.);
        // The failure, when there is one, gives way first: the endpoint stays
        // whole so the card still says where the connect would go.
        let detail: AnyElement = match offline_card_detail(group) {
            (Some(error), endpoint) => h_flex()
                .items_center()
                .gap(px(6.))
                .min_w_0()
                .text_size(muted_text)
                .text_color(muted)
                .child(div().flex_1().min_w_0().truncate().child(error))
                .child(div().flex_shrink_0().whitespace_nowrap().child(endpoint))
                .into_any_element(),
            (None, endpoint) => div()
                .truncate()
                .text_size(muted_text)
                .text_color(muted)
                .child(endpoint)
                .into_any_element(),
        };
        v_flex()
            .gap(px(4.))
            .mx(px(4.))
            .mt(px(4.))
            .mb(px(6.))
            .px(px(10.))
            .py(px(8.))
            .rounded(crate::ui::rounding::ROW_RADIUS)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(gpui::rems(12. / 16.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(fg)
                    .child(t_fmt(
                        L10nKey::SwitcherMachineNotConnected,
                        &[("machine", &group.label)],
                    )),
            )
            .child(detail)
            .child(
                h_flex().pt(px(2.)).child(
                    Button::new(gpui::SharedString::from(format!(
                        "switcher-connect-machine:{}",
                        group.key
                    )))
                    .label(t(L10nKey::Connect))
                    .xsmall()
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.connect_to_host(choice.clone(), cx);
                    })),
                ),
            )
            .into_any_element()
    }

    /// The orphan block under the local group (#596): one line per pane no
    /// window holds — id, owner, where it runs — and the way to stop it.
    fn render_orphan_panes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let Some(switcher) = self.switcher.as_ref() else {
            return v_flex();
        };
        // Resolved up front, while `cx` can still be borrowed as an `App`: the
        // render loop below needs it mutably for the Close listener.
        let lines: Vec<(u64, String, Option<String>)> = switcher
            .orphans
            .iter()
            .map(|orphan| {
                let mut bits = vec![format!("%{}", orphan.pane_id)];
                if let Some(owner) = &orphan.owner {
                    bits.push(owner_label(owner, workspace_name_of(owner, cx)));
                }
                if let Some(cwd) = &orphan.cwd {
                    bits.push(cwd.clone());
                } else if !orphan.title.is_empty() {
                    bits.push(orphan.title.clone());
                }
                (
                    orphan.pane_id,
                    bits.join(" · "),
                    switcher.orphan_failures.get(&orphan.pane_id).cloned(),
                )
            })
            .collect();
        let mut list = v_flex().gap(px(2.));
        for (pane_id, line, failure) in lines {
            list = list.child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    // The text takes the slack and gives it back: without
                    // `min_w_0` a flex child refuses to shrink below its
                    // content, and one long cwd pushed the Close button past
                    // the edge of the card — the row named a pane the user
                    // then had no way to stop.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(gpui::rems(11. / 16.))
                            .text_color(theme.foreground)
                            .child(line)
                            .when_some(failure, |el, failure| {
                                el.child(div().truncate().text_color(theme.danger).child(failure))
                            }),
                    )
                    .child(
                        div().flex_shrink_0().child(
                            Button::new(gpui::SharedString::from(format!(
                                "switcher-close-orphan:{pane_id}"
                            )))
                            .label(t(L10nKey::Close))
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(
                                move |this, _, _window, cx| {
                                    this.close_orphan_pane(pane_id, cx);
                                },
                            )),
                        ),
                    ),
            );
        }
        v_flex()
            .gap(px(4.))
            .mx(px(4.))
            .mt(px(6.))
            .mb(px(2.))
            .px(px(10.))
            .py(px(8.))
            .rounded(crate::ui::rounding::ROW_RADIUS)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(gpui::rems(11. / 16.))
                    .text_color(theme.muted_foreground)
                    .child(t(L10nKey::SwitcherOrphanPanes)),
            )
            .child(list)
    }

    /// The parked group's notice (#485): no retry button — nothing it could
    /// try would succeed — just the way back (a fresh profile rediscovers
    /// the session) and the two honest actions.
    fn render_parked_notice(&self, group: &Group, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let forget_key = group.key.clone();
        let dismiss_key = group.key.clone();
        let dismiss_target = group.target.clone();
        let entries: Vec<WorkspaceId> = group.rows.iter().map(|r| r.id).collect();
        v_flex()
            .gap(px(4.))
            .px(px(12.))
            .py(px(8.))
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(gpui::rems(11. / 16.))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "{} — {}",
                        group.label,
                        t(L10nKey::RemoteRouteParkedHint)
                    )),
            )
            .child(
                h_flex()
                    .gap(px(4.))
                    .child(
                        Button::new(gpui::SharedString::from(format!(
                            "switcher-forget:{}",
                            group.key
                        )))
                        .label(t(L10nKey::RemoteActionRemoveEntry))
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(
                            move |this, _, _window, cx| {
                                // Forget, never delete: nothing is sent to the
                                // machine, so the remote sessions keep running
                                // (#485). Adopt rows name no local entry and are
                                // skipped by the store.
                                for id in &entries {
                                    crate::ui::windows::forget_workspace(cx, *id);
                                }
                                this.remote_host_errors.remove(&forget_key);
                                this.parked_dismissed.remove(&forget_key);
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        Button::new(gpui::SharedString::from(format!(
                            "switcher-parked-dismiss:{}",
                            group.key
                        )))
                        .label(t(L10nKey::Dismiss))
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(
                            move |this, _, _window, cx| {
                                this.parked_dismissed.insert(dismiss_key.clone());
                                // A parked group can still carry the deterministic
                                // failure that predated the parking — retire it
                                // too, or it pops right back as the error block.
                                this.remote_host_errors.remove(&dismiss_key);
                                if let Some(ConnectFlow::Failed { choice, .. }) = &this.connect
                                    && Some(&choice.target) == dismiss_target.as_ref()
                                {
                                    this.connect = None;
                                }
                                cx.notify();
                            },
                        )),
                    ),
            )
    }

    fn render_install_progress(
        &self,
        label: &str,
        phase: InstallPhase,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let caption = crate::ui::remote_workspace::install_phase_caption(phase);

        v_flex()
            .gap(px(6.))
            .px(px(12.))
            .py(px(8.))
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(gpui::rems(11. / 16.))
                    .text_color(theme.muted_foreground)
                    .child(format!("{label} — {caption}")),
            )
            .child(crate::ui::remote_workspace::install_progress_bar(phase, cx))
    }
    fn render_row(
        &self,
        group: &Group,
        row: &Row,
        picked: bool,
        nav_at: Option<usize>,
        show_machine: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(sw) = self.switcher.as_ref()
            && let Some((id, input)) = sw.renaming.as_ref()
            && *id == row.id
        {
            return h_flex()
                .id(("switcher-rename", row.id.element_key() as usize))
                .items_center()
                .h(px(WS_ROW_H))
                .px(px(ROW_PAD))
                .rounded(px(LIST_RADIUS))
                .bg(hover_fill(cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Input::new(input).appearance(false).xsmall())
                .into_any_element();
        }

        let theme = cx.theme();
        let (fg, muted, warn) = (theme.foreground, theme.muted_foreground, theme.warning);
        // The picked row is a neutral step of the popover's own ladder, not
        // a solid accent slab: it is where the cursor is, not an action, and
        // a saturated block with white text was the loudest thing on screen.
        // The row keeps its own inks, so a warning still reads as one.
        let sf = rungs(cx);
        let (hover, picked_bg) = (gpui::rgb(sf.hover), gpui::rgb(sf.selected));
        let rref = RowRef::of(group, row);
        let click_ref = rref.clone();
        let menu_ref = rref.clone();
        let ctx_ref = rref.clone();
        let gref = GroupRef::of(group);
        let menu_host = gref.clone();
        let ctx_host = gref;
        let app = cx.entity().downgrade();
        let app2 = app.clone();
        let key = row.id.element_key() as usize;
        let holding = self.switcher.as_ref().is_some_and(|sw| sw.hold.is_some());
        // A machine with no link cannot show this row's panes right now; the
        // row stays, muted, and opening it is what asks for the connection.
        let unlit = group.target.is_some() && matches!(group.link, Link::Offline | Link::Failed);

        // "Open" is what this workspace would say either way, and it is the
        // wrong word for one another client is driving.
        let badge = if row.preempted {
            Some((t(L10nKey::SwitcherStatusTakenOver), false))
        } else if row.current {
            Some((t(L10nKey::SwitcherThisWindow), true))
        } else if row.open {
            Some((t(L10nKey::SwitcherOpen), false))
        } else if group.target.is_some() && group.link == Link::Offline {
            // Said in words where the other states go, not as a dot: the
            // normal case carries nothing extra, so the exception is the
            // only thing that speaks.
            Some((t(L10nKey::SwitcherOffline), false))
        } else {
            None
        };

        // Two lines rather than one: the left column is only LEFT_W wide, and a
        // workspace name plus path plus badge plus timestamp on one row pushes
        // the trailing pieces straight out over the divider. The second line
        // leads with the machine the workspace lives on — the flat list's only
        // grouping — except in scope, where every row is on that machine. A
        // reachable workspace carries nothing extra; an offline one says so in
        // the state column and dims its disc. Only the states that want
        // attention — a connect in flight, a failure, a machine another client
        // took over — keep a dot on the disc.
        let state_dot = match group.link {
            Link::Connected if group.preempted => Some(warn),
            Link::Connecting | Link::Reconnecting { .. } => Some(warn),
            Link::Failed => Some(theme.danger),
            Link::Local | Link::Connected | Link::Offline => None,
        };
        let meta = row_meta(group, row, show_machine);
        let row_fill = if picked {
            picked_bg
        } else {
            gpui::rgb(sf.base)
        };
        let initial: String = row
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "~".to_string());
        let avatar = div()
            .relative()
            .flex_shrink_0()
            .size(px(WS_AVATAR))
            .child(
                div()
                    .size(px(WS_AVATAR))
                    .when(unlit, |d| d.opacity(0.5))
                    .rounded_full()
                    .bg(theme.secondary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(gpui::rems(11. / 16.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(fg.opacity(0.6))
                    .child(initial),
            )
            .children(state_dot.map(|state_dot| {
                // The ring is the row's own fill, so the dot reads as cut
                // out of the disc whichever state the row is in.
                div()
                    .absolute()
                    .right(px(-3.))
                    .bottom(px(-3.))
                    .size(px(WS_DOT + 4.))
                    .rounded_full()
                    .border_2()
                    .border_color(row_fill)
                    .when(!picked, |d| {
                        d.group_hover("switcher-row", move |d| d.border_color(hover))
                    })
                    .bg(state_dot)
            }));

        let line = h_flex()
            .id(("switcher-row", key))
            .group("switcher-row")
            .items_center()
            .gap(px(10.))
            .h(px(WS_ROW_H))
            .flex_shrink_0()
            .px(px(ROW_PAD))
            .rounded(px(LIST_RADIUS))
            .overflow_hidden()
            .cursor_pointer()
            .when(picked, |r| r.bg(picked_bg))
            .anchor_scroll(self.switcher_anchor(Column::Left, picked))
            .hover(move |r| r.bg(if picked { picked_bg } else { hover }))
            .child(avatar)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(gpui::rems(13. / 16.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(match unlit {
                                        true => muted,
                                        false => fg,
                                    })
                                    .child(row.name.clone()),
                            )
                            // The workspace's number — what Go to Workspace N
                            // and the Workspaces menu call it — so the slot a
                            // shortcut reaches can be read off the list that
                            // does not sort by it (#760).
                            .children(row.slot.map(|slot| {
                                div()
                                    .flex_shrink_0()
                                    .text_size(gpui::rems(11. / 16.))
                                    .text_color(muted)
                                    .child((slot + 1).to_string())
                            })),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .min_w_0()
                            .text_size(gpui::rems(11.5 / 16.))
                            .text_color(muted)
                            // The path gives way first and the timestamp
                            // never does: with both in one truncating string
                            // the row ended in `~/repo/025/tty59 · …` every
                            // time, a dangling dot where the time had been.
                            .child(div().min_w_0().truncate().child(meta.join("  ·  ")))
                            .when(!row.when.is_empty(), |line| {
                                line.child(
                                    div()
                                        .flex_shrink_0()
                                        .whitespace_nowrap()
                                        .child(format!("  ·  {}", row.when)),
                                )
                            }),
                    ),
            )
            // Tab count over state, both words rather than chips: a filled
            // pill reads as a button, and these are states. Only "taken over"
            // keeps a colour — it is the one that warns.
            .child(
                v_flex()
                    .flex_shrink_0()
                    .items_end()
                    .gap(px(2.))
                    .text_size(gpui::rems(11.5 / 16.))
                    .text_color(muted)
                    // In words, the way the tab column's header says it: a
                    // bare `1` here sat a few pixels from the slot number after
                    // the name, and the two read as the same thing twice.
                    .when(!row.tabs.is_empty(), |c| c.child(tab_count(row.tabs.len())))
                    .children(badge.map(|(label, _here)| {
                        div()
                            .text_size(gpui::rems(11. / 16.))
                            .text_color(match row.preempted {
                                true => warn,
                                false => muted,
                            })
                            .child(label)
                    })),
            )
            .child(
                div()
                    .invisible()
                    .flex_shrink_0()
                    .group_hover("switcher-row", |x| x.visible())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        crate::ui::tab_strip::hit_target(
                            Button::new(("switcher-row-more", key))
                                .icon(IconName::Ellipsis)
                                .ghost()
                                .xsmall(),
                        )
                        .tooltip(t(L10nKey::TabTooltipMore))
                        .dropdown_menu(move |menu, _window, _cx| {
                            row_menu(menu, &menu_ref, &menu_host, app.clone())
                        }),
                    ),
            )
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                // One click aims the tab column at this workspace; opening it
                // takes a second click, Enter, or the platform modifier.
                if let Some(at) = nav_at {
                    this.switcher_point_at(at, cx);
                }
                let modified = ev.modifiers().secondary();
                if ev.click_count() >= 2 || modified {
                    this.switcher_open(click_ref.clone(), modified, window, cx);
                }
            }));

        // A held Ctrl makes every click a right click on macOS. Drop the menu
        // and take the right-button press as the pick instead, or clicking
        // during the gesture does nothing at all.
        match holding {
            true => line
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        if let Some(at) = nav_at {
                            this.switcher_point_at(at, cx);
                        }
                    }),
                )
                .into_any_element(),
            false => line
                .context_menu(move |menu, _window, _cx| {
                    row_menu(menu, &ctx_ref, &ctx_host, app2.clone())
                })
                .into_any_element(),
        }
    }

    /// The "New Workspace" form: a name, and one combobox row that folds
    /// however many machines are configured into a searchable dropdown.
    fn render_create_card(
        &self,
        form: &CreateForm,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let border = theme.border;
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        let sf = rungs(cx);
        let (hover, picked_bg) = (gpui::rgb(sf.hover), gpui::rgb(sf.selected));
        let viewport = window.viewport_size();
        let card_w = FORM_W
            .min(viewport.width.as_f32() - 2. * CARD_MARGIN)
            .max(320.);

        let chosen_label = form
            .chosen
            .as_ref()
            .map(|h| h.label.clone())
            .unwrap_or_else(|| t(L10nKey::SwitcherThisComputer).to_string());
        let chosen_local = form.chosen.is_none();
        let chosen_dot: Option<gpui::Hsla> = form.chosen.as_ref().map(|h| {
            match remote_connect::HostLinks::get(cx, h.target.host_id()).is_some() {
                true => gpui::rgb(crate::ui::tab_strip::LIVE_DOT).into(),
                false => gpui::rgb(crate::ui::tab_strip::UNKNOWN_DOT).into(),
            }
        });

        let header = h_flex()
            .items_center()
            .gap(px(6.))
            .h(px(SEARCH_H))
            .px(px(8.))
            .border_b_1()
            .border_color(border)
            .child(
                crate::ui::tab_strip::hit_target(
                    Button::new("switcher-form-back")
                        .icon(IconName::ChevronLeft)
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switcher_back_to_list(window, cx);
                        })),
                )
                .tooltip(t(L10nKey::SwitcherFormBack)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(gpui::rems(13. / 16.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(fg)
                    .child(t(L10nKey::AppMenuNewWorkspace)),
            )
            .child(div().pr(px(6.)).child(keycap("esc", cx)));

        let label_col = |text: &'static str| {
            div()
                .w(px(52.))
                .flex_shrink_0()
                .text_size(gpui::rems(13. / 16.))
                .text_color(muted)
                .child(text)
        };
        // The other cards' well — no outline, the faint fill is the shape —
        // so this form and the worktree prompt draw a field the same way.
        let well = crate::ui::dialog::well_fill(cx);
        let field = |inner: gpui::Div| {
            inner
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(6.))
                .px(px(8.))
                .h(px(crate::ui::dialog::FIELD_H))
                .rounded(crate::ui::rounding::ROW_RADIUS)
                .bg(well)
        };

        let name_row = h_flex()
            .items_center()
            .gap(px(8.))
            .child(label_col(t(L10nKey::SwitcherFormName)))
            .child(field(h_flex()).child(Input::new(&form.name).appearance(false).small()));

        // The trigger: the picked host while folded, the filter box while
        // open. One row either way, so the form does not jump.
        let host_glyph = |local: bool, dot: Option<gpui::Hsla>| match dot {
            Some(color) => div()
                .flex_shrink_0()
                .size(px(6.))
                .rounded_full()
                .bg(color)
                .into_any_element(),
            None if local => gpui::svg()
                .path("icons/machine-local.svg")
                .flex_shrink_0()
                .size(px(12.))
                .text_color(muted)
                .into_any_element(),
            None => gpui::svg()
                .path("icons/machine-remote.svg")
                .flex_shrink_0()
                .size(px(12.))
                .text_color(muted)
                .into_any_element(),
        };
        let trigger: AnyElement = match form.open {
            false => field(h_flex())
                .id("switcher-form-host")
                .cursor_pointer()
                .hover(move |r| r.bg(picked_bg))
                .child(host_glyph(chosen_local, chosen_dot))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(gpui::rems(13. / 16.))
                        .text_color(fg)
                        .child(chosen_label),
                )
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(px(ICON))
                        .text_color(muted),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.switcher_form_open_hosts(window, cx);
                }))
                .into_any_element(),
            true => field(h_flex())
                .child(Icon::new(IconName::Search).size(px(14.)).text_color(muted))
                .child(Input::new(&form.host).appearance(false).small())
                .into_any_element(),
        };

        let mut host_block = v_flex().flex_1().min_w_0().gap(px(4.)).child(trigger);
        if form.open {
            let items = self.form_items(form, cx);
            let mut list = v_flex().gap(px(1.)).p(px(4.));
            for (i, item) in items.iter().enumerate() {
                let picked = i == form.sel;
                let base = h_flex()
                    .id(("switcher-form-item", i))
                    .items_center()
                    .gap(px(8.))
                    .min_h(px(ROW_H))
                    .py(px(4.))
                    .px(px(ROW_PAD))
                    .rounded(crate::ui::rounding::ROW_RADIUS)
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(picked, |r| r.bg(picked_bg))
                    .hover(move |r| r.bg(if picked { picked_bg } else { hover }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.switcher_form_pick(i, window, cx);
                    }));
                let line = match item {
                    HostItem::Local => base
                        .child(host_glyph(true, None))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(gpui::rems(13. / 16.))
                                .text_color(fg)
                                .child(t(L10nKey::SwitcherThisComputer)),
                        )
                        .into_any_element(),
                    HostItem::Host(host) => {
                        let lit =
                            remote_connect::HostLinks::get(cx, host.target.host_id()).is_some();
                        base.child(host_glyph(
                            false,
                            Some(match lit {
                                true => gpui::rgb(crate::ui::tab_strip::LIVE_DOT).into(),
                                false => gpui::rgb(crate::ui::tab_strip::UNKNOWN_DOT).into(),
                            }),
                        ))
                        .child(
                            div()
                                .flex_shrink_0()
                                .max_w(px(180.))
                                .truncate()
                                .text_size(gpui::rems(13. / 16.))
                                .text_color(fg)
                                .child(host.label.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(gpui::rems(11. / 16.))
                                .text_color(muted)
                                .child(host.detail.clone()),
                        )
                        .into_any_element()
                    }
                    HostItem::AddHost => base
                        .border_t_1()
                        .border_color(border)
                        .rounded_none()
                        .child(Icon::new(IconName::Plus).size(px(14.)).text_color(muted))
                        .child(
                            div()
                                .text_size(gpui::rems(13. / 16.))
                                .text_color(muted)
                                .child(t(L10nKey::AddSshHost)),
                        )
                        .into_any_element(),
                };
                list = list.child(line);
            }
            host_block = host_block.child(
                div()
                    .rounded(crate::ui::rounding::ROW_RADIUS)
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .id("switcher-form-hosts")
                            .max_h(px(FORM_LIST_H))
                            .overflow_y_scroll()
                            .child(list),
                    ),
            );
        }

        let host_row = h_flex()
            .items_start()
            .gap(px(8.))
            .child(
                label_col(t(L10nKey::SwitcherFormHost))
                    .h(px(crate::ui::dialog::FIELD_H))
                    .flex()
                    .items_center(),
            )
            .child(host_block);

        // The same button row as the other cards. While the dropdown is open
        // Enter picks a host rather than creating, so the row says so.
        let footer = crate::ui::dialog::footer(cx)
            .when(form.open, |row| {
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .pl(px(crate::ui::dialog::INSET - 6.))
                        .text_size(gpui::rems(11. / 16.))
                        .text_color(muted)
                        .child(t(L10nKey::SwitcherFormPickHint)),
                )
            })
            .child(crate::ui::dialog::button(
                "switcher-form-cancel",
                t(L10nKey::Cancel),
                crate::ui::dialog::Tone::Secondary,
                true,
                sf,
                cx,
                cx.listener(|this, _, window, cx| this.switcher_back_to_list(window, cx)),
            ))
            .child(crate::ui::dialog::button(
                "switcher-form-create",
                t(L10nKey::SwitcherFormCreate),
                crate::ui::dialog::Tone::Primary,
                form.name(cx).is_some(),
                sf,
                cx,
                cx.listener(|this, _, window, cx| this.switcher_form_create(window, cx)),
            ));

        v_flex()
            .w(px(card_w))
            .map(|panel| crate::ui::theme::floating_surface(panel, cx))
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .child(header)
            .child(
                v_flex()
                    .px(px(crate::ui::dialog::INSET))
                    .pt(px(16.))
                    .pb(px(18.))
                    .gap(px(10.))
                    .child(name_row)
                    .child(host_row),
            )
            .child(footer)
            .into_any_element()
    }

    /// The right-hand column: the tabs of whichever workspace the left column
    /// is sitting on.
    fn render_tabs(
        &self,
        layout: &Layout,
        sel: usize,
        column: Column,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        let note = |text: String| {
            div()
                .px(px(ROW_PAD))
                .py(px(14.))
                .text_size(gpui::rems(13. / 16.))
                .text_color(muted)
                .child(text)
                .into_any_element()
        };

        let Some(row) = layout.subject_row(sel) else {
            return note(t(L10nKey::SwitcherPickAWorkspace).to_string());
        };
        if row.tabs.is_empty() {
            // A remote workspace this client has never opened has no tab
            // mirror to read — "no tabs" would be a claim nobody checked.
            let unseen = row.adopt.is_some() || (row.remote_id.is_some() && !row.open);
            return note(match unseen {
                true => t(L10nKey::SwitcherTabsAfterOpening).to_string(),
                false => t(L10nKey::SwitcherNoTabs).to_string(),
            });
        }

        let query = self
            .switcher
            .as_ref()
            .map(|sw| sw.text(cx))
            .unwrap_or_default();
        let hits = visible_tabs(row, &query);
        if hits.is_empty() {
            return note(t(L10nKey::SwitcherNoTabMatch).to_string());
        }

        let sf = rungs(cx);
        let (hover, picked_bg) = (gpui::rgb(sf.hover), gpui::rgb(sf.selected));
        let right_sel = self.switcher.as_ref().map(|sw| sw.right_sel).unwrap_or(0);
        let holding = self.switcher.as_ref().is_some_and(|sw| sw.hold.is_some());
        let ws = row.id;

        let live: gpui::Hsla = gpui::rgb(crate::ui::tab_strip::LIVE_DOT).into();
        let mut list = v_flex().gap(px(1.)).child(
            h_flex()
                .items_center()
                .gap(px(6.))
                .h(px(HOST_H))
                .flex_shrink_0()
                .px(px(ROW_PAD))
                .text_size(gpui::rems(11.5 / 16.))
                .text_color(muted)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(row.name.clone()),
                )
                .child(div().flex_shrink_0().child(tab_count(row.tabs.len()))),
        );

        for (nth, i) in hits.iter().enumerate() {
            let tab = &row.tabs[*i];
            let picked = nth == right_sel && column == Column::Right;
            let (id, index) = (tab.id, tab.index);
            // The second line is what tells two tabs on the same repo apart —
            // the branch, then the diff counts, the same words the tab sidebar
            // uses, all secondary. A named tab off any repo shows its path.
            let meta = match tab.git.as_ref() {
                Some(g) => {
                    let mut parts = vec![g.branch.clone()];
                    let counts = [
                        (g.added > 0).then(|| format!("+{}", g.added)),
                        (g.removed > 0).then(|| format!("−{}", g.removed)),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                    if !counts.is_empty() {
                        parts.push(counts.join(" "));
                    }
                    Some(parts.join("  ·  "))
                }
                None if tab.named && !tab.path.is_empty() => Some(tab.path.clone()),
                None => None,
            };
            let running = tab.status == Some(crate::core::cli_agent::AgentStatus::Working);
            let dot_on = self.working_dot_on;

            list = list.child(
                h_flex()
                    .id(("switcher-tab", index))
                    .items_center()
                    .gap(px(10.))
                    .h(px(TAB_ROW_H))
                    .flex_shrink_0()
                    .px(px(ROW_PAD))
                    .rounded(px(LIST_RADIUS))
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(picked, |r| r.bg(picked_bg))
                    .anchor_scroll(self.switcher_anchor(Column::Right, picked))
                    .hover(move |r| r.bg(if picked { picked_bg } else { hover }))
                    .child(self.tab_avatar(
                        ("switcher-avatar", index),
                        tab.agent,
                        tab.status,
                        tab.unread,
                        tab.ssh,
                        TAB_AVATAR,
                        cx,
                    ))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(gpui::rems(13. / 16.))
                                    .when(tab.active, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                                    .text_color(fg)
                                    .child(tab.label.clone()),
                            )
                            .children(meta.map(|meta| {
                                div()
                                    .truncate()
                                    .text_size(gpui::rems(11.5 / 16.))
                                    .text_color(muted)
                                    .child(meta)
                            })),
                    )
                    .children(tab.waited.clone().map(|waited| {
                        div()
                            .flex_shrink_0()
                            .text_size(gpui::rems(11. / 16.))
                            .text_color(muted)
                            .child(waited)
                    }))
                    .when(tab.active, |r| {
                        r.child(
                            div()
                                .flex_shrink_0()
                                .text_size(gpui::rems(11. / 16.))
                                .text_color(muted)
                                .child(t(L10nKey::SwitcherActiveTab)),
                        )
                    })
                    // A working agent's dot blinks in step with the sidebar's,
                    // so a column of tabs says which ones are still going.
                    .when(running, |r| {
                        r.child(
                            div()
                                .flex_shrink_0()
                                .size(px(RUN_DOT))
                                .rounded_full()
                                .bg(live)
                                .when(!dot_on, |d| d.opacity(0.25)),
                        )
                    })
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        this.switcher_open_tab(ws, id, ev.modifiers().secondary(), window, cx)
                    }))
                    // Mid-gesture a click arrives as a right press on macOS. It
                    // aims the cursor; releasing Ctrl is what commits.
                    .when(holding, |line| {
                        line.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                                this.switcher_point_tab(nth, cx);
                            }),
                        )
                    }),
            );
        }
        list.into_any_element()
    }
}

/// Tab indices of `row` the search leaves visible. A workspace matched by its
/// own name keeps all of them — the search told you which workspace, not which
/// tab.
fn visible_tabs(row: &Row, query: &str) -> Vec<usize> {
    if query.is_empty()
        || row.name.to_lowercase().contains(query)
        || row.path.to_lowercase().contains(query)
    {
        return (0..row.tabs.len()).collect();
    }
    let hits: Vec<usize> = row
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.matches(query))
        .map(|(i, _)| i)
        .collect();
    match hits.is_empty() {
        // The host name matched, so the workspace is on screen with nothing of
        // its own to narrow by. Show the lot rather than an empty column.
        true => (0..row.tabs.len()).collect(),
        false => hits,
    }
}

impl Row {
    /// How closely the workspace's own name answers `query`, best first: the
    /// name itself, the start of it, anywhere in it, then not at all (the row
    /// is listed for its path or a tab). Every row ties on an empty query.
    fn name_match(&self, query: &str) -> u8 {
        if query.is_empty() {
            return 0;
        }
        let name = self.name.to_lowercase();
        match () {
            _ if name == query => 0,
            _ if name.starts_with(query) => 1,
            _ if name.contains(query) => 2,
            _ => 3,
        }
    }

    /// A workspace stays in the list when its own name or path matches, and
    /// also when any of its tabs does — searching "claude" should surface the
    /// workspaces running one.
    fn matches(&self, query: &str) -> bool {
        self.name.to_lowercase().contains(query)
            || self.path.to_lowercase().contains(query)
            || self.tabs.iter().any(|t| t.matches(query))
    }
}

impl TabRow {
    fn matches(&self, query: &str) -> bool {
        self.label.to_lowercase().contains(query) || self.path.to_lowercase().contains(query)
    }
}

/// Names a tab of a workspace this window does not own.
///
/// The two surfaces used to read different sources and had to be talked into
/// agreeing: a local tab was named by its live terminal's title, this one by
/// the tree's copy of it (`PaneRecord::osc_title`). They now go through the one
/// renderer, [`crate::ui::tab_strip::label_of`] — a local tab is turned into
/// the same [`TabView`](crate::ui::machine_mirror::TabView) this one already
/// is, so neither column can rank the evidence its own way.
fn tab_view_label(
    view: &crate::ui::machine_mirror::TabView,
    index: usize,
    home: Option<&std::path::Path>,
) -> String {
    crate::ui::tab_strip::label_of(view, index, home)
}

impl Group {
    fn merge(&mut self, remote: &[RemoteWorkspaceRow], now: u64) {
        if self.target.is_none() {
            return;
        }
        let known: HashSet<WorkspaceId> = self.rows.iter().filter_map(|r| r.remote_id).collect();
        for r in remote {
            if known.contains(&r.id) {
                continue;
            }
            self.rows.push(Row {
                id: r.id,
                name: r.name.clone(),
                path: String::new(),
                when: crate::ui::home::relative_time(now, r.last_active),
                last_active: r.last_active,
                live: Liveness::Stopped,
                open: false,
                current: false,
                preempted: false,
                adopt: Some(Box::new(r.clone())),
                remote_id: Some(r.id),
                slot: None,
                // A workspace this client has never adopted has no local id to
                // hang a machine-tree lookup on. The tab column says so.
                tabs: Vec::new(),
            });
        }
    }
}

#[derive(Clone)]
struct GroupRef {
    label: String,
    target: Option<RemoteTarget>,
    link: Link,
}

impl GroupRef {
    fn of(g: &Group) -> Self {
        Self {
            label: g.label.clone(),
            target: g.target.clone(),
            link: g.link,
        }
    }
}

#[derive(Clone)]
struct RowRef {
    id: WorkspaceId,
    live: bool,
    adopt: Option<(RemoteTarget, Box<RemoteWorkspaceRow>)>,
}

impl RowRef {
    fn of(group: &Group, row: &Row) -> Self {
        Self {
            id: row.id,
            live: row.live == Liveness::Alive,
            adopt: match (&group.target, &row.adopt) {
                (Some(t), Some(r)) => Some((t.clone(), r.clone())),
                _ => None,
            },
        }
    }
}

/// What a host row offering the form says, or `None` for a machine that has no
/// SSH host behind it at all — WSL and the local stdio server are configured
/// nowhere this form could edit.
///
/// Shared with the tab menu, which offers the same row for the connection a tab
/// is on (#438), so the two surfaces cannot drift on which machines are
/// editable or on what the row is called.
pub(crate) fn host_form_label(target: &RemoteTarget) -> Option<&'static str> {
    match target {
        RemoteTarget::Profile { .. } => Some(t(L10nKey::SwitcherEditHost)),
        RemoteTarget::Alias { .. } | RemoteTarget::Direct { .. } => {
            Some(t(L10nKey::SwitcherSaveAsHost))
        }
        RemoteTarget::Wsl { .. } | RemoteTarget::LocalStdio { .. } => None,
    }
}

fn row_menu(
    menu: gpui_component::menu::PopupMenu,
    row: &RowRef,
    host: &GroupRef,
    app: gpui::WeakEntity<Tty59App>,
) -> gpui_component::menu::PopupMenu {
    let (a1, a2, a3, a4) = (app.clone(), app.clone(), app.clone(), app.clone());
    let (id, adopt) = (row.id, row.adopt.is_some());
    let stoppable = row.live;
    // Every workspace verb addresses the row by its local id, and a remote
    // this client has never adopted has none yet. That greyed them all out at
    // once, which reads as broken rather than as not-yet. Say what would make
    // them work, the way the tab pane already says it for the same rows.
    let menu = match adopt {
        true => menu.item(PopupMenuItem::label(t(L10nKey::SwitcherOpenToManage))),
        false => menu
            .item(
                PopupMenuItem::new(t(L10nKey::SwitcherRename)).on_click(move |_, window, cx| {
                    let _ = a1.update(cx, |this, cx| this.switcher_rename(id, window, cx));
                }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::SwitcherOpenInNewWindow)).on_click(
                    move |_, window, cx| {
                        let _ = a2.update(cx, |this, cx| {
                            this.close_switcher(window, cx);
                            crate::ui::windows::open(cx, Some(id));
                        });
                    },
                ),
            )
            .separator()
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuStopWorkspace))
                    .disabled(!stoppable)
                    .on_click(move |_, window, cx| {
                        let _ = a3.update(cx, |this, cx| {
                            this.close_switcher(window, cx);
                            this.stop_workspace(id, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuDeleteWorkspace)).on_click(
                    move |_, window, cx| {
                        let _ = a4.update(cx, |this, cx| {
                            this.close_switcher(window, cx);
                            this.delete_workspace(id, window, cx);
                        });
                    },
                ),
            ),
    };
    host_menu(menu, host, app)
}

/// The machine verbs that used to live on the group headers, now appended
/// under the machine's own name to every one of its rows. Local rows carry
/// none — the footer's New Workspace covers this computer.
fn host_menu(
    menu: gpui_component::menu::PopupMenu,
    host: &GroupRef,
    app: gpui::WeakEntity<Tty59App>,
) -> gpui_component::menu::PopupMenu {
    let Some(target) = host.target.clone() else {
        return menu;
    };
    let (a1, a2, a3, a4, a5) = (app.clone(), app.clone(), app.clone(), app.clone(), app);
    let menu = menu
        .separator()
        .item(PopupMenuItem::label(host.label.clone()));
    let engaged = link_is_engaged(host.link);
    let create_choice = HostChoice {
        target: target.clone(),
        label: host.label.clone(),
        detail: String::new(),
    };
    let menu = menu.item(
        PopupMenuItem::new(t(L10nKey::AppMenuNewWorkspace)).on_click(move |_, window, cx| {
            let _ = a1.update(cx, |this, cx| {
                this.switcher_to_form(Some(create_choice.clone()), window, cx);
            });
        }),
    );
    // The picker is already up — this menu belongs to one of its rows — so
    // the machine is one click away, its `@token` (if any) included.
    let scope = MachineScope::Machine(target.to_string());
    let menu = menu.item(
        PopupMenuItem::new(t(L10nKey::RemoteOpenWorkspacesOnMachine)).on_click(
            move |_, window, cx| {
                let _ = a5.update(cx, |this, cx| {
                    this.select_switcher_machine(scope.clone(), window, cx);
                });
            },
        ),
    );
    // The host as it is configured, reachable from the one place it is on
    // screen. A machine dialled by address has no profile yet, so the same
    // row offers to make one instead (#438).
    let menu = match host_form_label(&target) {
        Some(label) => {
            let for_edit = target.clone();
            menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let for_edit = for_edit.clone();
                let _ = a4.update(cx, |this, cx| {
                    this.close_switcher(window, cx);
                    this.edit_ssh_host_of_target(&for_edit, window, cx);
                });
            }))
        }
        None => menu,
    };
    let menu = match engaged {
        true => {
            let for_disconnect = target.clone();
            menu.item(PopupMenuItem::new(t(L10nKey::SwitcherDisconnect)).on_click(
                move |_, _window, cx| {
                    let _ = a2.update(cx, |this, cx| this.switcher_disconnect(&for_disconnect, cx));
                },
            ))
        }
        false => {
            let connect_choice = HostChoice {
                target: target.clone(),
                label: host.label.clone(),
                detail: String::new(),
            };
            menu.item(
                PopupMenuItem::new(t(L10nKey::Connect))
                    .disabled(matches!(host.link, Link::Connecting))
                    .on_click(move |_, _window, cx| {
                        let _ = a2.update(cx, |this, cx| {
                            this.connect_to_host(connect_choice.clone(), cx)
                        });
                    }),
            )
        }
    };
    if !target.hosts_our_server() {
        return menu;
    }
    let (label, for_restart) = (host.label.clone(), target);
    menu.item(
        PopupMenuItem::new(t(L10nKey::AppMenuRestartServer)).on_click(move |_, window, cx| {
            let _ = a3.update(cx, |this, cx| {
                this.confirm_restart_remote_server(for_restart.clone(), label.clone(), window, cx);
            });
        }),
    )
}

fn rungs(cx: &App) -> crate::ui::presets::Surface {
    cx.global::<crate::ui::presets::Surfaces>().popover
}

fn hover_fill(cx: &App) -> gpui::Rgba {
    gpui::rgb(rungs(cx).hover)
}

/// What the panel wants to do with a keystroke, before any of the state that
/// only the panel knows (which column has the cursor, whether the search box
/// has text) gets a say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Close,
    Step(bool),
    /// Alt+←/→: step the machine rail by one chip. Signed, so `+1` is a
    /// step to the right.
    Machine(isize),
    ToColumn(Column),
    Tab(bool),
    Confirm(bool),
    Pass,
}

fn key_intent(key: &str, mods: gpui::Modifiers) -> Key {
    if key == "escape" {
        return Key::Close;
    }
    // Alt+←/→ walks the machine rail, taken ahead of the Alt pass-through
    // below — which is the price: those chords were the search box's word
    // jumps. Alt alone, or not at all: Fn or any other modifier riding along
    // is somebody else's gesture and keeps passing through untouched.
    if mods.alt
        && !mods.function
        && !mods.control
        && !mods.secondary()
        && !mods.shift
        && matches!(key, "left" | "right")
    {
        return Key::Machine(if key == "left" { -1 } else { 1 });
    }
    // Alt and Fn belong to whoever else wants them, whatever the key.
    if mods.alt || mods.function {
        return Key::Pass;
    }
    // Careful: `control` cannot be lumped in with "some modifier is down", as
    // it *is* the secondary modifier off macOS — Ctrl+Enter has to reach the
    // new-window branch there, not fall through as a stray chord.
    let bare = !mods.control && !mods.secondary();
    match key {
        "up" | "down" if bare => Key::Step(key == "down"),
        "left" if bare => Key::ToColumn(Column::Left),
        "right" if bare => Key::ToColumn(Column::Right),
        // Tab keeps working with Ctrl held — that is the Ctrl+Tab gesture still
        // in progress. Off macOS that chord arrives as the NextTab action
        // instead, which lands in the same place.
        "tab" if !mods.secondary() => Key::Tab(!mods.shift),
        "enter" if bare || mods.secondary() => Key::Confirm(mods.secondary()),
        _ => Key::Pass,
    }
}

/// Wraps a cursor around a list of `n` items.
fn step(at: usize, n: usize, forward: bool) -> usize {
    match forward {
        true => (at + 1) % n,
        false => (at + n - 1) % n,
    }
}

use crate::ui::dialog::keycap;

/// "1 tab", "3 tabs" — how both columns count a workspace's tabs.
fn tab_count(n: usize) -> String {
    match n {
        1 => t(L10nKey::SwitcherTabCountOne).to_string(),
        n => t_fmt(L10nKey::SwitcherTabCount, &[("n", &n.to_string())]),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The host menu's Disconnect runs inside an update of this window's own
    /// app, and the disconnect reads that app to find the panes to let go of.
    /// Done in place, that read panicked on the double lease — inside a
    /// mouse-up handler, where a panic aborts the process and takes every
    /// window down with it.
    #[gpui::test]
    fn disconnecting_the_machine_this_window_is_on_does_not_crash(cx: &mut gpui::TestAppContext) {
        use crate::core::session::{RemoteRef, WindowView, WindowViews};
        use crate::ui::windows::WindowRegistry;

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        let target = RemoteTarget::Alias {
            alias: "build-box".into(),
        };
        let handle = vcx.window_handle();
        let workspace = vcx.update(|_, cx| app.read(cx).workspace);
        vcx.update(|_, cx| {
            WindowRegistry::init(cx);
            WindowRegistry::register(cx, workspace, handle, app.downgrade());
            let mut entry =
                WindowView::on_remote(RemoteRef::new(target.clone(), WorkspaceId::new()));
            entry.id = workspace;
            entry.open = true;
            WorkspaceStore::install_for_test(
                cx,
                WindowViews {
                    views: vec![entry],
                    active: None,
                },
            );
        });

        app.update(&mut vcx, |this, cx| this.switcher_disconnect(&target, cx));
        vcx.background_executor.run_until_parked();

        assert_eq!(
            vcx.update(|_, cx| RemoteLinks::status_of(cx, workspace)),
            Some(crate::ui::remote_workspace::RemoteStatus::Disconnected),
            "the disconnect still lands once the menu's update is over"
        );
    }

    /// The workspace row's shape: a `flex_1` column, and in it a truncating
    /// name sized by its own text beside the slot number. gpui measures that
    /// name once against the column's zero basis, and used to answer the
    /// content-size measure after it from the same cache — so every name in
    /// the list read "…" with the whole column free beside it.
    #[gpui::test]
    fn a_truncating_name_beside_its_number_takes_the_room_it_has(cx: &mut gpui::TestAppContext) {
        use gpui::{AvailableSpace, InteractiveElement as _, point, size};

        let vcx = cx.add_empty_window();
        vcx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(LEFT_W)),
                AvailableSpace::Definite(px(WS_ROW_H)),
            ),
            |_, _| {
                h_flex()
                    .w(px(LEFT_W))
                    .gap(px(10.))
                    .child(div().flex_shrink_0().size(px(WS_AVATAR)))
                    .child(
                        v_flex().flex_1().min_w_0().child(
                            h_flex()
                                .gap(px(6.))
                                .min_w_0()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .debug_selector(|| "name".into())
                                        .child("pale-seal"),
                                )
                                .child(div().flex_shrink_0().child("1")),
                        ),
                    )
            },
        );
        let name = vcx.debug_bounds("name").expect("the name was drawn");
        // Wider than an ellipsis by a long way: the whole name fits.
        assert!(
            name.size.width > px(40.),
            "the name collapsed to {:?}",
            name.size.width
        );
    }

    /// #485 on the path #645 did not cover. A machine the switcher knows only
    /// from a listing snapshot has no store entry to name it, so its group
    /// used to be labelled by the target's own spelling — and a `Profile`
    /// target spells itself as its config UUID. Delete the profile and every
    /// banner under the list announced a raw UUID.
    #[gpui::test]
    fn a_pending_machine_whose_profile_is_gone_is_not_named_by_its_uuid(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::core::session::RemoteTarget;

        let (app, _vcx) = crate::ui::app::test_window::harness(cx);

        // A profile id that is in no config: the state left behind when the
        // profile a machine was reached through is deleted.
        let id = uuid::Uuid::new_v4();
        let target = RemoteTarget::Profile { id };

        app.update(cx, |app, _| {
            app.host_snapshots.insert(
                target.host_id(),
                super::HostSnapshot {
                    target: target.clone(),
                    rows: Vec::new(),
                },
            );
        });

        app.update(cx, |app, cx| {
            let groups = app.switcher_groups(cx);
            let group = groups
                .iter()
                .find(|g| g.target.as_ref() == Some(&target))
                .expect("the snapshot puts its machine in the list");
            assert!(
                !group.label.contains(&id.to_string()),
                "the switcher named a machine by its raw profile UUID: {}",
                group.label
            );
            assert_eq!(
                group.label,
                t(L10nKey::RemoteProfileGone),
                "a gone profile is named here the way a pane's route names it"
            );
        });
    }

    /// A wrong hostname or a stale password used to be fixable only by
    /// finding the same machine again in Settings (#438). The machine is on
    /// screen here, so its host row is too — worded for what the row can
    /// actually do, since a machine reached by address has no profile to open.
    #[test]
    fn every_ssh_machine_offers_its_host_form_and_nothing_else_does() {
        crate::ui::i18n::set_locale("en");
        assert_eq!(
            host_form_label(&RemoteTarget::Profile {
                id: uuid::Uuid::new_v4()
            }),
            Some("Edit Host…")
        );
        assert_eq!(
            host_form_label(&RemoteTarget::Alias {
                alias: "prod".into()
            }),
            Some("Save as SSH Host…"),
            "an alias lives in ~/.ssh/config, which this form does not write"
        );
        assert_eq!(
            host_form_label(&RemoteTarget::direct("me", "10.0.0.5", 22)),
            Some("Save as SSH Host…")
        );
        assert_eq!(
            host_form_label(&RemoteTarget::Wsl {
                distro: "Ubuntu".into()
            }),
            None,
            "a WSL distro is configured nowhere this form could reach"
        );
        assert_eq!(
            host_form_label(&RemoteTarget::LocalStdio {
                program: "tty59-server".into(),
                args: Vec::new()
            }),
            None
        );
    }

    /// The switcher used to read the `HostLinks` table and nothing else, which
    /// cannot tell "being retried right now" from "never heard of". The group
    /// then collapsed mid-reconnect as if the machine had no workspaces.
    #[test]
    fn a_machine_being_retried_does_not_read_as_one_nobody_ever_connected_to() {
        let target = RemoteTarget::direct("me", "build-box", 22);
        let retrying = MachineStatus::Reconnecting {
            attempt: 2,
            last_error: Some("connection refused".into()),
        };

        assert_eq!(
            link_from(None, &target, Some(&retrying), false),
            Link::Reconnecting { attempt: 2 }
        );
        assert_eq!(link_from(None, &target, None, false), Link::Offline);
        assert_eq!(
            link_from(None, &target, Some(&MachineStatus::Attached), false),
            Link::Connected
        );
        assert_eq!(
            link_from(
                None,
                &target,
                Some(&MachineStatus::Failed("no route to host".into())),
                false
            ),
            Link::Failed,
            "a route the supervisor could not build has to reach the panel too"
        );
        // A link this window knows nothing about is still a link.
        assert_eq!(link_from(None, &target, None, true), Link::Connected);
    }

    #[test]
    fn this_windows_own_attempt_is_only_ever_about_its_own_machine() {
        let build = RemoteTarget::direct("me", "build-box", 22);
        let gpu = RemoteTarget::direct("me", "gpu-lab", 22);
        let flow = ConnectFlow::Connecting {
            choice: HostChoice {
                target: gpu,
                label: "gpu-lab".into(),
                detail: String::new(),
            },
        };

        assert_eq!(
            link_from(Some(&flow), &build, Some(&MachineStatus::Attached), true),
            Link::Connected,
            "a connect to the GPU box says nothing about the build box"
        );
    }

    fn tab(label: &str, path: &str) -> TabRow {
        TabRow {
            id: TabId::new(),
            index: 0,
            label: label.to_string(),
            path: path.to_string(),
            named: false,
            agent: None,
            status: None,
            unread: 0,
            waited: None,
            ssh: None,
            active: false,
            git: None,
        }
    }

    pub(super) fn row(name: &str, tabs: Vec<TabRow>) -> Row {
        Row {
            id: WorkspaceId::new(),
            name: name.to_string(),
            path: "~/code".to_string(),
            when: String::new(),
            last_active: 0,
            live: Liveness::Alive,
            open: true,
            current: false,
            preempted: false,
            adopt: None,
            remote_id: None,
            slot: None,
            tabs,
        }
    }

    fn aged(mut r: Row, last_active: u64) -> Row {
        r.last_active = last_active;
        r
    }

    pub(super) fn group(rows: Vec<Row>) -> Group {
        Group {
            key: String::new(),
            label: "This Computer".to_string(),
            endpoint: String::new(),
            target: None,
            link: Link::Local,
            error: None,
            installing: None,
            preempted: false,
            parked: false,
            rows,
        }
    }

    pub(super) fn named_group(label: &str, rows: Vec<Row>) -> Group {
        Group {
            key: label.to_lowercase(),
            label: label.to_string(),
            target: Some(RemoteTarget::direct("me", label, 22)),
            link: Link::Offline,
            ..group(rows)
        }
    }

    #[test]
    fn orphan_panes_of_keeps_only_live_panes_no_workspace_holds() {
        use tty59_core::daemon::protocol::PaneInfo;

        fn info(pane_id: u64, alive: bool) -> PaneInfo {
            PaneInfo {
                pane_id,
                cwd: Some(std::path::PathBuf::from("/tmp/x")),
                title: "zsh".to_string(),
                osc_title: None,
                alive,
                owner: Some("tty59-cli".to_string()),
            }
        }

        let held: HashSet<u64> = [2].into_iter().collect();
        let orphans = orphan_panes_of(vec![info(1, true), info(2, true), info(3, false)], &held);
        assert_eq!(
            orphans,
            vec![OrphanPane {
                pane_id: 1,
                title: "zsh".to_string(),
                cwd: Some("/tmp/x".to_string()),
                owner: Some("tty59-cli".to_string()),
            }],
            "%2 is held by a workspace and %3 is dead — neither is a leak to reap (#596)"
        );
    }

    #[test]
    fn an_orphans_owner_reads_as_a_workspace_never_as_a_whole_uuid() {
        let uuid = "e0d7bebd-2e46-4a8e-abbb-f2f109a9b61d";
        assert_eq!(
            owner_label(uuid, Some("seeg".to_string())),
            "seeg",
            "the workspace is still here, so say which one it is"
        );
        assert_eq!(
            owner_label(uuid, None),
            "e0d7bebd",
            "no workspace left to name: the short id `pane ls` prints, not all 36 \
             characters — the full one shoved the Close button off the card"
        );
        assert_eq!(
            owner_label("tty59-cli", None),
            "tty59-cli",
            "an older client's own label is not an id and reads fine whole"
        );
    }

    #[test]
    fn a_workspace_stays_in_the_list_when_only_one_of_its_tabs_matches() {
        let ws = row(
            "notes",
            vec![tab("zsh", "~/notes"), tab("claude", "~/notes")],
        );
        assert!(ws.matches("claude"));
        assert!(!ws.matches("codex"));
    }

    #[test]
    fn searching_a_tab_name_narrows_the_tab_column_to_the_hits() {
        let ws = row("notes", vec![tab("zsh", "~/a"), tab("claude", "~/b")]);
        assert_eq!(visible_tabs(&ws, "claude"), vec![1]);
    }

    #[test]
    fn searching_the_workspace_name_keeps_all_of_its_tabs() {
        let ws = row("notes", vec![tab("zsh", "~/a"), tab("claude", "~/b")]);
        assert_eq!(visible_tabs(&ws, "notes"), vec![0, 1]);
    }

    #[test]
    fn a_workspace_shown_only_because_its_host_matched_keeps_all_its_tabs() {
        // Nothing about the workspace or its tabs matched — the group header
        // did. An empty column here would read as "this workspace is empty".
        let ws = row("notes", vec![tab("zsh", "~/a")]);
        assert_eq!(visible_tabs(&ws, "dev-box"), vec![0]);
    }

    #[test]
    fn the_flat_list_is_most_recently_used_first_across_machines() {
        // Two machines, interleaved activity: the list must interleave too —
        // grouping by machine is exactly what this design retired.
        let groups = vec![
            group(vec![
                aged(row("old-local", vec![]), 10),
                aged(row("fresh-local", vec![]), 40),
            ]),
            named_group("devbox", vec![aged(row("remote", vec![]), 30)]),
        ];
        let names: Vec<&str> = flatten(&groups, "", &MachineScope::All)
            .into_iter()
            .map(|(g, r)| groups[g].rows[r].name.as_str())
            .collect();
        assert_eq!(names, vec!["fresh-local", "remote", "old-local"]);
    }

    #[test]
    fn this_windows_workspace_outranks_everything_however_stale() {
        let mut here = aged(row("here", vec![]), 1);
        here.current = true;
        let groups = vec![group(vec![aged(row("busy", vec![]), 99), here])];
        let first = flatten(&groups, "", &MachineScope::All)[0];
        assert_eq!(groups[first.0].rows[first.1].name, "here");
    }

    #[test]
    fn a_workspace_named_by_the_query_outranks_one_matched_by_its_tabs() {
        // This window's workspace has its tabs under `/tmp/t7qa`, so "qa"
        // lists it — but the workspace called qa is what was asked for, and
        // Enter takes the top row.
        let mut here = aged(row("keen-crane", vec![tab("zsh", "/tmp/t7qa/fixture")]), 50);
        here.current = true;
        let groups = vec![group(vec![here, aged(row("qa", vec![]), 10)])];
        let first = flatten(&groups, "qa", &MachineScope::All)[0];
        assert_eq!(groups[first.0].rows[first.1].name, "qa");
    }

    #[test]
    fn a_query_matching_a_machines_name_keeps_all_of_its_rows() {
        // Searching "devbox" is how the retired per-machine grouping is asked
        // for now, so a host-name hit must surface every row of that machine
        // and nothing of the others.
        let groups = vec![
            group(vec![row("local-notes", vec![])]),
            named_group("devbox", vec![row("api", vec![]), row("web", vec![])]),
        ];
        let hits = flatten(&groups, "devbox", &MachineScope::All);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|&(g, _)| g == 1));
    }

    /// Every link state names the dot its machine chip wears — reachable
    /// solid, configured-but-off hollow, the attention states in the rows'
    /// inks. The mapping is the behavior; the inks are the theme's, and no
    /// window is needed to pin one to the other.
    #[test]
    fn every_link_state_names_its_rail_dot() {
        assert_eq!(rail_dot_style(Link::Local), RailDotStyle::Solid);
        assert_eq!(rail_dot_style(Link::Connected), RailDotStyle::Solid);
        assert_eq!(rail_dot_style(Link::Offline), RailDotStyle::Hollow);
        assert_eq!(rail_dot_style(Link::Connecting), RailDotStyle::Warn);
        assert_eq!(
            rail_dot_style(Link::Reconnecting { attempt: 4 }),
            RailDotStyle::Warn
        );
        assert_eq!(rail_dot_style(Link::Failed), RailDotStyle::Danger);
    }

    #[test]
    fn a_machine_scope_lists_only_that_machines_rows_still_recent_first() {
        let groups = vec![
            group(vec![aged(row("local", vec![]), 50)]),
            named_group(
                "devbox",
                vec![aged(row("api", vec![]), 10), aged(row("web", vec![]), 30)],
            ),
        ];
        let scope = MachineScope::Machine("devbox".into());
        let names: Vec<&str> = flatten(&groups, "", &scope)
            .into_iter()
            .map(|(g, r)| groups[g].rows[r].name.as_str())
            .collect();
        assert_eq!(names, vec!["web", "api"]);
        assert!(flatten(&groups, "local", &scope).is_empty());
        assert_eq!(flatten(&groups, "", &MachineScope::local()).len(), 1);
    }

    /// A workspace the list would open in this window.
    fn closed(mut r: Row) -> Row {
        r.open = false;
        r
    }

    /// A machine workspace this client has not adopted: the row carries the
    /// snapshot it came from, and no window holds it.
    fn adoptable(mut r: Row) -> Row {
        let snapshot = RemoteWorkspaceRow {
            id: r.id,
            name: r.name.clone(),
            panes: 0,
            last_active: r.last_active,
        };
        r.open = false;
        r.remote_id = Some(r.id);
        r.adopt = Some(Box::new(snapshot));
        r
    }

    fn names_of<'a>(groups: &'a [Group], nav: &[Nav]) -> Vec<&'a str> {
        nav.iter()
            .map(|&(g, r)| groups[g].rows[r].name.as_str())
            .collect()
    }

    /// The regression the whole split exists for: narrowing to this computer
    /// puts its open workspaces together, so a recently closed one no longer
    /// sits above a live one. The old comparator's order is recorded in the
    /// milestone review, not reproduced here.
    #[test]
    fn a_scoped_list_puts_open_rows_before_a_more_recent_closed_one() {
        let groups = vec![group(vec![
            aged(row("deploy", vec![]), 10),
            closed(aged(row("notes", vec![]), 90)),
        ])];
        let scope = MachineScope::local();
        assert_eq!(
            names_of(&groups, &flatten(&groups, "", &scope)),
            vec!["deploy", "notes"]
        );
        assert_eq!(
            names_of(&groups, &flatten(&groups, "", &MachineScope::All)),
            vec!["notes", "deploy"],
            "All keeps the blended order it always had"
        );
    }

    /// Adopt rows are not open here, so under a machine scope they belong
    /// below the windows that are.
    #[test]
    fn a_scoped_machine_puts_open_workspaces_before_more_recent_adoptable_ones() {
        let groups = vec![named_group(
            "devbox",
            vec![
                adoptable(aged(row("web", vec![]), 90)),
                aged(row("api", vec![]), 10),
            ],
        )];
        let scope = MachineScope::Machine("devbox".into());
        assert_eq!(
            names_of(&groups, &flatten(&groups, "", &scope)),
            vec!["api", "web"]
        );
    }

    /// The split is one sort key, not a replacement for the search precedence:
    /// inside a section the exact name still leads, and a machine-name hit
    /// still brings all of its rows.
    #[test]
    fn the_scoped_split_keeps_the_queries_it_had() {
        let mut here = aged(row("keen-crane", vec![tab("zsh", "/tmp/t7qa/fixture")]), 50);
        here.current = true;
        let groups = vec![group(vec![here, aged(row("qa", vec![]), 10)])];
        let local = MachineScope::local();
        assert_eq!(
            names_of(&groups, &flatten(&groups, "qa", &local)),
            vec!["qa", "keen-crane"],
            "both rows are open, so the name match still decides inside the section"
        );

        let groups = vec![named_group(
            "devbox",
            vec![
                aged(row("web", vec![]), 10),
                closed(aged(row("api", vec![]), 90)),
            ],
        )];
        let scope = MachineScope::Machine("devbox".into());
        assert_eq!(
            names_of(&groups, &flatten(&groups, "devbox", &scope)),
            vec!["web", "api"],
            "a host-name search still lists every row of the machine, open first"
        );
    }

    #[test]
    fn scoped_sections_break_at_the_boundaries_without_moving_nav() {
        let groups = vec![group(vec![
            aged(row("deploy", vec![]), 10),
            closed(aged(row("notes", vec![]), 90)),
        ])];
        let scope = MachineScope::local();
        let nav = flatten(&groups, "", &scope);
        assert_eq!(nav, vec![(0, 0), (0, 1)], "the open row is first");
        assert_eq!(
            nav_sections(&nav, &groups, &scope, false),
            vec![Some(Section::Open), Some(Section::Closed)],
            "one heading per run, aligned with the nav slot it opens"
        );
        assert_eq!(
            nav_sections(&nav, &groups, &scope, true),
            vec![Some(Section::LastKnown); 2],
            "an offline machine's remembered rows are not split by open state"
        );
        assert_eq!(
            nav_sections(&nav, &groups, &MachineScope::All, true),
            vec![None; 2],
            "the unscoped list has no headings"
        );
    }

    #[test]
    fn only_a_reachable_down_machine_offers_the_connect_card() {
        let scope = MachineScope::Machine("devbox".into());
        let one = |link, parked| {
            let mut group = named_group("devbox", vec![]);
            group.link = link;
            group.parked = parked;
            vec![group]
        };
        for link in [Link::Offline, Link::Failed] {
            assert!(offline_connect_group(&scope, &one(link, false)).is_some());
            assert!(
                offline_connect_group(&scope, &one(link, true)).is_none(),
                "a parked route cannot be connected (#485)"
            );
        }
        for link in [
            Link::Connected,
            Link::Connecting,
            Link::Reconnecting { attempt: 2 },
            Link::Local,
        ] {
            assert!(
                offline_connect_group(&scope, &one(link, false)).is_none(),
                "{link:?} must not take the offline-connect shortcut"
            );
        }
        let local = vec![group(vec![])];
        assert!(offline_connect_group(&MachineScope::local(), &local).is_none());
        assert!(offline_connect_group(&MachineScope::All, &one(Link::Offline, false)).is_none());
    }

    #[test]
    fn orphan_panes_stay_on_this_computers_screens() {
        assert!(orphans_visible(&MachineScope::All));
        assert!(orphans_visible(&MachineScope::local()));
        assert!(
            !orphans_visible(&MachineScope::Machine("devbox".into())),
            "a remote scope is about another machine's workspaces"
        );
    }

    #[test]
    fn the_offline_card_dials_the_configured_endpoint_or_the_targets_own_name() {
        let mut machine = named_group("devbox", vec![]);
        assert_eq!(group_endpoint(&machine), "me@devbox");
        machine.endpoint = "me@devbox.example:2222".into();
        assert_eq!(group_endpoint(&machine), "me@devbox.example:2222");
        let choice = connect_choice(&machine).expect("a machine has a target");
        assert_eq!(choice.label, "devbox");
        assert_eq!(choice.detail, "me@devbox.example:2222");
        assert!(
            connect_choice(&group(vec![])).is_none(),
            "this computer connects to nothing"
        );
    }

    #[test]
    fn a_scoped_row_drops_the_machine_from_its_metadata_line() {
        let api = row("api", vec![]);
        let devbox = named_group("devbox", vec![]);
        assert_eq!(
            row_meta(&devbox, &api, true),
            vec!["devbox".to_string(), "~/code".to_string()]
        );
        assert_eq!(
            row_meta(&devbox, &api, false),
            vec!["~/code".to_string()],
            "the scope already says which machine these rows are on"
        );
        let local = group(vec![]);
        assert_eq!(
            row_meta(&local, &api, true),
            vec![
                t(L10nKey::SwitcherLocalHost).to_string(),
                "~/code".to_string()
            ]
        );
    }

    #[test]
    fn the_offline_card_shows_the_failure_then_the_endpoint() {
        let mut machine = named_group("devbox", vec![]);
        machine.endpoint = "me@devbox.example:2222".into();
        assert_eq!(
            offline_card_detail(&machine),
            (None, "me@devbox.example:2222".to_string()),
            "a machine with no failure behind it has only the endpoint to print"
        );
        machine.error = Some("connection refused".into());
        assert_eq!(
            offline_card_detail(&machine),
            (
                Some("connection refused".to_string()),
                "me@devbox.example:2222".to_string()
            ),
            "the failure rides on the line and the endpoint stays after it"
        );
    }

    #[test]
    fn an_empty_scoped_list_names_the_machine() {
        crate::ui::i18n::set_locale("en");
        let groups = vec![named_group("devbox", vec![])];
        assert_eq!(
            empty_list_text(&MachineScope::Machine("devbox".into()), &groups),
            "No workspaces on devbox"
        );
        assert_eq!(
            empty_list_text(&MachineScope::All, &groups),
            t(L10nKey::SwitcherNoMatch),
            "unscoped keeps the wording it always had"
        );
    }

    #[test]
    fn the_tab_column_follows_the_flat_cursor() {
        let layout = Layout {
            groups: vec![group(vec![row("a", vec![]), row("b", vec![])])],
            nav: vec![(0, 1), (0, 0)],
            rail: Vec::new(),
            scope: MachineScope::All,
        };
        assert_eq!(layout.subject(0), Some((0, 1)));
        assert_eq!(layout.subject(1), Some((0, 0)));
        assert_eq!(layout.subject(2), None, "past the end is nothing");
    }

    fn host(label: &str) -> HostChoice {
        HostChoice {
            target: RemoteTarget::direct("me", label, 22),
            label: label.to_string(),
            detail: format!("me@{label}:22"),
        }
    }

    #[test]
    fn the_host_dropdown_leads_local_and_pins_add_host_last() {
        let items = host_items(vec![host("devbox")], "", "This Computer");
        assert!(matches!(items.first(), Some(HostItem::Local)));
        assert!(matches!(items.last(), Some(HostItem::AddHost)));
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn filtering_the_dropdown_narrows_hosts_but_never_loses_the_way_out() {
        let items = host_items(
            vec![host("devbox"), host("staging")],
            "stag",
            "This Computer",
        );
        // "stag" matches neither the local label nor devbox.
        assert_eq!(items.len(), 2);
        assert!(
            matches!(&items[0], HostItem::Host(h) if h.label == "staging"),
            "the one matching machine survives"
        );
        assert!(
            matches!(items.last(), Some(HostItem::AddHost)),
            "Add SSH Host… is pinned, filter or no filter"
        );
    }

    #[test]
    fn the_local_row_answers_to_its_own_name() {
        let items = host_items(vec![host("devbox")], "this comp", "This Computer");
        assert!(matches!(items.first(), Some(HostItem::Local)));
    }

    #[test]
    fn the_cursor_wraps_at_both_ends() {
        assert_eq!(step(0, 3, true), 1);
        assert_eq!(step(2, 3, true), 0);
        assert_eq!(step(0, 3, false), 2);
    }

    #[test]
    fn a_tab_of_another_window_is_named_the_way_a_local_one_would_be() {
        // `title` here is the foreground process name the machine tree carries,
        // not a terminal title — it must not outrank the working directory the
        // local tab strip would be showing.
        let mut view = crate::ui::machine_mirror::TabView {
            id: TabId::new(),
            name: Some("  build  ".to_string()),
            title: "zsh".to_string(),
            osc_title: Some("✳ 修复 workspace switcher".to_string()),
            cwd: Some("/Users/x/repo/tty59".to_string()),
            agent: Some(crate::core::cli_agent::CLIAgent::Claude),
            status: None,
            live: true,
            panes: 1,
        };
        assert_eq!(tab_view_label(&view, 0, None), "build", "a given name wins");

        view.name = None;
        assert_eq!(
            tab_view_label(&view, 0, None),
            "修复 workspace switcher",
            "then the title the local strip would be showing — mark and all,              which is to say without the mark"
        );

        view.osc_title = Some("user@host:~/repo/025/tty59".to_string());
        assert_eq!(
            tab_view_label(&view, 0, None),
            crate::ui::tab_strip::short_title("user@host:~/repo/025/tty59", None),
            "a shell's title goes through the shortener the strip uses"
        );

        view.osc_title = Some("user@host:".to_string());
        assert_eq!(
            tab_view_label(&view, 0, None),
            "zsh",
            "a title that shortens away to nothing falls through"
        );

        view.osc_title = None;
        assert_eq!(
            tab_view_label(&view, 0, None),
            "Claude Code",
            "an agent names a tab that has told us nothing else"
        );

        view.agent = None;
        assert_eq!(
            tab_view_label(&view, 0, None),
            crate::ui::tab_strip::short_title("/Users/x/repo/tty59", None),
            "otherwise the directory, put through the same shortener as the strip"
        );

        view.cwd = None;
        assert_eq!(
            tab_view_label(&view, 0, None),
            "zsh",
            "process name is last"
        );

        view.title = String::new();
        assert!(tab_view_label(&view, 2, None).contains('3'));
    }
}

#[cfg(test)]
mod gpui_tests {
    use gpui::{Context, Modifiers, TestAppContext, Window};

    use super::Column;
    use super::MachineScope;
    use crate::ui::app::Tty59App;
    use crate::ui::app::test_window::harness_with_tabs;

    /// Deleting a remote workspace has to take the listing snapshot's row with
    /// it. The snapshot every window keeps for the switcher is merged into the
    /// panel every frame, and with the store entry gone nothing dedups that
    /// row any more — the workspace the user just deleted popped straight back
    /// as an adoptable machine row until the next reconnect replaced the
    /// snapshot.
    #[gpui::test]
    fn a_deleted_remote_workspace_leaves_the_switcher(cx: &mut TestAppContext) {
        use gpui::VisualContext as _;

        use crate::core::session::{RemoteRef, RemoteTarget, WindowView, WorkspaceId};

        let (app, vcx) = crate::ui::app::test_window::harness(cx);
        let handle = vcx.window_handle();
        let weak = app.downgrade();

        let target = RemoteTarget::Alias {
            alias: "build-box".into(),
        };
        let machine_ws = WorkspaceId::new();
        let kept_ws = WorkspaceId::new();
        let view = WindowView::on_remote(RemoteRef::new(target.clone(), machine_ws));
        let doomed = view.id;

        app.update(cx, |app, cx| {
            crate::core::session::WorkspaceStore::install_for_test(
                cx,
                crate::core::session::WindowViews {
                    views: vec![view],
                    active: None,
                },
            );
            crate::ui::windows::WindowRegistry::init(cx);
            crate::ui::windows::WindowRegistry::register(cx, app.workspace, handle, weak);
            app.host_snapshots.insert(
                target.host_id(),
                super::HostSnapshot {
                    target: target.clone(),
                    rows: vec![
                        crate::ui::remote_connect::RemoteWorkspaceRow {
                            id: machine_ws,
                            name: "doomed".into(),
                            panes: 0,
                            last_active: 0,
                        },
                        crate::ui::remote_connect::RemoteWorkspaceRow {
                            id: kept_ws,
                            name: "kept".into(),
                            panes: 0,
                            last_active: 0,
                        },
                    ],
                },
            );
        });

        // From a plain app context, the way `confirm_and_delete`'s spawned
        // task calls it — inside the entity's own update this would abort on
        // the reentrant read (#617's lesson).
        cx.update(|cx| {
            crate::ui::windows::delete_workspace(cx, doomed);
        });

        app.update(cx, |app, cx| {
            let listed: Vec<WorkspaceId> = app
                .switcher_groups(cx)
                .iter()
                .flat_map(|g| g.rows.iter().filter_map(|r| r.remote_id))
                .collect();
            assert!(
                !listed.contains(&machine_ws),
                "the deleted workspace came back from the listing snapshot"
            );
            assert!(
                listed.contains(&kept_ws),
                "its machine's other workspaces are still on offer"
            );
        });
    }

    /// The rail's chip click and Alt+←/→ both enter through this helper: the
    /// stored scope moves, and a leading `@token` — committed, half-typed or
    /// unresolved — leaves the query so its per-frame preview cannot override
    /// the pick. Everything typed after the token is still the search.
    #[gpui::test]
    fn selecting_a_machine_rewrites_the_query_and_sets_the_scope(cx: &mut TestAppContext) {
        fn select(
            app: &mut Tty59App,
            window: &mut Window,
            cx: &mut Context<Tty59App>,
            typed: &str,
            scope: MachineScope,
        ) -> (String, MachineScope) {
            let query = app.switcher.as_ref().expect("panel is open").query.clone();
            query.update(cx, |state, cx| state.set_value(typed, window, cx));
            app.select_switcher_machine(scope, window, cx);
            (
                query.read(cx).value().to_string(),
                app.switcher.as_ref().expect("still open").scope.clone(),
            )
        }

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        app.update_in(&mut vcx, |app, window, cx| app.open_switcher(window, cx));

        app.update_in(&mut vcx, |app, window, cx| {
            let devbox = MachineScope::Machine("devbox".into());
            {
                let sw = app.switcher.as_mut().expect("panel is open");
                sw.left_sel = 5;
                sw.right_sel = 4;
                sw.column = Column::Right;
            }
            assert_eq!(
                select(app, window, cx, "api", devbox.clone()),
                ("api".to_string(), devbox.clone()),
                "a plain query is the search and stays untouched"
            );
            let sw = app.switcher.as_ref().expect("still open");
            assert_eq!(
                (sw.left_sel, sw.right_sel),
                (0, 0),
                "a pick sends both cursors back to the top"
            );
            assert_eq!(sw.column, Column::Left);
            assert_eq!(
                select(app, window, cx, "@dev api x", devbox.clone()),
                ("api x".to_string(), devbox.clone()),
                "a committed token goes and the rest stays"
            );
            assert_eq!(
                select(app, window, cx, "@dev", devbox.clone()),
                (String::new(), devbox.clone()),
                "a half-typed token takes the box with it"
            );
            let local = MachineScope::local();
            assert_eq!(
                select(app, window, cx, "@nope api", local.clone()),
                ("api".to_string(), local.clone()),
                "a click wins over an unresolved token"
            );
            assert_eq!(
                select(app, window, cx, "", MachineScope::All),
                (String::new(), MachineScope::All),
                "an empty box has no token to drop"
            );
        });
    }

    /// The stored scope and the preview can disagree: with All stored and
    /// `@dev` typed, the list shows devbox. Picking All must drop the token
    /// and reset the cursor even though the stored scope does not change — the
    /// early return that skipped the assignments left the tab column where it
    /// was.
    #[gpui::test]
    fn selecting_all_over_a_resolved_preview_clears_the_token_and_resets_the_cursor(
        cx: &mut TestAppContext,
    ) {
        use crate::core::session::RemoteTarget;

        use super::HostSnapshot;

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        let target = RemoteTarget::Alias {
            alias: "devbox".into(),
        };
        let key = target.to_string();
        app.update(cx, |app, _| {
            app.host_snapshots.insert(
                target.host_id(),
                HostSnapshot {
                    target: target.clone(),
                    rows: vec![],
                },
            );
        });
        app.update_in(&mut vcx, |app, window, cx| app.open_switcher(window, cx));

        app.update_in(&mut vcx, |app, window, cx| {
            let query = app.switcher.as_ref().expect("panel is open").query.clone();
            query.update(cx, |state, cx| state.set_value("@dev", window, cx));
            assert_eq!(
                app.switcher_layout(cx).scope,
                MachineScope::Machine(key.clone()),
                "the half-typed token previews devbox"
            );
            {
                let sw = app.switcher.as_mut().expect("panel is open");
                sw.left_sel = 4;
                sw.right_sel = 3;
                sw.column = Column::Right;
            }
            app.select_switcher_machine(MachineScope::All, window, cx);
        });
        app.update(cx, |app, cx| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(sw.scope, MachineScope::All);
            assert_eq!(sw.query.read(cx).value(), "", "the preview token is gone");
            assert_eq!(
                (sw.left_sel, sw.right_sel),
                (0, 0),
                "the unchanged stored scope still sends the cursors to the top"
            );
            assert_eq!(sw.column, Column::Left);
        });
    }

    /// Clicking the chip whose scope is already stored is still a pick: it
    /// puts the keyboard cursor back on the top row.
    #[gpui::test]
    fn picking_the_scope_already_stored_still_resets_the_cursor(cx: &mut TestAppContext) {
        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        app.update_in(&mut vcx, |app, window, cx| app.open_switcher(window, cx));

        app.update_in(&mut vcx, |app, window, cx| {
            let devbox = MachineScope::Machine("devbox".into());
            app.select_switcher_machine(devbox.clone(), window, cx);
            {
                let sw = app.switcher.as_mut().expect("panel is open");
                sw.left_sel = 2;
                sw.right_sel = 1;
                sw.column = Column::Right;
            }
            app.select_switcher_machine(devbox, window, cx);
        });
        app.update(cx, |app, _| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(sw.scope, MachineScope::Machine("devbox".into()));
            assert_eq!((sw.left_sel, sw.right_sel), (0, 0));
            assert_eq!(sw.column, Column::Left);
        });
    }

    /// Typing `@machine` and a space commits the token: the stored scope moves
    /// to that machine and only the rest of the query stays in the box. A
    /// half-typed or unresolved token is left for the per-frame preview, and
    /// once the token is gone further typing does not move the scope again.
    #[gpui::test]
    fn a_committed_machine_token_sets_the_scope_and_rewrites_the_query(cx: &mut TestAppContext) {
        use crate::core::session::RemoteTarget;

        use super::HostSnapshot;

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        let target = RemoteTarget::Alias {
            alias: "devbox".into(),
        };
        let key = target.to_string();
        app.update(cx, |app, _| {
            app.host_snapshots.insert(
                target.host_id(),
                HostSnapshot {
                    target: target.clone(),
                    rows: vec![],
                },
            );
        });
        app.update_in(&mut vcx, |app, window, cx| app.open_switcher(window, cx));

        let type_into_query =
            |app: &mut Tty59App, window: &mut Window, cx: &mut Context<Tty59App>, text: &str| {
                let query = app.switcher.as_ref().expect("panel is open").query.clone();
                // `replace` replaces the selection, not the document, so clear
                // first. `set_value` is silent in this dependency; `replace`
                // takes the same path as typing, so the Change subscriber runs.
                query.update(cx, |state, cx| {
                    state.set_value("", window, cx);
                    state.replace(text, window, cx);
                });
            };

        app.update_in(&mut vcx, |app, window, cx| {
            type_into_query(app, window, cx, "@dev")
        });
        app.update(cx, |app, cx| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(sw.scope, MachineScope::All, "a half-typed token previews");
            assert_eq!(sw.query.read(cx).value(), "@dev", "and stays in the box");
        });

        app.update_in(&mut vcx, |app, window, cx| {
            type_into_query(app, window, cx, "@nope ")
        });
        app.update(cx, |app, cx| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(
                sw.scope,
                MachineScope::All,
                "an unresolved token commits nothing"
            );
            assert_eq!(sw.query.read(cx).value(), "@nope ");
        });

        app.update_in(&mut vcx, |app, window, cx| {
            type_into_query(app, window, cx, "@dev api")
        });
        app.update(cx, |app, cx| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(
                sw.scope,
                MachineScope::Machine(key.clone()),
                "the committed token moves the stored scope"
            );
            assert_eq!(
                sw.query.read(cx).value(),
                "api",
                "the token leaves and the rest keeps searching"
            );
            assert!(
                !sw.committing_token,
                "the rewrite is over, so the next Change is read as typing"
            );
        });

        app.update_in(&mut vcx, |app, window, cx| {
            type_into_query(app, window, cx, "api v2")
        });
        app.update(cx, |app, cx| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(
                sw.scope,
                MachineScope::Machine(key.clone()),
                "later typing without a token leaves the scope alone"
            );
            assert_eq!(sw.query.read(cx).value(), "api v2");
        });
    }

    /// Backspace is the way back out of a machine when there is nothing left
    /// to delete. A chorded Backspace still belongs to the input, and a
    /// non-empty box still deletes a character.
    #[gpui::test]
    fn backspace_on_an_empty_scoped_search_returns_to_all(cx: &mut TestAppContext) {
        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_switcher(window, cx);
            app.set_switcher_scope(MachineScope::local(), cx);
        });
        // The key listener lives on the rendered card.
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        vcx.simulate_keystrokes("alt-backspace");
        app.update(cx, |app, _| {
            assert_eq!(
                app.switcher.as_ref().expect("panel is open").scope,
                MachineScope::local(),
                "a chorded Backspace is the input's"
            );
        });

        // A non-empty box still deletes a character, and the machine stays.
        app.update_in(&mut vcx, |app, window, cx| {
            let query = app.switcher.as_ref().expect("panel is open").query.clone();
            query.update(cx, |state, cx| state.set_value("api", window, cx));
        });
        vcx.simulate_keystrokes("backspace");
        app.update(cx, |app, cx| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(sw.scope, MachineScope::local());
            assert_eq!(
                sw.query.read(cx).value(),
                "ap",
                "the input deleted the char"
            );
        });

        // Shift keeps Backspace's meaning, so an empty scoped box steps out.
        vcx.simulate_keystrokes("backspace");
        vcx.simulate_keystrokes("shift-backspace");
        app.update(cx, |app, cx| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(
                sw.scope,
                MachineScope::local(),
                "both presses had a character to delete"
            );
            assert_eq!(sw.query.read(cx).value(), "");
        });

        // Whitespace is text: Backspace deletes it before the scope goes.
        app.update_in(&mut vcx, |app, window, cx| {
            let query = app.switcher.as_ref().expect("panel is open").query.clone();
            query.update(cx, |state, cx| state.set_value(" ", window, cx));
        });
        vcx.simulate_keystrokes("backspace");
        app.update(cx, |app, cx| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(
                sw.scope,
                MachineScope::local(),
                "the space was deleted first"
            );
            assert_eq!(sw.query.read(cx).value(), "");
        });

        vcx.simulate_keystrokes("backspace");
        app.update(cx, |app, _| {
            assert_eq!(
                app.switcher.as_ref().expect("panel is open").scope,
                MachineScope::All
            );
        });

        // Not scoped: the bare key stays the input's own.
        vcx.simulate_keystrokes("backspace");
        app.update(cx, |app, _| {
            assert_eq!(
                app.switcher.as_ref().expect("panel is open").scope,
                MachineScope::All
            );
        });
    }

    /// The footer and the create form read the same effective scope as the
    /// list, so All and this computer start a local form, a machine fills in
    /// its own host, and a half-typed `@token` previews it before the space.
    #[gpui::test]
    fn the_create_preset_follows_the_effective_scope(cx: &mut TestAppContext) {
        use crate::core::session::RemoteTarget;
        use crate::ui::i18n::{L10nKey, t, t_fmt};

        use super::HostSnapshot;

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        let target = RemoteTarget::Alias {
            alias: "devbox".into(),
        };
        let key = target.to_string();
        app.update(cx, |app, _| {
            app.host_snapshots.insert(
                target.host_id(),
                HostSnapshot {
                    target: target.clone(),
                    rows: vec![],
                },
            );
        });
        app.update_in(&mut vcx, |app, window, cx| app.open_switcher(window, cx));

        app.update_in(&mut vcx, |app, window, cx| {
            let (label, chosen) = app.create_preset(cx);
            assert_eq!(label, t(L10nKey::AppMenuNewWorkspace));
            assert!(chosen.is_none(), "All starts on this computer");

            app.set_switcher_scope(MachineScope::local(), cx);
            let (label, chosen) = app.create_preset(cx);
            assert_eq!(
                label,
                t_fmt(
                    L10nKey::SwitcherNewWorkspaceOnMachine,
                    &[("machine", t(L10nKey::SwitcherThisComputer))],
                )
            );
            assert!(chosen.is_none(), "this computer has no host to connect");

            app.set_switcher_scope(MachineScope::Machine(key.clone()), cx);
            let (label, chosen) = app.create_preset(cx);
            assert_eq!(
                label,
                t_fmt(
                    L10nKey::SwitcherNewWorkspaceOnMachine,
                    &[("machine", "devbox")],
                )
            );
            let chosen = chosen.expect("a remote scope supplies its host");
            assert_eq!(chosen.target, target);
            assert_eq!(chosen.label, "devbox");

            // Half-typed token: stored All, but the preview is devbox.
            app.set_switcher_scope(MachineScope::All, cx);
            let query = app.switcher.as_ref().expect("panel is open").query.clone();
            query.update(cx, |state, cx| state.set_value("@dev", window, cx));
            let (label, chosen) = app.create_preset(cx);
            assert_eq!(
                label,
                t_fmt(
                    L10nKey::SwitcherNewWorkspaceOnMachine,
                    &[("machine", "devbox")],
                )
            );
            assert_eq!(
                chosen.expect("the preview supplies its host").target,
                target
            );
        });
    }

    /// Opening the create form while scoped presets it, but a host the caller
    /// names (the row menu's own New Workspace) is never replaced. A machine
    /// with no link queues the existing PendingCreate path.
    #[gpui::test]
    fn the_scoped_preset_is_used_unless_a_host_is_named(cx: &mut TestAppContext) {
        use crate::core::session::RemoteTarget;

        use super::{HostChoice, HostSnapshot, Page};

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        let scoped = RemoteTarget::Alias {
            alias: "devbox".into(),
        };
        let explicit = RemoteTarget::Alias {
            alias: "ws-scope-testbox".into(),
        };
        app.update(cx, |app, _| {
            for target in [scoped.clone(), explicit.clone()] {
                app.host_snapshots.insert(
                    target.host_id(),
                    HostSnapshot {
                        target: target.clone(),
                        rows: vec![],
                    },
                );
            }
        });
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_switcher(window, cx);
            app.set_switcher_scope(MachineScope::Machine(scoped.to_string()), cx);
            app.open_workspace_form(window, cx);
            let sw = app.switcher.as_ref().expect("panel is open");
            let Page::Create(form) = &sw.page else {
                panic!("the form is up");
            };
            assert_eq!(
                form.chosen.as_ref().map(|c| c.target.clone()),
                Some(scoped.clone()),
                "the scope presets the form"
            );
        });

        app.update_in(&mut vcx, |app, window, cx| {
            app.switcher_back_to_list(window, cx);
            app.set_switcher_scope(MachineScope::Machine(scoped.to_string()), cx);
            app.switcher_to_form(
                Some(HostChoice {
                    target: explicit.clone(),
                    label: "explicit".into(),
                    detail: String::new(),
                }),
                window,
                cx,
            );
            let sw = app.switcher.as_ref().expect("panel is open");
            let Page::Create(form) = &sw.page else {
                panic!("the form is up");
            };
            assert_eq!(
                form.chosen.as_ref().map(|c| c.target.clone()),
                Some(explicit.clone()),
                "an explicit host choice wins over the scope"
            );
            let name = form.name.clone();
            name.update(cx, |state, cx| state.set_value("demo", window, cx));
            app.switcher_form_create(window, cx);
        });

        app.update(cx, |app, _| {
            let pending = app
                .pending_create
                .as_ref()
                .expect("a machine with no link waits for its connect");
            assert_eq!(pending.target, explicit);
            assert_eq!(pending.name.as_deref(), Some("demo"));
            assert!(
                matches!(app.connect, Some(super::ConnectFlow::Failed { .. })),
                "the test alias resolves to no ssh config entry, so the \
                 existing connect path reports the failure"
            );
            assert!(matches!(
                app.switcher.as_ref().expect("panel is open").page,
                Page::List
            ));
        });
    }

    /// A scoped open lands on the machine with a reset cursor, and a plain
    /// open after it goes back to All.
    #[gpui::test]
    fn a_scoped_open_resets_the_cursor_and_a_plain_open_resets_the_scope(cx: &mut TestAppContext) {
        use crate::core::session::RemoteTarget;

        use super::HostSnapshot;

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        let target = RemoteTarget::Alias {
            alias: "devbox".into(),
        };
        let key = target.to_string();
        app.update(cx, |app, _| {
            app.host_snapshots.insert(
                target.host_id(),
                HostSnapshot {
                    target: target.clone(),
                    rows: vec![],
                },
            );
        });

        app.update_in(&mut vcx, |app, window, cx| {
            app.open_switcher(window, cx);
            let sw = app.switcher.as_mut().expect("panel is open");
            sw.left_sel = 3;
            sw.right_sel = 2;
            sw.column = Column::Right;
            app.open_switcher_scoped(MachineScope::Machine(key.clone()), window, cx);
        });
        app.update(cx, |app, _| {
            let sw = app.switcher.as_ref().expect("panel is open");
            assert_eq!(sw.scope, MachineScope::Machine(key.clone()));
            assert_eq!((sw.left_sel, sw.right_sel), (0, 0));
            assert_eq!(sw.column, Column::Left);
        });

        app.update_in(&mut vcx, |app, window, cx| {
            app.close_switcher(window, cx);
            app.open_switcher(window, cx);
        });
        app.update(cx, |app, _| {
            assert_eq!(
                app.switcher.as_ref().expect("panel is open").scope,
                MachineScope::All,
                "ordinary opens always restart from All"
            );
        });
    }

    /// The rail carries itself to whichever chip the list is effectively
    /// scoped to — including the machine a half-typed `@token` previews,
    /// which is the one move no click or key asks for by name. The request
    /// is issued only when the chip's index moves, so a steady frame never
    /// drags the rail back and a hand browsing along it is not fought.
    ///
    /// The assertions reach real rendering, not a plan for it: the
    /// scroller's prepaint records its own bounds and its direct children's
    /// bounds into the `ScrollHandle` and consumes any pending
    /// `scroll_to_item` request in the same pass. Prepaint only runs when a
    /// frame is actually drawn, so every state change below is followed by
    /// an explicit draw before anything is claimed about `offset()`,
    /// `bounds()` or `bounds_for_item`.
    #[gpui::test]
    fn the_rail_scrolls_a_distant_at_preview_into_view(cx: &mut TestAppContext) {
        use gpui::{VisualTestContext, point, px, size};
        use tty59_core::core::session::RemoteTarget;

        fn settle(vcx: &mut VisualTestContext) {
            // `run_until_parked` only drains the executor; `draw` is what
            // paints a frame, and with it the scroll geometry under test.
            vcx.run_until_parked();
            let _ = vcx.update(|window, cx| window.draw(cx));
        }

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        vcx.simulate_resize(size(px(1200.), px(800.)));
        // Enough machines that the rail overflows the card several chips
        // over, with the ones the token previews and the step lands on far
        // past the right edge.
        app.update(cx, |app, _| {
            for name in [
                "aa-mill",
                "bb-press",
                "cc-drill",
                "dd-lathe",
                "ee-forge",
                "ff-motor",
                "gg-piston",
                "hh-rocket",
                "ii-saucer",
                "jj-tender",
                "kk-ultra",
                "ll-vapor",
                "mm-wharf",
                "nn-yard",
            ] {
                let target = RemoteTarget::Alias {
                    alias: name.to_string(),
                };
                app.host_snapshots.insert(
                    target.host_id(),
                    super::HostSnapshot {
                        target,
                        rows: Vec::new(),
                    },
                );
            }
        });
        app.update_in(&mut vcx, |app, window, cx| app.open_switcher(window, cx));
        settle(&mut vcx);
        assert_eq!(
            app.update_in(&mut vcx, |app, _, _| app
                .switcher
                .as_ref()
                .expect("panel is open")
                .rail_scroll
                .offset()
                .x),
            px(0.),
            "a rail that opens on All has nothing to scroll to"
        );

        // Half a token, never committed: the preview alone narrows the list,
        // so it alone has to carry the rail along.
        app.update_in(&mut vcx, |app, window, cx| {
            let query = app.switcher.as_ref().expect("panel is open").query.clone();
            query.update(cx, |state, cx| state.set_value("@mm", window, cx));
        });
        settle(&mut vcx);

        let (offset, chip, view) = app.update_in(&mut vcx, |app, _, cx| {
            let sw = app.switcher.as_ref().expect("panel is up");
            let ix = app
                .switcher_layout(cx)
                .rail
                .iter()
                .position(|e| e.scope == MachineScope::Machine("mm-wharf".into()))
                .expect("the previewed machine has a chip");
            (
                sw.rail_scroll.offset().x,
                sw.rail_scroll
                    .bounds_for_item(ix)
                    .expect("chips are drawn children"),
                sw.rail_scroll.bounds(),
            )
        });
        assert!(
            offset < px(0.),
            "the rail never followed the preview: still at {offset:?}"
        );
        assert!(
            chip.left() + offset >= view.left() - px(1.)
                && chip.right() + offset <= view.right() + px(1.),
            "the previewed chip {chip:?} never came into view {view:?} at {offset:?}"
        );

        // A hand that scrolls the rail back while the scope stands still is
        // not fought: a steady frame issues no new request.
        app.update_in(&mut vcx, |app, _, cx| {
            app.switcher
                .as_ref()
                .expect("panel is up")
                .rail_scroll
                .set_offset(point(px(0.), px(0.)));
            cx.notify();
        });
        settle(&mut vcx);
        assert_eq!(
            app.update_in(&mut vcx, |app, _, _| app
                .switcher
                .as_ref()
                .expect("panel is up")
                .rail_scroll
                .offset()
                .x),
            px(0.),
            "the rail re-centered itself on a frame where nothing moved"
        );

        // Alt+→ from a preview, dispatched as a real keystroke through the
        // focused search box rather than handed to the handler: the step
        // must survive the route a user's key actually takes. It starts
        // where the eye is — the previewed chip — picks the one after it as
        // the stored scope, and the token that previewed goes with it.
        vcx.simulate_keystrokes("alt-right");
        settle(&mut vcx);

        let (text, scope, rail) = app.update_in(&mut vcx, |app, _, cx| {
            let sw = app.switcher.as_ref().expect("panel is up");
            let ix = app
                .switcher_layout(cx)
                .rail
                .iter()
                .position(|e| e.scope == MachineScope::Machine("nn-yard".into()))
                .expect("the stepped-to machine has a chip");
            (
                sw.query.read(cx).value().to_string(),
                sw.scope.clone(),
                (
                    sw.rail_followed.get() == Some(ix),
                    sw.rail_scroll.offset().x,
                    sw.rail_scroll
                        .bounds_for_item(ix)
                        .expect("chips are drawn children"),
                    sw.rail_scroll.bounds(),
                ),
            )
        });
        let (followed, offset, chip, view) = rail;
        assert_eq!(text, "", "the step took the half-typed token with it");
        assert_eq!(
            scope,
            MachineScope::Machine("nn-yard".into()),
            "Alt+→ stepped from the previewed chip to the one after it"
        );
        assert!(
            followed,
            "the rail asked for the stepped-to chip, not the one before it"
        );
        assert!(
            offset < px(0.),
            "the rail never followed the step: still at {offset:?}"
        );
        assert!(
            chip.left() + offset >= view.left() - px(1.)
                && chip.right() + offset <= view.right() + px(1.),
            "the stepped-to chip {chip:?} never came into view {view:?} at {offset:?}"
        );
    }

    #[gpui::test]
    fn ctrl_tab_raises_the_panel_on_the_previously_used_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.simulate_modifiers_change(Modifiers::control());

        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));

        app.update(cx, |app, _| {
            let sw = app.switcher.as_ref().expect("Ctrl+Tab raises the panel");
            assert_eq!(sw.column, Column::Right, "the tab column takes the cursor");
            assert_eq!(sw.right_sel, 1, "the cursor lands on the previous tab");
            assert!(
                sw.mru,
                "Ctrl+Tab orders the column most-recently-used first"
            );
            assert!(sw.hold.is_some(), "the held modifier is what commits later");
        });
    }

    #[gpui::test]
    fn holding_ctrl_and_pressing_tab_again_walks_further_down(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.simulate_modifiers_change(Modifiers::control());

        app.update_in(&mut vcx, |app, window, cx| {
            app.tab_switch(true, window, cx);
            app.tab_switch(true, window, cx);
        });

        app.update(cx, |app, _| {
            assert_eq!(app.switcher.as_ref().expect("still up").right_sel, 2);
            assert_eq!(app.active, 0, "nothing is committed while the key is held");
        });
    }

    #[gpui::test]
    fn releasing_ctrl_commits_the_highlighted_tab_and_closes_the_panel(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));

        vcx.simulate_modifiers_change(Modifiers::none());

        app.update(cx, |app, _| {
            assert!(app.switcher.is_none(), "the panel comes down on release");
            assert_eq!(app.active, 1, "the highlighted tab is now the active one");
        });
    }

    #[gpui::test]
    fn a_second_ctrl_tab_goes_back_to_where_it_came_from(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));
        vcx.simulate_modifiers_change(Modifiers::none());
        vcx.run_until_parked();
        app.update(cx, |app, _| assert_eq!(app.active, 1));

        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));
        vcx.simulate_modifiers_change(Modifiers::none());

        app.update(cx, |app, _| {
            assert_eq!(
                app.active, 0,
                "most-recently-used ordering makes the gesture a toggle"
            );
        });
    }

    #[gpui::test]
    fn a_lone_tab_still_opens_the_panel_but_does_not_hold(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_modifiers_change(Modifiers::control());

        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));

        app.update(cx, |app, _| {
            let sw = app
                .switcher
                .as_ref()
                .expect("with nothing to cycle it still opens the switcher");
            assert_eq!(
                sw.column,
                Column::Left,
                "the workspace column is the useful one"
            );
            assert!(sw.hold.is_none(), "nothing to commit, so nothing to hold");
        });
    }

    #[gpui::test]
    fn a_lone_tabs_panel_survives_letting_go_of_ctrl(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));

        vcx.simulate_modifiers_change(Modifiers::none());

        app.update(cx, |app, _| {
            assert!(
                app.switcher.is_some(),
                "a panel opened without a hold must not close on release"
            );
        });
    }

    #[gpui::test]
    fn picking_a_tab_mid_gesture_commits_it_on_release(cx: &mut TestAppContext) {
        // What a click during the hold amounts to: aim the cursor, then let go.
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));

        app.update(cx, |app, cx| app.switcher_point_tab(2, cx));
        vcx.simulate_modifiers_change(Modifiers::none());

        app.update(cx, |app, _| {
            assert!(app.switcher.is_none());
            assert_eq!(app.active, 2, "the tab the pointer picked is now active");
        });
    }

    #[gpui::test]
    fn a_held_gesture_hides_the_context_menus_it_would_trip_over(cx: &mut TestAppContext) {
        // macOS reports Ctrl+click as a right click, which is exactly what the
        // context menu listens for. Nothing to assert on the element tree from
        // here, so this pins the flag the render path branches on.
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));

        app.update(cx, |app, _| {
            assert!(
                app.switcher.as_ref().is_some_and(|sw| sw.hold.is_some()),
                "the render path drops the menus while this is set"
            );
        });
    }

    #[gpui::test]
    fn the_new_workspace_gesture_raises_the_create_form(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update_in(&mut vcx, |app, window, cx| {
            app.open_workspace_form(window, cx)
        });

        app.update(cx, |app, cx| {
            let sw = app.switcher.as_ref().expect("the switcher is up");
            let super::Page::Create(form) = &sw.page else {
                panic!("the card shows the create form");
            };
            assert!(form.chosen.is_none(), "the host defaults to this computer");
            assert!(!form.open, "the dropdown starts folded");
            assert!(
                form.name(cx).is_none(),
                "the name box starts empty; nothing is suggested for the user"
            );
        });
    }

    #[gpui::test]
    fn the_create_form_will_not_create_without_a_name(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let before = app.update(cx, |app, _| app.workspace);

        app.update_in(&mut vcx, |app, window, cx| {
            app.open_workspace_form(window, cx);
            app.switcher_form_create(window, cx);
        });

        app.update(cx, |app, _| {
            assert_eq!(app.workspace, before, "no workspace was created");
            let sw = app.switcher.as_ref().expect("the form stays up");
            assert!(
                matches!(sw.page, super::Page::Create(_)),
                "the card is still the create form, waiting for a name"
            );
        });
    }

    /// Creating a workspace switches this window over to it, and the pull that
    /// follows used to read the window back out of the registry to see what it
    /// was showing — the very entity `switch_workspace` is holding. gpui
    /// aborts the process for that, so the window had to be registered here for
    /// the lookup to find anything and the crash to reproduce (#617).
    #[gpui::test]
    fn creating_a_workspace_does_not_read_the_window_making_it(cx: &mut TestAppContext) {
        use gpui::VisualContext;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let handle = vcx.window_handle();
        let weak = app.downgrade();
        app.update(cx, |app, cx| {
            crate::ui::windows::WindowRegistry::init(cx);
            crate::ui::windows::WindowRegistry::register(cx, app.workspace, handle, weak);
        });

        app.update_in(&mut vcx, |app, window, cx| {
            app.open_workspace_form(window, cx)
        });
        app.update_in(&mut vcx, |app, window, cx| {
            app.switcher_form_create(window, cx)
        });

        app.update(cx, |app, cx| {
            assert!(
                crate::ui::windows::WindowRegistry::app_for(cx, app.workspace).is_some(),
                "the window followed its new workspace into the registry"
            );
        });
    }

    /// The name the form collected has to reach the create, not chase it. It
    /// used to go out as a rename the moment the window switched — before the
    /// workspace existed on the machine to be renamed — so it was answered
    /// `NotFound`, dropped, and the generated name stood (#618).
    #[gpui::test]
    fn a_name_typed_into_the_form_is_held_for_the_create(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update_in(&mut vcx, |app, window, cx| {
            app.name_fresh_workspace(Some("deploy".into()), window, cx)
        });

        app.update(cx, |app, cx| {
            assert_eq!(
                crate::ui::tree_sync::chosen_name_for(cx, app.workspace).as_deref(),
                Some("deploy"),
                "the create this window is about to make spends it"
            );
        });
    }

    #[gpui::test]
    fn the_form_folds_back_into_the_list_rather_than_closing(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_workspace_form(window, cx)
        });

        app.update_in(&mut vcx, |app, window, cx| {
            app.switcher_back_to_list(window, cx)
        });

        app.update(cx, |app, _| {
            let sw = app
                .switcher
                .as_ref()
                .expect("still up — Esc backs out one step");
            assert!(matches!(sw.page, super::Page::List));
        });
    }

    /// One of the places on the card a test wants to put the pointer.
    #[derive(Clone, Copy)]
    enum Spot {
        Workspaces,
        Tabs,
    }

    /// Where that part of the card lands on screen. The card is centred and
    /// its columns are laid out from `CARD_W` / `LEFT_W`, so the geometry is
    /// worth recomputing here rather than hard-coding pixels that move with
    /// the window size. Vertically the columns sit under the search row and
    /// the machine rail, both of which the spots have to step over.
    fn card_point(vcx: &mut gpui::VisualTestContext, spot: Spot) -> gpui::Point<gpui::Pixels> {
        use gpui::{point, px};

        let viewport = vcx.update(|window, _| window.viewport_size());
        let card_w = super::CARD_W
            .min(viewport.width.as_f32() - 2. * super::CARD_MARGIN)
            .max(320.);
        let left_w = super::LEFT_W.min(card_w * 0.5);
        let card_left = (viewport.width.as_f32() - card_w) / 2.;
        let (dx, dy) = match spot {
            Spot::Workspaces => (
                20.,
                super::SEARCH_H + super::RAIL_H + super::COLUMN_PAD + 16.,
            ),
            // Past the tab column's own header row, onto its first tab.
            Spot::Tabs => (
                left_w + 40.,
                super::SEARCH_H
                    + super::RAIL_H
                    + super::COLUMN_PAD
                    + super::HOST_H
                    + super::TAB_ROW_H / 2.,
            ),
        };
        point(px(card_left + dx), px(super::CARD_TOP + dy))
    }

    /// Ctrl+Tab, then reach for the mouse: the pointer leaves the tab column
    /// for the workspace list, and letting go of Ctrl there must not slam the
    /// panel shut — switching workspaces by hand is exactly what the user is
    /// in the middle of doing.
    #[gpui::test]
    fn releasing_ctrl_over_the_workspace_list_keeps_the_panel_up(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));
        vcx.run_until_parked();

        let at = card_point(&mut vcx, Spot::Workspaces);
        vcx.simulate_mouse_move(at, None, Modifiers::control());
        vcx.simulate_modifiers_change(Modifiers::none());

        app.update(cx, |app, _| {
            let sw = app
                .switcher
                .as_ref()
                .expect("the panel stays up for the mouse to finish in");
            assert!(sw.hold.is_none(), "the hold is spent, not re-armed");
            assert_eq!(app.active, 0, "the release picked nothing");
        });
    }

    /// The pointer over the tab column is the ordinary gesture: release still
    /// commits.
    #[gpui::test]
    fn releasing_ctrl_over_the_tab_column_still_commits(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));
        vcx.run_until_parked();

        let at = card_point(&mut vcx, Spot::Tabs);
        vcx.simulate_mouse_move(at, None, Modifiers::control());
        vcx.simulate_modifiers_change(Modifiers::none());

        app.update(cx, |app, _| {
            assert!(app.switcher.is_none(), "the panel comes down on release");
            assert_eq!(app.active, 1, "the highlighted tab is now the active one");
        });
    }

    /// macOS reports Ctrl+click as a right click, so a tab row picked with
    /// the mouse mid-gesture arrives on the right button. The row takes that
    /// press as the pick; nothing between it and the window may swallow it
    /// first.
    #[gpui::test]
    fn ctrl_clicking_a_tab_row_mid_gesture_picks_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| {
            app.tab_switch(true, window, cx);
            // Two steps down, so the row the pointer lands on below is not
            // the one the keyboard had already reached.
            app.tab_switch(true, window, cx);
        });
        vcx.run_until_parked();
        app.update(cx, |app, _| {
            assert_eq!(app.switcher.as_ref().expect("up").right_sel, 2);
        });

        let at = card_point(&mut vcx, Spot::Tabs);
        vcx.simulate_mouse_move(at, None, Modifiers::control());
        vcx.simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::control());

        app.update(cx, |app, _| {
            let sw = app.switcher.as_ref().expect("the panel stays up");
            assert_eq!(
                sw.right_sel, 0,
                "the row under the pointer took the press, not the keyboard's row 2"
            );
            assert!(
                sw.hold.is_some(),
                "the gesture is still on until Ctrl is up"
            );
        });

        vcx.simulate_modifiers_change(Modifiers::none());
        app.update(cx, |app, _| {
            assert!(app.switcher.is_none(), "release commits and closes");
            assert_eq!(
                app.active, 0,
                "the first row of a most-recently-used column is this very tab"
            );
        });
    }

    #[gpui::test]
    fn losing_focus_drops_the_hold_so_the_panel_cannot_hang(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));

        vcx.deactivate_window();

        app.update(cx, |app, _| {
            let sw = app.switcher.as_ref().expect("the panel stays up");
            assert!(
                sw.hold.is_none(),
                "a release over another window never reaches us"
            );
        });
    }

    /// A bare Enter, as the panel's own key handler sees it — not through the
    /// input's bindings, which is why the event is built here rather than
    /// typed. Typing would also pump the executor, and this test is about the
    /// synchronous dispatch to `connect_to_host`.
    fn enter_event(modifiers: Modifiers) -> gpui::KeyDownEvent {
        gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers,
                key: "enter".into(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    /// One disconnected machine, scoped, with nothing under the cursor. The
    /// snapshot is enough to put the machine on the rail; a `Direct` target
    /// routes without touching the network.
    fn disconnected_machine(
        cx: &mut TestAppContext,
        column: crate::ui::switcher::Column,
    ) -> (
        gpui::Entity<crate::ui::app::Tty59App>,
        gpui::VisualTestContext,
        crate::core::session::RemoteTarget,
    ) {
        use crate::core::session::RemoteTarget;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let target = RemoteTarget::direct("me", "build-box", 22);
        app.update(cx, |app, _| {
            app.host_snapshots.insert(
                target.host_id(),
                super::HostSnapshot {
                    target: target.clone(),
                    rows: Vec::new(),
                },
            );
        });
        app.update_in(&mut vcx, |app, window, cx| app.open_switcher(window, cx));
        app.update(cx, |app, cx| {
            app.set_switcher_scope(super::MachineScope::Machine(target.to_string()), cx);
            if let Some(sw) = app.switcher.as_mut() {
                sw.column = column;
            }
        });
        (app, vcx, target)
    }

    /// Enter with no row selected on a machine that is not connected is the
    /// way to ask for the connection, not a no-op.
    #[gpui::test]
    fn bare_enter_on_a_disconnected_machine_asks_to_connect(cx: &mut TestAppContext) {
        use crate::ui::remote_workspace::ConnectFlow;

        let (app, mut vcx, target) = disconnected_machine(cx, super::Column::Left);
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.switcher_layout(cx).nav.is_empty(), "nothing to open");
            app.on_switcher_key(&enter_event(Modifiers::none()), window, cx);
        });

        app.update(cx, |app, _| {
            let Some(ConnectFlow::Connecting { choice }) = &app.connect else {
                panic!("bare Enter did not ask for the connection");
            };
            assert_eq!(
                choice.target, target,
                "the scoped machine is the one dialled"
            );
        });
    }

    /// The tab column can stay the active one after a query emptied the list,
    /// so the empty-nav fallback must run before the column-specific dispatch.
    #[gpui::test]
    fn bare_enter_with_the_tab_column_active_still_connects_an_empty_scope(
        cx: &mut TestAppContext,
    ) {
        use crate::ui::remote_workspace::ConnectFlow;

        let (app, mut vcx, target) = disconnected_machine(cx, super::Column::Right);
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.switcher_layout(cx).nav.is_empty(), "nothing to open");
            app.on_switcher_key(&enter_event(Modifiers::none()), window, cx);
        });

        app.update(cx, |app, _| {
            let Some(ConnectFlow::Connecting { choice }) = &app.connect else {
                panic!("the tab column swallowed the empty-nav Enter");
            };
            assert_eq!(choice.target, target);
        });
    }

    /// The new-window chord is a different gesture and keeps doing nothing
    /// when the list is empty: there is no window to open and no pick to take.
    #[gpui::test]
    fn secondary_enter_does_not_take_the_bare_connect_fallback(cx: &mut TestAppContext) {
        let (app, mut vcx, _target) = disconnected_machine(cx, super::Column::Left);
        app.update_in(&mut vcx, |app, window, cx| {
            app.on_switcher_key(&enter_event(Modifiers::secondary_key()), window, cx);
        });

        app.update(cx, |app, _| {
            assert!(
                app.connect.is_none(),
                "secondary Enter opened nothing and must not connect"
            );
        });
    }

    /// A layout the router can fail synchronously: the group looks connectable
    /// but its profile is not in the config, so `control_route` refuses before
    /// any link is attempted. This asserts the target `connect_to_host` was
    /// handed, not a copy the test made up.
    #[gpui::test]
    fn the_connect_fallback_hands_connect_to_host_the_scoped_target(cx: &mut TestAppContext) {
        use crate::core::session::RemoteTarget;
        use crate::ui::remote_workspace::ConnectFlow;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let target = RemoteTarget::Profile {
            id: uuid::Uuid::new_v4(),
        };
        let key = target.to_string();
        let mut group = super::tests::named_group("ghost-machine", vec![]);
        group.key = key.clone();
        group.target = Some(target.clone());
        group.link = super::Link::Offline;
        group.parked = false;
        let layout = super::Layout {
            groups: vec![group],
            nav: Vec::new(),
            rail: Vec::new(),
            scope: super::MachineScope::Machine(key),
        };

        app.update_in(&mut vcx, |app, window, cx| {
            app.open_switcher(window, cx);
            app.switcher_confirm(&layout, false, window, cx);
        });

        app.update(cx, |app, _| {
            let Some(ConnectFlow::Failed { choice, .. }) = &app.connect else {
                panic!("the fallback never reached connect_to_host");
            };
            assert_eq!(
                choice.target, target,
                "connect_to_host got exactly the scoped machine"
            );
        });
    }

    /// The synthetic layout above must not stand in for the real route
    /// classification: a profile that is gone is parked by `switcher_groups`,
    /// and a parked machine offers no connect card at all.
    #[gpui::test]
    fn a_profile_that_no_longer_resolves_is_parked_and_gets_no_connect_card(
        cx: &mut TestAppContext,
    ) {
        use crate::core::session::RemoteTarget;

        let (app, _vcx) = crate::ui::app::test_window::harness(cx);
        let target = RemoteTarget::Profile {
            id: uuid::Uuid::new_v4(),
        };
        app.update(cx, |app, cx| {
            app.host_snapshots.insert(
                target.host_id(),
                super::HostSnapshot {
                    target: target.clone(),
                    rows: Vec::new(),
                },
            );
            let groups = app.switcher_groups(cx);
            let group = groups
                .iter()
                .find(|g| g.target.as_ref() == Some(&target))
                .expect("the snapshot puts the machine on the rail");
            assert!(group.parked, "a profile that no longer resolves is parked");
            assert!(
                super::offline_connect_group(
                    &super::MachineScope::Machine(group.key.clone()),
                    &groups
                )
                .is_none(),
                "a parked machine must not offer a connect that can only fail"
            );
        });
    }

    /// The offline card carries the failure, so the scoped error band must not
    /// repeat it. Every other state keeps the band it had: All still shows it,
    /// and a parked or in-flight machine (which has no card) keeps its own.
    #[gpui::test]
    fn the_offline_card_replaces_the_scoped_error_band_only(cx: &mut TestAppContext) {
        let (app, _vcx) = crate::ui::app::test_window::harness(cx);
        let machine = |link, error: Option<&str>, parked| {
            let mut group = super::tests::named_group("devbox", vec![]);
            group.link = link;
            group.error = error.map(str::to_string);
            group.parked = parked;
            group
        };
        let scoped = |group| super::Layout {
            groups: vec![group],
            nav: Vec::new(),
            rail: Vec::new(),
            scope: super::MachineScope::Machine("devbox".into()),
        };

        app.update(cx, |app, cx| {
            let failed = || machine(super::Link::Failed, Some("connection refused"), false);
            assert!(
                app.render_banners(&scoped(failed()), cx).is_empty(),
                "the offline card already names the failure"
            );
            let all = super::Layout {
                scope: super::MachineScope::All,
                ..scoped(failed())
            };
            assert_eq!(
                app.render_banners(&all, cx).len(),
                1,
                "All keeps the error band it always had"
            );
            assert_eq!(
                app.render_banners(
                    &scoped(machine(
                        super::Link::Failed,
                        Some("connection refused"),
                        true
                    )),
                    cx
                )
                .len(),
                1,
                "a parked machine has no card, so its banner stays"
            );
            assert_eq!(
                app.render_banners(&scoped(machine(super::Link::Connecting, None, false)), cx)
                    .len(),
                1,
                "a connect in flight has no card and keeps its own band"
            );
        });
    }
}

#[cfg(test)]
mod key_tests {
    use gpui::Modifiers;

    use super::{Column, Key, key_intent};

    #[test]
    fn bare_arrows_and_enter_drive_the_panel() {
        let none = Modifiers::none();
        assert_eq!(key_intent("down", none), Key::Step(true));
        assert_eq!(key_intent("up", none), Key::Step(false));
        assert_eq!(key_intent("left", none), Key::ToColumn(Column::Left));
        assert_eq!(key_intent("right", none), Key::ToColumn(Column::Right));
        assert_eq!(key_intent("enter", none), Key::Confirm(false));
        assert_eq!(key_intent("escape", none), Key::Close);
    }

    #[test]
    fn the_secondary_modifier_turns_enter_into_a_new_window() {
        // ⌘ on macOS, Ctrl everywhere else. Off macOS this is the case that a
        // blanket "control means not ours" check would have swallowed.
        assert_eq!(
            key_intent("enter", Modifiers::secondary_key()),
            Key::Confirm(true)
        );
    }

    #[test]
    fn tab_walks_the_tab_column_in_both_directions() {
        assert_eq!(key_intent("tab", Modifiers::none()), Key::Tab(true));
        assert_eq!(key_intent("tab", Modifiers::shift()), Key::Tab(false));
    }

    #[test]
    fn a_held_control_keeps_tab_working_but_parks_the_arrows() {
        // Mid Ctrl+Tab gesture: Tab still steps, but an arrow key is somebody
        // else's chord. Only macOS sees the raw key here — everywhere else
        // Ctrl is the secondary modifier, the chord arrives as the NextTab
        // action instead, and the raw key must fall through untouched.
        let ctrl = Modifiers::control();
        match cfg!(target_os = "macos") {
            true => assert_eq!(key_intent("tab", ctrl), Key::Tab(true)),
            false => assert_eq!(key_intent("tab", ctrl), Key::Pass),
        }
        assert_eq!(key_intent("up", ctrl), Key::Pass);
    }

    #[test]
    fn alt_arrows_step_the_machine_rail_in_both_directions() {
        // Alt+←/→ is the rail's gesture, classified ahead of the Alt
        // pass-through — the price is the search box's word jumps.
        assert_eq!(key_intent("left", Modifiers::alt()), Key::Machine(-1));
        assert_eq!(key_intent("right", Modifiers::alt()), Key::Machine(1));
    }

    #[test]
    fn alt_arrows_with_company_are_somebody_elses_chord() {
        // The rail wants Alt alone. Fn riding along is the macOS
        // function-key family, Shift selects, Ctrl and the secondary belong
        // elsewhere — every one of those still passes through untouched, so
        // the search box only ever loses exactly two chords.
        let alt = Modifiers::alt();
        assert_eq!(
            key_intent(
                "left",
                Modifiers {
                    function: true,
                    ..alt
                }
            ),
            Key::Pass
        );
        assert_eq!(
            key_intent("right", Modifiers { shift: true, ..alt }),
            Key::Pass
        );
        assert_eq!(
            key_intent(
                "left",
                Modifiers {
                    control: true,
                    ..alt
                }
            ),
            Key::Pass
        );
        assert_eq!(
            key_intent(
                "right",
                Modifiers {
                    alt: true,
                    ..Modifiers::secondary_key()
                }
            ),
            Key::Pass
        );
        // Fn on its own keeps its arrows too.
        assert_eq!(key_intent("left", Modifiers::function()), Key::Pass);
        assert_eq!(key_intent("right", Modifiers::function()), Key::Pass);
    }

    #[test]
    fn alt_and_fn_chords_are_left_alone() {
        assert_eq!(key_intent("down", Modifiers::alt()), Key::Pass);
        assert_eq!(key_intent("enter", Modifiers::alt()), Key::Pass);
    }

    #[test]
    fn escape_closes_even_mid_chord() {
        assert_eq!(key_intent("escape", Modifiers::alt()), Key::Close);
        assert_eq!(key_intent("escape", Modifiers::secondary_key()), Key::Close);
    }

    #[test]
    fn the_secondary_glyph_matches_the_platform() {
        let glyph = crate::ui::keymap::secondary_glyph();
        match cfg!(target_os = "macos") {
            true => assert_eq!(glyph, "⌘"),
            false => assert_eq!(glyph, "Ctrl"),
        }
    }
}
