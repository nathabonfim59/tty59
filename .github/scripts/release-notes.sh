#!/usr/bin/env bash
# Release notes for one tty59 version, written to stdout:
#
#   release-notes.sh <version> <changelog> <dist-dir>
#
# Fork credit, a Downloads table naming only the assets actually in <dist-dir>
# (an experimental leg that failed drops out instead of being advertised), then
# the version's section from the changelog. Fails when the changelog has no
# `## [<version>]` section, so a release cannot go out with empty notes.
set -euo pipefail

VERSION="$1"
CHANGELOG="$2"
DIST="$3"

have() { [ -e "$DIST/$1" ]; }

# The `## [VERSION]` section's body, up to the next `## [` heading.
SECTION="$(awk -v head="## [$VERSION]" '
  index($0, head) == 1 { inside = 1; next }
  inside && /^## \[/   { exit }
  inside               { print }
' "$CHANGELOG")"
if [ -z "$(tr -d '[:space:]' <<<"$SECTION")" ]; then
  echo "release-notes: $CHANGELOG has no ## [$VERSION] section" >&2
  exit 1
fi

cat <<EOF
tty59 is a fork of [tty7](https://github.com/l0ng-ai/tty7) by [l0ng-ai](https://github.com/l0ng-ai). The workbench is theirs. The only difference in this fork is the terminal engine, now [libghostty-vt](https://github.com/ghostty-org/ghostty) (Ghostty, MIT).

## Downloads

| Platform | File |
|---|---|
EOF

arch="tty59-${VERSION}-1-x86_64.pkg.tar.zst"
have "$arch" && echo "| Arch Linux (x86_64) | \`$arch\`: \`sudo pacman -U $arch\` |"

appimage="tty59-${VERSION}-linux-x86_64.AppImage"
if have "$appimage"; then
  echo "| Linux (x86_64) | \`$appimage\`, or the \`.tar.gz\`. Needs glibc 2.39 or newer |"
fi

mac=()
have "tty59-${VERSION}-macos-arm64.dmg" && mac+=("\`tty59-${VERSION}-macos-arm64.dmg\` (Apple silicon)")
have "tty59-${VERSION}-macos-x86_64.dmg" && mac+=("\`tty59-${VERSION}-macos-x86_64.dmg\` (Intel)")
if [ ${#mac[@]} -gt 0 ]; then
  echo "| macOS | $(printf '%s, ' "${mac[@]}" | sed 's/, $//') |"
fi

win=()
for a in x86_64 arm64; do
  f="tty59-${VERSION}-windows-${a}-setup.exe"
  have "$f" && win+=("\`$f\`")
done
if [ ${#win[@]} -gt 0 ]; then
  echo "| Windows | $(printf '%s, ' "${win[@]}")or the portable \`.zip\` |"
fi

if compgen -G "$DIST/tty59-server-*" >/dev/null; then
  echo "| Remote server | \`tty59-server-*\` for Linux (musl), macOS and Windows |"
fi

printf '\n`checksums.txt` holds the sha256 of every asset.\n'
printf '%s\n' "$SECTION"
