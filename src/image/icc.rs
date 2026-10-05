//! Reads the human-readable description out of an ICC profile — `Display P3`, `sRGB IEC61966-2.1` — without a
//! color-management engine. Only the `desc` tag is parsed; nothing else in the profile is interpreted.

/// The longest description returned, in characters. The text is whatever the profile's author wrote, so it is bounded
/// rather than trusted.
const MAX_CHARS: usize = 64;

/// The fixed-size header every ICC profile starts with; the tag table follows it.
const HEADER_LEN: usize = 128;

/// Returns the description stored in the ICC `profile`'s `desc` tag, trimmed and capped at [`MAX_CHARS`] characters.
///
/// Both encodings in use are read: the ICC v2 `textDescriptionType` (ASCII) and the v4 `multiLocalizedUnicodeType`
/// (UTF-16BE records), where the English record is preferred and the first record is the fallback. Anything that
/// is not a well-formed profile with a non-empty description gives `None`; no input panics.
pub(super) fn description(profile: &[u8]) -> Option<String> {
    if profile.get(36..40)? != b"acsp" {
        return None;
    }

    // Each tag-table entry is a signature, an offset and a size. `map_while` stops at the first entry past the end, so
    // a garbage count costs nothing.
    let count = read_u32(profile, HEADER_LEN)? as usize;
    let tag = (0..count)
        .map_while(|i| {
            let entry = HEADER_LEN + 4 + i * 12;
            profile.get(entry..entry + 12)
        })
        .find(|entry| &entry[0..4] == b"desc")?;

    let offset = read_u32(tag, 4)? as usize;
    let size = read_u32(tag, 8)? as usize;
    let data = profile.get(offset..offset.checked_add(size)?)?;

    let text = match data.get(0..4)? {
        b"desc" => text_description(data)?,
        b"mluc" => multi_localized(data)?,
        _ => return None,
    };

    let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!text.is_empty()).then(|| text.chars().take(MAX_CHARS).collect::<String>().trim_end().to_owned())
}

/// ICC v2 `textDescriptionType`: the signature, 4 reserved bytes, an ASCII count (which includes the terminating
/// NUL), then the ASCII text. The Unicode and ScriptCode variants that follow it repeat the same text and are skipped.
fn text_description(data: &[u8]) -> Option<String> {
    let count = read_u32(data, 8)? as usize;
    let ascii = data.get(12..12usize.checked_add(count)?)?;
    let end = ascii.iter().position(|&b| b == 0).unwrap_or(ascii.len());
    Some(String::from_utf8_lossy(&ascii[..end]).into_owned())
}

/// ICC v4 `multiLocalizedUnicodeType`: the signature, 4 reserved bytes, a record count and a record size, then one
/// record per locale — language, country, byte length and an offset from the tag's start — pointing at UTF-16BE text.
fn multi_localized(data: &[u8]) -> Option<String> {
    let count = read_u32(data, 8)? as usize;
    let record_size = read_u32(data, 12)? as usize;
    if record_size < 12 {
        return None;
    }

    let records: Vec<&[u8]> = (0..count)
        .map_while(|i| {
            let start = 16usize.checked_add(i.checked_mul(record_size)?)?;
            data.get(start..start + 12)
        })
        .collect();
    let record = records
        .iter()
        .find(|record| &record[0..2] == b"en")
        .or_else(|| records.first())?;

    let length = read_u32(record, 4)? as usize;
    let offset = read_u32(record, 8)? as usize;
    let utf16 = data.get(offset..offset.checked_add(length)?)?;
    let units: Vec<u16> = utf16
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_be_bytes(pair))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes(slice.try_into().ok()?))
}
