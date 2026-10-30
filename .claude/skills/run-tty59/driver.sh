#!/usr/bin/env bash
# Drive the real tty59 GUI on niri/Wayland with an isolated config directory.
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)
STATE=${TTY59_DRIVER_DIR:-/tmp/tty59-run}
CONFIG_DIR="$STATE/config"
SHOT_DIR="$STATE/screenshots"
APP_PID_FILE="$STATE/app.pid"
DAEMON_PID_FILE="$STATE/daemon.pid"
INPUT_SCALE=${TTY59_DRIVER_INPUT_SCALE:-2.5}
export YDOTOOL_SOCKET=${YDOTOOL_SOCKET:-/run/user/$(id -u)/.ydotool_socket}

fail() {
  printf 'driver: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: driver.sh COMMAND [ARGS]

  build                         Build target/debug/tty59-app
  test [CARGO-TEST-ARGS]        Run cargo test with the same native setup
  launch                        Launch with isolated config; focus + fullscreen
  set-config KEY JSON           Merge one JSON value into the live config
  type TEXT                     Type text into the focused app
  key CODE...                   Send ydotool key events (example: 28:1 28:0)
  new-tab                       Open a tab with Ctrl+Shift+T
  click-raw X Y [left|right]    Click raw ydotool coordinates
  click X Y [left|right]        Click screenshot pixels (uses INPUT_SCALE)
  wait SECONDS                  Wait for delayed UI state
  shot NAME [X Y W H]           Capture fullscreen or a screenshot-pixel crop
  status                        Print the demo window JSON
  focus                         Focus the demo window
  stop                          Stop only this driver's GUI and daemon
EOF
}

need_niri() {
  if [[ -z ${NIRI_SOCKET:-} ]]; then
    local socket
    for socket in /run/user/$(id -u)/niri.wayland-*.sock; do
      if [[ -S $socket ]]; then
        export NIRI_SOCKET=$socket
        break
      fi
    done
  fi
  [[ -n ${NIRI_SOCKET:-} && -S $NIRI_SOCKET ]] || \
    fail "no niri socket; set NIRI_SOCKET to the active compositor socket"
}

app_pid() {
  [[ -r $APP_PID_FILE ]] || fail "not launched; run driver.sh launch"
  local pid
  read -r pid < "$APP_PID_FILE"
  printf '%s\n' "$pid"
}

window_id() {
  need_niri
  local pid
  pid=$(app_pid)
  niri msg --json windows | \
    jq -er --argjson pid "$pid" 'first(.[] | select(.pid == $pid) | .id)'
}

ensure_focused() {
  local id focused
  id=$(window_id) || fail "demo window not found; launch again"
  focused=$(niri msg --json focused-window | jq -r '.id // empty')
  [[ $focused == "$id" ]] || \
    fail "demo window is not focused; refusing input/capture (use driver.sh focus first)"
}

find_ghostty() {
  local prefix
  for prefix in \
    ${TTY59_GHOSTTY_DIR:-} \
    "$ROOT/../ghostty-vt-out" \
    "$ROOT/../../../../ghostty-vt-out"; do
    if [[ -n $prefix && -f $prefix/lib/libghostty-vt.a ]]; then
      printf '%s\n' "$prefix"
      return 0
    fi
  done
  return 1
}

pid_is_ours() {
  local pid=$1
  [[ -r /proc/$pid/cmdline ]] || return 1
  tr '\0' ' ' < "/proc/$pid/cmdline" | grep -Fq -- "$CONFIG_DIR"
}

stop_pid() {
  local pid=$1 signal=$2
  if pid_is_ours "$pid"; then
    kill "-$signal" "$pid" 2>/dev/null || true
  fi
}

mkdir -p "$SHOT_DIR"
command=${1:-help}
[[ $# -eq 0 ]] || shift

case "$command" in
  build)
    prefix=$(find_ghostty) || \
      fail "set TTY59_GHOSTTY_DIR to a prefix containing lib/libghostty-vt.a"
    if [[ -x $HOME/.cargo/bin/cargo ]]; then
      cargo=$HOME/.cargo/bin/cargo
    else
      cargo=$(command -v cargo) || fail "cargo not found"
    fi
    cd "$ROOT"
    TTY59_GHOSTTY_DIR="$prefix" "$cargo" build --bin tty59-app
    ;;

  test)
    prefix=$(find_ghostty) || \
      fail "set TTY59_GHOSTTY_DIR to a prefix containing lib/libghostty-vt.a"
    if [[ -x $HOME/.cargo/bin/cargo ]]; then
      cargo=$HOME/.cargo/bin/cargo
    else
      cargo=$(command -v cargo) || fail "cargo not found"
    fi
    cd "$ROOT"
    TTY59_GHOSTTY_DIR="$prefix" "$cargo" test "$@"
    ;;

  launch)
    [[ -x $ROOT/target/debug/tty59-app ]] || fail "build first: driver.sh build"
    if [[ -r $APP_PID_FILE ]]; then
      old=$(app_pid)
      kill -0 "$old" 2>/dev/null && fail "already launched as PID $old"
    fi
    mkdir -p "$CONFIG_DIR" "$SHOT_DIR"
    if [[ ! -f $CONFIG_DIR/config.json ]]; then
      cat > "$CONFIG_DIR/config.json" <<'EOF'
{
  "theme": "dark",
  "theme_preset": "dark",
  "tab_bar_position": "left",
  "sidebar_auto_grouping": true,
  "restore_session": false,
  "check_for_updates": false,
  "auto_download_updates": false,
  "show_tray_icon": false,
  "install_cli_on_path": false,
  "mobile_access": false,
  "confirm_close": "never",
  "gui_language": "en"
}
EOF
    fi
    nohup "$ROOT/target/debug/tty59-app" --config-dir "$CONFIG_DIR" \
      > "$STATE/app.log" 2>&1 &
    pid=$!
    printf '%s\n' "$pid" > "$APP_PID_FILE"
    id=''
    for _ in {1..150}; do
      id=$(window_id 2>/dev/null || true)
      [[ -n $id ]] && break
      kill -0 "$pid" 2>/dev/null || {
        printf 'driver: app exited; log follows\n' >&2
        command tail -n 40 "$STATE/app.log" >&2
        exit 1
      }
      sleep 0.1
    done
    [[ -n $id ]] || fail "window did not appear; see $STATE/app.log"
    rm -f "$DAEMON_PID_FILE"
    for _ in {1..50}; do
      daemon=$(pgrep -P "$pid" -f 'tty59-app --daemon' | command sed -n '1p' || true)
      [[ -n $daemon ]] && break
      sleep 0.1
    done
    if [[ -n ${daemon:-} ]]; then
      printf '%s\n' "$daemon" > "$DAEMON_PID_FILE"
    fi
    niri msg action focus-window --id "$id"
    if [[ ${TTY59_DRIVER_FULLSCREEN:-1} == 1 ]]; then
      niri msg action fullscreen-window --id "$id"
    fi
    # Niri updates focus before the fullscreen transition finishes. Screenshots
    # during that animation can still contain the previously focused window.
    sleep 0.5
    printf 'launched pid=%s window=%s config=%s\n' "$pid" "$id" "$CONFIG_DIR"
    ;;

  set-config)
    [[ $# -eq 2 ]] || fail "usage: set-config KEY JSON"
    [[ -f $CONFIG_DIR/config.json ]] || fail "launch first"
    tmp="$CONFIG_DIR/config.json.tmp"
    jq --arg key "$1" --argjson value "$2" '.[$key] = $value' \
      "$CONFIG_DIR/config.json" > "$tmp"
    mv "$tmp" "$CONFIG_DIR/config.json"
    printf 'set %s = %s\n' "$1" "$2"
    ;;

  type)
    [[ $# -ge 1 ]] || fail "usage: type TEXT"
    ensure_focused
    ydotool type "$*"
    ;;

  key)
    [[ $# -ge 1 ]] || fail "usage: key CODE..."
    ensure_focused
    ydotool key "$@"
    ;;

  new-tab)
    ensure_focused
    ydotool key 29:1 42:1 20:1 20:0 42:0 29:0
    ;;

  click-raw)
    [[ $# -ge 2 && $# -le 3 ]] || fail "usage: click-raw X Y [left|right]"
    ensure_focused
    case ${3:-left} in
      left) button=0xC0 ;;
      right) button=0xC1 ;;
      *) fail "button must be left or right" ;;
    esac
    ydotool mousemove --absolute -- "$1" "$2"
    ydotool click "$button"
    ;;

  click)
    [[ $# -ge 2 && $# -le 3 ]] || fail "usage: click X Y [left|right]"
    raw_x=$(jq -n --argjson value "$1" --argjson scale "$INPUT_SCALE" \
      '($value / $scale) | round')
    raw_y=$(jq -n --argjson value "$2" --argjson scale "$INPUT_SCALE" \
      '($value / $scale) | round')
    "$0" click-raw "$raw_x" "$raw_y" "${3:-left}"
    ;;

  wait)
    [[ $# -eq 1 ]] || fail "usage: wait SECONDS"
    sleep "$1"
    ;;

  shot)
    [[ $# -eq 1 || $# -eq 5 ]] || fail "usage: shot NAME [X Y W H]"
    [[ $1 =~ ^[A-Za-z0-9._-]+$ ]] || fail "screenshot name has unsafe characters"
    ensure_focused
    file="$SHOT_DIR/$1.png"
    if [[ $# -eq 5 ]]; then
      grim -g "$2,$3 $4x$5" "$file"
    else
      grim "$file"
    fi
    printf 'screenshot: %s\n' "$file"
    ;;

  status)
    need_niri
    pid=$(app_pid)
    niri msg --json windows | jq --argjson pid "$pid" '.[] | select(.pid == $pid)'
    ;;

  focus)
    id=$(window_id)
    niri msg action focus-window --id "$id"
    ;;

  stop)
    app=''
    daemon=''
    [[ -r $APP_PID_FILE ]] && read -r app < "$APP_PID_FILE"
    [[ -r $DAEMON_PID_FILE ]] && read -r daemon < "$DAEMON_PID_FILE"
    for pid in $app $daemon; do
      [[ -n $pid ]] && stop_pid "$pid" TERM
    done
    for _ in {1..20}; do
      alive=0
      for pid in $app $daemon; do
        if [[ -n $pid ]] && kill -0 "$pid" 2>/dev/null; then
          alive=1
        fi
      done
      [[ $alive -eq 0 ]] && break
      sleep 0.1
    done
    for pid in $app $daemon; do
      [[ -n $pid ]] && stop_pid "$pid" KILL
    done
    rm -f "$APP_PID_FILE" "$DAEMON_PID_FILE"
    printf 'stopped; artifacts remain in %s\n' "$STATE"
    ;;

  help|-h|--help)
    usage
    ;;

  *)
    usage >&2
    fail "unknown command: $command"
    ;;
esac
