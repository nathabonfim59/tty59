# First window can stay black until resized under Xvfb

Status: open investigation, 2026-11-16. Observed on exact base `78501af3` and the frozen A2 candidate. This may be a pre-existing app/backend issue or an Xvfb-only issue. It is unconfirmed outside Xvfb. No app fix is included in A2.

## Observations and counts

The following two serial campaigns required the first main-window screenshot mean to exceed `0.03` before any scene input. Each launch used a fresh app, config directory, HOME and X server. The wait sampled a 64x40 downscaled root image up to 40 times, sleeping 0.5 seconds between attempts. Both campaigns stopped at their first failed launch. These are observed counts, not estimated failure rates.

| Campaign | Base launches / blank | A2 launches / blank |
| --- | --- | --- |
| Final calibration P3, first attempt | 5 / 1 | 0 / 0, not reached |
| Startup diagnostic, unchanged initial launch procedure | 5 / 0 | 2 / 1 |
| Instrumented total before the resize procedure | 10 / 1 | 2 / 1 |

The base failed on launch 5 before any Settings input. Its mean remained `0` through the wait and its app log was empty. That campaign exited 3 and accepted no performance result. The diagnostic later failed on A2 launch 2, also before any switcher input, with mean `0`. Its original screenshot is entirely black. Xwininfo nevertheless reported the main window as `IsViewable`, 1440x900 at 0,0, depth 32.

There was one additional earlier base occurrence: P2 saved an entirely black idle reference. Its normalized mean was `0`, compared with `0.103657` for the earlier proper base idle image; their normalized RMSE was `0.110795`, above the unchanged `0.02` scene bound. P2 made 5 launches per side, but its paint-wait loop did not assert success after exhausting the wait. Those launches therefore cannot supply reliable first-paint success counts and are excluded from the instrumented total. The bad reference was rejected before measurement.

## Resize recovery

On the same failing A2 process and window, the diagnostic captured the blank original, resized the width from 1440 to 1439, waited one second, restored 1440, and waited two seconds. The resulting full-size image mean was `0.103982` and the terminal/sidebar UI was visible. No key, click or application action was sent between those two images. The diagnostic still rejected that run and accepted no recovered reference.

Original PNG SHA-256 values:

- Blank: `c60e95daf500ceb9f6f7433b5ce0439040053374890f40a4a211cce22fd6cad6`.
- After resize: `c64e3d5c220aa00b754d0857b4b09f626e86b1a070a3e103b86ff941955f7abd`.

The original images were viewed with the canonical image reader. Evidence is in the A2 worktree under `.pr-assets/A2/perf-final/diagnostic1/007/`, including the two images, X window information and ownership/step logs. The diagnostic scratch on Boat is `/tmp/a2p.N4pwEy/007`. Base failure scratch is `/tmp/a2p.OzgRDm/005`.

## Environment and reproduction procedure

Same Boat VM `bx_hp5szh5y`, Ubuntu Linux kernel `6.8.0-117-generic`, 8 CPUs. Fresh X server per launch, display `:98`, invoked as `Xvfb :98 -screen 0 1440x900x24 -nolisten tcp -noreset +extension GLX`. The controller starts no window manager or compositor. It sets and verifies `Xft.dpi: 96`, clears Wayland, and runs the app with `env -i`, X11, `LIBGL_ALWAYS_SOFTWARE=1`, dark theme, English locale and `TTY59_PROFILE=1`. No forced frame-rate or GPUI scale variable is present. The app launches directly in a small seeded Git repository with its own `--config-dir` and daemon socket.

The controller waits for the pinned app executable and visible X window, then uses xdotool to move it to 0,0, size it to 1440x900, raise and focus it. A mouse move to 1,1 precedes the bounded paint wait. On the failures this procedure did not produce a painted first window. There was no app/Xvfb process left after either rejected campaign's cleanup.

Base binary SHA-256: `250942bef63ec985bf8933cd717e47b249a6354ed01cb3ce8817056e8a1df524`, exact `78501af3`, toolkit f5d9ab30. A2 binary: `63423fdc75b56c5682a94ecb3bc737f967a02e0c00ceafd915d3303261f6905f`, source manifest `0d33f2ba7a3432c94bc7323bcbbc690d828d7df1ff42acaa981466f7c1d411a4`, toolkit 0280dc24. Both are debug builds. This is not evidence that A2 introduced the problem.

## Performance-only procedure change

The orchestrator approved applying the exact 1439-to-1440 width cycle to every launch on both sides, before readiness checks, settling and sampling. Controller `31722e15a6c29ba8af34af64f32e1917d683dd6ea112a4622ac866488fce3af4` adds only that unconditional initial resize. The mandatory paint gate, image RMSE bound and complete 50-run requirement remain in force. This procedure does not establish that startup itself works without a resize, and it changes no app code. Its calibration and measurement outcomes are recorded separately.

## Logs and limits

- Failed P3 log `.pr-assets/A2/logs/perf-final-syntax-cleanup-calibration-pass3-round1-boat.log`, SHA-256 `e46f905afe944d871ad708cc7a985d0b7114c3c02a72e06824b9bd28d5a25241`.
- Diagnostic log `.pr-assets/A2/logs/perf-final-startup-diagnostic-round1-boat.log`, SHA-256 `4fe021c3fe054eca681b69711175f1485664bf669ebc514cd558fac65c08ce2b`.
- Diagnostic controller SHA-256 `607c72a5793c2056f0eb3387ed83a17f36c58a6a41c9d54f83f45ab209f50725`.

No native desktop, real GPU, compositor or window-manager reproduction has been attempted. No causal claim about GPUI, graphics initialization, focus or application render scheduling is established. The resize recovery is an observation, not a diagnosed cause. Keep any fix separate from A2.
