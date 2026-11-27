#!/bin/sh
# tty59-remote shim, written by `tty59-server skill install`.
#
# Runs this machine's tty59-server pane verbs. The server is installed under a
# name that changes with its wire dialect (tty59-server-c<N>p<M>), so this looks
# for the right one rather than naming it, in this order:
#   1. $TTY59_SERVER_BIN, when set;
#   2. the binary of the daemon that is running now, which is the one that is
#      sure to speak its own dialect;
#   3. the binary that wrote this shim;
#   4. the newest one tty59 installed under ~/.local/share/tty59/bin;
#   5. tty59-server on PATH.
installed={{SERVER}}

run() {
	if [ -n "$1" ] && [ -x "$1" ]; then
		bin=$1
		shift
		exec "$bin" "$@"
	fi
}

run "${TTY59_SERVER_BIN:-}" "$@"

cfg=${TTY59_CONFIG_DIR:-$HOME/.config/tty59}
pid=$(cat "$cfg/daemon.pid" 2>/dev/null)
case $pid in
'' | *[!0-9]*) ;;
*)
	if [ -r "/proc/$pid/exe" ]; then
		# A binary replaced on disk reads as "<path> (deleted)": skip it.
		exe=$(readlink "/proc/$pid/exe" 2>/dev/null)
	else
		exe=$(ps -o comm= -p "$pid" 2>/dev/null)
	fi
	case $exe in
	*tty59-server*) run "$exe" "$@" ;;
	esac
	;;
esac

run "$installed" "$@"

newest=$(ls -t "$HOME"/.local/share/tty59/bin/tty59-server-c* 2>/dev/null | grep -v '\.tmp$' | head -n 1)
run "$newest" "$@"

if command -v tty59-server >/dev/null 2>&1; then
	exec tty59-server "$@"
fi
echo "tty59: no tty59-server on this machine (looked in ~/.local/share/tty59/bin and PATH)" >&2
exit 3
