//! Reading the parts of a `.task` model bundle (a zip archive of `.tflite` models; stored or
//! deflated entries, zip64 sizes) and float16 conversion. Adapted from the zip reader of
//! EffectCraft's `effectcraft-segment` (same authors, MIT OR Apache-2.0). Anything unexpected is
//! an error, never a panic.

use std::collections::HashMap;

type Result<T> = std::result::Result<T, String>;

fn u16_at(b: &[u8], o: usize) -> Result<usize> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize).ok_or_else(|| "truncated zip".to_string())
}
fn u32_at(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64).ok_or_else(|| "truncated zip".to_string())
}
fn u64_at(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 8).and_then(|s| s.try_into().ok()).map(u64::from_le_bytes).ok_or_else(|| "truncated zip".to_string())
}

/// One archive entry: where its data starts, its sizes and compression.
struct Entry {
    data: usize,
    size: usize,
    stored: usize,
    method: usize,
}

/// The entries of a zip archive by name.
fn zip_entries(b: &[u8]) -> Result<HashMap<String, Entry>> {
    // End of central directory: the last `PK\x05\x06` (a comment may follow it).
    let eocd = (0..b.len().saturating_sub(21))
        .rev()
        .find(|&i| b.get(i..i + 4) == Some(b"PK\x05\x06"))
        .ok_or("not a zip archive (no end of central directory)")?;
    let mut count = u16_at(b, eocd + 10)? as u64;
    let mut cd = u32_at(b, eocd + 16)?;
    // Zip64: the locator precedes the record.
    if (count == 0xffff || cd == 0xffff_ffff) && eocd >= 20 && b.get(eocd - 20..eocd - 16) == Some(b"PK\x06\x07") {
        let rec = u64_at(b, eocd - 12)? as usize;
        if b.get(rec..rec + 4) == Some(b"PK\x06\x06") {
            count = u64_at(b, rec + 32)?;
            cd = u64_at(b, rec + 48)?;
        }
    }
    let mut out = HashMap::new();
    let mut p = cd as usize;
    for _ in 0..count.min(1 << 20) {
        if b.get(p..p + 4) != Some(b"PK\x01\x02") {
            return Err("damaged zip central directory".into());
        }
        let method = u16_at(b, p + 10)?;
        let mut stored = u32_at(b, p + 20)?;
        let mut size = u32_at(b, p + 24)?;
        let (nl, xl, cl) = (u16_at(b, p + 28)?, u16_at(b, p + 30)?, u16_at(b, p + 32)?);
        let mut local = u32_at(b, p + 42)?;
        let name = String::from_utf8_lossy(b.get(p + 46..p + 46 + nl).ok_or("truncated zip")?).to_string();
        // Zip64 extended information: the 0xffffffff fields, in order.
        let mut x = p + 46 + nl;
        let xend = x + xl;
        while x + 4 <= xend {
            let (id, len) = (u16_at(b, x)?, u16_at(b, x + 2)?);
            if id == 1 {
                let mut f = x + 4;
                for v in [&mut size, &mut stored, &mut local] {
                    if *v == 0xffff_ffff {
                        *v = u64_at(b, f)?;
                        f += 8;
                    }
                }
            }
            x += 4 + len;
        }
        let l = local as usize;
        if b.get(l..l + 4) != Some(b"PK\x03\x04") {
            return Err(format!("damaged zip entry {name}"));
        }
        let data = l + 30 + u16_at(b, l + 26)? + u16_at(b, l + 28)?;
        out.insert(name, Entry { data, size: size as usize, stored: stored as usize, method });
        p += 46 + nl + xl + cl;
    }
    Ok(out)
}

/// An entry's bytes (stored, or raw-deflated).
fn read_entry<'a>(b: &'a [u8], e: &Entry) -> Result<std::borrow::Cow<'a, [u8]>> {
    let raw = b.get(e.data..e.data + e.stored).ok_or("truncated zip entry")?;
    match e.method {
        0 => Ok(std::borrow::Cow::Borrowed(raw)),
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, e.size.max(1))
            .map(std::borrow::Cow::Owned)
            .map_err(|e| format!("bad deflate data: {e:?}")),
        m => Err(format!("unsupported zip compression {m}")),
    }
}

/// The bytes of the archive entry `name` (model bundles are zip archives too).
pub fn zip_file<'a>(b: &'a [u8], name: &str) -> Result<std::borrow::Cow<'a, [u8]>> {
    let entries = zip_entries(b)?;
    read_entry(b, entries.get(name).ok_or_else(|| format!("{name} is missing from the archive"))?)
}

pub fn half_to_f32(h: u16) -> f32 {
    let (s, e, m) = ((h >> 15) as u32, ((h >> 10) & 0x1f) as u32, (h & 0x3ff) as u32);
    let bits = match (e, m) {
        (0, 0) => s << 31,
        (0, m) => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            (s << 31) | (e << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, m) => (s << 31) | (0xff << 23) | (m << 13),
        (e, m) => (s << 31) | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}
