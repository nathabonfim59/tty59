//! Showing an image file in the editor's own place.
//!
//! [`decode`] decides what bytes are and turns a picture into pixels under
//! the viewer's limits; nothing in it touches gpui, so it tests headless.

pub(crate) mod decode;
pub(crate) mod view;

pub(crate) use view::{ImageData, ImageView};
