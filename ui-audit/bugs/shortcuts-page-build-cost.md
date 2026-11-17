# Shortcuts page build cost

Status: follow-up optimization idea, 2026-11-16. The orchestrator accepted this measured cost for the A2 milestone. No optimization or new idle campaign is included in A2.

## Measured result

A matched comparison on one fresh dedicated Boat VM completed50 valid runs,5 repetitions per side in each of5 common scenes, plus2 valid warm-ups. Exact base78501af3 binary SHA-256 `250942bef63ec985bf8933cd717e47b249a6354ed01cb3ce8817056e8a1df524` was compared with A2 binary `63423fdc75b56c5682a94ecb3bc737f967a02e0c00ceafd915d3303261f6905f`. The A2 source manifest is `0d33f2ba7a3432c94bc7323bcbbc690d828d7df1ff42acaa981466f7c1d411a4`.

On the Keyboard shortcuts page, median page-build time rose from2.856 to3.876ms, an increase of1.020ms or35.7%. The five before values span2.834..3.146ms; after values3.487..4.022ms. Those observed ranges are disjoint. The profiler measures element-tree construction, not layout, painting, end-to-end interaction latency or a whole frame.

Median app CPU during6-second samples rose from34.27% to39.19% of one core, an increase of4.92 percentage points or14.4% by ratio of medians. All Shortcuts samples contain24 hover moves between the same two existing chord cells, at a250ms cadence. Every same-repetition CPU pair is higher after, though the full before/after CPU ranges overlap. Median app RSS changed by+0.336MiB. All other scene CPU/build ranges overlap.

The controller required all50 unique rows, stable app identity, frozen environment, valid page labels, original-reference RMSE≤0.02 and residual CPU noise≤30% of one core. Maximum recorded noise was16.96%; all40 non-idle samples issued24 moves. The final audit found no live app, Xvfb, Cargo or rustc process left. Earlier incomplete noisy campaigns were rejected and were not mixed into the result.

## Idle CPU is not established

The existing data does not contain a no-input CPU interval with the Shortcuts page open. Its CPU measurements all include hover input. The separate idle scene measures only the main window:6.50% before and6.83% after, with overlapping ranges. Those values cannot establish idle Shortcuts CPU or be substituted for it.

The page-build timings are costs per actual build invocation during the measured interaction workload. They do not prove that the extra cost stops when input stops, because application notifications could also cause builds while the page is idle. The focus/reveal source has event-owned work and measurement during render/prepaint, but source reading is not an idle CPU measurement. We cannot confirm from this campaign that idle Shortcuts CPU matches base. No new campaign was run for that question, as instructed.

## Possible investigation

The comparison changes the whole A2 tree and toolkit pin, so it does not isolate the extra millisecond to a particular helper. Per-cell focus state, shared ring wrappers, measurement canvases and metadata construction are plausible profiling targets, not proven causes. Explore lazy creation or lighter per-cell state/measurement only after attributing the cost. The current handles are keyed and reused; any change must preserve stable identity, real Tab traversal to offscreen cells, reveal without explicit draws, recorder behavior, Reset self-unmount and separate SettingsWindow lifetime.

A follow-up should measure no-input page CPU separately from hover/keyboard interactions and record build frequency as well as build duration. Use matched source variants and retain all existing focus/reveal/recording controls. Do not trade away keyboard reachability to remove the cost. Native/release profiling is separate from this debug/software-GL result.

## Evidence

The full report is `.tty59/worktrees/fx-e/.pr-assets/A2/perf-final/review.md`. The dedicated campaign stream SHA-256 is `30739c3179b002f99958774398bf9944d1b28d9a6b9a483b2492bb62ff973c7f`. Raw `runs.tsv` hashes to `597aeadf2a9abf1c29ceaa39102d81c50caf053efe3412a554cc18e6100032d0`; `labels.tsv` to `19697dc065e59ad99a3d3798db872ee7fc22e32eb419a6a84614bf8ac07dc6d2`; `summary.txt` to `892e7b5caf8ce6ea9a84af97766b6a6896bf04acc352d552f86840819492ace4`. They and the actual scene/ownership logs are retained under `perf-final/dedicated/a2p.1ia8IF/` in that worktree.

Environment: fresh8-CPU/16GB Boat VM bx_64k4n8qf, debug binaries, software GL, Xvfb1440x900 at1x, dark theme. Both sides use the same VM, runtime/fonts, initial resize and input cadence. The initial resize is the performance-only procedure documented separately in `startup-blank-until-resize.md`. Native desktop latency and idle Shortcuts CPU remain unmeasured.
