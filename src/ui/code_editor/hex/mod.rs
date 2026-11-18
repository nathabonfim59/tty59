//! The hex editor: a byte-level view of any file, next to the text editor and the image
//! viewer. `model` is the pure part (pages, edits, undo, search, parsers); `io` is the host's
//! half (reading pages, saving); the per-tab state and the view build on them.

pub(super) mod data;
pub(super) mod io;
pub(super) mod model;
#[cfg(test)]
mod test_support;
pub(super) mod view;
