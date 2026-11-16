//! What bytes are, and what a picture becomes under the viewer's limits.
//!
//! Everything here runs on a host's blocking thread, on bytes nobody has
//! vouched for. The rules, in the order they bite:
//!
//! * **Content decides, the name only hints.** [`sniff`] looks at the bytes
//!   (magic numbers; for SVG a scan past the BOM, XML declaration, comments
//!   and DOCTYPE to a root `<svg`). The extension is consulted by the caller
//!   only to word an error for something that is neither a picture nor text.
//! * **Inputs are bounded before they are decoded.** At most
//!   [`MAX_IMAGE_BYTES`] of file, [`MAX_PIXELS`] of picture, and
//!   [`MAX_RGBA_BYTES`] of retained pixels — summed over every frame of an
//!   animation, each frame also charged for its own bookkeeping. Dimensions
//!   are read from the header and checked before any pixel buffer exists,
//!   and the decoders are told the same allocation limit.
//! * **Nothing is fetched.** SVG goes through `tty59-mermaid`'s hardened
//!   pipeline: no file or network resource is read, and a document that
//!   links or embeds an image is refused rather than drawn with a hole.
//! * **Animation is kept only if it fits.** GIF and animated WebP keep every
//!   frame (gpui animates a multi-frame image natively) while the total
//!   stays inside the budget; past it the first frame is shown and the file
//!   says so. A frame that fails to decode makes the whole file corrupt.

use std::io::Cursor;
use std::path::Path;

use image::codecs::{
    bmp::BmpDecoder, gif::GifDecoder, ico::IcoDecoder, jpeg::JpegDecoder, png::PngDecoder,
    webp::WebPDecoder,
};
use image::{
    AnimationDecoder, Delay, DynamicImage, Frame, ImageDecoder, Limits, Rgba, RgbaImage,
    imageops::FilterType,
};

/// The most encoded bytes of a picture that are read at all.
pub(crate) const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
/// The most pixels (width × height) a picture may have: 4096 × 4096.
pub(crate) const MAX_PIXELS: u64 = 16_777_216;
/// The most decoded RGBA a picture may keep, over all its frames.
pub(crate) const MAX_RGBA_BYTES: u64 = 64 * 1024 * 1024;
/// The longest side a single picture is kept at.
///
/// Not a limit on what may be opened: gpui's atlas stores a picture as one
/// tile of one texture (`gpui_wgpu::WgpuAtlas::allocate` fails — the picture
/// then paints nothing — when it does not fit) and the largest texture is
/// whatever the GPU reports, which the app cannot ask for. 8192 is a
/// conservative upload target, not a guarantee about any particular hardware
/// (the atlas source clamps to the device's own limit). A wider or taller
/// picture that is inside the pixel limit (a panorama) is reduced to this
/// side, keeps its logical size, and says so.
pub(crate) const MAX_TILE_SIDE: u32 = 8192;
/// How many pixels along its long side an SVG is drawn with at least, so it
/// can be enlarged without going soft straight away.
const SVG_MIN_LONG_SIDE: f32 = 1024.0;
/// What a frame costs besides its pixels.
const FRAME_OVERHEAD: u64 = std::mem::size_of::<Frame>() as u64;
/// A frame this short is shown for [`FALLBACK_DELAY_MS`] instead, as every
/// browser does, or a delay of zero would spin the animation.
const MIN_DELAY_MS: u32 = 20;
const FALLBACK_DELAY_MS: u32 = 100;

/// A picture format the viewer can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Ico,
    Svg,
}

impl Format {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Gif => "GIF",
            Format::WebP => "WebP",
            Format::Bmp => "BMP",
            Format::Ico => "ICO",
            Format::Svg => "SVG",
        }
    }
}

/// What the bytes themselves say they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sniffed {
    Raster(Format),
    Svg,
    /// A picture format recognised by its signature that the viewer cannot
    /// decode.
    Unsupported(&'static str),
    /// Neither; the caller decides whether it is text.
    NotImage,
}

/// Why a picture is not shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ImageError {
    /// More than [`MAX_IMAGE_BYTES`].
    TooLarge { bytes: u64 },
    /// More than [`MAX_PIXELS`] pixels.
    TooManyPixels { width: u32, height: u32 },
    /// Decoding would need more memory than [`MAX_RGBA_BYTES`].
    OverBudget,
    /// It says it is a picture and is not a whole one.
    Corrupt,
    /// A picture format, or a part of one, the viewer does not do.
    Unsupported(Unsupported),
    /// Called a picture by its name only: the bytes are neither a picture
    /// nor text.
    NotAnImage,
}

/// What the viewer does not do, as data: the words belong to the UI, in the
/// user's language, not to a string made here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unsupported {
    /// A picture format known by its signature, named for the user
    /// ("TIFF").
    Format(&'static str),
    /// Something inside a format the decoder declined (an exotic
    /// compression, say).
    Feature,
    /// An SVG that links or embeds another image.
    SvgImages,
}

/// What became of an animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Animation {
    Still,
    Animated {
        frames: usize,
    },
    /// Only the first frame is kept, because the rest would not fit.
    FirstFrameOnly,
}

/// A picture, decoded.
pub(crate) struct Decoded {
    pub(crate) format: Format,
    /// The picture's own size, which is what 100 % shows: its pixels for a
    /// raster, its user units for an SVG.
    pub(crate) natural: (f32, f32),
    /// The size of the pixels held, which differs from `natural` for an SVG
    /// (drawn larger) and for a picture reduced to [`MAX_TILE_SIDE`].
    pub(crate) pixels: (u32, u32),
    /// One buffer per frame, in **BGRA** order, straight alpha — what gpui's
    /// `RenderImage` takes.
    pub(crate) frames: Vec<Frame>,
    pub(crate) animation: Animation,
    /// The encoded size, for the info line.
    pub(crate) bytes: u64,
    /// Whether the pixels held have fewer pixels than the picture is wide and
    /// tall: a raster cut down to [`MAX_TILE_SIDE`], or an SVG the pixel
    /// budget or the side limit drew below its own size. Set where it
    /// happens, not guessed from rounded sizes.
    reduced: bool,
}

impl Decoded {
    /// Whether the pixels held are a reduced copy of the picture, so the
    /// viewer says so.
    pub(crate) fn reduced(&self) -> bool {
        self.reduced
    }
}

// ---- Sniffing ----

/// What `bytes` are, from their content alone.
pub(crate) fn sniff(bytes: &[u8]) -> Sniffed {
    if let Some(format) = raster_signature(bytes) {
        return Sniffed::Raster(format);
    }
    if let Some(name) = unsupported_signature(bytes) {
        return Sniffed::Unsupported(name);
    }
    if looks_like_svg(bytes) {
        return Sniffed::Svg;
    }
    Sniffed::NotImage
}

fn raster_signature(b: &[u8]) -> Option<Format> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some(Format::WebP)
    } else if is_bmp(b) {
        Some(Format::Bmp)
    } else if is_ico(b) {
        Some(Format::Ico)
    } else {
        None
    }
}

fn le16(b: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes([b[at], b[at + 1]]))
}

fn le32(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
}

/// `BM` alone is a word in a text file; a BMP also has one of the DIB header
/// sizes where its info header starts.
fn is_bmp(b: &[u8]) -> bool {
    b.len() >= 26
        && b.starts_with(b"BM")
        && matches!(le32(b, 14), 12 | 40 | 52 | 56 | 64 | 108 | 124)
}

/// An icon directory: reserved 0, type 1, a count, and a first entry whose
/// own bytes lie inside the file.
fn is_ico(b: &[u8]) -> bool {
    if b.len() < 22 || b[..4] != [0, 0, 1, 0] {
        return false;
    }
    let count = le16(b, 4);
    let (size, offset) = (le32(b, 14), le32(b, 18));
    (1..=256).contains(&count)
        && b[9] == 0
        && offset >= 6 + 16 * count
        && size > 0
        && offset.checked_add(size).is_some_and(|end| end <= b.len())
}

/// Picture formats that are recognised so they can be named in an error
/// instead of being offered to a text decoder.
fn unsupported_signature(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        return Some("TIFF");
    }
    if b.starts_with(&[0xFF, 0x0A]) || b.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n") {
        return Some("JPEG XL");
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        return match &b[8..12] {
            b"avif" | b"avis" => Some("AVIF"),
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => Some("HEIC"),
            _ => None,
        };
    }
    None
}

/// Whether the document's root element is `<svg`: past a BOM, whitespace,
/// the XML declaration and other processing instructions, comments and a
/// DOCTYPE. Only the start is looked at; whether the rest is a drawable SVG
/// is the decoder's question, and "no" there is a corrupt picture.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let mut rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    loop {
        rest = rest.trim_ascii_start();
        let next = if rest.starts_with(b"<?") {
            after(rest, b"?>")
        } else if rest.starts_with(b"<!--") {
            after(rest, b"-->")
        } else if rest.len() >= 9 && rest[..9].eq_ignore_ascii_case(b"<!DOCTYPE") {
            after_doctype(rest)
        } else {
            break;
        };
        match next {
            Some(tail) => rest = tail,
            None => return false,
        }
    }
    rest.len() > 4
        && rest.starts_with(b"<svg")
        && matches!(rest[4], b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/')
}

/// What follows the first `pattern` in `bytes`.
fn after<'a>(bytes: &'a [u8], pattern: &[u8]) -> Option<&'a [u8]> {
    let at = bytes.windows(pattern.len()).position(|w| w == pattern)?;
    Some(&bytes[at + pattern.len()..])
}

/// What follows a DOCTYPE, whose internal subset (`[ ... ]`) may itself hold
/// `>`.
fn after_doctype(bytes: &[u8]) -> Option<&[u8]> {
    let mut depth = 0usize;
    for (i, &byte) in bytes.iter().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => depth = depth.saturating_sub(1),
            b'>' if depth == 0 => return Some(&bytes[i + 1..]),
            _ => {}
        }
    }
    None
}

/// The extensions that make a file *called* a picture. A hint for wording
/// an error, never a reason to open something as one.
pub(crate) fn has_image_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "jpe"
                | "gif"
                | "webp"
                | "bmp"
                | "ico"
                | "svg"
                | "svgz"
                | "tif"
                | "tiff"
                | "avif"
                | "heic"
                | "heif"
                | "jxl"
        )
    })
}

// ---- Decoding ----

/// Decodes what [`sniff`] found.
pub(crate) fn decode(bytes: &[u8], sniffed: Sniffed) -> Result<Decoded, ImageError> {
    decode_with_budget(bytes, sniffed, MAX_RGBA_BYTES)
}

/// [`decode`] with the retained-pixel budget a caller chooses; tests pass a
/// small one to exercise the animation fallback without a 64 MiB GIF.
pub(crate) fn decode_with_budget(
    bytes: &[u8],
    sniffed: Sniffed,
    budget: u64,
) -> Result<Decoded, ImageError> {
    let len = bytes.len() as u64;
    if len > MAX_IMAGE_BYTES {
        return Err(ImageError::TooLarge { bytes: len });
    }
    match sniffed {
        Sniffed::Raster(format) => decode_raster(bytes, format, budget),
        Sniffed::Svg => decode_svg(bytes),
        Sniffed::Unsupported(name) => Err(ImageError::Unsupported(Unsupported::Format(name))),
        Sniffed::NotImage => Err(ImageError::NotAnImage),
    }
}

/// The decoders' own limits: the same allocation bound the whole picture has.
fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_RGBA_BYTES);
    limits
}

fn map_image_error(error: image::ImageError) -> ImageError {
    match error {
        image::ImageError::Limits(_) => ImageError::OverBudget,
        image::ImageError::Unsupported(e) => {
            log::debug!("image feature not supported: {e}");
            ImageError::Unsupported(Unsupported::Feature)
        }
        other => {
            log::debug!("image decode failed: {other}");
            ImageError::Corrupt
        }
    }
}

/// Checks a size read from a header, before any buffer for it exists.
fn check_size(width: u32, height: u32) -> Result<(), ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::Corrupt);
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImageError::TooManyPixels { width, height });
    }
    Ok(())
}

/// A decode before it is made presentable: RGBA frames, and how much of an
/// animation survived.
struct Raw {
    /// The size the header declared.
    declared: (u32, u32),
    frames: Vec<Frame>,
    animation: Animation,
}

fn decode_raster(bytes: &[u8], format: Format, budget: u64) -> Result<Decoded, ImageError> {
    let source = Cursor::new(bytes);
    let raw = match format {
        Format::Png => still(bounded_png(source)?)?,
        Format::Jpeg => still(JpegDecoder::new(source).map_err(map_image_error)?)?,
        Format::Bmp => still(BmpDecoder::new(source).map_err(map_image_error)?)?,
        Format::Ico => decode_ico(bytes)?,
        Format::Gif => animated(GifDecoder::new(source).map_err(map_image_error)?, budget)?,
        Format::WebP => {
            let mut decoder = WebPDecoder::new(source).map_err(map_image_error)?;
            if decoder.has_animation() {
                // Frames composite over transparency, as gpui's own loader
                // does for the same format.
                let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
                animated(decoder, budget)?
            } else {
                still(decoder)?
            }
        }
        Format::Svg => return Err(ImageError::Corrupt),
    };
    present(format, raw, bytes.len() as u64)
}

/// The one way a PNG decoder is made here, and the only one: with the limits
/// applied at construction. Reading a PNG's header already inflates its colour
/// profile, and `set_limits` afterwards does not reach the reader that did it
/// (the library says so in a TODO); a profile past the limit is dropped by the
/// bounded reader, the picture is not.
fn bounded_png<R: std::io::BufRead + std::io::Seek>(
    source: R,
) -> Result<PngDecoder<R>, ImageError> {
    PngDecoder::with_limits(source, limits()).map_err(map_image_error)
}

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The least a directory entry's payload can be: a PNG's signature. A real
/// BMP or PNG is longer, and the library sniffs this many bytes at the offset
/// whatever length the entry declares.
const ICO_ENTRY_MIN: usize = PNG_SIGNATURE.len();

/// One directory entry of an icon file, shown to lie inside the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IcoEntry {
    width: u8,
    height: u8,
    bits: u16,
    offset: usize,
    length: usize,
}

impl IcoEntry {
    /// A side of 0 means 256.
    fn side(byte: u8) -> u32 {
        if byte == 0 { 256 } else { u32::from(byte) }
    }

    /// The library's measure of an entry: colour depth first, then area.
    fn score(&self) -> (u16, u32) {
        (self.bits, Self::side(self.width) * Self::side(self.height))
    }

    fn payload<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        &bytes[self.offset..self.offset + self.length]
    }
}

/// Checks every entry of an icon's directory against the file, then picks the
/// one the library would.
///
/// `IcoDecoder` ignores the length an entry declares when it sniffs and
/// decodes, and hands a PNG to a decoder that has no limits; a bad entry that
/// is not the first, or not even the one chosen, must not be left to it. So no
/// entry is skipped: a count outside 1..=256, a directory that runs past the
/// file, a payload shorter than a signature, one that starts inside the
/// directory or ends past the file (checked, so a huge length cannot wrap)
/// make the whole file corrupt.
fn preflight_ico(bytes: &[u8]) -> Result<IcoEntry, ImageError> {
    if bytes.len() < 6 {
        return Err(ImageError::Corrupt);
    }
    let count = le16(bytes, 4);
    let directory_end = 6 + 16 * count;
    if !(1..=256).contains(&count) || bytes.len() < directory_end {
        return Err(ImageError::Corrupt);
    }
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let at = 6 + 16 * index;
        let (planes, bits) = (le16(bytes, at + 4), le16(bytes, at + 6));
        let (length, offset) = (le32(bytes, at + 8), le32(bytes, at + 12));
        let end = offset.checked_add(length).ok_or(ImageError::Corrupt)?;
        // Planes and bit depth are hotspot coordinates in a cursor; the
        // library refuses either past 256.
        if planes > 256
            || bits > 256
            || length < ICO_ENTRY_MIN
            || offset < directory_end
            || end > bytes.len()
        {
            return Err(ImageError::Corrupt);
        }
        entries.push(IcoEntry {
            width: bytes[at],
            height: bytes[at + 1],
            bits: bits as u16,
            offset,
            length,
        });
    }
    // The library's choice: the last entry, unless an earlier one scores
    // strictly higher.
    let mut best = entries.pop().ok_or(ImageError::Corrupt)?;
    for entry in entries {
        if entry.score() > best.score() {
            best = entry;
        }
    }
    Ok(best)
}

/// An icon: its directory is checked, and the entry the library would show is
/// decoded. A PNG entry goes straight to the bounded PNG decoder, over its own
/// bytes only, with the checks the library applies to it (the entry's size
/// matches the picture's, and it is RGBA); the PNG is parsed once. A BMP entry
/// is the library's to decode, and its header size is checked before any
/// pixels exist like everything else.
fn decode_ico(bytes: &[u8]) -> Result<Raw, ImageError> {
    let entry = preflight_ico(bytes)?;
    let payload = entry.payload(bytes);
    if payload.starts_with(PNG_SIGNATURE) {
        let decoder = bounded_png(Cursor::new(payload))?;
        let (width, height) = decoder.dimensions();
        if IcoEntry::side(entry.width) != width.min(256)
            || IcoEntry::side(entry.height) != height.min(256)
            || decoder.color_type() != image::ColorType::Rgba8
        {
            return Err(ImageError::Corrupt);
        }
        return still(decoder);
    }
    // A BMP entry is the library's. It is given an icon that holds only this
    // entry, so that whatever it does to choose, it cannot choose a PNG and
    // send it to an unbounded constructor.
    let alone = single_entry_icon(&entry, bytes);
    still(IcoDecoder::new(Cursor::new(&alone[..])).map_err(map_image_error)?)
}

/// An icon file with exactly `entry` in it, its payload moved to the first
/// byte after the directory.
fn single_entry_icon(entry: &IcoEntry, bytes: &[u8]) -> Vec<u8> {
    let mut icon = Vec::with_capacity(22 + entry.length);
    icon.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    icon.extend_from_slice(&[entry.width, entry.height, 0, 0]);
    icon.extend_from_slice(&1u16.to_le_bytes());
    icon.extend_from_slice(&entry.bits.to_le_bytes());
    icon.extend_from_slice(&(entry.length as u32).to_le_bytes());
    icon.extend_from_slice(&22u32.to_le_bytes());
    icon.extend_from_slice(entry.payload(bytes));
    icon
}

/// One picture, with its size checked and the decoder's limits set first.
fn still<D: ImageDecoder>(mut decoder: D) -> Result<Raw, ImageError> {
    let declared = decoder.dimensions();
    check_size(declared.0, declared.1)?;
    decoder.set_limits(limits()).map_err(map_image_error)?;
    let image = DynamicImage::from_decoder(decoder).map_err(map_image_error)?;
    Ok(Raw {
        declared,
        frames: vec![Frame::new(image.into_rgba8())],
        animation: Animation::Still,
    })
}

/// Every frame of an animation while the total fits `budget`; the first
/// frame alone when it does not.
fn animated<'a, D: ImageDecoder + AnimationDecoder<'a>>(
    mut decoder: D,
    budget: u64,
) -> Result<Raw, ImageError> {
    let declared = decoder.dimensions();
    check_size(declared.0, declared.1)?;
    decoder.set_limits(limits()).map_err(map_image_error)?;

    let mut frames: Vec<Frame> = Vec::new();
    let mut charged = 0u64;
    let mut cut = false;
    for frame in decoder.into_frames() {
        // A frame that does not decode is a file that does not: showing the
        // ones before it would be showing a different animation.
        let frame = retimed(frame.map_err(map_image_error)?);
        let cost = frame.buffer().as_raw().len() as u64 + FRAME_OVERHEAD;
        // The first frame is always kept: it is inside the pixel limit, so
        // it is a single picture the budget was sized for.
        if !frames.is_empty() && charged + cost > budget {
            cut = true;
            break;
        }
        charged += cost;
        frames.push(frame);
    }
    if frames.is_empty() {
        return Err(ImageError::Corrupt);
    }
    let animation = if cut {
        frames.truncate(1);
        frames.shrink_to_fit();
        Animation::FirstFrameOnly
    } else if frames.len() > 1 {
        Animation::Animated {
            frames: frames.len(),
        }
    } else {
        Animation::Still
    };
    Ok(Raw {
        declared,
        frames,
        animation,
    })
}

/// A frame too brief to see is given the delay every browser gives it.
fn retimed(frame: Frame) -> Frame {
    let (numer, denom) = frame.delay().numer_denom_ms();
    if denom != 0 && u64::from(numer) >= u64::from(MIN_DELAY_MS) * u64::from(denom) {
        return frame;
    }
    let (left, top) = (frame.left(), frame.top());
    Frame::from_parts(
        frame.into_buffer(),
        left,
        top,
        Delay::from_numer_denom_ms(FALLBACK_DELAY_MS, 1),
    )
}

/// Makes a decode presentable: a side over [`MAX_TILE_SIDE`] reduces the
/// picture (and ends an animation, whose every frame would be that big), and
/// the channels go to gpui's BGRA order.
fn present(format: Format, raw: Raw, bytes: u64) -> Result<Decoded, ImageError> {
    let Raw {
        declared,
        mut frames,
        mut animation,
    } = raw;
    let (width, height) = frames
        .first()
        .map(|f| f.buffer().dimensions())
        .ok_or(ImageError::Corrupt)?;
    let natural = (declared.0 as f32, declared.1 as f32);
    let long = width.max(height);
    let mut pixels = (width, height);
    let mut reduced = false;
    if long > MAX_TILE_SIDE {
        reduced = true;
        if frames.len() > 1 {
            frames.truncate(1);
            frames.shrink_to_fit();
            animation = Animation::FirstFrameOnly;
        }
        let ratio = f64::from(MAX_TILE_SIDE) / f64::from(long);
        let scaled = |side: u32| ((f64::from(side) * ratio).round() as u32).max(1);
        pixels = (scaled(width), scaled(height));
        let first = frames.remove(0);
        let (left, top, delay) = (first.left(), first.top(), first.delay());
        let smaller =
            image::imageops::resize(first.buffer(), pixels.0, pixels.1, FilterType::Triangle);
        frames.push(Frame::from_parts(smaller, left, top, delay));
    }
    let frames = frames
        .into_iter()
        .map(|frame| {
            let (left, top, delay) = (frame.left(), frame.top(), frame.delay());
            let mut buffer = frame.into_buffer();
            for pixel in buffer.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            Frame::from_parts(buffer, left, top, delay)
        })
        .collect();
    Ok(Decoded {
        format,
        natural,
        pixels,
        frames,
        animation,
        bytes,
        reduced,
    })
}

fn decode_svg(bytes: &[u8]) -> Result<Decoded, ImageError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ImageError::Corrupt)?;
    let document = tty59_mermaid::rasterize_document(text, SVG_MIN_LONG_SIDE, MAX_TILE_SIDE)
        .map_err(|error| match error {
            tty59_mermaid::SvgDocumentError::LinkedImages => {
                ImageError::Unsupported(Unsupported::SvgImages)
            }
            other => {
                log::debug!("SVG could not be shown: {other}");
                ImageError::Corrupt
            }
        })?;
    let raster = document.raster;
    // Drawn below its own size (scale under 1) only when the side limit or
    // the pixel budget said so: the picture on screen is then a reduced copy.
    let reduced = raster.scale < 1.0;
    // `Raster` is straight-alpha BGRA already: the buffer goes in as it is.
    let buffer =
        RgbaImage::from_raw(raster.width, raster.height, raster.bgra).ok_or(ImageError::Corrupt)?;
    Ok(Decoded {
        format: Format::Svg,
        natural: (document.width, document.height),
        pixels: (raster.width, raster.height),
        frames: vec![Frame::new(buffer)],
        animation: Animation::Still,
        bytes: bytes.len() as u64,
        reduced,
    })
}

#[cfg(test)]
mod tests {
    use image::{ColorType, ImageEncoder, ImageFormat, Rgba, RgbaImage};

    use super::*;

    fn encode(format: ImageFormat, image: &RgbaImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image.clone())
            .write_to(&mut out, format)
            .expect("encode");
        out.into_inner()
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(rgba))
    }

    fn gif_of(frames: u32, width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            for i in 0..frames {
                let shade = (i * 40) as u8;
                let frame = Frame::from_parts(
                    solid(width, height, [shade, 50, 200, 255]),
                    0,
                    0,
                    Delay::from_numer_denom_ms(80, 1),
                );
                encoder.encode_frame(frame).unwrap();
            }
        }
        out
    }

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16" viewBox="0 0 32 16"><rect width="32" height="16" fill="#336699"/></svg>"##;

    #[test]
    fn every_raster_format_is_recognised_by_its_content() {
        let image = solid(4, 3, [10, 20, 30, 255]);
        for (format, want) in [
            (ImageFormat::Png, Format::Png),
            (ImageFormat::Jpeg, Format::Jpeg),
            (ImageFormat::Gif, Format::Gif),
            (ImageFormat::WebP, Format::WebP),
            (ImageFormat::Bmp, Format::Bmp),
            (ImageFormat::Ico, Format::Ico),
        ] {
            let bytes = encode(format, &image);
            assert_eq!(sniff(&bytes), Sniffed::Raster(want), "{format:?}");
        }
    }

    #[test]
    fn svg_is_recognised_past_a_bom_a_declaration_comments_and_a_doctype() {
        for doc in [
            SVG.to_owned(),
            format!("\u{feff}{SVG}"),
            format!("  \n\t{SVG}"),
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{SVG}"),
            format!("<!-- made by hand -->\n{SVG}"),
            format!(
                "<?xml version=\"1.0\"?>\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \
                 \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\" [ <!ENTITY a \"b\"> ]>\n{SVG}"
            ),
            "<svg>".to_owned(),
            "<svg/>".to_owned(),
        ] {
            assert_eq!(sniff(doc.as_bytes()), Sniffed::Svg, "{doc}");
        }
    }

    #[test]
    fn text_that_only_resembles_a_picture_is_not_one() {
        for text in [
            "",
            "just words",
            "BMW is a car maker, and this is a long enough line of text to matter",
            "<html><body><svg></svg></body></html>",
            "<svgfoo></svgfoo>",
            "<!-- never closed <svg>",
            "<?xml version=\"1.0\"?><note/>",
            "GIF is pronounced two ways",
            "II* is not a tiff header here",
        ] {
            assert_eq!(sniff(text.as_bytes()), Sniffed::NotImage, "{text:?}");
        }
        // A binary that is not a picture.
        assert_eq!(
            sniff(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0"),
            Sniffed::NotImage
        );
    }

    #[test]
    fn signatures_of_formats_the_viewer_cannot_draw_are_named() {
        assert_eq!(sniff(b"II*\0\x08\0\0\0"), Sniffed::Unsupported("TIFF"));
        assert_eq!(sniff(b"MM\0*\0\0\0\x08"), Sniffed::Unsupported("TIFF"));
        assert_eq!(
            sniff(b"\0\0\0\x1cftypavif\0\0\0\0avifmif1"),
            Sniffed::Unsupported("AVIF")
        );
        assert_eq!(
            sniff(b"\0\0\0\x18ftypheic\0\0\0\0mif1heic"),
            Sniffed::Unsupported("HEIC")
        );
        // An MP4 is not a picture.
        assert_eq!(
            sniff(b"\0\0\0\x18ftypisom\0\0\0\0isomiso2"),
            Sniffed::NotImage
        );
    }

    #[test]
    fn the_extension_is_only_a_hint() {
        assert!(has_image_extension(Path::new("a/b/shot.PNG")));
        assert!(has_image_extension(Path::new("logo.svg")));
        assert!(!has_image_extension(Path::new("notes.txt")));
        assert!(!has_image_extension(Path::new("png")));
        // Content, not name: a PNG with no extension is still a PNG.
        let bytes = encode(ImageFormat::Png, &solid(2, 2, [0, 0, 0, 255]));
        assert_eq!(sniff(&bytes), Sniffed::Raster(Format::Png));
    }

    #[test]
    fn a_png_decodes_to_bgra_at_its_own_size() {
        let bytes = encode(ImageFormat::Png, &solid(5, 3, [10, 20, 30, 255]));
        let decoded = decode(&bytes, sniff(&bytes)).expect("decodes");
        assert_eq!(decoded.format, Format::Png);
        assert_eq!(decoded.natural, (5.0, 3.0));
        assert_eq!(decoded.pixels, (5, 3));
        assert_eq!(decoded.animation, Animation::Still);
        assert_eq!(decoded.frames.len(), 1);
        assert!(!decoded.reduced());
        assert_eq!(decoded.bytes, bytes.len() as u64);
        let first = &decoded.frames[0].buffer().as_raw()[..4];
        assert_eq!(first, &[30, 20, 10, 255], "straight alpha, blue first");
    }

    #[test]
    fn every_still_format_decodes() {
        let image = solid(6, 4, [200, 100, 50, 255]);
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
            ImageFormat::Bmp,
            ImageFormat::Ico,
        ] {
            let bytes = encode(format, &image);
            let decoded =
                decode(&bytes, sniff(&bytes)).unwrap_or_else(|e| panic!("{format:?}: {e:?}"));
            assert_eq!(decoded.pixels, (6, 4), "{format:?}");
            assert_eq!(decoded.frames.len(), 1, "{format:?}");
        }
    }

    #[test]
    fn a_truncated_picture_is_corrupt_not_text() {
        let bytes = encode(ImageFormat::Png, &solid(64, 64, [1, 2, 3, 255]));
        let cut = &bytes[..bytes.len() / 2];
        assert_eq!(sniff(cut), Sniffed::Raster(Format::Png));
        assert_eq!(decode(cut, sniff(cut)).err(), Some(ImageError::Corrupt));
    }

    #[test]
    fn a_picture_over_the_pixel_limit_is_refused_before_it_is_decoded() {
        // 4097 x 4097 is one row and one column over 4096 x 4096.
        let side = 4097u32;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &vec![0u8; (side * side) as usize],
                side,
                side,
                ColorType::L8.into(),
            )
            .unwrap();
        assert_eq!(
            decode(&bytes, sniff(&bytes)).err(),
            Some(ImageError::TooManyPixels {
                width: side,
                height: side
            })
        );
    }

    #[test]
    fn a_picture_exactly_at_the_pixel_limit_is_accepted() {
        let side = 4096u32;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &vec![7u8; (side * side) as usize],
                side,
                side,
                ColorType::L8.into(),
            )
            .unwrap();
        let decoded = decode(&bytes, sniff(&bytes)).expect("at the limit");
        assert_eq!(decoded.pixels, (4096, 4096));
        assert_eq!(
            decoded.frames[0].buffer().as_raw().len() as u64,
            MAX_RGBA_BYTES
        );
    }

    #[test]
    fn input_over_the_byte_limit_is_refused() {
        let big = vec![0u8; MAX_IMAGE_BYTES as usize + 1];
        assert_eq!(
            decode(&big, Sniffed::Raster(Format::Png)).err(),
            Some(ImageError::TooLarge {
                bytes: MAX_IMAGE_BYTES + 1
            })
        );
    }

    #[test]
    fn a_panorama_inside_the_pixel_limit_is_reduced_to_the_tile_side() {
        // 8200 x 3 is far under 16 M pixels and over the tile side.
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&vec![90u8; 8200 * 3], 8200, 3, ColorType::L8.into())
            .unwrap();
        let decoded = decode(&bytes, sniff(&bytes)).expect("decodes");
        assert_eq!(decoded.natural, (8200.0, 3.0), "it keeps its logical size");
        assert_eq!(decoded.pixels.0, MAX_TILE_SIDE);
        assert!(decoded.pixels.1 >= 1);
        assert!(decoded.reduced());
    }

    #[test]
    fn an_animation_inside_the_budget_keeps_every_frame() {
        let bytes = gif_of(3, 8, 8);
        let decoded = decode(&bytes, sniff(&bytes)).expect("decodes");
        assert_eq!(decoded.animation, Animation::Animated { frames: 3 });
        assert_eq!(decoded.frames.len(), 3);
    }

    #[test]
    fn an_animation_over_the_budget_shows_its_first_frame_and_says_so() {
        let bytes = gif_of(4, 8, 8);
        let frame = 8 * 8 * 4 + FRAME_OVERHEAD;
        // Room for exactly two frames.
        let decoded = decode_with_budget(&bytes, sniff(&bytes), 2 * frame).expect("decodes");
        assert_eq!(decoded.animation, Animation::FirstFrameOnly);
        assert_eq!(decoded.frames.len(), 1);
        // The same budget holds exactly two frames, and not a third.
        let two = gif_of(2, 8, 8);
        let kept = decode_with_budget(&two, sniff(&two), 2 * frame).expect("decodes");
        assert_eq!(kept.animation, Animation::Animated { frames: 2 });
        let three = gif_of(3, 8, 8);
        let cut = decode_with_budget(&three, sniff(&three), 2 * frame).expect("decodes");
        assert_eq!(cut.animation, Animation::FirstFrameOnly);
    }

    #[test]
    fn a_first_frame_is_kept_even_when_it_alone_is_over_a_tiny_budget() {
        let bytes = gif_of(2, 8, 8);
        let decoded = decode_with_budget(&bytes, sniff(&bytes), 1).expect("decodes");
        assert_eq!(decoded.animation, Animation::FirstFrameOnly);
        assert_eq!(decoded.frames.len(), 1);
    }

    #[test]
    fn a_frame_too_brief_to_see_gets_a_visible_delay() {
        let quick = Frame::from_parts(
            solid(1, 1, [0, 0, 0, 255]),
            0,
            0,
            Delay::from_numer_denom_ms(0, 1),
        );
        let (n, d) = retimed(quick).delay().numer_denom_ms();
        assert_eq!(n / d, FALLBACK_DELAY_MS);
        let slow = Frame::from_parts(
            solid(1, 1, [0, 0, 0, 255]),
            0,
            0,
            Delay::from_numer_denom_ms(80, 1),
        );
        let (n, d) = retimed(slow).delay().numer_denom_ms();
        assert_eq!(n / d, 80);
    }

    #[test]
    fn an_svg_is_drawn_large_enough_to_enlarge_and_keeps_its_own_size() {
        let decoded = decode(SVG.as_bytes(), sniff(SVG.as_bytes())).expect("decodes");
        assert_eq!(decoded.format, Format::Svg);
        assert_eq!(decoded.natural, (32.0, 16.0));
        assert_eq!(decoded.pixels, (1024, 512));
        assert_eq!(decoded.frames.len(), 1);
        assert!(!decoded.reduced());
        // #336699, blue first.
        let px = &decoded.frames[0].buffer().as_raw()[..4];
        assert_eq!(px, &[0x99, 0x66, 0x33, 0xff]);
    }

    #[test]
    fn an_svg_that_is_not_well_formed_is_corrupt() {
        let broken = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\"><rect width=\"3";
        assert_eq!(sniff(broken.as_bytes()), Sniffed::Svg);
        assert_eq!(
            decode(broken.as_bytes(), Sniffed::Svg).err(),
            Some(ImageError::Corrupt)
        );
    }

    #[test]
    fn an_svg_that_links_or_embeds_a_picture_is_unsupported_never_drawn_with_a_hole() {
        for href in [
            "/etc/hostname",
            "file:///etc/hostname",
            "other.png",
            "data:image/png;base64,iVBORw0KGgo=",
        ] {
            let doc = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8"><image href="{href}" width="8" height="8"/></svg>"#
            );
            assert_eq!(
                decode(doc.as_bytes(), sniff(doc.as_bytes())).err(),
                Some(ImageError::Unsupported(Unsupported::SvgImages)),
                "{href}"
            );
        }
    }

    #[test]
    fn an_svg_that_is_not_utf8_is_corrupt() {
        assert_eq!(
            decode(
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>\xff",
                Sniffed::Svg
            )
            .err(),
            Some(ImageError::Corrupt)
        );
    }

    #[test]
    fn a_picture_format_the_viewer_does_not_do_is_named() {
        let tiff = b"II*\0\x08\0\0\0";
        assert_eq!(
            decode(tiff, sniff(tiff)).err(),
            Some(ImageError::Unsupported(Unsupported::Format("TIFF")))
        );
    }

    #[test]
    fn an_svg_drawn_below_its_own_size_is_reduced_and_one_drawn_above_is_not() {
        // Natural size is above the side the raster may have.
        let huge = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20000" height="100"><rect width="20000" height="100" fill="#336699"/></svg>"##;
        let reduced = decode(huge.as_bytes(), sniff(huge.as_bytes())).expect("decodes");
        assert_eq!(reduced.natural, (20000.0, 100.0));
        assert_eq!(reduced.pixels.0, MAX_TILE_SIDE);
        assert!(reduced.reduced(), "the raster is smaller than the drawing");
        // Drawn larger than natural to be enlargeable: not a reduction.
        let small = decode(SVG.as_bytes(), sniff(SVG.as_bytes())).expect("decodes");
        assert!(!small.reduced());
        // Its natural size already past the minimum: drawn as it is.
        let natural = r##"<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="10"><rect width="2000" height="10" fill="#336699"/></svg>"##;
        let same = decode(natural.as_bytes(), sniff(natural.as_bytes())).expect("decodes");
        assert_eq!(same.pixels, (2000, 10));
        assert!(!same.reduced());
    }

    /// A PNG whose colour profile inflates past the budget while the header is
    /// read, before any pixel is decoded.
    fn png_with_huge_profile() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        encoder
            .set_icc_profile(vec![0; 64 * 1024 * 1024 + 1024])
            .unwrap();
        encoder
            .write_image(&[20, 40, 60, 255], 1, 1, ColorType::Rgba8.into())
            .unwrap();
        bytes
    }

    /// An icon file of `entries`, `(width, height, bits, length, offset)`,
    /// followed by `payload`.
    fn ico_of(entries: &[(u8, u8, u16, u32, u32)], payload: &[u8]) -> Vec<u8> {
        let mut ico = vec![0, 0, 1, 0];
        ico.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for &(width, height, bits, length, offset) in entries {
            ico.extend_from_slice(&[width, height, 0, 0]);
            ico.extend_from_slice(&1u16.to_le_bytes());
            ico.extend_from_slice(&bits.to_le_bytes());
            ico.extend_from_slice(&length.to_le_bytes());
            ico.extend_from_slice(&offset.to_le_bytes());
        }
        ico.extend_from_slice(payload);
        ico
    }

    /// Reading a PNG's header inflates its colour profile, and the library's
    /// `set_limits` afterwards does not reach the reader that did it. The
    /// bounded constructor does: the profile past the limit is dropped, and the
    /// picture, which is one pixel, is still shown.
    #[test]
    fn png_metadata_is_limited_while_the_decoder_is_constructed() {
        assert_eq!(MAX_RGBA_BYTES, 64 * 1024 * 1024, "the approved bound");
        let bytes = png_with_huge_profile();
        assert!(
            bytes.len() < 20 * 1024 * 1024,
            "input is under the file cap"
        );
        assert_eq!(sniff(&bytes), Sniffed::Raster(Format::Png));
        let mut decoder = bounded_png(Cursor::new(&bytes[..])).expect("builds");
        // A boolean, not `assert_eq!`: a regression must not print a 64 MiB
        // profile into the log.
        assert!(
            decoder.icc_profile().expect("reads").is_none(),
            "the oversized profile is dropped, never inflated and kept"
        );
        let decoded = decode(&bytes, sniff(&bytes)).expect("the picture itself is fine");
        assert_eq!(decoded.pixels, (1, 1));
    }

    /// An icon's PNG goes through the same bounded constructor, once.
    #[test]
    fn an_icon_holding_a_png_with_a_huge_profile_still_shows_its_picture() {
        let png = png_with_huge_profile();
        let ico = ico_of(&[(1, 1, 32, png.len() as u32, 22)], &png);
        assert!(ico.len() < 20 * 1024 * 1024);
        assert_eq!(sniff(&ico), Sniffed::Raster(Format::Ico));
        let decoded = decode(&ico, sniff(&ico)).expect("decodes");
        assert_eq!(decoded.pixels, (1, 1));
    }

    /// Reviewer regression: the second entry is the one the library prefers
    /// (more bits), and its length is as large as a length can be.
    #[test]
    fn a_later_preferred_icon_entry_cannot_skip_the_bounded_preflight() {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[20, 40, 60, 255], 1, 1, ColorType::Rgba8.into())
            .unwrap();
        let mut ico = vec![0, 0, 1, 0, 2, 0];
        for (depth, size, offset) in [
            (8u16, png.len() as u32, 38u32),
            (32, u32::MAX, 38 + png.len() as u32),
        ] {
            ico.extend_from_slice(&[1, 1, 0, 0]);
            ico.extend_from_slice(&1u16.to_le_bytes());
            ico.extend_from_slice(&depth.to_le_bytes());
            ico.extend_from_slice(&size.to_le_bytes());
            ico.extend_from_slice(&offset.to_le_bytes());
        }
        ico.extend_from_slice(&png);
        ico.extend_from_slice(&png);
        assert_eq!(
            sniff(&ico),
            Sniffed::Raster(Format::Ico),
            "the first entry is valid"
        );
        assert_eq!(
            decode(&ico, sniff(&ico)).err(),
            Some(ImageError::Corrupt),
            "the library prefers the later 32-bit entry but must not see it unchecked"
        );
        assert_eq!(preflight_ico(&ico), Err(ImageError::Corrupt));
    }

    #[test]
    fn every_icon_entry_is_held_to_the_file() {
        let png = encode(ImageFormat::Png, &solid(1, 1, [1, 2, 3, 255]));
        let len = png.len() as u32;
        let good = (1u8, 1u8, 8u16, len, 22 + 16);
        // A second entry that is wrong in one way each. `22 + 16` is where
        // the payload starts with two entries (6 + 2 * 16 = 38).
        let start = 6 + 2 * 16;
        let wrong: [(&str, (u8, u8, u16, u32, u32)); 6] = [
            ("zero length", (1, 1, 32, 0, start)),
            ("shorter than a signature", (1, 1, 32, 7, start)),
            ("starts inside the directory", (1, 1, 32, len, 10)),
            (
                "starts at the directory's edge minus one",
                (1, 1, 32, len, start - 1),
            ),
            ("ends past the file", (1, 1, 32, len + 1, start)),
            ("length that wraps", (1, 1, 32, u32::MAX, start)),
        ];
        for (what, entry) in wrong {
            let first = (good.0, good.1, good.2, len, start);
            let ico = ico_of(&[first, entry], &png);
            assert_eq!(
                decode(&ico, Sniffed::Raster(Format::Ico)).err(),
                Some(ImageError::Corrupt),
                "{what}"
            );
        }
        // The same entries, sound, decode: the guard is not simply refusing
        // two-entry icons.
        let ico = ico_of(&[(1, 1, 8, len, start), (1, 1, 32, len, start)], &png);
        assert!(decode(&ico, Sniffed::Raster(Format::Ico)).is_ok());
    }

    #[test]
    fn an_icon_directory_must_be_whole_and_in_range() {
        let png = encode(ImageFormat::Png, &solid(1, 1, [1, 2, 3, 255]));
        let entry = (1u8, 1u8, 32u16, png.len() as u32, 22u32);
        // No entries.
        let none = ico_of(&[], &[]);
        assert_eq!(
            decode(&none, Sniffed::Raster(Format::Ico)).err(),
            Some(ImageError::Corrupt)
        );
        // A count that says more entries than the file holds.
        let mut cut = ico_of(&[entry], &png);
        cut[4] = 9;
        assert_eq!(
            decode(&cut, Sniffed::Raster(Format::Ico)).err(),
            Some(ImageError::Corrupt)
        );
        // Too short to hold a header.
        assert_eq!(
            decode(&[0, 0, 1, 0], Sniffed::Raster(Format::Ico)).err(),
            Some(ImageError::Corrupt)
        );
    }

    #[test]
    fn an_icon_entry_that_disagrees_with_its_picture_is_corrupt() {
        let png = encode(ImageFormat::Png, &solid(16, 16, [1, 2, 3, 255]));
        // The entry says 8 x 8, the PNG is 16 x 16.
        let ico = ico_of(&[(8, 8, 32, png.len() as u32, 22)], &png);
        assert_eq!(
            decode(&ico, Sniffed::Raster(Format::Ico)).err(),
            Some(ImageError::Corrupt)
        );
    }

    #[test]
    fn the_icon_entry_is_chosen_as_the_library_chooses_it() {
        // Two entries holding different pictures: the one with more bits wins,
        // and with equal bits the larger; the last wins a tie.
        let small = encode(ImageFormat::Png, &solid(4, 4, [10, 10, 10, 255]));
        let large = encode(ImageFormat::Png, &solid(8, 8, [20, 20, 20, 255]));
        let start = 6 + 2 * 16;
        let at_large = start + small.len() as u32;
        let payload = [small.clone(), large.clone()].concat();
        let by_bits = ico_of(
            &[
                (4, 4, 32, small.len() as u32, start),
                (8, 8, 24, large.len() as u32, at_large),
            ],
            &payload,
        );
        assert_eq!(
            decode(&by_bits, Sniffed::Raster(Format::Ico))
                .unwrap()
                .pixels,
            (4, 4)
        );
        let by_area = ico_of(
            &[
                (4, 4, 32, small.len() as u32, start),
                (8, 8, 32, large.len() as u32, at_large),
            ],
            &payload,
        );
        assert_eq!(
            decode(&by_area, Sniffed::Raster(Format::Ico))
                .unwrap()
                .pixels,
            (8, 8)
        );
        let tie = ico_of(
            &[
                (8, 8, 32, large.len() as u32, start),
                (8, 8, 32, small.len() as u32, start + large.len() as u32),
            ],
            &[large.clone(), small.clone()].concat(),
        );
        // The last entry's PNG is the 4 x 4 one, but its entry says 8 x 8:
        // the library chooses it, and the mismatch makes it corrupt.
        assert_eq!(
            decode(&tie, Sniffed::Raster(Format::Ico)).err(),
            Some(ImageError::Corrupt)
        );
    }

    /// An icon file whose entries are `(width, height, bits, payload)`, the
    /// payloads laid out in order straight after the directory.
    fn ico_from(entries: &[(u8, u8, u16, Vec<u8>)]) -> Vec<u8> {
        let mut offset = 6 + 16 * entries.len() as u32;
        let mut directory = Vec::new();
        for (width, height, bits, payload) in entries {
            directory.push((*width, *height, *bits, payload.len() as u32, offset));
            offset += payload.len() as u32;
        }
        let payloads: Vec<u8> = entries.iter().flat_map(|e| e.3.clone()).collect();
        ico_of(&directory, &payloads)
    }

    /// A 1 x 1, 32-bit DIB as an icon stores it: no file header, a height that
    /// counts the AND mask too, the one pixel, the mask row.
    fn bmp_entry(b: u8, g: u8, r: u8) -> Vec<u8> {
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend_from_slice(&[b, g, r, 255]);
        dib.extend_from_slice(&[0, 0, 0, 0]);
        dib
    }

    /// The first pixel decoded, BGRA.
    fn first_pixel(ico: &[u8]) -> [u8; 4] {
        let decoded = decode(ico, Sniffed::Raster(Format::Ico)).expect("decodes");
        decoded.frames[0].buffer().as_raw()[..4].try_into().unwrap()
    }

    /// The library keeps the *first* of equal best entries it scans, starting
    /// from the last: not `max_by_key`, which would take the last of them. A
    /// PNG and a BMP of equal score make the difference visible, and a BMP
    /// chosen must never let the library reach a PNG.
    #[test]
    fn the_icon_entry_is_the_library_s_with_a_png_and_a_bmp_of_equal_score() {
        let png = encode(ImageFormat::Png, &solid(1, 1, [10, 20, 30, 255]));
        let bmp = bmp_entry(50, 100, 200);
        let png_pixel = [30, 20, 10, 255];
        let bmp_pixel = [50, 100, 200, 255];
        let low = bmp_entry(1, 2, 3);

        // Equal best entries first, a lower one last: the first of them stays.
        let ico = ico_from(&[
            (1, 1, 32, png.clone()),
            (1, 1, 32, bmp.clone()),
            (1, 1, 24, low.clone()),
        ]);
        assert_eq!(first_pixel(&ico), png_pixel, "[PNG, BMP, lower]");
        let ico = ico_from(&[
            (1, 1, 32, bmp.clone()),
            (1, 1, 32, png.clone()),
            (1, 1, 24, low.clone()),
        ]);
        assert_eq!(first_pixel(&ico), bmp_pixel, "[BMP, PNG, lower]");

        // Equal best entries with nothing lower after them: the last of them
        // is where the scan starts, and nothing is strictly greater.
        let ico = ico_from(&[(1, 1, 32, bmp.clone()), (1, 1, 32, png.clone())]);
        assert_eq!(first_pixel(&ico), png_pixel, "[BMP, PNG]");
        let ico = ico_from(&[(1, 1, 32, png.clone()), (1, 1, 32, bmp.clone())]);
        assert_eq!(first_pixel(&ico), bmp_pixel, "[PNG, BMP]");

        // More bits wins whatever the order.
        let ico = ico_from(&[(1, 1, 8, png.clone()), (1, 1, 32, bmp.clone())]);
        assert_eq!(first_pixel(&ico), bmp_pixel, "[PNG 8, BMP 32]");
        let ico = ico_from(&[(1, 1, 32, png), (1, 1, 8, bmp)]);
        assert_eq!(first_pixel(&ico), png_pixel, "[PNG 32, BMP 8]");
    }

    /// A BMP entry is decoded from an icon of its own, so the library has no
    /// other entry to choose, whatever the directory around it holds.
    #[test]
    fn a_bmp_entry_is_decoded_alone() {
        let bmp = bmp_entry(50, 100, 200);
        let ico = ico_from(&[(1, 1, 32, bmp.clone())]);
        let entry = preflight_ico(&ico).expect("sound");
        let alone = single_entry_icon(&entry, &ico);
        assert_eq!(alone.len(), 22 + bmp.len());
        assert_eq!(&alone[22..], &bmp[..]);
        assert_eq!(le16(&alone, 4), 1, "one entry");
        assert_eq!(first_pixel(&ico), [50, 100, 200, 255]);
    }

    #[test]
    fn an_ordinary_png_and_an_icon_holding_one_still_decode() {
        let image = solid(16, 16, [10, 20, 30, 255]);
        let png = encode(ImageFormat::Png, &image);
        let mut ico = vec![0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0];
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&png);
        assert_eq!(sniff(&ico), Sniffed::Raster(Format::Ico));
        let decoded = decode(&ico, sniff(&ico)).expect("decodes");
        assert_eq!(decoded.pixels, (16, 16));
        assert!(!decoded.reduced());
    }
}
