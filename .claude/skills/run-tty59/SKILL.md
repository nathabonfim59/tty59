---
name: run-tty59
description: Build, run, test, screenshot, and drive the tty59 desktop app. Use when asked to start tty59, interact with its GUI, capture its UI, or run its tests.
---

tty59 is a native GPUI desktop app. Drive it through
`.claude/skills/run-tty59/driver.sh` on niri/Wayland. The driver uses an isolated
config under `/tmp`, matches processes by that config, and refuses keyboard,
pointer, or screenshot operations when its demo window is not focused.

All paths below are relative to the repository root. This path was verified on
Arch Linux with niri; it is not an X11/xvfb recipe.

## Prerequisites

Required: Rust/Cargo, `jq`, `grim`, `ydotool` with `ydotoold` running, and an
active niri session. No OS packages were installed while authoring this skill.

```bash
command -v jq grim ydotool niri && pgrep -a ydotoold
```

A prebuilt `lib/libghostty-vt.a` must exist under `TTY59_GHOSTTY_DIR`. The driver
also checks the normal sibling `../ghostty-vt-out` and the shared ancestor used
by this repository's worktrees.

## Build

```bash
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh build
```

## Run (agent path)

Build, launch, change live config, capture, and stop:

```bash
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh build
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh launch
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh set-config tab_group_shortcuts true
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh shot driver-final 0 150 350 290
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh stop
```

Screenshots land in `/tmp/tty59-run-skill/screenshots/`; the app log is
`/tmp/tty59-run-skill/app.log`. Without `TTY59_DRIVER_DIR`, the state root is
`/tmp/tty59-run`. `launch` focuses and fullscreens only the demo window.

Drive the focused app with real keyboard input:

```bash
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh type 'export PS1="$ "; clear'
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh key 28:1 28:0
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh new-tab
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh wait 0.5
```

| Command | Purpose |
|---|---|
| `status` | Print the demo window's niri JSON |
| `focus` | Explicitly focus the demo before more input |
| `type TEXT` | Type into the focused demo |
| `key CODE...` | Send numeric Linux input events, such as `28:1 28:0` for Enter or `56:1`/`56:0` for Alt down/up |
| `new-tab` | Send Ctrl+Shift+T |
| `click X Y [left\|right]` | Click screenshot-pixel coordinates |
| `click-raw X Y [left\|right]` | Click unscaled ydotool coordinates |
| `wait SECONDS` | Wait for delayed badges, shell startup, or rendering |
| `shot NAME [X Y W H]` | Capture the full output or a crop, only while the demo is focused |
| `stop` | TERM then KILL only the matching demo GUI/daemon |

Do not type or click in another window while driving the app. The focus guard
blocks input when focus has already moved, but global input is inherently racy.

## Test

The test command uses the same Cargo and Ghostty discovery as the build. This
exact filter ran 50 keymap tests successfully:

```bash
TTY59_DRIVER_DIR=/tmp/tty59-run-skill .claude/skills/run-tty59/driver.sh test -p tty59 --bin tty59-app ui::keymap::
```

## Gotchas

- **The mise Cargo shim rewrites `TTY59_GHOSTTY_DIR` relative to a worktree.**
  The driver prefers `$HOME/.cargo/bin/cargo` directly and finds the shared
  archive instead of accepting the shim's rewritten path.
- **`NIRI_SOCKET` may be unset.** The driver discovers
  `/run/user/<uid>/niri.wayland-*.sock`; set `NIRI_SOCKET` explicitly if more
  than one compositor is running.
- **Use numeric key events.** `ydotool key enter` did not submit here;
  `28:1 28:0` did.
- **Input coordinates are not screenshot pixels on this display.** At 1920×1200
  with niri scale 1.25, screenshot pixels were 2.5× the raw ydotool coordinates.
  `click` applies that factor; override `TTY59_DRIVER_INPUT_SCALE` on another
  display, or calibrate with `click-raw` and a screenshot.
- **Niri focus changes before the fullscreen animation ends.** `launch` waits
  0.5 seconds so a screenshot cannot catch the previously focused window.
- **Config isolation also isolates the daemon.** `stop` verifies `/proc` command
  lines against the demo config before signalling anything; never use a broad
  `pkill tty59-app` for cleanup.

## Troubleshooting

- **`libghostty-vt not found`**: point `TTY59_GHOSTTY_DIR` at the prefix that
  actually contains `lib/libghostty-vt.a`, then rerun the driver build.
- **`Unable to retrieve socket path` from `swaymsg`**: this is niri, not Sway;
  use the driver, which speaks to niri directly.
- **`demo window is not focused; refusing input/capture`**: another window has
  focus. Bring the demo back with `focus`; do not bypass the guard.
- **A capture contains the previous window**: it was taken during the fullscreen
  transition. Keep the 0.5-second launch wait and recapture after `wait 0.5`.
