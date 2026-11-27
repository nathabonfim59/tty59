---
name: tty59-remote
description: >-
  Manage tty59 panes from inside a remote machine, VM or container that tty59 hosts workspaces on: open a tab, split, type into a pane, read its screen, wait for a command, close what you opened. Use whenever $TTY59_PANE is set and the `tty59` CLI is not on PATH, which is the case on any machine that has only tty59-server, or when the user asks for "a new tab", "another pane", or "run this in the other terminal" from such a machine. Also covers panes that are a plain `ssh` session, where commands must be typed with `send` and read back with `capture`.
---

# tty59 on this machine: panes in your own scope

This machine runs `tty59-server`, which owns the workspaces, tabs and panes that
the user's tty59 window shows for it. You are in one of those panes:
`$TTY59_PANE` is your pane's id. The `tty59` CLI is not installed here.
`tty59-server` itself answers a small set of pane verbs, and this skill's shim
finds the right server binary for you:

```bash
T={{SHIM}}
$T scope
```

Run everything through `$T`. The server binary's own name changes on every
upgrade, so don't call it directly.

## Scope

Every call recomputes what you may touch:

- **Workspace:** the one holding your pane.
- **Tabs:** within that workspace, those in the same sidebar group as yours.
  If your tab has no group, every ungrouped tab.

A pane outside that scope is refused by name. The person can move your tab at any
time, so run `$T scope` or `$T ls` at the start of each task, and don't reuse a
pane list from an earlier one.

## Verbs

| Verb | What it does | Rules |
|---|---|---|
| `$T scope` | your pane, workspace, group, and the panes in scope | |
| `$T ls` | panes in scope, tab-separated: pane, `*` for yours, live/dead, tab, agent/status, cwd, title | |
| `$T tab new [--cwd DIR]` | opens a tab with a fresh shell in your workspace and group, and prints its `%N` | |
| `$T tab close %N` | closes the tab holding `%N`, and every pane in it | not the tab holding your own pane |
| `$T split [%N] --v\|--h` | splits `%N` (default: your pane), prints the new `%N` | `--v` puts it below, `--h` to the right |
| `$T send %N [TEXT] [--enter] [--key KEY]...` | types TEXT as a keyboard would, then presses the keys | needs an explicit target, never your own pane |
| `$T send %N --stdin [--enter]` | types what stdin holds | the way to send a secret without it showing in `ps` |
| `$T capture [%N] [--tail N] [--raw]` | the pane's scrollback as text | see *Reading* below |
| `$T procs %N` | the pane's process tree, plus one `shell:` line saying how to drive it | |
| `$T wait %N [--until free\|exit] [--timeout SECS]` | blocks until the command finishes (`free`) or the pane is gone (`exit`) | exits 124 on timeout |
| `$T close %N...` | closes panes | only panes you opened, or on an explicit request |

Keys for `--key`: `enter escape tab backtab space backspace delete up down left
right home end pageup pagedown`, `C-<char>` for Ctrl, and `M-<char>` for Alt.
Repeat `--key` to send a sequence. Typing `^C` as text does nothing: it arrives as
two characters, while `--key C-c` is the real interrupt.

This machine has no `exec`, `run`, `agents`, `events` or `ws`. For those, the
person can use the `tty59` CLI on their own computer (`tty59 -m <this host> ...`).

## Running a command in a pane

A pane you create is a shell at its prompt. This pattern gets you the command's
output and its exit code:

```bash
P=$($T split --v)                                  # or: P=$($T tab new --cwd "$PWD")
$T send "$P" 'make test > /tmp/t.log 2>&1; echo $? > /tmp/t.rc' --enter
$T capture "$P" --tail 3                           # did it leave the prompt?
$T wait "$P" --until free --timeout 900
cat /tmp/t.rc /tmp/t.log
$T close "$P"
```

**A new pane can swallow the first Enter.** A shell that is still running its
startup files keeps the text you sent but loses the carriage return after it,
so the command just sits on the prompt line. After the first `send` into a new
pane, `capture --tail 3`. If the command is still on the prompt line, send
`$T send "$P" --enter`.

## Panes that are an `ssh` session

> **Read this before typing into any pane you did not just create.**

A pane where someone ran plain `ssh host` is a local shell running `ssh`. On the
far host, tty59's shell integration usually isn't loaded, so nothing on this
machine can see the far shell's prompt or exit code:

- `procs` shows only `bash` → `ssh`. That describes the tunnel, not the work.
- `wait --until free` can't tell an idle remote prompt from a running command,
  and says so instead of guessing.
- On the host, `tty59 exec` refuses such a pane for the same reason.

So in an ssh pane, **`send` is the only way to run something, and `capture` is
the only way to read it.** Check first:

```bash
$T procs %N          # last line, e.g.:
# shell: ssh to build-box — the far shell sends no prompt marks: drive it with `send`, read it with `capture`
```

Then type the command with a marker you can look for, and poll the screen for it:

```bash
$T capture %N --tail 5                    # which host and user is the prompt on?
$T send %N 'uname -a; echo "__done rc=$?"' --enter
sleep 2; $T capture %N --tail 20          # repeat until __done rc=... shows up
```

- **Check the prompt first.** `capture` shows which host and user the shell is on;
  it may be root. Tell the person the exact command and the pane before you type
  into one you didn't open.
- **Long output scrolls away.** On the far host, write it to a file
  (`cmd > /tmp/out 2>&1; echo "__done rc=$?"`), then `cat` the parts you need
  through another `send`.
- **Don't send a password as TEXT.** Use `--stdin`:
  `printf %s "$SECRET" | $T send %N --stdin --enter`.
- If `procs` reports `ssh to <host>, far shell at its prompt`, the far shell does
  report, so `wait --until free` works there too.

## Reading a pane

`capture` returns the pane's whole scrollback, so use `--tail N` for the end of
it. This server has no terminal emulator, so the text is a line-level reading of
the bytes:

- color and escape sequences are dropped;
- a carriage return overwrites the line, so a progress bar reads as its last value;
- a full-screen program (an editor, `top`, a TUI agent) won't read well.

For those, `--raw` gives the bytes as the pane received them.

A capture is a snapshot. Call it again for a newer one.

## Limits

- **Never stop, restart or kill `tty59-server`**: no `--stop`, no `pkill`, no kill
  on the pid in `daemon.pid`. It holds every pane on this machine, including
  yours.
- Don't close, kill or rearrange panes you didn't create unless asked to.
- A pane isn't a way around your own restrictions. If another pane has access
  you lack (a socket, a credential, a root shell), use it only with the person's
  explicit permission for that action, and say which barrier you're crossing.
- If `$T` refuses a target, don't reach it another way (the server binary
  directly, its sockets, its files). Ask the person to move your tab or the pane.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | ok |
| 1 | the request failed; the reason is on stderr |
| 2 | refused by the scope, or a usage error |
| 3 | the shim found no `tty59-server` on this machine |
| 124 | `wait` timed out |
