//! Finds the color profile the `avif`/`heif`/`webp` containers declare, by walking their boxes and chunks rather than
//! asking the codecs, none of which report it.
//!
//! - **WebP:** the RIFF `ICCP` chunk, which an extended (`VP8X`) file places ahead of the image data.
//! - **AVIF and HEIF:** the primary item's `colr` property, reached from `meta` through `pitm`, `iprp`/`ipco` and
//!   `ipma`. It holds either an ICC profile (`prof`/`rICC`) or CICP codes (`nclx`).

use super::ImageFormat;
use super::icc;

/// The bytes ran out before the answer was found: the input is a prefix of the file, or a truncated file.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Truncated;

/// A RIFF chunk or an ISO-BMFF box split off the front of some bytes: its four-character type, its contents, and the
/// bytes that follow it.
type Split<'a> = ([u8; 4], &'a [u8], &'a [u8]);

/// Returns the name of the color profile the container in `bytes` declares, `Ok(None)` when it declares none (or one
/// with no name), or [`Truncated`] when `bytes` ends before that could be decided.
///
/// Native formats are not containers this reads; they give `Ok(None)`.
pub(super) fn color_profile(bytes: &[u8], format: ImageFormat) -> Result<Option<String>, Truncated> {
    match format {
        ImageFormat::WebP => webp(bytes),
        ImageFormat::Avif | ImageFormat::Heif => isobmff(bytes),
        _ => Ok(None),
    }
}

// ── WebP ───────────────────────────────────────────────────────────────────────────────────────────────────────

/// The `VP8X` flag that says an `ICCP` chunk follows.
const VP8X_ICC: u8 = 0x20;

fn webp(bytes: &[u8]) -> Result<Option<String>, Truncated> {
    if bytes.len() < 12 {
        return Err(Truncated);
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Ok(None);
    }

    let mut rest = &bytes[12..];
    let mut first = true;
    loop {
        let (kind, data, next) = riff_chunk(rest)?;
        match &kind {
            // A simple (lossy or lossless) file has no `VP8X`, and so no room for a profile.
            b"VP8X" if first => {
                if data.first().is_none_or(|flags| flags & VP8X_ICC == 0) {
                    return Ok(None);
                }
            }
            b"ICCP" => return Ok(icc::description(data)),
            // The profile must come before the image data; reaching that means it is not there.
            _ => return Ok(None),
        }
        first = false;
        rest = next;
    }
}

/// Splits the RIFF chunk at the front of `bytes` into its four-character code, its data, and what follows it (chunks
/// are padded to an even length).
fn riff_chunk(bytes: &[u8]) -> Result<Split<'_>, Truncated> {
    let header = bytes.get(0..8).ok_or(Truncated)?;
    let kind: [u8; 4] = header[0..4].try_into().expect("four bytes");
    let size = u32::from_le_bytes(header[4..8].try_into().expect("four bytes")) as usize;

    let data = bytes
        .get(8..8usize.checked_add(size).ok_or(Truncated)?)
        .ok_or(Truncated)?;
    let padded = (8 + size + (size & 1)).min(bytes.len());
    Ok((kind, data, &bytes[padded..]))
}

// ── AVIF and HEIF ──────────────────────────────────────────────────────────────────────────────────────────────

fn isobmff(bytes: &[u8]) -> Result<Option<String>, Truncated> {
    // Top-level boxes are walked until `meta`. Running out first is truncation: the `meta` box may sit after `mdat`.
    let mut rest = bytes;
    let meta = loop {
        match next_box(rest)? {
            Some((kind, body, _)) if &kind == b"meta" => break body,
            Some((_, _, next)) => rest = next,
            None => return Err(Truncated),
        }
    };

    // Everything below is inside a box whose whole extent is present, so a short read there is a malformed file,
    // not a truncated one, and it gives no profile.
    Ok(primary_colr(meta).and_then(|colrs| profile_from_colrs(&colrs)))
}

/// The bodies of the `colr` properties associated with the primary item, from the body of the `meta` box.
fn primary_colr(meta: &[u8]) -> Option<Vec<&[u8]>> {
    // `meta` is a full box: a version byte and three bytes of flags before its children.
    let meta = children(meta.get(4..)?)?;

    let pitm = find(&meta, b"pitm")?;
    let primary = if *pitm.first()? == 0 {
        u32::from(read_u16(pitm, 4)?)
    } else {
        read_u32(pitm, 4)?
    };

    let iprp = children(find(&meta, b"iprp")?)?;
    let properties = children(find(&iprp, b"ipco")?)?;

    let mut colrs = Vec::new();
    for (_, ipma) in iprp.iter().filter(|(kind, _)| kind == b"ipma") {
        for index in associations(ipma, primary)? {
            // Indices are 1-based; 0 means "no property".
            if let Some((kind, body)) = index.checked_sub(1).and_then(|i| properties.get(i))
                && kind == b"colr"
            {
                colrs.push(*body);
            }
        }
    }
    Some(colrs)
}

/// The `ipco` indices the `ipma` box associates with `item`.
fn associations(ipma: &[u8], item: u32) -> Option<Vec<usize>> {
    let version = *ipma.first()?;
    let wide_index = ipma.get(3)? & 1 == 1;
    let count = read_u32(ipma, 4)?;

    let mut at = 8;
    for _ in 0..count {
        let id = if version < 1 {
            let id = u32::from(read_u16(ipma, at)?);
            at += 2;
            id
        } else {
            let id = read_u32(ipma, at)?;
            at += 4;
            id
        };
        let n = usize::from(*ipma.get(at)?);
        at += 1;

        let width = if wide_index { 2 } else { 1 };
        if id == item {
            let entries = ipma.get(at..at + n * width)?;
            return Some(
                entries
                    .chunks_exact(width)
                    .map(|entry| match entry {
                        [high, low] => usize::from(u16::from_be_bytes([*high, *low]) & 0x7FFF),
                        [byte] => usize::from(byte & 0x7F),
                        _ => unreachable!("chunks are one or two bytes"),
                    })
                    .collect(),
            );
        }
        at += n * width;
    }
    Some(Vec::new())
}

/// The name for a set of `colr` bodies: an embedded ICC profile's description when there is a profile, otherwise the
/// table name of a coded color description.
fn profile_from_colrs(colrs: &[&[u8]]) -> Option<String> {
    let icc = colrs
        .iter()
        .find(|body| matches!(body.get(0..4), Some(b"prof" | b"rICC")));
    if let Some(body) = icc {
        return icc::description(&body[4..]);
    }

    let nclx = colrs.iter().find(|body| body.get(0..4) == Some(b"nclx"))?;
    nclx_name(read_u16(nclx, 4)?, read_u16(nclx, 6)?).map(str::to_owned)
}

/// Names a CICP color description (ITU-T H.273 primaries and transfer characteristics). Anything outside the table
/// gives `None` rather than a guess.
pub(super) fn nclx_name(primaries: u16, transfer: u16) -> Option<&'static str> {
    const BT709: u16 = 1;
    const BT2020: u16 = 9;
    const P3_D65: u16 = 12;

    const TRANSFER_BT709: u16 = 1;
    const TRANSFER_BT601: u16 = 6;
    const TRANSFER_SRGB: u16 = 13;
    const TRANSFER_BT2020_10: u16 = 14;
    const TRANSFER_BT2020_12: u16 = 15;
    const TRANSFER_PQ: u16 = 16;
    const TRANSFER_HLG: u16 = 18;

    match (primaries, transfer) {
        (BT709, TRANSFER_SRGB) => Some("sRGB"),
        (P3_D65, TRANSFER_SRGB) => Some("Display P3"),
        (BT2020, TRANSFER_BT709 | TRANSFER_BT601 | TRANSFER_SRGB | TRANSFER_BT2020_10 | TRANSFER_BT2020_12) => {
            Some("Rec. 2020")
        }
        (BT2020, TRANSFER_PQ) => Some("Rec. 2100 PQ"),
        (BT2020, TRANSFER_HLG) => Some("Rec. 2100 HLG"),
        _ => None,
    }
}

/// Splits the ISO-BMFF box at the front of `bytes` into its type, its body and what follows it; `Ok(None)` when
/// `bytes` is empty.
fn next_box(bytes: &[u8]) -> Result<Option<Split<'_>>, Truncated> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let header = bytes.get(0..8).ok_or(Truncated)?;
    let kind: [u8; 4] = header[4..8].try_into().expect("four bytes");

    let (header_len, size) = match u32::from_be_bytes(header[0..4].try_into().expect("four bytes")) {
        // The box runs to the end of the file.
        0 => (8, bytes.len() as u64),
        // A 64-bit size follows the type.
        1 => (
            16,
            u64::from_be_bytes(bytes.get(8..16).ok_or(Truncated)?.try_into().expect("eight bytes")),
        ),
        size => (8, u64::from(size)),
    };

    // A size smaller than its own header cannot be walked past. It is reported the same as running out: at the top
    // level that costs a re-read of the whole file, and below it the walk gives no profile either way.
    let size = usize::try_from(size).map_err(|_| Truncated)?;
    if size < header_len {
        return Err(Truncated);
    }
    let body = bytes.get(header_len..size).ok_or(Truncated)?;
    Ok(Some((kind, body, &bytes[size..])))
}

/// Every child box in `body`, or `None` if they cannot all be walked.
fn children(mut body: &[u8]) -> Option<Vec<([u8; 4], &[u8])>> {
    let mut boxes = Vec::new();
    while let Some((kind, child, rest)) = next_box(body).ok()? {
        boxes.push((kind, child));
        body = rest;
    }
    Some(boxes)
}

fn find<'a>(boxes: &[([u8; 4], &'a [u8])], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes.iter().find(|(k, _)| k == kind).map(|(_, body)| *body)
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}
