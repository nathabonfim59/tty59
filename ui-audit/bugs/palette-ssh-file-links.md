# File links in palette SSH panes

Follow-up idea; outside the image-preview change.

An SSH pane opened from the command palette can browse files through SFTP,
but its terminal file links are suppressed. This is inherited behavior, also
affecting text files. It must not fall back to the local filesystem when a
remote path happens to exist locally.

Observed on the image-preview candidate built from `54c7057e`: the palette
SSH connection succeeded, but modifier-clicking a neutral absolute image
path produced neither a document nor an SFTP stat/read for that file. The
same image opened locally. The image candidate's app SHA-256 was
`66397ba8995f70e7708755b0f40a3783f62937cacc377ddcc788d416c136e855`.
No image-decoder failure was observed in that attempt.

The current source explains the boundary:

- `TerminalView::set_workspace` obtains its `HostId` from the workspace.
  A palette SSH pane in a local workspace still has the local host ID.
- `TerminalView::cwd_is_on_host` rejects a pane whose remote context does
  not match that host. `resolve_link_at` consequently disables file probes.
  The existing `a_pane_whose_paths_are_elsewhere_does_not_link_local_files`
  regression protects this rule.
- The SFTP browser obtains a separate `SftpHost` in `sftp_editor_host` and
  passes that handle directly to `editor_open_on_host`.
- A connected remote workspace uses a registered `RemoteHost`, which is
  already the authority for its terminal links and editor reads.

A future change would need to give an authoritative native SSH route to
both the terminal probe and the open request. The open event would need to
carry that host instead of assuming the active workspace's host. It would
also need to invalidate cached probes when the route changes and preserve
the distinction between a native SSH pane and a further SSH command typed
inside it. A nested connection without its own accessible host must remain
unresolved.

Tests should cover identical local/remote path spellings with different
contents, route changes during an outstanding probe/read, disconnects,
nested SSH, and the file-open mode policy. Unknown probes must not create
an underline or authorize a local fallback.
