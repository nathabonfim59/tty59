# Path-link corpus (neutral PTY recordings)

Raw bytes of commands run on a Boat VM through a real PTY sized 80x24, with
neutral generated content only (no real users, hosts, paths or processes).
`manifest.json` holds, per capture, the exact command, working directory, exit
code, tool versions, input, sha256 of the input and of the captured bytes, the
expected paths and the logical rows left on the screen.

Source archive: `corpus-r4.tgz`, sha256
`f353e8042fee48391e37aa2a1196280b027714b3574431bd26fe2d3b73b69267`.
The `.pty` files are copied byte for byte from its `captures/`; nothing was
edited or substituted.

Two kinds of recording, labelled per case in `manifest.json`:

- `engineered formatter recording`: GNU `fold`, `sed`, `awk`, `nl` and `cat`
  over a generated path, to produce soft wraps, hard breaks at the terminal
  width, wraps before the width, indentation, box and numbered gutters, splits
  after `/`, `-` and `:`, a quoted path with spaces, and negatives.
- `real application trace`: `rustc`, `git status`, `git diff --stat`, `rg
  --heading` and `ls`, whose real output contains a long path or listing.

These are not captures of any AI terminal UI, and no case claims to be.

They are replayed by `../../gpui_tests/path_links.rs`.
