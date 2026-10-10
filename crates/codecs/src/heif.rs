//! HEIC / HEIF still images: the ISO-BMFF item handling lives in [`crate::isobmff_still`]; this module
//! decodes each HEVC intra item with the clean-room pure-Rust `filmcraft-hevc` (ITU-T H.265, Main /
//! Main 10, 4:2:0).
//!
//! Supported: primary item `hvc1` or a `grid` of `hvc1` tiles; `colr` (`nclx` / ICC), `irot` / `imir`
//! (reported as an EXIF orientation, not applied), `clap` crop, auxiliary alpha, EXIF and XMP items.
//! Not supported (reported as errors, never panics): `iovl` overlays, 4:2:2 / 4:4:4 / monochrome HEVC
//! (so an alpha plane coded as 4:0:0, as Apple writes it, is dropped), HDR transfer functions beyond a
//! plain sRGB / Rec.709 reading.

use crate::isobmff_still::{Codec, Prop, Still, Yuv, bad};
use crate::{DecodeOptions, Decoded, Error, Format, Result};
use filmcraft_hevc::{Decoder, Plane};

const F: Format = Format::Heif;

fn plane16(p: &Plane) -> Vec<u16> {
    match p {
        Plane::U8(v) => v.iter().map(|&x| x as u16).collect(),
        Plane::U16(v) => v.clone(),
    }
}

fn hevc_err(e: filmcraft_hevc::Error) -> Error {
    match e {
        filmcraft_hevc::Error::Unsupported(_) => Error::Unsupported(F, "HEVC feature not supported by the decoder (e.g. 4:0:0 / 4:2:2 / 4:4:4)"),
        e => bad(F, format!("HEVC: {e}")),
    }
}

fn decode_tile(h: &Still<'_>, id: u32) -> Result<Yuv> {
    let item = h.item(id).ok_or_else(|| bad(F, format!("missing item {id}")))?;
    if &item.kind != b"hvc1" && &item.kind != b"hev1" {
        return Err(Error::Unsupported(F, "primary item is not an HEVC image or grid (overlay / AV1 / other)"));
    }
    let hvcc = h
        .item_props(id)
        .into_iter()
        .find_map(|p| if let Prop::Hvcc(c) = p { Some(c.clone()) } else { None })
        .ok_or_else(|| bad(F, "tile has no hvcC"))?;
    let data = h.data(id)?;
    let mut dec = Decoder::from_hvcc(&hvcc).map_err(hevc_err)?;
    let mut pics = dec.decode(&data, 0).map_err(hevc_err)?;
    pics.extend(dec.flush());
    if let Some(e) = dec.take_error() {
        return Err(hevc_err(e));
    }
    let p = pics.into_iter().next().ok_or_else(|| bad(F, "HEVC stream produced no picture"))?;
    let (w, ht) = (p.width as usize, p.height as usize);
    let (cw, ch) = (p.chroma_width as usize, p.chroma_height as usize);
    if cw != w.div_ceil(2) || ch != ht.div_ceil(2) {
        return Err(Error::Unsupported(F, "only 4:2:0 HEVC images are supported"));
    }
    // Re-pack to tight strides.
    let (y16, u16v, v16) = (plane16(&p.y), plane16(&p.u), plane16(&p.v));
    let tight = |src: &[u16], stride: usize, w: usize, h: usize| -> Vec<u16> {
        let mut o = Vec::with_capacity(w * h);
        for r in 0..h {
            o.extend_from_slice(src.get(r * stride..r * stride + w).unwrap_or(&[]));
        }
        o
    };
    Ok(Yuv {
        w,
        h: ht,
        cw,
        ch,
        ssx: 1,
        ssy: 1,
        mono: false,
        bit_depth: p.bit_depth,
        y: tight(&y16, p.y_stride, w, ht),
        u: tight(&u16v, p.uv_stride, cw, ch),
        v: tight(&v16, p.uv_stride, cw, ch),
        vui: (p.color.full_range, p.color.matrix, p.color.primaries),
        cicp: None,
        alpha: None,
    })
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    crate::isobmff_still::decode(&Codec { fmt: F, tile: decode_tile, alpha_unsupported_ok: true }, bytes, opts)
}
