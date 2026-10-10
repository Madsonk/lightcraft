//! Lens facts that only live in the maker note: lens name, focal range, widest aperture.
//!
//! Most modern bodies also write the Exif `LensModel`, which wins; the maker note fills what Exif lacks
//! (a Canon EOS that stores only a focal length as its Exif lens name, older Panasonic bodies, Nikon and
//! Fujifilm lens ranges). Tag numbers and array layouts follow the public tag listings of the vendors'
//! maker notes (Canon `CameraSettings`, Panasonic `LensType`, Nikon `LensInfo`, Fujifilm 0x1404-0x1407);
//! nothing else was consulted. Everything read here is untrusted: counts are bounded and values checked.

use lightcraft_tiff::{Tiff, makernote, tags as t};

use crate::Metadata;

/// Canon maker note: `CameraSettings` (array of u16, entry 0 = its byte size).
const CANON_CAMERA_SETTINGS: u16 = 0x0001;
/// Canon maker note: `LensModel` (text), written by newer bodies.
const CANON_LENS_MODEL: u16 = 0x0095;
/// Panasonic maker note: `LensType` (text).
const PANASONIC_LENS_TYPE: u16 = 0x0051;
/// Nikon maker note: `LensInfo` (4 rationals: min/max focal, min/max aperture).
const NIKON_LENS_INFO: u16 = 0x0084;
/// Fujifilm maker note: min focal, max focal, widest aperture at each end.
const FUJI_MIN_FOCAL: u16 = 0x1404;
const FUJI_MAX_FOCAL: u16 = 0x1405;
const FUJI_AP_AT_MIN: u16 = 0x1406;
const FUJI_AP_AT_MAX: u16 = 0x1407;

/// Canon's aperture code → f-number: `f = 2^(code / 64)` (the code counts 1/32 stop of aperture value, and one
/// stop of aperture value is a factor √2 of the f-number).
pub(crate) fn canon_f_number(code: u16) -> Option<f64> {
    let signed = f64::from(code as i16);
    let f = (signed / 64.0).exp2();
    (code != 0 && code != 0xffff && (0.7..=64.0).contains(&f)).then_some(f)
}

/// Fill `m`'s lens fields from the maker note of the Exif block `data` (its start is the TIFF header).
pub(crate) fn read_maker_lens(data: &[u8], tiff: &Tiff, m: &mut Metadata) {
    let Some(entry) = tiff.exif().and_then(|e| e.get(t::MAKER_NOTE)) else { return };
    let make = m.make.clone().unwrap_or_default();
    let Some(mn) = makernote::parse_makernote(data, entry.offset, entry.count() as u64, tiff.order, &make) else { return };
    let ifd = &mn.ifd;
    let lower = make.to_ascii_lowercase();
    let mut spec: Option<[f64; 4]> = None;
    if lower.starts_with("canon") {
        if let Some(name) = ifd.string(CANON_LENS_MODEL) {
            fill_name(m, name);
        }
        if let Some(cs) = ifd.u64s(CANON_CAMERA_SETTINGS).filter(|v| v.len() > 27 && v.len() < 4096) {
            let at = |i: usize| cs.get(i).copied().unwrap_or(0).min(u64::from(u16::MAX)) as u16;
            let id = at(22);
            if id != 0 && id != 0xffff {
                m.lens_type_id = Some(u32::from(id));
            }
            let units = f64::from(at(25).max(1));
            let (long, short) = (f64::from(at(23)) / units, f64::from(at(24)) / units);
            if short > 0.0 && long >= short && long < 5000.0 {
                // the maker note stores the widest aperture at the short and long end; entry 26 is the widest
                let ap = canon_f_number(at(26));
                m.lens_max_aperture = m.lens_max_aperture.or(ap);
                spec = Some([short, long, ap.unwrap_or(0.0), ap.unwrap_or(0.0)]);
            }
        }
    } else if lower.starts_with("panasonic") || lower.starts_with("leica") {
        if let Some(name) = ifd.string(PANASONIC_LENS_TYPE) {
            fill_name(m, name);
        }
    } else if lower.starts_with("nikon") {
        if let Some(v) = ifd.f64s(NIKON_LENS_INFO).filter(|v| v.len() == 4 && v[0] > 0.0 && v[1] >= v[0]) {
            spec = Some([v[0], v[1], v[2], v[3]]);
        }
    } else if lower.starts_with("fuji") {
        let f = |tag: u16| ifd.f64(tag).filter(|v| v.is_finite() && *v > 0.0);
        if let (Some(a), Some(b)) = (f(FUJI_MIN_FOCAL), f(FUJI_MAX_FOCAL)) {
            spec = Some([a, b, f(FUJI_AP_AT_MIN).unwrap_or(0.0), f(FUJI_AP_AT_MAX).unwrap_or(0.0)]);
        }
    }
    if let Some(s) = spec {
        match m.lens_spec {
            None => m.lens_spec = Some(s),
            // Exif's LensSpecification sometimes lacks the apertures (0): take them from the maker note
            Some(mut e) if e[2] <= 0.0 && s[2] > 0.0 && (e[0] - s[0]).abs() < 0.6 => {
                e[2] = s[2];
                e[3] = if e[3] > 0.0 { e[3] } else { s[3] };
                m.lens_spec = Some(e);
            }
            _ => {}
        }
        if m.lens_max_aperture.is_none() && s[2] > 0.0 {
            m.lens_max_aperture = Some(s[2]);
        }
    }
}

/// Set the lens name when Exif gave none (or only a bare focal length like `50mm`, which the maker note's
/// longer name replaces).
fn fill_name(m: &mut Metadata, name: String) {
    let name = name.trim().to_string();
    if name.is_empty() {
        return;
    }
    let bare = |s: &str| {
        let t = s.trim().trim_end_matches("mm").trim();
        !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    };
    match &m.lens_model {
        None => m.lens_model = Some(name),
        Some(cur) if bare(cur) && !bare(&name) => m.lens_model = Some(name),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canon_aperture_codes() {
        // 56 → f/1.8, 288 → f/22 (values read from a real 5D Mark III file)
        assert!((canon_f_number(56).unwrap() - 1.834).abs() < 0.01);
        assert!((canon_f_number(288).unwrap() - 22.6).abs() < 0.1);
        assert_eq!(canon_f_number(0), None);
        assert_eq!(canon_f_number(0xffff), None);
    }

    /// A little-endian TIFF with Make "Canon", an Exif IFD (LensModel "50mm", SubjectDistance 2 m) and a Canon
    /// maker note whose `CameraSettings` array holds the lens facts of a real EOS 5D Mark III file.
    fn canon_file() -> Vec<u8> {
        fn u16s(v: &mut Vec<u8>, x: u16) {
            v.extend_from_slice(&x.to_le_bytes());
        }
        fn u32s(v: &mut Vec<u8>, x: u32) {
            v.extend_from_slice(&x.to_le_bytes());
        }
        fn entry(v: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: u32) {
            u16s(v, tag);
            u16s(v, typ);
            u32s(v, count);
            u32s(v, value);
        }
        let mut v = b"II\x2a\0".to_vec();
        u32s(&mut v, 8);
        // IFD0 at 8: Make (ASCII, out of line at 38), ExifIFD pointer (-> 44)
        u16s(&mut v, 2);
        entry(&mut v, 0x010f, 2, 6, 38);
        entry(&mut v, 0x8769, 4, 1, 44);
        u32s(&mut v, 0);
        v.extend_from_slice(b"Canon\0");
        // Exif IFD at 44 (3 entries: 2 + 36 + 4 = 42 bytes → ends at 86): SubjectDistance → 86 (8 bytes),
        // LensModel → 94 (5 bytes, padded to 6), MakerNote → 100 (74 bytes)
        u16s(&mut v, 3);
        entry(&mut v, 0x9206, 5, 1, 86);
        entry(&mut v, 0x927c, 7, 74, 100);
        entry(&mut v, 0xa434, 2, 5, 94);
        u32s(&mut v, 0);
        assert_eq!(v.len(), 86);
        u32s(&mut v, 2);
        u32s(&mut v, 1);
        v.extend_from_slice(b"50mm\0\0");
        assert_eq!(v.len(), 100);
        // maker note: one entry, CameraSettings (SHORT x 28) at 100 + 2 + 12 + 4 = 118
        u16s(&mut v, 1);
        entry(&mut v, 0x0001, 3, 28, 118);
        u32s(&mut v, 0);
        let mut cs = [0u16; 28];
        cs[0] = 56;
        cs[22] = 220;
        cs[23] = 50;
        cs[24] = 50;
        cs[25] = 1;
        cs[26] = 56;
        cs[27] = 288;
        for c in cs {
            u16s(&mut v, c);
        }
        assert_eq!(v.len(), 174);
        v
    }

    #[test]
    fn canon_maker_note_lens_facts() {
        let m = crate::read_exif(&canon_file());
        assert_eq!(m.make.as_deref(), Some("Canon"));
        assert_eq!(m.lens_model.as_deref(), Some("50mm"));
        assert_eq!(m.lens_type_id, Some(220));
        assert!((m.lens_max_aperture.unwrap() - 1.834).abs() < 0.01);
        let s = m.lens_spec.unwrap();
        assert_eq!((s[0], s[1]), (50.0, 50.0));
        assert!((s[2] - 1.834).abs() < 0.01);
        assert_eq!(m.subject_distance, Some(2.0));
    }

    #[test]
    fn a_truncated_maker_note_gives_no_lens_facts_and_no_panic() {
        let f = canon_file();
        for cut in [100, 110, 118, 130, 160] {
            let m = crate::read_exif(&f[..cut]);
            assert!(m.lens_type_id.is_none() || cut >= 174, "cut {cut}");
        }
    }

    #[test]
    fn bare_focal_name_is_replaced_by_the_maker_note_name() {
        let mut m = Metadata { lens_model: Some("50mm".into()), ..Default::default() };
        fill_name(&mut m, "EF50mm f/1.8 STM".into());
        assert_eq!(m.lens_model.as_deref(), Some("EF50mm f/1.8 STM"));
        let mut m = Metadata { lens_model: Some("EF24-105mm f/4L IS USM".into()), ..Default::default() };
        fill_name(&mut m, "other".into());
        assert_eq!(m.lens_model.as_deref(), Some("EF24-105mm f/4L IS USM"));
        let mut m = Metadata::default();
        fill_name(&mut m, "  ".into());
        assert_eq!(m.lens_model, None);
    }
}
