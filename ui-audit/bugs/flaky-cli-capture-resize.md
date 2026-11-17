# CLI capture suffix after resize

Status: unresolved intermittent failure, recorded 2026-11-17. No CLI production change, skip or assertion relaxation is included in A2 for this finding.

## Symptom

`capture_still_answers_after_a_resize` fails at `crates/tty59-cli/tests/cli_e2e.rs:992`: the default capture's newest segment is not a suffix of the whole raw ring. The test sends a marker, resizes from 100 to 80 columns, reads the whole ring before and after the other captures, and checks the suffix only when those whole-ring readings match and both raw/plain captures contain the marker.

The retained failure message is `the newest segment has to be the end of the ring it came from`. Before/after ring equality is part of the existing test, not evidence that the implementation's resize and prompt-redraw state is atomic. The root cause remains unproven.

## Paired baseline evidence

The earlier A2 comparison ran ten focused serial instances per side on one Boat VM, using exact base `78501af364f1c55b83107d0b4ab8f63534cdd4a3` and the frozen A2 candidate:

| Side | Passed | Failed |
| --- | ---: | ---: |
| Exact base | 6 | 4 |
| A2 | 8 | 2 |

Every failure reached this suffix assertion. There were no wrong-site failures, timeouts, anomalies or orphans. These small samples establish neither a failure rate nor an A2 improvement. The paired campaign also ran the SCM witness ten times per side, for 40 total records. Its stream SHA-256 is `24c76b57b549a7d06206cb2a2920ec0632675402b429d87d5fbffebc0989b785`.

The parent approved a temporary name filter in throwaway CLI harness copies because this target has a custom main rather than a libtest filter. Test bodies were byte-identical, the filter was removed and source/modes verified before full runs. That five-line insertion shifts the assertion to line 997 in comparison logs; the unchanged original site is line 992. The report is retained at `.tty59/worktrees/fx-e/.pr-assets/A2/review/common-flake-comparison.md`.

## A2 CI-follow-up occurrence

On 2026-11-17, the follow-up to PR #40 reproduced the same assertion during the warnings-denied default-member run. Base is `b51669b4f243e7635cef79ff6145ab80235fa29d`. There is no CLI or other crate source delta in this follow-up. Its test-only candidate before this note has source manifest `c0e470aeddef30eecda0514c7930be8962b5fee397b80b23829b76b052c16ea6`.

On the same VM and candidate, the full workspace leg passed 6,109 tests with zero failures and the same 17 ignored tests, including this CLI test. The subsequent default-member command failed its CLI E2E target:

```text
cargo --config 'build.rustflags=["-D","warnings"]' test --locked --no-fail-fast
```

The exact panic is again at line 992, with the same suffix message. The captured newest string begins with prompt-redraw control sequences; the whole ring carries the marker and prompt output, but does not end with that raw newest string. This observation does not isolate which snapshot/projection or timing edge is responsible.

The failed gate is retained, not counted green. Its stream hashes to `a15e2c86daf8fd49fb697a8596495b39ad6a3f506dfd6985a995253321836737`. Raw evidence is under `.tty59/worktrees/fx-e/.pr-assets/A2/ci-followup/`, with the source/mode checks and process cleanup. The orchestrator confirmed this is the previously observed site and authorized one unchanged retry of the default-member run only. No name filter, skip, allow or CLI edit is used for that retry; another failure must stop and be reported.

The single authorized retry also failed at line 992 with the identical suffix message and prompt-redraw shape, at 2026-11-17 03:37 UTC. No other target failed. The default-member gate remains failed; the agent stopped without another retry, commit or push. All 1,070 prior files were verified unchanged before this note was added, and all 1,071 files and modes remained exact through the retry. Its stream SHA-256 is `2936d0a1923c472b63b762cfe6586ec1823b0b8e558b101f32f22593837e5dd2`. This paragraph records the result after the run; it changes no executable source or test.

## Follow-up investigation

Investigate snapshot consistency and raw segment boundaries around resize and shell prompt redraw. Retain the real resize, marker, whole-ring stability checks and raw/plain observations. Do not replace the suffix assertion with a weaker predicate merely to make the test green. A future fix needs a distinguishing regression and baseline/candidate evidence on the same runner.
