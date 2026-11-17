# PowerShell PTY test: PSReadLine runtime-directory failure

CI run 44011873948 on A2 commit e1bfc644 failed the Linux test `daemon::shell_integration::tests::pwsh_reports_the_full_prompt_cycle_over_a_real_pty_on_unix` at `crates/tty59-core/src/daemon/shell_integration.rs:2625`. It expected OSC 133;C after typing a command. PowerShell instead printed "Oops, something went wrong." twice.

The captured exception came from PSReadLine 2.4.5 on PowerShell 7.6.10, Ubuntu 26.04.1. `System.Threading.Mutex.CreateMutexCore` could not create its `PSReadLineHistoryFile_482274945` mutex. `mkdirat` for `/run/user/1001/.dotnet/shm/session50738` returned `ENOENT`. The log does not establish why that runtime directory was missing.

There is no `crates/tty59-core` diff from A2's frozen upstream 9ff90f7e to e1bfc644. A2 does not change this test or the PowerShell integration. This occurrence is recorded as an apparent separate runner/PSReadLine failure, not a proved application defect or a fixed flake. The normal Linux gates run the test unchanged. A recurrence needs investigation before changing its assertions or setup.

Evidence: [Linux job 133800592250](https://github.com/nathabonfim59/tty59/actions/runs/44011873948/job/133800592250). The retained full job log hashes to `0e134e2031a33dfa3064363d978628b8c50139d5096401dd112a27f76ac98742`.

It recurred in the timing-only diagnostic branch `8e659cdb`, [run 44019764170, Linux job 133826243264](https://github.com/nathabonfim59/tty59/actions/runs/44019764170/job/133826243264). The only change from e1bfc644 was SCM test timing output. All 3,720 app tests passed; the same core assertion failed, with the same versions and named-mutex exception, now for `/run/user/1001/.dotnet/shm/session50599`. The recurrence was reported before any PowerShell change. Its cause remains unisolated.

The separate test-isolation fix is [PR #42](https://github.com/nathabonfim59/tty59/pull/42). It gives that PowerShell child private HOME and XDG directories; its owner reports RED/GREEN and 50/50 repetitions on Boat with PowerShell 7.6.10 and PSReadLine 2.4.5. A2 contains no PowerShell code or test changes.
