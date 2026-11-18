//! The hex editor: a byte-level view of any file, next to the text editor and the image
//! viewer. `model` is the pure part (pages, edits, undo, search, parsers); the host IO, the
//! per-tab state and the view build on it.

pub(super) mod model;
