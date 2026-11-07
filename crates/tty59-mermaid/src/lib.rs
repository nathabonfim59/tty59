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

use std::fmt;

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
}

impl MermaidTheme {
    /// A neutral light theme. Part A fills in the palette.
    pub fn light() -> Self {
        todo_theme(false)
    }

    /// A neutral dark theme. Part A fills in the palette.
    pub fn dark() -> Self {
        todo_theme(true)
    }
}

// FOUNDATION STUB — part A replaces this with real light/dark palettes.
fn todo_theme(dark_mode: bool) -> MermaidTheme {
    let (bg, fg) = if dark_mode {
        (Rgba::rgb(0x1e, 0x1e, 0x1e), Rgba::rgb(0xe6, 0xe6, 0xe6))
    } else {
        (Rgba::rgb(0xff, 0xff, 0xff), Rgba::rgb(0x1e, 0x1e, 0x1e))
    };
    MermaidTheme {
        dark_mode,
        font_family: "sans-serif".into(),
        background: bg,
        text: fg,
        primary: bg,
        primary_text: fg,
        primary_border: fg,
        secondary: bg,
        tertiary: bg,
        line: fg,
        edge_label_background: bg,
        cluster_background: bg,
        cluster_border: fg,
        note_background: bg,
        note_border: fg,
        actor_background: bg,
        actor_border: fg,
        error: Rgba::rgb(0xdc, 0x26, 0x26),
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
    // FOUNDATION STUB — part A implements this.
    let _ = (source, theme);
    Err(MermaidError::Render("not implemented yet".into()))
}

/// SVG → straight-alpha BGRA at `scale` (device pixels per SVG unit; the GUI
/// passes the window's scale factor). See [`MAX_PIXELS`] for the size cap.
/// Text is laid out with the system fonts (loaded once per process).
pub fn rasterize(svg: &str, scale: f32) -> Result<Raster, MermaidError> {
    // FOUNDATION STUB — part A implements this.
    let _ = (svg, scale);
    Err(MermaidError::Render("not implemented yet".into()))
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
