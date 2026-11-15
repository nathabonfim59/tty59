# Flaky: a dropped file's test read sees an empty destination

Pre-existing test timing issue in `src/ui/file_tree.rs`, outside the Diff header work that exposed it. No production code is changed by the fix below.

## Symptom

`a_dropped_file_is_copied_in_and_shows_up_in_the_tree` failed once in a full workspace run (5252 passed, 1 failed, 17 ignored): the final assertion read the destination file and found it empty where it expected `hello`.

## Evidence

- Run alone, on the same source, the test passes: Boat command `cmd_ozvccazkhpbf`, 1 passed in 0.56 s.
- The test source is byte-identical to `ui-overhaul`: blob `333b1f0601ef3fd8549936f6b76ebda4d7e7ebf9`.
- The test waited for the copy with `root.join("note.txt").exists()` and then read the file immediately.

## Cause

The copy runs on a `HostOps` worker, a real OS thread the test executor does not own. `copy_file` in `src/ui/file_copy.rs` copies with `std::fs::copy` (or `File::create` and `io::copy` on a WSL share), which creates the destination before it has written its bytes. The file can therefore exist, and satisfy the wait, while still empty. The failure is consistent with the read reaching that window; the exact interleaving was not captured.

`HostOps` exposes no completion signal to tests (its pool state is private), so the test cannot wait on the job itself.

## Fix

Wait on what the test asserts: the predicate in the same 20 s `wait_until` helper reads the file and is true only when its content is `hello`. The final content and tree-row assertions are unchanged.
