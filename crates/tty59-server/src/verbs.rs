//! Pane verbs for an agent running *on* this machine, in one of its panes.
//!
//! A remote machine, a VM or a container that hosts tty59 workspaces gets only
//! this binary: the `tty59` CLI never goes there. An agent working in one of
//! those panes still needs to open a tab, split, type into a pane it started,
//! read it back and close it. These verbs are that, and no more.
//!
//! Every verb is scoped the way the host's `tty59s` helper is: to the caller's
//! own workspace (the one holding `$TTY59_PANE`) and, within it, to the tabs in
//! the caller's sidebar group (or every ungrouped tab, when the caller's tab
//! has no group). A pane outside that scope is refused by name, and the verbs
//! that type or close never take the caller's own pane.
//!
//! The scope is recomputed from the server's tree on every call, because the
//! person can move the caller's tab at any time.
//!
//! Hand-parsed on purpose: this binary's one dependency is `tty59-core`.

use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use tty59_core::client::{ControlClient, PaneClient};
use tty59_core::core::group_key::GroupId;
use tty59_core::core::machine::{Axis, Machine, PaneSeed, Tab, Workspace};
use tty59_core::core::send_keys;
use tty59_core::core::session::WorkspaceId;
use tty59_core::daemon::control::{ControlHello, ControlRequest, ReplyOk};
use tty59_core::daemon::protocol::{DaemonMsg, PaneProcs, WinSize};

/// The first word of each verb this module answers. `main` dispatches here
/// only on one of these, so every existing `--flag` invocation is untouched.
pub const VERBS: &[&str] = &[
    "scope", "ls", "tab", "split", "send", "capture", "procs", "close", "wait",
];

pub const USAGE: &str = "\
PANE VERBS (for an agent inside one of this machine's panes; scoped to its
workspace and sidebar group, recomputed on every call):
    tty59-server scope                         workspace, group and panes in scope
    tty59-server ls                            panes in scope; * marks your own
    tty59-server tab new [--cwd DIR]           new tab with a shell, in your group
    tty59-server tab close %PANE               close the tab holding %PANE
    tty59-server split [%PANE] --v|--h         split a pane (yours by default)
    tty59-server send %PANE [TEXT] [--enter] [--key KEY]... [--stdin]
    tty59-server capture [%PANE] [--tail N] [--raw]
    tty59-server procs %PANE                   process tree and where its shell is
    tty59-server wait %PANE [--until free|exit] [--timeout SECS]
    tty59-server close %PANE...                close panes
";

const SESSION_SIZE: WinSize = WinSize {
    cols: 120,
    rows: 30,
    cell_w: 8,
    cell_h: 16,
};
const REPLAY_FIRST_WAIT: Duration = Duration::from_secs(10);
const REPLAY_SETTLE: Duration = Duration::from_millis(300);
const WAIT_INTERVAL: Duration = Duration::from_millis(500);

/// A failure with the exit code it should end the process with.
struct Fail(u8, String);

type Res<T> = Result<T, Fail>;

fn usage(msg: impl Into<String>) -> Fail {
    Fail(2, msg.into())
}

fn failed(msg: impl Into<String>) -> Fail {
    Fail(1, msg.into())
}

fn io_fail(what: &str) -> impl FnOnce(io::Error) -> Fail + '_ {
    move |e| failed(format!("{what}: {e}"))
}

pub fn run(args: &[String]) -> ExitCode {
    match dispatch(args) {
        Ok(code) => code,
        Err(Fail(code, msg)) => {
            eprintln!("tty59-server: {msg}");
            ExitCode::from(code)
        }
    }
}

fn dispatch(args: &[String]) -> Res<ExitCode> {
    let verb = args[0].as_str();
    let rest = &args[1..];
    if rest.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let mut cx = Cx::connect()?;
    let scope = cx.scope()?;
    match verb {
        "scope" => scope_cmd(&scope),
        "ls" => ls(&scope, &cx.machine),
        "tab" => match rest.first().map(String::as_str) {
            Some("new") => tab_new(&mut cx, &scope, &rest[1..]),
            Some("close") => tab_close(&mut cx, &scope, &rest[1..]),
            _ => Err(usage(
                "`tab` takes `new [--cwd DIR]` or `close %PANE`; the other tab verbs reach \
                 outside your scope",
            )),
        },
        "split" => split(&mut cx, &scope, rest),
        "send" => send(&mut cx, &scope, rest),
        "capture" => capture(&mut cx, &scope, rest),
        "procs" => procs(&mut cx, &scope, rest),
        "wait" => wait(&mut cx, &scope, rest),
        "close" => close(&mut cx, &scope, rest),
        other => Err(usage(format!("unknown verb `{other}`"))),
    }
}

/// The connections, opened once per call.
struct Cx {
    control: ControlClient,
    panes: PaneClient,
    machine: Machine,
}

impl Cx {
    fn connect() -> Res<Cx> {
        let hello = ControlHello::host_rpc(
            format!("tty59-server-verbs-{}", std::process::id()),
            std::env::var("HOSTNAME").unwrap_or_else(|_| "tty59-server".to_string()),
        );
        let control = ControlClient::connect(&hello).map_err(|e| {
            failed(format!(
                "cannot reach the tty59 server on this machine ({e}); these verbs only work \
                 inside one of its panes, with $TTY59_CONFIG_DIR as the pane set it"
            ))
        })?;
        let mut cx = Cx {
            control,
            panes: PaneClient::local(),
            machine: Machine::default(),
        };
        cx.refresh()?;
        Ok(cx)
    }

    fn refresh(&mut self) -> Res<()> {
        self.machine = match self.request(ControlRequest::MachineGet)? {
            ReplyOk::MachineTree(m) => *m,
            other => {
                return Err(failed(format!(
                    "the server answered MachineGet with {other:?}"
                )));
            }
        };
        Ok(())
    }

    fn request(&self, req: ControlRequest) -> Res<ReplyOk> {
        self.control
            .request(req)
            .map_err(|e| failed(format!("the server refused the request: {e}")))
    }

    fn scope(&self) -> Res<Scope> {
        Scope::of(&self.machine, caller()?)
    }

    fn spawn_shell(&self, workspace: WorkspaceId, cwd: Option<String>) -> Res<u64> {
        let ws = workspace.to_string();
        let session = self
            .panes
            .spawn(
                cwd.map(PathBuf::from),
                SESSION_SIZE,
                None,
                Some(ws.clone()),
                Some(ws),
            )
            .map_err(io_fail("spawning a shell"))?;
        let pane = session.pane_id();
        session
            .detach()
            .map_err(io_fail("detaching from the new shell"))?;
        Ok(pane)
    }

    /// Hang up the panes a close removed from the tree, the way the CLI does:
    /// the tree forgets them, the daemon would otherwise keep them running.
    fn hang_up(&self, request: &str, reply: ReplyOk) -> Res<()> {
        let panes = match reply {
            ReplyOk::Panes(panes) => panes,
            other => {
                return Err(failed(format!(
                    "the server answered {request} with {other:?}"
                )));
            }
        };
        let mut failures = Vec::new();
        for pane in panes {
            if let Err(e) = self.panes.kill(pane) {
                failures.push(format!("%{pane}: {e}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failed(format!(
                "could not hang up pane(s) removed by {request}: {}",
                failures.join("; ")
            )))
        }
    }
}

fn caller() -> Res<u64> {
    let raw = std::env::var("TTY59_PANE").map_err(|_| {
        failed("$TTY59_PANE is not set: these verbs work only inside a tty59 pane on this machine")
    })?;
    parse_pane(&raw).map_err(|_| failed(format!("$TTY59_PANE holds {raw:?}, not a pane id")))
}

fn parse_pane(s: &str) -> Result<u64, ()> {
    s.strip_prefix('%').unwrap_or(s).parse().map_err(|_| ())
}

/// What the caller may touch.
struct Scope {
    me: u64,
    workspace: WorkspaceId,
    workspace_name: Option<String>,
    group: Option<GroupId>,
    group_name: Option<String>,
    /// The tabs in scope, in workspace order.
    tabs: Vec<Tab>,
}

impl Scope {
    fn of(machine: &Machine, me: u64) -> Res<Scope> {
        let (ws, mine) = machine
            .workspaces
            .iter()
            .find_map(|ws| {
                ws.tabs
                    .iter()
                    .find(|t| t.root.contains(me))
                    .map(|t| (ws, t))
            })
            .ok_or_else(|| {
                failed(format!(
                    "your pane %{me} is in no tab of this machine's server — is \
                     $TTY59_CONFIG_DIR the one this pane was started with?"
                ))
            })?;
        Ok(Scope {
            me,
            workspace: ws.id,
            workspace_name: ws.name.clone(),
            group: mine.group,
            group_name: group_name(ws, mine.group),
            tabs: ws
                .tabs
                .iter()
                .filter(|t| t.group == mine.group)
                .cloned()
                .collect(),
        })
    }

    fn panes(&self) -> Vec<u64> {
        self.tabs.iter().flat_map(|t| t.root.pane_ids()).collect()
    }

    fn describe(&self) -> String {
        let ws = self
            .workspace_name
            .clone()
            .unwrap_or_else(|| short(&self.workspace.to_string()));
        match (&self.group, &self.group_name) {
            (Some(_), Some(name)) => format!("workspace {ws:?}, group {name:?}"),
            (Some(id), None) => format!("workspace {ws:?}, group {}", short(&id.to_string())),
            (None, _) => format!("workspace {ws:?}, ungrouped tabs"),
        }
    }

    fn listing(&self) -> String {
        self.panes()
            .iter()
            .map(|p| format!("%{p}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Resolve an address the caller gave, refusing anything out of scope.
    fn target(&self, raw: &str) -> Res<u64> {
        if raw.starts_with('@') {
            return Err(usage(format!(
                "tab address {raw:?} refused: name a pane (%N) instead; `ls` shows them"
            )));
        }
        let pane = parse_pane(raw).map_err(|_| usage(format!("{raw:?} is not a pane address")))?;
        if !self.panes().contains(&pane) {
            return Err(usage(format!(
                "pane %{pane} is outside your scope ({}); in scope: {}",
                self.describe(),
                self.listing()
            )));
        }
        Ok(pane)
    }

    /// Like [`Scope::target`], and never the caller's own pane: typing into
    /// it would write onto the caller's own screen, closing it would end the
    /// caller.
    fn other(&self, raw: &str, verb: &str) -> Res<u64> {
        let pane = self.target(raw)?;
        if pane == self.me {
            return Err(usage(format!(
                "`{verb}` on your own pane (%{pane}) is refused; name another pane"
            )));
        }
        Ok(pane)
    }

    fn tab_of(&self, pane: u64) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.root.contains(pane))
    }
}

fn group_name(ws: &Workspace, group: Option<GroupId>) -> Option<String> {
    ws.groups.get(group?)?.name.clone()
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

fn is_address(s: &str) -> bool {
    s.starts_with('@')
        || s.strip_prefix('%')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn scope_cmd(scope: &Scope) -> Res<ExitCode> {
    println!("me        %{}", scope.me);
    println!(
        "workspace {} ({})",
        scope.workspace_name.as_deref().unwrap_or("-"),
        scope.workspace
    );
    match scope.group {
        Some(id) => println!(
            "group     {} ({id})",
            scope.group_name.as_deref().unwrap_or("-")
        ),
        None => println!("group     (ungrouped)"),
    }
    println!("panes     {}", scope.listing());
    Ok(ExitCode::SUCCESS)
}

fn ls(scope: &Scope, machine: &Machine) -> Res<ExitCode> {
    println!("# {}", scope.describe());
    println!("PANE\tSELF\tLIVE\tTAB\tAGENT\tCWD\tTITLE");
    for tab in &scope.tabs {
        let tab_name = tab
            .name
            .clone()
            .unwrap_or_else(|| short(&tab.id.to_string()));
        for pane in tab.root.pane_ids() {
            let rec = machine.panes.iter().find(|p| p.id == pane);
            let agent = rec
                .and_then(|r| r.agent.as_ref())
                .map(|a| {
                    let status = a
                        .status
                        .map(|s| format!("{s:?}").to_ascii_lowercase())
                        .unwrap_or_else(|| "-".into());
                    format!("{}/{status}", a.agent.slug())
                })
                .unwrap_or_else(|| "-".into());
            println!(
                "%{pane}\t{}\t{}\t{tab_name}\t{agent}\t{}\t{}",
                if pane == scope.me { "*" } else { "" },
                if rec.is_some_and(|r| r.live) {
                    "live"
                } else {
                    "dead"
                },
                rec.and_then(|r| r.cwd.as_deref()).unwrap_or("-"),
                rec.map(|r| r.osc_title.as_deref().unwrap_or(&r.title))
                    .unwrap_or(""),
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn tab_new(cx: &mut Cx, scope: &Scope, args: &[String]) -> Res<ExitCode> {
    let mut cwd = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--cwd" {
            cwd = Some(
                it.next()
                    .ok_or_else(|| usage("--cwd needs a directory"))?
                    .clone(),
            );
        } else if let Some(v) = a.strip_prefix("--cwd=") {
            cwd = Some(v.to_string());
        } else {
            return Err(usage(format!(
                "`tab new` takes only --cwd here (the workspace and group are yours); got {a:?}"
            )));
        }
    }
    let pane = cx.spawn_shell(scope.workspace, cwd.clone())?;
    let tab = match cx.request(ControlRequest::TabCreate {
        workspace: scope.workspace,
        at: None,
        pane: seed(pane, cwd),
        tab: None,
    })? {
        ReplyOk::TabTree(tab) => *tab,
        other => {
            return Err(failed(format!(
                "the server answered TabCreate with {other:?}"
            )));
        }
    };
    if let Some(group) = scope.group {
        match cx.request(ControlRequest::TabSetGroup {
            workspace: scope.workspace,
            tab: tab.id,
            group: Some(group),
            last_auto: None,
        })? {
            ReplyOk::Unit => {}
            other => {
                return Err(failed(format!(
                    "tab {} was created (pane %{pane}) but not filed in your group: the server \
                     answered TabSetGroup with {other:?}",
                    tab.id
                )));
            }
        }
    }
    println!("%{pane}");
    Ok(ExitCode::SUCCESS)
}

fn tab_close(cx: &mut Cx, scope: &Scope, args: &[String]) -> Res<ExitCode> {
    let [raw] = args else {
        return Err(usage(
            "`tab close` takes one %PANE: the tab holding it is closed",
        ));
    };
    let pane = scope.other(raw, "tab close")?;
    let tab = scope
        .tab_of(pane)
        .expect("a pane in scope is in a scoped tab");
    if tab.root.contains(scope.me) {
        return Err(usage(format!(
            "the tab holding %{pane} also holds your own pane (%{}); close %{pane} alone \
             with `close %{pane}`",
            scope.me
        )));
    }
    let reply = cx.request(ControlRequest::TabClose {
        workspace: scope.workspace,
        tab: tab.id,
    })?;
    cx.hang_up("TabClose", reply)?;
    Ok(ExitCode::SUCCESS)
}

fn split(cx: &mut Cx, scope: &Scope, args: &[String]) -> Res<ExitCode> {
    let mut target = None;
    let mut axis = None;
    for a in args {
        match a.as_str() {
            "--v" | "--vertical" => axis = Some(Axis::Vertical),
            "--h" | "--horizontal" => axis = Some(Axis::Horizontal),
            a if target.is_none() && !a.starts_with("--") => target = Some(a),
            a => return Err(usage(format!("`split` does not take {a:?}"))),
        }
    }
    let axis = axis.ok_or_else(|| usage("`split` needs an axis: --v (below) or --h (right)"))?;
    let pane = match target {
        Some(raw) => scope.target(raw)?,
        None => scope.me,
    };
    let cwd = cx
        .machine
        .panes
        .iter()
        .find(|p| p.id == pane)
        .and_then(|p| p.cwd.clone());
    let new = cx.spawn_shell(scope.workspace, cwd.clone())?;
    cx.request(ControlRequest::PaneSplit {
        workspace: scope.workspace,
        pane,
        axis,
        ratio: 0.5,
        new: seed(new, cwd),
        first: false,
    })?;
    println!("%{new}");
    Ok(ExitCode::SUCCESS)
}

fn seed(pane: u64, cwd: Option<String>) -> PaneSeed {
    PaneSeed {
        pane,
        cwd,
        ssh_spec: None,
        agent: None,
        shell: None,
    }
}

fn send(cx: &mut Cx, scope: &Scope, args: &[String]) -> Res<ExitCode> {
    let mut target = None;
    let mut text: Option<Vec<u8>> = None;
    let mut keys = Vec::new();
    let mut stdin = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--enter" => keys.push(send_keys::parse("enter").expect("enter is a key")),
            "--key" => {
                let name = it.next().ok_or_else(|| usage("--key needs a key name"))?;
                keys.push(send_keys::parse(name).map_err(usage)?);
            }
            "--stdin" => stdin = true,
            "--" => {
                let rest: Vec<&str> = it.by_ref().map(String::as_str).collect();
                text = Some(rest.join(" ").into_bytes());
            }
            a if target.is_none() && is_address(a) => target = Some(a.to_string()),
            a if a.starts_with("--") => return Err(usage(format!("`send` does not take {a:?}"))),
            a if text.is_none() => text = Some(a.as_bytes().to_vec()),
            a => {
                return Err(usage(format!(
                    "`send` takes one TEXT; quote it as a single argument ({a:?} is extra)"
                )));
            }
        }
    }
    let raw = target.ok_or_else(|| usage("`send` needs an explicit %PANE"))?;
    let pane = scope.other(&raw, "send")?;
    if stdin {
        if text.is_some() {
            return Err(usage("--stdin and TEXT ask for two different things"));
        }
        let mut buf = Vec::new();
        io::stdin()
            .read_to_end(&mut buf)
            .map_err(io_fail("reading stdin"))?;
        text = Some(buf);
    }
    if text.is_none() && keys.is_empty() {
        return Err(usage(
            "nothing to send: give TEXT, --stdin, --enter or --key",
        ));
    }
    if let Some(bytes) = text.filter(|b| !b.is_empty()) {
        cx.panes
            .send_input(pane, &bytes)
            .map_err(io_fail("sending input"))?;
    }
    // Each key as its own write, 200 ms apart after the first: a raw-mode TUI
    // reads a burst as a paste, which is what the CLI's own `send` avoids too.
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(200));
        }
        cx.panes
            .send_input(pane, &key.bytes)
            .map_err(io_fail("sending a key"))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn capture(cx: &mut Cx, scope: &Scope, args: &[String]) -> Res<ExitCode> {
    let mut target = None;
    let mut tail = None;
    let mut raw = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--raw" => raw = true,
            // The CLI's spelling; text is already the default here.
            "--plain" | "--scrollback" => {}
            "--tail" => {
                let n = it
                    .next()
                    .ok_or_else(|| usage("--tail needs a line count"))?;
                tail = Some(parse_count(n)?);
            }
            a if a.starts_with("--tail=") => tail = Some(parse_count(&a[7..])?),
            a if target.is_none() && !a.starts_with("--") => target = Some(a),
            a => return Err(usage(format!("`capture` does not take {a:?}"))),
        }
    }
    let pane = match target {
        Some(t) => scope.target(t)?,
        None => scope.me,
    };
    let bytes = replay(&cx.panes, pane)?;
    let text = if raw {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        to_text(&bytes)
    };
    let text = match tail {
        Some(n) => {
            let lines: Vec<&str> = text.lines().collect();
            lines[lines.len().saturating_sub(n)..].join("\n")
        }
        None => text,
    };
    let mut out = io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    if !text.ends_with('\n') && !text.is_empty() {
        let _ = out.write_all(b"\n");
    }
    Ok(ExitCode::SUCCESS)
}

fn parse_count(s: &str) -> Res<usize> {
    match s.parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(usage(format!(
            "--tail takes a line count of at least 1, not {s:?}"
        ))),
    }
}

/// Every segment of the pane's scrollback ring, concatenated.
fn replay(panes: &PaneClient, pane: u64) -> Res<Vec<u8>> {
    let mut session = panes
        .observe(pane, SESSION_SIZE)
        .map_err(io_fail("observing the pane"))?;
    let _ = session.set_recv_timeout(Some(REPLAY_FIRST_WAIT));
    let mut bytes = Vec::new();
    loop {
        match session.recv() {
            Ok(DaemonMsg::Snapshot(chunk)) => {
                if !bytes.is_empty() && !bytes.ends_with(b"\n") {
                    bytes.push(b'\n');
                }
                bytes.extend_from_slice(&chunk);
                let _ = session.set_recv_timeout(Some(REPLAY_SETTLE));
            }
            Ok(DaemonMsg::Output(_)) | Ok(DaemonMsg::Exited { .. }) => break,
            Ok(_) => {
                let _ = session.set_recv_timeout(Some(REPLAY_SETTLE));
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::UnexpectedEof
                ) =>
            {
                break;
            }
            Err(e) => return Err(failed(format!("reading the pane replay: {e}"))),
        }
    }
    let _ = session.detach();
    Ok(bytes)
}

/// A pane's bytes as text, without a terminal emulator.
///
/// This binary has no VT engine, so this is a line-level reading: escape
/// sequences are dropped, a carriage return goes back to the start of the
/// line and later text overwrites it (so a progress bar reads as its last
/// value), backspace steps back, and erase-to-end-of-line is honoured. A
/// full-screen program, which places text by cursor address, will not read
/// well; for those, `--raw` gives the bytes.
pub fn to_text(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out: Vec<String> = Vec::new();
    let mut line: Vec<char> = Vec::new();
    let mut col = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                // CSI: parameters, then one final byte in @..~.
                Some('[') => {
                    let mut params = String::new();
                    for p in chars.by_ref() {
                        if ('@'..='~').contains(&p) {
                            if p == 'K' {
                                let mode = params.trim_start_matches('?');
                                if mode.is_empty() || mode == "0" {
                                    line.truncate(col);
                                } else if mode == "2" {
                                    line.clear();
                                }
                            }
                            break;
                        }
                        params.push(p);
                    }
                }
                // OSC, DCS, APC, PM, SOS: until BEL or ST.
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    while let Some(p) = chars.next() {
                        if p == '\x07' {
                            break;
                        }
                        if p == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Charset designations take one more character.
                Some('(' | ')' | '*' | '+') => {
                    chars.next();
                }
                _ => {}
            },
            '\r' => col = 0,
            '\n' => {
                out.push(line.iter().collect::<String>().trim_end().to_string());
                line.clear();
                col = 0;
            }
            '\x08' => col = col.saturating_sub(1),
            '\t' => {
                let next = (col / 8 + 1) * 8;
                while line.len() < next {
                    line.push(' ');
                }
                col = next;
            }
            c if c.is_control() => {}
            c => {
                while line.len() < col {
                    line.push(' ');
                }
                if col < line.len() {
                    line[col] = c;
                } else {
                    line.push(c);
                }
                col += 1;
            }
        }
    }
    if !line.is_empty() {
        out.push(line.iter().collect::<String>().trim_end().to_string());
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let first = out.iter().position(|l| !l.is_empty()).unwrap_or(out.len());
    out[first..].join("\n")
}

fn procs(cx: &mut Cx, scope: &Scope, args: &[String]) -> Res<ExitCode> {
    let [raw] = args else {
        return Err(usage("`procs` takes one %PANE"));
    };
    let pane = scope.target(raw)?;
    let answer = cx
        .panes
        .procs(pane)
        .map_err(io_fail("reading the processes"))?;
    for p in &answer.procs {
        println!(
            "{}{}{} ({})",
            "  ".repeat(p.depth as usize),
            if p.foreground { "* " } else { "" },
            p.name,
            p.pid
        );
    }
    for port in &answer.ports {
        println!("port {} {} ({})", port.port, port.name, port.pid);
    }
    println!("shell: {}", where_the_shell_is(&answer));
    Ok(ExitCode::SUCCESS)
}

/// One line on how the pane can be driven, which is what an agent reading
/// `procs` actually needs to decide between `send` and waiting for a prompt.
fn where_the_shell_is(answer: &PaneProcs) -> String {
    let Some(ctx) = &answer.context else {
        return "unknown (this server is too old to say)".into();
    };
    match &ctx.remote {
        Some(remote) if !ctx.remote_prompt_seen => format!(
            "ssh to {} — the far shell sends no prompt marks: drive it with `send`, read it \
             with `capture`",
            remote.target
        ),
        Some(remote) => format!(
            "ssh to {}, far shell {}",
            remote.target,
            prompt_word(ctx.at_prompt)
        ),
        None => format!("local, {}", prompt_word(ctx.at_prompt)),
    }
}

fn prompt_word(at_prompt: Option<bool>) -> &'static str {
    match at_prompt {
        Some(true) => "at its prompt",
        Some(false) => "running a command",
        None => "no prompt marks yet",
    }
}

enum Freeness {
    Free,
    Busy,
    Unknown(String),
}

/// The CLI's `wait --until free` reading, minus what only it needs.
fn freeness(answer: &PaneProcs) -> Freeness {
    let bare = !answer.procs.is_empty() && answer.procs.iter().all(|p| p.depth == 0);
    let Some(ctx) = &answer.context else {
        return if bare { Freeness::Free } else { Freeness::Busy };
    };
    if let Some(remote) = &ctx.remote {
        return match ctx.at_prompt {
            Some(true) => Freeness::Free,
            _ if ctx.local_pty && bare => Freeness::Free,
            Some(false) if ctx.remote_prompt_seen => Freeness::Busy,
            _ => Freeness::Unknown(format!(
                "the pane is an ssh session to {} whose far shell sends no prompt marks, so \
                 nothing here can tell an idle prompt from a running command. Have the command \
                 print a marker (`; echo DONE-$?`) and poll `capture --tail 5` for it",
                remote.target
            )),
        };
    }
    if bare || ctx.at_prompt == Some(true) {
        Freeness::Free
    } else {
        Freeness::Busy
    }
}

fn wait(cx: &mut Cx, scope: &Scope, args: &[String]) -> Res<ExitCode> {
    let mut target = None;
    let mut until = "free".to_string();
    let mut timeout = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--until" => {
                until = it
                    .next()
                    .ok_or_else(|| usage("--until needs a state"))?
                    .clone()
            }
            "--timeout" => {
                let s = it.next().ok_or_else(|| usage("--timeout needs seconds"))?;
                timeout =
                    Some(Duration::from_secs(s.parse().map_err(|_| {
                        usage(format!("--timeout takes seconds, not {s:?}"))
                    })?));
            }
            a if target.is_none() && !a.starts_with("--") => target = Some(a),
            a => return Err(usage(format!("`wait` does not take {a:?}"))),
        }
    }
    if until != "free" && until != "exit" {
        return Err(usage(format!(
            "--until takes `free` or `exit` here, not {until:?}; agent states need the host's \
             `tty59 wait`"
        )));
    }
    let raw = target.ok_or_else(|| usage("`wait` needs an explicit %PANE"))?;
    let pane = scope.other(raw, "wait")?;
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        let alive = cx
            .panes
            .list()
            .map_err(io_fail("listing panes"))?
            .iter()
            .any(|p| p.pane_id == pane && p.alive);
        if !alive {
            println!("exit");
            return Ok(if until == "exit" {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            });
        }
        if until == "free" {
            let answer = cx
                .panes
                .procs(pane)
                .map_err(io_fail("reading the processes"))?;
            match freeness(&answer) {
                Freeness::Free => {
                    println!("free");
                    return Ok(ExitCode::SUCCESS);
                }
                Freeness::Busy => {}
                Freeness::Unknown(why) => return Err(failed(why)),
            }
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            eprintln!("tty59-server: timed out waiting for %{pane} to be {until}");
            return Ok(ExitCode::from(124));
        }
        std::thread::sleep(WAIT_INTERVAL);
    }
}

fn close(cx: &mut Cx, scope: &Scope, args: &[String]) -> Res<ExitCode> {
    if args.is_empty() {
        return Err(usage("`close` needs at least one %PANE"));
    }
    let panes = args
        .iter()
        .map(|a| scope.other(a, "close"))
        .collect::<Res<Vec<_>>>()?;
    let mut failures = Vec::new();
    for pane in panes {
        let outcome = cx
            .request(ControlRequest::PaneClose {
                workspace: scope.workspace,
                pane,
            })
            .and_then(|reply| cx.hang_up("PaneClose", reply));
        if let Err(Fail(_, msg)) = outcome {
            failures.push(format!("%{pane}: {msg}"));
        }
    }
    if failures.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(failed(failures.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_drops_escapes_and_keeps_lines() {
        let raw = b"\x1b]133;A\x07$ \x1b[1;32mls\x1b[0m\r\nfoo  bar\r\n\x1b]133;D;0\x07";
        assert_eq!(to_text(raw), "$ ls\nfoo  bar");
    }

    #[test]
    fn a_carriage_return_overwrites_like_a_terminal() {
        assert_eq!(to_text(b"10%\r50%\r100%\n"), "100%");
        // A shorter rewrite that clears the rest of the line.
        assert_eq!(to_text(b"long line\rok\x1b[K\n"), "ok");
    }

    #[test]
    fn backspace_and_tabs() {
        assert_eq!(to_text(b"ab\x08c\n"), "ac");
        assert_eq!(to_text(b"a\tb\n"), "a       b");
    }

    #[test]
    fn st_terminated_osc_is_dropped() {
        assert_eq!(to_text(b"\x1b]0;title\x1b\\hello\n"), "hello");
    }

    #[test]
    fn addresses() {
        assert!(is_address("%12"));
        assert!(is_address("@3"));
        assert!(!is_address("%s/foo/bar/"));
        assert!(!is_address("12"));
        assert_eq!(parse_pane("%12"), Ok(12));
        assert_eq!(parse_pane("12"), Ok(12));
    }

    fn ssh(remote_prompt_seen: bool, at_prompt: Option<bool>) -> PaneProcs {
        use tty59_core::daemon::protocol::{PaneContext, ProcEntry, RemoteContext, RemoteKind};
        let entry = |pid, name: &str, depth| ProcEntry {
            pid,
            name: name.into(),
            depth,
            foreground: depth == 1,
            ..Default::default()
        };
        PaneProcs {
            procs: vec![entry(1, "bash", 0), entry(2, "ssh", 1)],
            ports: Vec::new(),
            probe: Default::default(),
            context: Some(PaneContext {
                remote: Some(RemoteContext {
                    kind: RemoteKind::Ssh,
                    argv: vec!["ssh".into(), "vm".into()],
                    target: "vm".into(),
                }),
                local_pty: true,
                at_prompt,
                remote_prompt_seen,
                bracketed_paste: None,
            }),
        }
    }

    #[test]
    fn a_bare_ssh_session_is_unknown_and_says_to_send() {
        assert!(matches!(
            freeness(&ssh(false, Some(false))),
            Freeness::Unknown(_)
        ));
        assert!(where_the_shell_is(&ssh(false, Some(false))).contains("drive it with `send`"));
        assert!(matches!(freeness(&ssh(true, Some(false))), Freeness::Busy));
        assert!(matches!(freeness(&ssh(true, Some(true))), Freeness::Free));
    }
}
