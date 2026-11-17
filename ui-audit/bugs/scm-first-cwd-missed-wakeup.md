# A pane's directory arrives without asking for a frame

Status: pre-existing panel bug, found 2026-11-17 while diagnosing the macOS CI failure of `the_first_failure_repaints_by_itself` (run 44011873948, job 133800592245). Present on upstream `9ff90f7e` and on `e1bfc644`; not introduced by A2.

## Symptom

The Changes panel can sit on "No working directory" after the pane has learned its directory, until something else repaints. Nothing the panel does is waiting for the status: the chain that reads the repository root and then the status never starts, because frame 1 saw no directory and no event asks for frame 2.

## Cause

`DaemonMsg::Cwd` is handled on the pane's reader thread (`src/terminal/remote.rs`, the `DaemonMsg::Cwd` arm). It stores the path in a mutex and sends no event. Several existing arms that change visible state (`Size`, `AuthPrompt`, `SshStatus`, `AgentStatus`, `Lease`) send `VtEvent::Wakeup`, which the view turns into `cx.notify()` for a displayed pane. The only later repaint is `TerminalView::poll_foreground`, a 300ms timer that sets `git_status_cwd`, probes, and so repaints the window.

`Tty59App::scm_pane_target` reads `effective_host_cwd()`, which is `git_status_cwd` when set and otherwise the mutex. So the case this note covers is the first directory (None to Some); for a later change `git_status_cwd` still holds the previous directory until the poll runs, and a wakeup alone would not change what the panel reads.

## Actual latency

Timer-dependent, and not a measured bound. In the app the catch-up waits for the next tick of the real 300ms poll and then for the scheduling and the probe it starts, so it is about that long plus whatever those cost on the machine. Under a virtual clock that nobody advances it never catches up.

The original witness races the pane's reader thread against the first frame drawn inside `open_changes`. On an unloaded runner the thread wins and the witness passes (10/10 at 0.10-0.12s on both trees). The probe forces the other ordering deterministically on both trees, and that ordering explains the symptom: a full 20s `drawn_unprompted` wait with nothing to repaint the panel.

What the macOS log shows is the same assertion failing after that full wait (it ended at 03:51:29.985, the predecessor finished at 03:51:09.814, and sibling tests that notify every loop passed in 1-3s). It does not record the panel's root counters or the order in which the Cwd and the first frame happened, so that this was the late-Cwd ordering on that runner is inferred from the probe, not observed there.

## Evidence

Deterministic probe on Boat, same source on both trees (panel module SHA-256 `b61ebd21ec476da19dc1f1993f142addb0157e0d6c13ca96976cfd2e07766e4f`): frame 1 is drawn with no directory, the `Cwd` message is then sent, the test waits on the real clock until the live pane reports it (no notify, no draw, no clock advance), and then waits for Retry with no further help.

| Tree | Untouched witness | Controls | Late-Cwd probe | Pane poll only |
| --- | --- | --- | --- | --- |
| `9ff90f7e` | 10/10 | 17/17 | fails at its own assertion, 5.05s | passes, 0.43s |
| `e1bfc644` | 10/10 | 17/17 | fails at its own assertion, 5.04s | passes, 0.40s |

The poll-only control only supports recovery by timers or the pane's poll, not that nothing else is missing.

## Fix and tests

A `Cwd` frame that moves the pane's directory sends one `Wakeup`; a repeated `Cwd` sends none. No panel, view or default changes.

- `terminal::remote::replay_tests::a_cwd_that_moved_wakes_the_view_and_a_repeat_does_not` sends Cwd A, Cwd A, Cwd B and `Exited`, reads until the first `Exit`, and requires exactly `[Wakeup, Wakeup, Exit]` and the final directory B. The first `Exit` is the engine's own, sent by the teardown's `exit()` before the teardown's `Wakeup`, so it is the barrier after every Cwd frame and the teardown's `Wakeup` is not counted. Two events precede it: one for the first directory and one for the change, none for the repeat. The order is the barrier, so it does not rely on a timeout.
- `ui::scm::panel::status_lifecycle_witness_gpui_tests::a_first_cwd_after_the_first_frame_repaints_by_itself` is the probe above, kept as a regression.

The original `the_first_failure_repaints_by_itself` is unchanged.

## Windows occurrence on a tree with no A2 change

PR #41 (a CLI-test-only change, head `80ca680f` on base `9ff90f7e`) failed the original witness on Windows at `src\ui\scm\panel.rs:7294`, after the same full wait (the predecessor finished at 04:58:24.664 and the witness failed at 04:58:44.803): https://github.com/nathabonfim59/tty59/actions/runs/44019348809/job/133824874025. So the failure is not macOS-only and does not need A2. Like the macOS log, it does not record the Cwd order, so the late-Cwd ordering there is inferred from the probe.

## Component results (Boat, Linux)

- Old arm, corrected tests: the new unit fails at its own assertion (`[Exit]` against `[Wakeup, Wakeup, Exit]`) and the panel regression fails at its own assertion (`(root_lookups, roots) = (0, 0)`); the original witness passes.
- With the arm: both new tests pass; the focused remote, SCM and idle/quiesce selection passed 361 with none failed or skipped; the original witness and the new regression each passed 10/10; the restored four-test run passed.
- Mutants: suppressing the `Wakeup` fails both the unit and the panel regression; a duplicated `Wakeup` fails the unit while the panel regression and the DA1 controls stay green.

## Limits

This is Linux evidence. Whether the native macOS and Windows runners lose the race the same way is left to CI. Only the first directory (None to Some) is claimed to repaint at once; a later change still waits for the poll. The catch-up latency in the app is not measured.
