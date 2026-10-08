//! AVIF still images: the ISO-BMFF item handling lives in [`crate::isobmff_still`]; this module decodes
//! each `av01` item with the clean-room pure-Rust `filmcraft-av1` (AV1, 8 / 10 bits, 4:2:0, 4:2:2,
//! 4:4:4 and monochrome as far as that decoder supports them).
//!
//! Supported: primary item `av01` or a `grid` of `av01` tiles; `colr` (`nclx` / ICC; the AV1 sequence
//! header's CICP when there is none), `irot` / `imir` (reported as an EXIF orientation, not applied),
//! `clap` crop, auxiliary alpha (`auxl`), EXIF and XMP items. 10-bit images decode to a 16-bit buffer.
//! Not supported: `iovl` overlays, image sequences (`avis`), HDR transfer functions beyond a plain
//! sRGB / Rec.709 reading.

use crate::isobmff_still::{Codec, Prop, Still, Yuv, bad};
use crate::{DecodeOptions, Decoded, Error, Format, Result};
use filmcraft_av1::Decoder;

const F: Format = Format::Avif;

fn av1_err(e: filmcraft_av1::Error) -> Error {
    match e {
        filmcraft_av1::Error::Unsupported(_) => Error::Unsupported(F, "AV1 feature not supported by the decoder"),
        e => bad(F, format!("AV1: {e}")),
    }
}

fn decode_tile(h: &Still<'_>, id: u32) -> Result<Yuv> {
    let item = h.item(id).ok_or_else(|| bad(F, format!("missing item {id}")))?;
    if &item.kind != b"av01" {
        return Err(Error::Unsupported(F, "primary item is not an AV1 image or grid (overlay / other)"));
    }
    // av1C: 4 bytes of configuration, then the configuration OBUs (the sequence header).
    let mut stream: Vec<u8> = h.item_props(id).into_iter().find_map(|p| if let Prop::Av1c(c) = p { Some(c.get(4..).unwrap_or(&[]).to_vec()) } else { None }).unwrap_or_default();
    stream.extend_from_slice(&h.data(id)?);
    // One thread, decoding on this thread: `Decoder::new()` hands frames to worker threads, and a panic
    // inside a worker (filmcraft-av1 indexes out of bounds on some corrupt streams, see
    // `tests/data/av1_frame_worker_panic.avif`) leaves the waiting caller blocked forever. Here the
    // panic unwinds into `decode`'s `catch_unwind` and becomes an error. Costs about 3x on a 12 MP still.
    let mut dec = Decoder::with_threads(1);
    let mut pics = dec.decode(&stream).map_err(av1_err)?;
    pics.extend(dec.flush_result().map_err(av1_err)?);
    let p = pics.into_iter().next().ok_or_else(|| bad(F, "AV1 stream produced no picture"))?;
    let (w, ht) = (p.width as usize, p.height as usize);
    let (ssx, ssy) = (p.subsampling_x as u32, p.subsampling_y as u32);
    let [y, u, v] = p.planes;
    let (cw, ch) = if p.mono_chrome { (0, 0) } else { (w.div_ceil(1 << ssx), ht.div_ceil(1 << ssy)) };
    if y.len() != w * ht || (!p.mono_chrome && (u.len() != cw * ch || v.len() != cw * ch)) {
        return Err(bad(F, "AV1 picture planes do not match its size"));
    }
    if p.bit_depth > 16 || p.bit_depth < 8 {
        return Err(Error::Unsupported(F, "AV1 bit depth"));
    }
    Ok(Yuv {
        w,
        h: ht,
        cw,
        ch,
        ssx,
        ssy,
        mono: p.mono_chrome,
        bit_depth: p.bit_depth as u32,
        y,
        u,
        v,
        vui: (p.full_range, p.matrix_coefficients, p.color_primaries),
        cicp: Some((p.color_primaries as u16, p.transfer_characteristics as u16)),
        alpha: None,
    })
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    crate::isobmff_still::decode(&Codec { fmt: F, tile: decode_tile, alpha_unsupported_ok: false }, bytes, opts)
}
