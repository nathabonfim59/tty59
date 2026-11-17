# CLI capture suffix after resize

Status: fixed in the tests on 2026-11-17. The cause is established, the test-only fix passed the 50-trial stress campaign, the three independent mutation controls failed as intended, and the final gate passed on Boat (Linux). Windows and macOS were not exercised natively; see Limits. The records below the line are earlier occurrences, kept as they were reported.

## Cause

`capture_still_answers_after_a_resize` asserted `whole.ends_with(newest_text)`, where `whole` is `capture --scrollback` and `newest_text` is the default `capture`. Those two are not required to be related that way. The default form is built by `what_was_asked_for` in `crates/tty59-cli/src/backend/real.rs`: it drops empty segments, folds every segment older than the newest through the core `TerminalModes`, and **prepends the modes those left on** to the newest segment. A resize seals the shell prompt's segment while bracketed paste is on, and the redraw the shell paints after SIGWINCH opens the newest one, so the default form is `ESC[?2004h` plus that redraw. The prefix is part of neither the raw newest segment nor the end of the ring. The documented contract holds; the test's expectation was wrong, and it failed whenever the shell's redraw did not itself begin with that sequence.

This is not a capture atomicity defect and not a moving snapshot. The before/after whole-ring check in the old test was accurate: the ring did hold still, and the assertion was false of that stable ring.

## Evidence

Everything below ran on Boat, on one owned VM, on exact base `9ff90f7e99dc9638ec01965543d6265a33471a40` (base archive `base-9ff90f7e.tgz`, SHA-256 `2b2e31fb753fce288bc022ae746afbba2fd8024fbd9663b6570a2cf727503cc2`, source manifest SHA-256 `edec649109933c3106052e0abac3dc20b2151343627ee84061f42498a4f3fbac`). The target has a custom `main` (`harness = false`), so a name filter does not exist; the campaigns used a scratch-only environment selector in the custom main with the test body byte-identical, and the source was restored and checked afterwards.

| Campaign | Trials | Concurrency | Passed | Failed |
| --- | ---: | ---: | ---: | ---: |
| Base, unchanged test | 50 | 4 | 48 | 2 (both the suffix assertion) |
| Candidate (frozen R1) | 50 | 4 | 50 | 0 |

Both campaigns had independent trials, a private daemon directory per trial, no retries, no timeouts and no leftover processes. Failed trials were kept, not rerun. The lower failure frequency here than in the earlier samples below is reported as observed; these counts establish no rate.

Both baseline failures have a newest string starting `ESC[?2004h`, then carriage return and erase-line, then the prompt redraw, while the whole ring ends in that redraw with bracketed paste switched on earlier, before the resize.

A separate instrumented diagnostic run of ten trials (not mixed into the 50 above) gave 6 passed and 4 failed, all at the same suffix assertion. Its trace (SHA-256 of the archive `13843a84548d7bf57d6a95469514e83a402c259584d6d15239204d4b91d65c7d`) shows for the failures: the raw 100-column segment ends with bracketed paste on and the prompt, the raw 80-column segment starts with carriage return and erase-line, the default capture returns `ESC[?2004h` followed by that 80-column segment, and the raw ring is identical before and after.

A deterministic counterexample, with no timing and no PTY, pins this through the real `what_was_asked_for` on two fixed segments, `[100 cols: ESC[?2004h A CRLF][80 cols: B CRLF]`. The whole text is `ESC[?2004h A CRLF B CRLF` and the default text is `ESC[?2004h B CRLF`; both exact checks pass, and the original `whole.ends_with(default)` predicate fails, every time. The first attempt at this counterexample ran `cargo test --lib` against a binary-only crate and failed at setup; that setup error is retained and is not a red result. The corrected run uses `--bin tty59`, executed one test and produced the intended failure (exit 101, at the expected assertion). Evidence is under `.ui-audit/capture-resize/` in the fx-d worktree (`baseline`, `trace`, `counterexample-r2`).

## Fix

Test only, in `crates/tty59-cli/tests/cli_e2e.rs` and the tests of `crates/tty59-cli/src/backend/real.rs`. No production change: capture behaviour is as documented and is unchanged.

- The resize case keeps its name and registration but drives a child of the test executable in raw mode instead of a shell. The child writes a first segment (`ESC[?2004h ESC[?1007h`, two markers) and blocks until it receives one ack byte, then writes a second segment and holds. It has no SIGWINCH handler, so on Unix the resize itself makes it print nothing.
- The test waits for the whole first segment including its CRLF, resizes 100 to 80 columns, and waits for the daemon's `Size(80)` echo, which is sent in stream order after the ring has been sealed. No delay or retry is involved.
- Before the ack the ring is exactly the output at 100 columns and an empty segment at 80. This keeps the original regression for #841: the default capture answers with the output, not with zero bytes.
- After the ack the ring is the first segment and exactly `TTY59_RESIZE_B` CRLF. The raw newest segment is pinned exactly; the default capture is pinned as the restored modes, literally `ESC[?2004h ESC[?1007h` on Unix, followed by that segment; `--scrollback` is every segment. Byte counts, raw and plain markers and the ordering of the newest segment are asserted.
- The raw replay is read with `observe` through the first snapshot after `Size(80)` (empty segments included), which is the finite end of the ring replay, so nothing waits for a quiet stream. A leading size-less mode prelude is kept and fails the test if present; nothing is stripped. Two raw readings around the captures must be equal, as an assertion rather than a retry loop.
- Two literal unit tests in `backend/real.rs` pin the prelude through `what_was_asked_for` on every platform: the two-mode prelude in the order the program last set the modes, with the default not a suffix of the whole text, and a mode switched off in an older segment giving no prelude. They keep the end-to-end oracle from only checking production against itself.

Candidate source hashes: `cli_e2e.rs` `adb839739558c78d0abc5e8c90e4c42039bd55e99f276ac4a8ab0d60e73304ef`, `real.rs` `cec8f1f9127068b5a5a525c903e6d296e08261ae4a2b89fd8fc9858e06b1687a`. Frozen R1 input archive SHA-256 `6b820390397e39424be31548b4163e5d07e58446e1895f07e934979c897de723`; its 50-trial results archive SHA-256 `7bf163863be284552ab76d0137cf1bf3873bcb341d8e95006f4433caa20ddd7f`.

## Limits

On Windows the console host can print control bytes of its own around a resize. There the end-to-end test requires the raw newest segment to end with the child's output and takes the restored modes from the core `TerminalModes` fold of the raw older segments, with nothing stripped; the literal unit tests still pin the prelude. That branch cannot be exercised on Boat (Linux) and is checked only by CI. The macOS path uses the same Unix fixture and literal assertions but was likewise not run natively here; only the Linux PTY was exercised. The real-shell resize smoke is replaced by the fixture; the other capture tests keep real shells.

## Controls and final gate

Independent controls are complete. Each mutation failed as intended, and the restored backend passed with the source byte-exact afterwards:

- Empty-segment filter removed: the candidate fails at the before-ack check against the exact first segment. The default capture is then only the restored modes, `ESC[?2004h ESC[?1007h`, which is not empty and carries a positive byte count, so a "non-empty with bytes" assertion alone would not have caught it; the exact comparison against the first segment does.
- Wrong first segment kept (newest-segment selection mutated): fails the after-ack exact default check.
- Mode prefix omitted: fails the new literal unit test in `backend/real.rs`.

Final gate, on the same Boat VM, all passing:

- `cargo fmt --all --check` clean. The only change from the Boat formatter was line wrapping in the candidate's own test code; no assertion changed.
- The warnings-denied CLI build (`-D warnings`) passed.
- The complete unfiltered `tty59-cli` test run with `-D warnings`: 191 unit tests passed, 0 failed, 4 ignored (those four were already ignored), plus all 22 cases of the custom CLI e2e target, including the rewritten resize case.
- The focused `backend::real` tests: 10 passed, including the two new literal prelude tests.
- The scratch name selector used for the stress campaigns is absent from the final source.

The source hashes under Fix are those of the frozen R1 candidate that ran the stress campaign; the committed files may differ from them by that formatting only, and the lead records the final tree.

---

# Earlier records

## B2 occurrence

On Boat base `tty59-ui-2614`, the B2 full gate failed once in `capture_still_answers_after_a_resize` at `crates/tty59-cli/tests/cli_e2e.rs:992`. The assertion said the newest segment was not an exact suffix of the ring, even though the captures before and after it matched.

The tree was `ui-fix/B2-truncation` based on `ui-overhaul@4ffdc3906bf642eb01fdf3ddce3663a35af5c84b`, plus the reviewed uncommitted B2 snapshot. Its 61 source hashes are recorded in `.pr-assets/B2/full-gate-round19/raw/source.json`. B2 changes no CLI, capture or server code.

The full gate returned 101. Formatting and the warnings-denied build passed. Normal Rust test summaries reported 4,983 passed, zero failed and 11 ignored. The custom CLI e2e harness reported 21 passed and one failed.

Failing log, relative to the fx-b worktree: `.pr-assets/B2/full-gate-round19/raw/tests.log`, line 3133. Full path: `/home/nathanael/Work/OSS/tty59/.tty59/worktrees/fx-b/.pr-assets/B2/full-gate-round19/raw/tests.log`.

An unchanged CLI e2e rerun on the same retained VM, `bx_3p625dtj`, passed all 22 cases, including the resize case, with exit 0. Command: `cargo test --locked -p tty59-cli --test cli_e2e`. Logs: `.pr-assets/B2/full-gate-round19/cli-rerun/stdout.log` and `stderr.log` in the fx-b worktree.

The complete gate rerun with the original workspace command and feature graph passed: 5,005 tests, zero failures and 11 ignored, with formatting and the warnings-denied build also passing. Its logs are under `.pr-assets/B2/full-gate-recheck-round23/raw/`. No assertion was changed and no test was excluded.

At the time, the status was FLAKY with the cause not established. The newest segment at tests.log:3133 starts with `ESC[?2004h` followed by carriage return and erase-line and a prompt redraw, which matches the cause above.

## A4 follow-up trace

The same assertion failed during A4's Commit-header follow-up on retained Boat VM `bx_b2qz2d47`, using the frozen `ab68524c` tree plus `ui24-commit-header-full-v2.patch` (`53d91decaaf86c076cdd3b92553a4d935195aeeb3c8b0b0798d51b12cf72804a`). The CLI e2e source blob is `3031bc1074eace13ade3414bf5e10b1490898f79`, identical to `ui-overhaul`. The correction changes only the Diff header and its UI regression.

Formatting and the warnings-denied build passed. All 2,908 app tests passed. The CLI resize test failed at line 992 because its newest capture started with bracketed-paste-on followed by carriage-return and erase-line, while that exact prefix was absent from the full ring's suffix. The before/after whole-ring check accepted those snapshots as stable. This is the same assertion as the B2 report; at the time the trace did not establish the cause.

The complete failed run and original assertion text are preserved in `.pr-assets/A4/reviews/commit-header-v1/workspace-failed-v1.tgz`, member `a4-commit-workspace-v1/workspace.log`. The failed run is not the A4 gate. No CLI code, assertion or test selection was changed. The orchestrator requested one complete rerun on the same frozen source.

The unchanged complete rerun passed with 5,254 tests, zero failures and 17 ignored in the standard Rust summaries, plus all 22 custom CLI e2e cases. Formatting and the warnings-denied build passed again. All 999 manifested files matched the same frozen source afterward. Evidence: `.pr-assets/A4/reviews/commit-header-v1/workspace-clean-v2.tgz`. No CLI assertion was changed or excluded.

## A2 paired baseline and CI follow-up

Recorded 2026-11-17. The earlier A2 comparison ran ten focused serial instances per side on one Boat VM, using exact base `78501af364f1c55b83107d0b4ab8f63534cdd4a3` and the frozen A2 candidate:

| Side | Passed | Failed |
| --- | ---: | ---: |
| Exact base | 6 | 4 |
| A2 | 8 | 2 |

Every failure reached the suffix assertion at line 992. There were no wrong-site failures, timeouts, anomalies or orphans. These small samples establish neither a failure rate nor an A2 improvement. The paired campaign also ran the SCM witness ten times per side, for 40 total records, so the 40-record comparison included both CLI and SCM cases and is not 40 CLI resize samples. Its stream SHA-256 is `24c76b57b549a7d06206cb2a2920ec0632675402b429d87d5fbffebc0989b785`.

The parent approved a temporary name filter in throwaway CLI harness copies because this target has a custom main rather than a libtest filter. Test bodies were byte-identical, the filter was removed and source/modes verified before full runs. That five-line insertion shifts the assertion to line 997 in comparison logs; the unchanged original site is line 992. The report is retained at `.tty59/worktrees/fx-e/.pr-assets/A2/review/common-flake-comparison.md`.

The follow-up to PR #40 reproduced the same assertion during the warnings-denied default-member run. Base is `b51669b4f243e7635cef79ff6145ab80235fa29d`. There is no CLI or other crate source delta in this follow-up. Its test-only candidate before this note has source manifest `c0e470aeddef30eecda0514c7930be8962b5fee397b80b23829b76b052c16ea6`.

On the same VM and candidate, the full workspace leg passed 6,109 tests with zero failures and the same 17 ignored tests, including this CLI test. The subsequent default-member command failed its CLI E2E target:

```text
cargo --config 'build.rustflags=["-D","warnings"]' test --locked --no-fail-fast
```

The exact panic is again at line 992, with the same suffix message. The captured newest string begins with prompt-redraw control sequences; the whole ring carries the marker and prompt output, but does not end with that raw newest string. The failed gate is retained, not counted green. Its stream hashes to `a15e2c86daf8fd49fb697a8596495b39ad6a3f506dfd6985a995253321836737`. Raw evidence is under `.tty59/worktrees/fx-e/.pr-assets/A2/ci-followup/`, with the source/mode checks and process cleanup. The orchestrator confirmed this is the previously observed site and authorized one unchanged retry of the default-member run only; no name filter, skip, allow or CLI edit was used for that retry.
