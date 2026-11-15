# Flaky CLI capture after resize

On Boat base `tty59-ui-2614`, the B2 full gate failed once in `capture_still_answers_after_a_resize` at `crates/tty59-cli/tests/cli_e2e.rs:992`. The assertion said the newest segment was not an exact suffix of the ring, even though the captures before and after it matched.

The tree was `ui-fix/B2-truncation` based on `ui-overhaul@4ffdc3906bf642eb01fdf3ddce3663a35af5c84b`, plus the reviewed uncommitted B2 snapshot. Its 61 source hashes are recorded in `.pr-assets/B2/full-gate-round19/raw/source.json`. B2 changes no CLI, capture or server code.

The full gate returned 101. Formatting and the warnings-denied build passed. Normal Rust test summaries reported 4,983 passed, zero failed and 11 ignored. The custom CLI e2e harness reported 21 passed and one failed.

Failing log, relative to the fx-b worktree: `.pr-assets/B2/full-gate-round19/raw/tests.log`, line 3133. Full path: `/home/nathanael/Work/OSS/tty59/.tty59/worktrees/fx-b/.pr-assets/B2/full-gate-round19/raw/tests.log`.

An unchanged CLI e2e rerun on the same retained VM, `bx_3p625dtj`, passed all 22 cases, including the resize case, with exit 0. Command: `cargo test --locked -p tty59-cli --test cli_e2e`. Logs: `.pr-assets/B2/full-gate-round19/cli-rerun/stdout.log` and `stderr.log` in the fx-b worktree.

The complete gate rerun with the original workspace command and feature graph passed: 5,005 tests, zero failures and 11 ignored, with formatting and the warnings-denied build also passing. Its logs are under `.pr-assets/B2/full-gate-recheck-round23/raw/`. No assertion was changed and no test was excluded.

Status: FLAKY. The cause is not established. Fix separately from the B2 UI changes.

## A4 follow-up trace

The same assertion failed during A4's Commit-header follow-up on retained Boat VM `bx_b2qz2d47`, using the frozen `ab68524c` tree plus `ui24-commit-header-full-v2.patch` (`53d91decaaf86c076cdd3b92553a4d935195aeeb3c8b0b0798d51b12cf72804a`). The CLI e2e source blob is `3031bc1074eace13ade3414bf5e10b1490898f79`, identical to `ui-overhaul`. The correction changes only the Diff header and its UI regression.

Formatting and the warnings-denied build passed. All 2,908 app tests passed. The CLI resize test failed at line 992 because its newest capture started with bracketed-paste-on followed by carriage-return and erase-line, while that exact prefix was absent from the full ring's suffix. The before/after whole-ring check accepted those snapshots as stable. This is the same assertion as the B2 report; the trace does not establish the cause.

The complete failed run and original assertion text are preserved in `.pr-assets/A4/reviews/commit-header-v1/workspace-failed-v1.tgz`, member `a4-commit-workspace-v1/workspace.log`. The failed run is not the A4 gate. No CLI code, assertion or test selection was changed. The orchestrator requested one complete rerun on the same frozen source.

The unchanged complete rerun passed with 5,254 tests, zero failures and 17 ignored in the standard Rust summaries, plus all 22 custom CLI e2e cases. Formatting and the warnings-denied build passed again. All 999 manifested files matched the same frozen source afterward. Evidence: `.pr-assets/A4/reviews/commit-header-v1/workspace-clean-v2.tgz`. No CLI assertion was changed or excluded.
