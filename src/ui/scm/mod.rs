//! Source control: the panel, its file rows, the commit box, and the graph.
//!
//! Every file here hangs `impl Tty59App` blocks, the same shape `sftp.rs` and
//! `file_tree.rs` use. The directory only keeps the surface from piling into
//! `right_panel.rs`.

// The graph and the commit detail view both have callers now, so their allows
// are gone. What is left is `status_rank`, which is the file tree's to use.
pub(crate) mod actions;
pub(crate) mod detail;
pub(crate) mod graph;
pub(crate) mod panel;
pub(crate) mod path;
mod row_menu;
#[cfg(test)]
mod rows_guard_tests;
#[cfg(test)]
mod rows_info_tests;
#[cfg(test)]
mod rows_keys_tests;
#[cfg(test)]
mod rows_keyup_header_tests;
#[cfg(test)]
mod rows_keyup_tests;
#[cfg(test)]
mod rows_landing_fanout_tests;
#[cfg(test)]
mod rows_notarepo_fanout_tests;
#[cfg(test)]
mod rows_notarepo_tests;
#[cfg(test)]
mod rows_ownop_tests;
#[cfg(all(test, unix))]
mod rows_ring_inside_tests;
#[cfg(test)]
mod rows_shared_tests;
#[cfg(test)]
mod rows_stale_tests;
pub(crate) mod rows_window;
#[cfg(test)]
mod rows_window_tests;
pub(crate) mod state;
#[allow(dead_code)]
pub(crate) mod status;
pub(crate) mod tree;

pub(crate) use actions::ScmIntent;
pub(crate) use state::{GraphState, ScmPanelState};
