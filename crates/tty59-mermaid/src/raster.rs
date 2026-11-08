//! Rasterization: SVG string → straight-alpha BGRA pixels.
//!
//! The pipeline is [`usvg`] parse → bounded pixel size → [`resvg`] draw on a
//! transparent [`tiny_skia`] pixmap → premultiplied-RGBA → straight-BGRA
//! conversion. Every failure mode (bad scale, unparseable SVG, upstream
//! panic, absurd dimensions) comes back as [`crate::MermaidError::Render`];
//! nothing here panics on any input.
//!
//! Text uses one process-wide font database ([`font_stack`]) built from the
//! system fonts, with the CSS generic families pointed at faces that are
//! actually installed. merman measures diagram labels with a proportional
//! sans profile (lowercase ≈ 0.43 em, digits 0.56 em, m/w 0.78 em), so the
//! generic `sans-serif` resolves to Liberation Sans or Arial — metric
//! matches for that profile — and unknown/system families
//! (`ui-sans-serif`, `system-ui`, …) fall back to the same faces instead of
//! a serif font wider than the boxes merman sized for the text.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, OnceLock};

use resvg::tiny_skia;
use resvg::usvg;
use resvg::usvg::fontdb;

use crate::{MAX_PIXELS, MermaidError, Raster};

/// Preference order for the CSS `sans-serif` generic. Liberation Sans is
/// metrically identical to Arial and both match merman's measurement
/// profile; the rest are common defaults ordered by how close their
/// proportions stay to it.
const SANS_FAMILIES: &[&str] = &[
    "Liberation Sans",
    "Arial",
    "FreeSans",
    "DejaVu Sans",
    "Noto Sans",
    "Roboto",
    "Ubuntu",
    "Cantarell",
    "Adwaita Sans",
    "Verdana",
    "Trebuchet MS",
    "Helvetica",
];

/// Preference order for the CSS `serif` generic.
const SERIF_FAMILIES: &[&str] = &[
    "Liberation Serif",
    "Times New Roman",
    "FreeSerif",
    "DejaVu Serif",
    "Noto Serif",
];

/// Preference order for the CSS `monospace` generic.
const MONOSPACE_FAMILIES: &[&str] = &[
    "Liberation Mono",
    "DejaVu Sans Mono",
    "JetBrains Mono",
    "Noto Sans Mono",
    "FreeMono",
    "Adwaita Mono",
];

/// Maps a CSS keyword that names a *system* font (not a real family) to the
/// generic family it stands for, so `font-family: system-ui` text keeps
/// merman's sans profile instead of missing every installed face.
/// Matched case-insensitively; real family names are left alone so an
/// installed face always wins by its own name.
fn system_font_alias(name: &str) -> Option<fontdb::Family<'static>> {
    match name.to_ascii_lowercase().as_str() {
        "system-ui" | "ui-sans-serif" | "-apple-system" | "blinkmacsystemfont" => {
            Some(fontdb::Family::SansSerif)
        }
        "ui-monospace" => Some(fontdb::Family::Monospace),
        "ui-serif" | "ui-rounded" => Some(fontdb::Family::Serif),
        _ => None,
    }
}

/// The process-wide font database plus the family names the generics
/// resolved to. Built once; every [`rasterize`] call shares it.
struct FontStack {
    db: Arc<fontdb::Database>,
    /// Used as `usvg`'s default family for `<text>` without `font-family`.
    default_family: String,
}

static FONT_STACK: OnceLock<FontStack> = OnceLock::new();

fn font_stack() -> &'static FontStack {
    FONT_STACK.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();

        let sans = pick_installed(&db, SANS_FAMILIES)
            .or_else(|| scan_installed(&db, "sans"))
            .or_else(|| scan_installed(&db, ""));
        let serif = pick_installed(&db, SERIF_FAMILIES).or_else(|| scan_installed(&db, "serif"));
        let mono = pick_installed(&db, MONOSPACE_FAMILIES)
            .or_else(|| scan_installed(&db, "mono"))
            .or_else(|| scan_installed(&db, ""));

        if let Some(name) = &sans {
            db.set_sans_serif_family(name.clone());
            // Cursive/fantasy have no metric profile in merman; keep them
            // proportional sans rather than serif.
            db.set_cursive_family(name.clone());
            db.set_fantasy_family(name.clone());
        }
        if let Some(name) = &serif {
            db.set_serif_family(name.clone());
        }
        if let Some(name) = &mono {
            db.set_monospace_family(name.clone());
        }

        FontStack {
            db: Arc::new(db),
            default_family: sans.unwrap_or_else(|| "sans-serif".to_owned()),
        }
    })
}

/// First name in `names` that has a face in `db`.
fn pick_installed(db: &fontdb::Database, names: &[&str]) -> Option<String> {
    for name in names {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        if db.query(&query).is_some() {
            return Some((*name).to_owned());
        }
    }
    None
}

/// First installed family whose name contains `needle` (case-insensitive),
/// skipping mono faces unless they are what was asked for. The empty needle
/// accepts any non-mono family, so a machine with fonts but none in the
/// preference lists still gets a usable generic.
fn scan_installed(db: &fontdb::Database, needle: &str) -> Option<String> {
    db.faces().find_map(|face| {
        face.families
            .iter()
            .map(|(name, _)| name)
            .find(|name| {
                let lower = name.to_ascii_lowercase();
                lower.contains(needle) && (needle == "mono") == lower.contains("mono")
            })
            .cloned()
    })
}

/// One entry of the font-family list on a `<text>` element, mapped to what
/// fontdb understands: CSS generics stay generics, system keywords become
/// their generic, and everything else is looked up by name.
fn map_family(family: &usvg::FontFamily) -> fontdb::Family<'_> {
    match family {
        usvg::FontFamily::Serif => fontdb::Family::Serif,
        usvg::FontFamily::SansSerif => fontdb::Family::SansSerif,
        usvg::FontFamily::Cursive => fontdb::Family::Cursive,
        usvg::FontFamily::Fantasy => fontdb::Family::Fantasy,
        usvg::FontFamily::Monospace => fontdb::Family::Monospace,
        usvg::FontFamily::Named(name) => {
            system_font_alias(name).unwrap_or(fontdb::Family::Name(name.as_str()))
        }
    }
}

/// The SVG `font-stretch` property in fontdb's terms.
fn map_stretch(stretch: usvg::FontStretch) -> fontdb::Stretch {
    match stretch {
        usvg::FontStretch::UltraCondensed => fontdb::Stretch::UltraCondensed,
        usvg::FontStretch::ExtraCondensed => fontdb::Stretch::ExtraCondensed,
        usvg::FontStretch::Condensed => fontdb::Stretch::Condensed,
        usvg::FontStretch::SemiCondensed => fontdb::Stretch::SemiCondensed,
        usvg::FontStretch::Normal => fontdb::Stretch::Normal,
        usvg::FontStretch::SemiExpanded => fontdb::Stretch::SemiExpanded,
        usvg::FontStretch::Expanded => fontdb::Stretch::Expanded,
        usvg::FontStretch::ExtraExpanded => fontdb::Stretch::ExtraExpanded,
        usvg::FontStretch::UltraExpanded => fontdb::Stretch::UltraExpanded,
    }
}

/// Picks the face closest to the requested weight/style among `candidates`.
fn best_face(
    db: &fontdb::Database,
    candidates: &[fontdb::ID],
    weight: u16,
    style: fontdb::Style,
) -> Option<fontdb::ID> {
    candidates
        .iter()
        .filter_map(|id| db.face(*id))
        .min_by_key(|face| {
            let weight_distance = (i32::from(face.weight.0) - i32::from(weight)).abs();
            let style_penalty = if face.style == style { 0 } else { 100_000 };
            weight_distance + style_penalty
        })
        .map(|face| face.id)
}

/// Font selection for one `<text>` span, honoring the family list's CSS
/// order: each family is fully resolved — exact name, then
/// case-insensitively, since CSS matching is case-insensitive and fontdb's
/// query is exact-case (merman writes e.g. a lowercase `arial`) — before the
/// next one is tried. System keywords (`system-ui`, `ui-sans-serif`, …)
/// resolve as their generic in place. Only when nothing in the list has an
/// installed face does the fallback kick in: generic `sans-serif`, because
/// merman sized the label boxes with a proportional sans profile and labels
/// must not silently become serif, then any installed face so text stays
/// visible.
fn select_font(font: &usvg::Font, db: &mut Arc<fontdb::Database>) -> Option<fontdb::ID> {
    for family in font.families() {
        match map_family(family) {
            fontdb::Family::Name(name) => {
                if let Some(id) = query_family(db, fontdb::Family::Name(name), font) {
                    return Some(id);
                }
                let candidates: Vec<fontdb::ID> = db
                    .faces()
                    .filter(|face| {
                        face.families
                            .iter()
                            .any(|(face_name, _)| face_name.eq_ignore_ascii_case(name))
                    })
                    .map(|face| face.id)
                    .collect();
                if let Some(id) = best_face(
                    db,
                    &candidates,
                    font.weight(),
                    fontdb::Style::from(font.style()),
                ) {
                    return Some(id);
                }
            }
            generic => {
                if let Some(id) = query_family(db, generic, font) {
                    return Some(id);
                }
            }
        }
    }

    if let Some(id) = query_family(db, fontdb::Family::SansSerif, font) {
        return Some(id);
    }
    // A machine whose fonts are all outside the preference lists: keep text
    // visible with any face rather than dropping it.
    let candidates: Vec<fontdb::ID> = db.faces().map(|face| face.id).collect();
    best_face(
        db,
        &candidates,
        font.weight(),
        fontdb::Style::from(font.style()),
    )
}

/// Queries one family with the span's weight, stretch and style.
fn query_family(
    db: &fontdb::Database,
    family: fontdb::Family<'_>,
    font: &usvg::Font,
) -> Option<fontdb::ID> {
    let query = fontdb::Query {
        families: &[family],
        weight: fontdb::Weight(font.weight()),
        stretch: map_stretch(font.stretch()),
        style: fontdb::Style::from(font.style()),
    };
    db.query(&query)
}

/// Pixel dimensions to draw at and the scale actually used. `scale` must be
/// finite and positive; `vw`/`vh` are the SVG's logical size in units.
///
/// The budget is [`MAX_PIXELS`] total pixels. The scale is only ever reduced,
/// never raised: a diagram that already fits keeps the requested scale even
/// when one axis lands below a pixel (that axis is still drawn 1 px wide),
/// and a diagram that does not fit gets the largest scale whose total pixel
/// count does. Because a subpixel axis still occupies ≥ 1 px, a single axis
/// alone may not exceed the whole budget (the pixmap dimensions are u32 and
/// each pixel costs 4 bytes), so each axis is bounded by the same
/// [`MAX_PIXELS`]. Rounding to whole pixels can nudge the total a hair over
/// the budget; the larger axis is then shaved by whole pixels until it fits.
fn bounded_size(vw: f64, vh: f64, scale: f32) -> (u32, u32, f32) {
    let budget = MAX_PIXELS as f64;
    let mut s = f64::from(scale);
    s = s.min((budget / (vw * vh)).sqrt());
    s = s.min(budget / vw).min(budget / vh);

    let mut width = (vw * s).round().clamp(1.0, budget) as u32;
    let mut height = (vh * s).round().clamp(1.0, budget) as u32;
    while u64::from(width) * u64::from(height) > MAX_PIXELS {
        if width >= height {
            width -= 1;
        } else {
            height -= 1;
        }
    }
    (width, height, s as f32)
}

/// Short, panic-free rendering of an upstream panic payload.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    let first_line = message.lines().next().unwrap_or("unknown panic");
    first_line.chars().take(120).collect()
}

/// Converts tiny-skia's premultiplied RGBA8888 pixels to the straight-alpha
/// BGRA byte order gpui's `RenderImage` frames take. Zero-alpha pixels are
/// all zero (they carry no color information), opaque pixels are a plain
/// channel reorder, and fractional alpha is divided out with integer
/// arithmetic that rounds to nearest: `(v * 255 + a/2) / a`, which cannot
/// overflow u16 because `v <= a` in valid premultiplied data.
pub(crate) fn premultiplied_rgba_to_straight_bgra(premultiplied_rgba: &[u8]) -> Vec<u8> {
    let mut bgra = Vec::with_capacity(premultiplied_rgba.len());
    for px in premultiplied_rgba.as_chunks::<4>().0 {
        let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
        if a == 0 {
            bgra.extend_from_slice(&[0, 0, 0, 0]);
        } else if a == u8::MAX {
            bgra.extend_from_slice(&[b, g, r, a]);
        } else {
            let straight = |v: u8| ((u16::from(v) * 255 + u16::from(a) / 2) / u16::from(a)) as u8;
            bgra.extend_from_slice(&[straight(b), straight(g), straight(r), a]);
        }
    }
    bgra
}

/// SVG → [`Raster`]. See [`crate::rasterize`] for the contract; this is its
/// implementation, kept private so the frozen public API stays in `lib.rs`.
pub(crate) fn rasterize(svg: &str, scale: f32) -> Result<Raster, MermaidError> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(MermaidError::Render(format!("invalid scale {scale}")));
    }
    // One boundary around the entire operation — font database build, SVG
    // parse, render, conversion — so an upstream panic on hostile input
    // still comes back as an error.
    catch_unwind(AssertUnwindSafe(|| draw(svg, scale))).unwrap_or_else(|payload| {
        Err(MermaidError::Render(format!(
            "rasterization panicked: {}",
            panic_message(payload)
        )))
    })
}

fn draw(svg: &str, scale: f32) -> Result<Raster, MermaidError> {
    let stack = font_stack();
    let options = usvg::Options {
        font_family: stack.default_family.clone(),
        fontdb: stack.db.clone(),
        font_resolver: usvg::FontResolver {
            select_font: Box::new(select_font),
            select_fallback: usvg::FontResolver::default_fallback_selector(),
        },
        ..usvg::Options::default()
    };

    let tree = usvg::Tree::from_str(svg, &options)
        .map_err(|e| MermaidError::Render(format!("SVG parse failed: {e}")))?;

    let size = tree.size();
    let (vw, vh) = (f64::from(size.width()), f64::from(size.height()));
    if !vw.is_finite() || !vh.is_finite() || vw <= 0.0 || vh <= 0.0 {
        return Err(MermaidError::Render(format!(
            "SVG has no drawable size ({vw}x{vh})"
        )));
    }

    let (width, height, actual_scale) = bounded_size(vw, vh, scale);
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or_else(|| {
        MermaidError::Render(format!("pixel buffer {width}x{height} is too large"))
    })?;
    let transform = tiny_skia::Transform::from_scale(actual_scale, actual_scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    Ok(Raster {
        width,
        height,
        scale: actual_scale,
        bgra: premultiplied_rgba_to_straight_bgra(pixmap.data()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SVG `width`x`height` with a red rect covering the whole canvas.
    fn svg_with_size(width: &str, height: &str) -> String {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">"#)
            + &format!(r##"<rect width="{width}" height="{height}" fill="#ff0000"/></svg>"##)
    }

    /// SVG with one line of black text in `family`.
    fn svg_with_text(family: &str) -> String {
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="30">"#.to_owned()
            + &format!(
                r##"<text x="4" y="21" font-family="{family}" font-size="16" fill="#000000">Ag 12</text></svg>"##
            )
    }

    fn pixel(raster: &Raster, x: u32, y: u32) -> [u8; 4] {
        let i = (y * raster.width + x) as usize * 4;
        raster.bgra[i..i + 4].try_into().unwrap()
    }

    fn assert_close(actual: [u8; 4], expected: [u8; 4], tolerance: u8) {
        for i in 0..4 {
            let delta = i16::from(actual[i]) - i16::from(expected[i]);
            assert!(
                delta.abs() <= i16::from(tolerance),
                "pixel {actual:?} not within {tolerance} of {expected:?}"
            );
        }
    }

    #[test]
    fn alpha_conversion_reorders_channels_and_unpremultiplies() {
        // Opaque: pure channel reorder RGBA → BGRA.
        assert_eq!(
            premultiplied_rgba_to_straight_bgra(&[255, 0, 0, 255, 0, 255, 0, 255]),
            vec![0, 0, 255, 255, 0, 255, 0, 255]
        );
        // Half-alpha red: premultiplied r=128 → straight r=255.
        assert_eq!(
            premultiplied_rgba_to_straight_bgra(&[128, 0, 0, 128]),
            vec![0, 0, 255, 128]
        );
        // Zero-alpha carries no color.
        assert_eq!(
            premultiplied_rgba_to_straight_bgra(&[99, 99, 99, 0]),
            vec![0, 0, 0, 0]
        );
        // Rounding to nearest: (2 * 255 + 1) / 3 = 170, (1 * 255 + 1) / 3 = 85.
        assert_eq!(
            premultiplied_rgba_to_straight_bgra(&[2, 1, 2, 3]),
            vec![170, 85, 170, 3]
        );
    }

    #[test]
    fn opaque_known_color_and_dimensions() {
        let raster = rasterize(&svg_with_size("20", "10"), 1.0).unwrap();
        assert_eq!((raster.width, raster.height), (20, 10));
        assert_eq!(raster.scale, 1.0);
        assert_eq!(raster.bgra.len(), 20 * 10 * 4);
        // Pure red straight BGRA: B=0 G=0 R=255 A=255.
        for (x, y) in [(0, 0), (10, 5), (19, 9)] {
            assert_eq!(pixel(&raster, x, y), [0, 0, 255, 255]);
        }
    }

    #[test]
    fn semitransparent_known_colors_keep_straight_alpha() {
        for (fill, expected) in [("#ff0000", [0, 0, 255, 128]), ("#00ff00", [0, 255, 0, 128])] {
            let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">"#
                .to_owned()
                + &format!(
                    r#"<rect width="10" height="10" fill="{fill}" fill-opacity="0.5"/></svg>"#
                );
            let raster = rasterize(&svg, 1.0).unwrap();
            assert_close(pixel(&raster, 5, 5), expected, 2);
        }
    }

    #[test]
    fn untouched_pixels_stay_fully_transparent() {
        // Rect covers only the top-left quadrant; the rest must be zero.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">"#.to_owned()
            + r##"<rect x="0" y="0" width="10" height="10" fill="#ff0000"/></svg>"##;
        let raster = rasterize(&svg, 1.0).unwrap();
        for (x, y) in [(15, 15), (19, 0), (0, 19), (11, 11)] {
            assert_eq!(pixel(&raster, x, y), [0, 0, 0, 0]);
        }
        // And a content-free SVG is uniformly transparent.
        let empty = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"></svg>"#;
        let raster = rasterize(empty, 1.0).unwrap();
        assert!(raster.bgra.iter().all(|b| *b == 0));
    }

    #[test]
    fn malformed_svg_is_an_error_not_a_panic() {
        for bad in [
            "",
            "   ",
            "not an svg",
            "<svg",
            " garbage \u{1}\u{2}",
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><circle></svg>"#,
        ] {
            match rasterize(bad, 1.0) {
                Err(MermaidError::Render(msg)) => {
                    assert!(!msg.is_empty());
                }
                other => panic!("expected Render error for {bad:?}, got {other:?}"),
            }
        }

        // No size attributes at all is *not* an error: usvg falls back to
        // its 100x100 default viewport and draws an empty canvas.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
        let raster = rasterize(svg, 1.0).unwrap();
        assert_eq!((raster.width, raster.height), (100, 100));
    }

    #[test]
    fn invalid_scales_are_rejected() {
        for scale in [0.0, -0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            match rasterize(&svg_with_size("10", "10"), scale) {
                Err(MermaidError::Render(_)) => {}
                other => panic!("expected Render error for scale {scale}, got {other:?}"),
            }
        }
    }

    #[test]
    fn huge_square_is_downscaled_to_the_pixel_budget() {
        let raster = rasterize(&svg_with_size("20000", "20000"), 1.0).unwrap();
        assert_eq!(raster.width, 4096);
        assert_eq!(raster.height, 4096);
        assert_eq!(
            u64::from(raster.width) * u64::from(raster.height),
            MAX_PIXELS
        );
        assert_eq!(raster.bgra.len(), 4096 * 4096 * 4);
        assert!(raster.scale < 1.0 && raster.scale > 0.2);
        // The logical size survives the cap: 4096 / 0.2048 = 20000.
        let logical = f64::from(raster.width) / f64::from(raster.scale);
        assert!((logical - 20000.0).abs() < 2.0, "logical width {logical}");
        // Content still covers the whole canvas.
        assert_eq!(pixel(&raster, 0, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&raster, 4095, 4095), [0, 0, 255, 255]);
    }

    #[test]
    fn asymmetric_fitting_diagram_keeps_the_requested_scale() {
        for (w, h) in [("20000", "1"), ("1", "20000")] {
            let raster = rasterize(&svg_with_size(w, h), 1.0).unwrap();
            let expected = if w == "20000" { (20000, 1) } else { (1, 20000) };
            assert_eq!((raster.width, raster.height), expected, "case {w}x{h}");
            assert_eq!(raster.scale, 1.0, "case {w}x{h}");
            assert_eq!(raster.bgra.len(), 20000 * 4);
        }
    }

    #[test]
    fn subpixel_logical_size_draws_one_pixel() {
        let raster = rasterize(&svg_with_size("0.5", "0.25"), 1.0).unwrap();
        assert_eq!((raster.width, raster.height), (1, 1));
        assert_eq!(raster.scale, 1.0);

        let raster = rasterize(&svg_with_size("0.001", "0.002"), 100.0).unwrap();
        assert_eq!((raster.width, raster.height), (1, 1));
        // The requested scale is reported as used, never raised.
        assert_eq!(raster.scale, 100.0);
    }

    #[test]
    fn extreme_requested_scales_stay_bounded() {
        // A scale that would blow the budget is capped to the largest that
        // fits: sqrt(MAX_PIXELS / (100*100)) = 40.96 → 4096x4096.
        let raster = rasterize(&svg_with_size("100", "100"), 1.0e30).unwrap();
        assert_eq!((raster.width, raster.height), (4096, 4096));
        assert!(
            raster.scale > 40.0 && raster.scale < 42.0,
            "{}",
            raster.scale
        );

        // The smallest positive scale is honored as-is and lands at 1x1.
        let tiny = f32::MIN_POSITIVE;
        let raster = rasterize(&svg_with_size("100", "100"), tiny).unwrap();
        assert_eq!((raster.width, raster.height), (1, 1));
        assert_eq!(raster.scale, tiny);
    }

    #[test]
    fn extreme_aspect_ratio_never_overflows_the_budget() {
        // 1e9 x 1e-9 units at scale 100: the area fits, but one axis alone
        // would need 1e11 pixels — wider than u32 — so the same budget caps
        // that axis at MAX_PIXELS and the subpixel axis rounds up to 1.
        let svg = svg_with_size("1000000000", "0.000000001");
        let raster = rasterize(&svg, 100.0).unwrap();
        assert_eq!(raster.height, 1);
        assert!(raster.width >= 16_777_214, "width {}", raster.width);
        assert!(u64::from(raster.width) * u64::from(raster.height) <= MAX_PIXELS);

        // Equally extreme the other way around.
        let svg = svg_with_size("0.000000001", "1000000000");
        let raster = rasterize(&svg, 100.0).unwrap();
        assert_eq!(raster.width, 1);
        assert!(u64::from(raster.width) * u64::from(raster.height) <= MAX_PIXELS);
    }

    #[test]
    fn normal_scales_multiply_the_dimensions() {
        let raster = rasterize(&svg_with_size("100", "50"), 2.5).unwrap();
        assert_eq!((raster.width, raster.height), (250, 125));
        assert_eq!(raster.scale, 2.5);

        // Tiny-but-fitting scales keep fractional sizes honest.
        let raster = rasterize(&svg_with_size("100", "50"), 0.001).unwrap();
        assert_eq!((raster.width, raster.height), (1, 1));
        assert_eq!(raster.scale, 0.001);
    }

    #[test]
    fn named_families_resolve_in_order_case_insensitively() {
        // Regression: a named family must resolve to its own face even when
        // written in the wrong case, in list order, before any generic
        // fallback is consulted. Liberation faces are the fixture; skip on
        // machines without them.
        let db = &font_stack().db;
        let installed = |name: &str| {
            db.query(&fontdb::Query {
                families: &[fontdb::Family::Name(name)],
                weight: fontdb::Weight::NORMAL,
                stretch: fontdb::Stretch::Normal,
                style: fontdb::Style::Normal,
            })
            .is_some()
        };
        if !installed("Liberation Serif") || !installed("Liberation Sans") {
            return;
        }

        let serif_exact = rasterize(&svg_with_text("Liberation Serif"), 2.0).unwrap();
        let serif_lower = rasterize(&svg_with_text("liberation serif"), 2.0).unwrap();
        let sans = rasterize(&svg_with_text("Liberation Sans"), 2.0).unwrap();

        // Wrong case still finds the same face...
        assert_eq!(serif_exact.bgra, serif_lower.bgra);
        // ...and that face is the serif one, not the sans fallback.
        assert_ne!(serif_lower.bgra, sans.bgra);
        assert_eq!(serif_exact.width, sans.width);
        let visible = serif_lower
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] > 0)
            .count();
        assert!(visible > 10, "serif text invisible: {visible} pixels");
    }

    #[test]
    fn text_renders_visibly_for_generic_and_system_families() {
        for family in [
            "sans-serif",
            "system-ui",
            "ui-sans-serif",
            "-apple-system",
            "Liberation Sans",
            "liberation sans",
            "arial",
            "NoSuchFamilyAnywhere",
        ] {
            let raster = rasterize(&svg_with_text(family), 2.0).unwrap();
            let visible = raster
                .bgra
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|px| px[3] > 0)
                .count();
            assert!(
                visible > 10,
                "family {family:?}: only {visible} visible pixels"
            );
            // Black text: straight color stays near-black at every alpha.
            let dark = raster
                .bgra
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|px| px[3] > 0)
                .all(|px| px[0] < 64 && px[1] < 64 && px[2] < 64);
            assert!(dark, "family {family:?}: text is not black");
        }
    }

    #[test]
    fn output_is_deterministic() {
        let svg = svg_with_text("sans-serif");
        let first = rasterize(&svg, 2.0).unwrap();
        let second = rasterize(&svg, 2.0).unwrap();
        assert_eq!(first.bgra, second.bgra);
        assert_eq!((first.width, first.height), (second.width, second.height));
    }
}
