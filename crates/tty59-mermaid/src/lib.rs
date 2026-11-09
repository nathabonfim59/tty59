//! Mermaid diagrams for tty59's markdown preview.
//!
//! Two steps, both pure and safe to run on a background thread:
//!
//! 1. [`render_svg`]: mermaid source + [`MermaidTheme`] → an SVG string that
//!    `usvg`/`resvg` can draw (merman's raster-safe pipeline, themed from
//!    the theme's colors).
//! 2. [`rasterize`]: that SVG → [`Raster`], straight-alpha **BGRA** pixels at a
//!    given scale — the layout gpui's `RenderImage` frames take, so the GUI
//!    side (`ui::markdown_mermaid`) only wraps them.
//!
//! [`render`] does both. Nothing here depends on gpui.
//!
//! This crate is written from scratch on [`merman`] (MIT OR Apache-2.0). Zed's
//! `mermaid_render` crate covers the same ground but is GPL-3.0-or-later:
//! never copy code from it here.
//!
//! # Contract (frozen; changes go through the orchestrator)
//!
//! - Every public item below keeps its signature.
//! - No function panics on any input: bad mermaid, empty input, huge
//!   diagrams and malformed SVG all come back as [`MermaidError`].
//! - [`rasterize`] never allocates more than [`MAX_PIXELS`] pixels: a diagram
//!   whose scaled size exceeds it is drawn at the largest scale that fits
//!   (the returned [`Raster::scale`] says which), never refused.
//! - Output is deterministic for the same input (no clocks, no randomness),
//!   so the GUI can cache by `(source, theme, scale)`.
//! - [`MermaidTheme::accents`] empty means the neutral look, byte-for-byte
//!   what it was before accents existed; non-empty colors each shape from the
//!   list, deterministically.

mod raster;

use std::fmt;

mod svg;

/// An sRGB color with straight (non-premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// `#rrggbb` when opaque, `#rrggbbaa` otherwise — the form merman's
    /// theme variables and CSS both accept.
    pub fn to_css(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

/// The colors and font a diagram is drawn with. The GUI fills this from the
/// active tty59 theme; [`MermaidTheme::light`] / [`MermaidTheme::dark`] are
/// neutral defaults for tests and as a base to override.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MermaidTheme {
    pub dark_mode: bool,
    /// A CSS font-family list, e.g. `"Inter, ui-sans-serif, sans-serif"`.
    pub font_family: String,
    /// The preview's background. Diagrams draw on a transparent canvas;
    /// this is what merman blends edge-label and fill colors against.
    pub background: Rgba,
    /// Body text: labels, titles, edge text.
    pub text: Rgba,
    /// Node fill / text / border.
    pub primary: Rgba,
    pub primary_text: Rgba,
    pub primary_border: Rgba,
    pub secondary: Rgba,
    pub tertiary: Rgba,
    /// Edges and arrows.
    pub line: Rgba,
    pub edge_label_background: Rgba,
    /// Subgraph boxes.
    pub cluster_background: Rgba,
    pub cluster_border: Rgba,
    /// Sequence-diagram notes and actors.
    pub note_background: Rgba,
    pub note_border: Rgba,
    pub actor_background: Rgba,
    pub actor_border: Rgba,
    /// Errors and critical markers inside diagrams.
    pub error: Rgba,
    /// Per-shape accent colors: when non-empty, the renderer gives the
    /// diagram's shapes (flowchart nodes, sequence participants, class and
    /// state boxes, mindmap branches, …) distinct colors by cycling through
    /// this list in a stable order, so the same diagram always colors the
    /// same shape the same way. Empty = the neutral look (every shape uses
    /// `primary`/`primary_border`/`primary_text`).
    pub accents: Vec<Accent>,
}

/// One accent: a shape's fill, its outline and the text drawn on it. The
/// GUI derives these from the active tty59 theme; `text` must stay readable
/// on `fill`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Accent {
    pub fill: Rgba,
    pub border: Rgba,
    pub text: Rgba,
}

/// The default font stack. Proportional faces are deliberate: merman's
/// deterministic text measurer estimates Arial-like advances, and resvg's
/// `system-fonts` fallback resolves these names to a close installed face.
const DEFAULT_FONT_FAMILY: &str = "Arial, Liberation Sans, sans-serif";

impl MermaidTheme {
    /// A neutral light theme.
    pub fn light() -> Self {
        Self {
            dark_mode: false,
            font_family: DEFAULT_FONT_FAMILY.into(),
            background: Rgba::rgb(0xff, 0xff, 0xff),
            text: Rgba::rgb(0x1f, 0x23, 0x28),
            primary: Rgba::rgb(0xf6, 0xf8, 0xfa),
            primary_text: Rgba::rgb(0x1f, 0x23, 0x28),
            primary_border: Rgba::rgb(0xd0, 0xd7, 0xde),
            secondary: Rgba::rgb(0xea, 0xee, 0xf2),
            tertiary: Rgba::rgb(0xf6, 0xf8, 0xfa),
            line: Rgba::rgb(0x57, 0x60, 0x6a),
            edge_label_background: Rgba::rgb(0xff, 0xff, 0xff),
            cluster_background: Rgba::rgb(0xf6, 0xf8, 0xfa),
            cluster_border: Rgba::rgb(0xd0, 0xd7, 0xde),
            note_background: Rgba::rgb(0xff, 0xf8, 0xc5),
            note_border: Rgba::rgb(0xd4, 0xa7, 0x2c),
            actor_background: Rgba::rgb(0xf6, 0xf8, 0xfa),
            actor_border: Rgba::rgb(0xd0, 0xd7, 0xde),
            error: Rgba::rgb(0xcf, 0x22, 0x2e),
            accents: Vec::new(),
        }
    }

    /// A neutral dark theme.
    pub fn dark() -> Self {
        Self {
            dark_mode: true,
            font_family: DEFAULT_FONT_FAMILY.into(),
            background: Rgba::rgb(0x0d, 0x11, 0x17),
            text: Rgba::rgb(0xe6, 0xed, 0xf3),
            primary: Rgba::rgb(0x16, 0x1b, 0x22),
            primary_text: Rgba::rgb(0xe6, 0xed, 0xf3),
            primary_border: Rgba::rgb(0x30, 0x36, 0x3d),
            secondary: Rgba::rgb(0x21, 0x26, 0x2d),
            tertiary: Rgba::rgb(0x16, 0x1b, 0x22),
            line: Rgba::rgb(0x8b, 0x94, 0x9e),
            edge_label_background: Rgba::rgb(0x0d, 0x11, 0x17),
            cluster_background: Rgba::rgb(0x16, 0x1b, 0x22),
            cluster_border: Rgba::rgb(0x30, 0x36, 0x3d),
            note_background: Rgba::rgb(0x3d, 0x2f, 0x00),
            note_border: Rgba::rgb(0x9e, 0x6a, 0x03),
            actor_background: Rgba::rgb(0x16, 0x1b, 0x22),
            actor_border: Rgba::rgb(0x30, 0x36, 0x3d),
            error: Rgba::rgb(0xf8, 0x51, 0x49),
            accents: Vec::new(),
        }
    }
}

/// The largest raster [`rasterize`] will allocate, in pixels (width × height).
/// 4096 × 4096: well past any diagram that fits a preview pane at 2× scale,
/// and under gpui's texture-atlas limits.
pub const MAX_PIXELS: u64 = 4096 * 4096;

/// Straight-alpha BGRA pixels, row-major, `width * height * 4` bytes.
#[derive(Clone, PartialEq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// The scale the SVG was drawn at. The diagram's logical size is
    /// `width / scale` × `height / scale`; the GUI lays the image out at that
    /// size. Equals the requested scale unless [`MAX_PIXELS`] capped it.
    pub scale: f32,
    pub bgra: Vec<u8>,
}

impl fmt::Debug for Raster {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Raster")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("scale", &self.scale)
            .field("bytes", &self.bgra.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MermaidError {
    /// The source isn't valid mermaid (or is empty). The message is merman's,
    /// cleaned up to one or a few lines a user can act on.
    #[error("{0}")]
    Syntax(String),
    /// merman parsed the source but produced no SVG, or the SVG couldn't be
    /// parsed/drawn.
    #[error("could not render diagram: {0}")]
    Render(String),
}

/// Mermaid source → themed, raster-safe SVG.
pub fn render_svg(source: &str, theme: &MermaidTheme) -> Result<String, MermaidError> {
    svg::render(source, theme)
}

/// SVG → straight-alpha BGRA at `scale` (device pixels per SVG unit; the GUI
/// passes the window's scale factor). See [`MAX_PIXELS`] for the size cap.
/// Text is laid out with the system fonts (loaded once per process).
pub fn rasterize(svg: &str, scale: f32) -> Result<Raster, MermaidError> {
    raster::rasterize(svg, scale)
}

/// [`render_svg`] then [`rasterize`].
pub fn render(source: &str, theme: &MermaidTheme, scale: f32) -> Result<Raster, MermaidError> {
    rasterize(&render_svg(source, theme)?, scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_colors() {
        assert_eq!(Rgba::rgb(0x12, 0xab, 0x0f).to_css(), "#12ab0f");
        assert_eq!(
            Rgba {
                r: 1,
                g: 2,
                b: 3,
                a: 4
            }
            .to_css(),
            "#01020304"
        );
    }
}
