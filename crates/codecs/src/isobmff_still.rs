//! ISO-BMFF still-image container shared by HEIC/HEIF ([`crate::heif`]) and AVIF ([`crate::avif`]):
//! `meta` item parsing (`pitm`, `iinf`, `iloc`, `iref`, `iprp`), `grid` assembly in the YCbCr domain,
//! auxiliary alpha, `clap` crop, then one YCbCr → RGB conversion. The codec-specific part (decoding one
//! coded item to planar YCbCr) is a [`TileFn`] supplied by the caller.
//!
//! Supported: primary item or a `grid` of tiles; `colr` (`nclx` / ICC); `irot` / `imir` (reported as an
//! EXIF orientation, not applied); `clap` (applied, only when it precedes any `irot` / `imir`); an
//! auxiliary alpha item (`auxl` + `auxC` alpha URN); EXIF and XMP items; 4:2:0, 4:2:2, 4:4:4 and
//! monochrome planes, 8..=16 bits. Not supported (errors, never panics): `iovl` overlays, derived
//! images other than `grid` / `iden`, HDR transfer functions beyond a plain sRGB / Rec.709 reading.

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::space::{NamedSpace, SourceSpace, SpaceOrigin, Trc};
use crate::{DecodeOptions, Decoded, Error, Format, Result};
use std::collections::HashMap;

pub(crate) fn bad(f: Format, m: impl Into<String>) -> Error {
    Error::Malformed(f, m.into())
}

/// Bounds-checked big-endian reader.
pub(crate) struct Rd<'a> {
    f: Format,
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    pub(crate) fn new(f: Format, b: &'a [u8]) -> Self {
        Rd { f, b, p: 0 }
    }
    fn left(&self) -> usize {
        self.b.len().saturating_sub(self.p)
    }
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).ok_or_else(|| bad(self.f, "length overflow"))?;
        let s = self.b.get(self.p..end).ok_or_else(|| bad(self.f, "truncated box"))?;
        self.p = end;
        Ok(s)
    }
    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    pub(crate) fn u32(&mut self) -> Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Result<u64> {
        let s = self.take(8)?;
        Ok(u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
    }
    /// A 0 / 4 / 8-byte unsigned field (iloc size fields).
    fn un(&mut self, n: u8) -> Result<u64> {
        match n {
            0 => Ok(0),
            4 => Ok(self.u32()? as u64),
            8 => self.u64(),
            _ => Err(bad(self.f, "unsupported iloc field size")),
        }
    }
    fn cstr(&mut self) -> Result<&'a [u8]> {
        let rest = self.b.get(self.p..).ok_or_else(|| bad(self.f, "truncated string"))?;
        let n = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
        let s = &rest[..n];
        self.p += (n + 1).min(rest.len());
        Ok(s)
    }
}

type FourCc = [u8; 4];

/// Split a run of boxes into `(type, payload)`.
fn boxes(f: Format, data: &[u8]) -> Result<Vec<(FourCc, &[u8])>> {
    let mut out = Vec::new();
    let mut r = Rd::new(f, data);
    while r.left() >= 8 {
        let size = r.u32()? as u64;
        let t = r.take(4)?;
        let kind = [t[0], t[1], t[2], t[3]];
        let body_len = match size {
            0 => r.left() as u64,
            1 => r.u64()?.checked_sub(16).ok_or_else(|| bad(f, "bad box size"))?,
            n => n.checked_sub(8).ok_or_else(|| bad(f, "bad box size"))?,
        };
        let body = r.take(usize::try_from(body_len).map_err(|_| bad(f, "box too large"))?)?;
        out.push((kind, body));
    }
    Ok(out)
}

/// FullBox header: (version, flags, payload).
fn full(f: Format, body: &[u8]) -> Result<(u8, u32, &[u8])> {
    let mut r = Rd::new(f, body);
    let v = r.u8()?;
    let fl = r.take(3)?;
    Ok((v, u32::from_be_bytes([0, fl[0], fl[1], fl[2]]), body.get(4..).unwrap_or(&[])))
}

pub(crate) struct Item {
    pub id: u32,
    pub kind: FourCc,
    content_type: Vec<u8>,
}

struct Loc {
    id: u32,
    method: u8,
    base: u64,
    extents: Vec<(u64, u64)>,
}

/// `Hvcc` / `Av1c` are each read by one codec only (feature-gated).
#[allow(dead_code)]
#[derive(Clone)]
pub(crate) enum Prop {
    Hvcc(Vec<u8>),
    Av1c(Vec<u8>),
    Ispe(u32, u32),
    Irot(u8),
    Imir(u8),
    /// `clap`: width, height, horizontal offset, vertical offset as (numerator, denominator).
    Clap([(i64, i64); 4]),
    AuxC(Vec<u8>),
    Nclx {
        primaries: u16,
        transfer: u16,
        matrix: u16,
        full_range: bool,
    },
    Icc(Vec<u8>),
    Other,
}

pub(crate) struct Still<'a> {
    pub fmt: Format,
    file: &'a [u8],
    idat: Option<&'a [u8]>,
    primary: u32,
    pub items: Vec<Item>,
    locs: Vec<Loc>,
    props: Vec<Prop>,
    assoc: HashMap<u32, Vec<usize>>,
    refs: Vec<(FourCc, u32, Vec<u32>)>,
}

fn parse(fmt: Format, file: &[u8]) -> Result<Still<'_>> {
    let top = boxes(fmt, file)?;
    let meta = match top.iter().find(|(k, _)| k == b"meta") {
        Some(m) => m.1,
        None if top.iter().any(|(k, _)| k == b"moov") => return Err(Error::Unsupported(fmt, "image sequence without a still image item")),
        None => return Err(bad(fmt, "no meta box")),
    };
    let (_, _, meta_body) = full(fmt, meta)?;
    let mut h =
        Still { fmt, file, idat: None, primary: 0, items: Vec::new(), locs: Vec::new(), props: Vec::new(), assoc: HashMap::new(), refs: Vec::new() };
    let mut have_primary = false;
    for (k, body) in boxes(fmt, meta_body)? {
        match &k {
            b"pitm" => {
                let (v, _, p) = full(fmt, body)?;
                let mut r = Rd::new(fmt, p);
                h.primary = if v == 0 { r.u16()? as u32 } else { r.u32()? };
                have_primary = true;
            }
            b"iinf" => {
                let (v, _, p) = full(fmt, body)?;
                let mut r = Rd::new(fmt, p);
                let _count = if v == 0 { r.u16()? as u32 } else { r.u32()? };
                for (ik, ib) in boxes(fmt, p.get(r.p..).unwrap_or(&[]))? {
                    if &ik != b"infe" {
                        continue;
                    }
                    let (iv, _, ip) = full(fmt, ib)?;
                    if iv < 2 {
                        continue;
                    }
                    let mut ir = Rd::new(fmt, ip);
                    let id = if iv == 2 { ir.u16()? as u32 } else { ir.u32()? };
                    let _prot = ir.u16()?;
                    let t = ir.take(4)?;
                    let kind = [t[0], t[1], t[2], t[3]];
                    let _name = ir.cstr()?;
                    let content_type = if &kind == b"mime" { ir.cstr()?.to_vec() } else { Vec::new() };
                    h.items.push(Item { id, kind, content_type });
                }
            }
            b"iloc" => {
                let (v, _, p) = full(fmt, body)?;
                let mut r = Rd::new(fmt, p);
                let a = r.u8()?;
                let b = r.u8()?;
                let (osz, lsz) = (a >> 4, a & 15);
                let (bsz, isz) = (b >> 4, if v == 1 || v == 2 { b & 15 } else { 0 });
                let count = if v < 2 { r.u16()? as u32 } else { r.u32()? };
                for _ in 0..count {
                    let id = if v < 2 { r.u16()? as u32 } else { r.u32()? };
                    let method = if v == 1 || v == 2 { (r.u16()? & 15) as u8 } else { 0 };
                    let _dref = r.u16()?;
                    let base = r.un(bsz)?;
                    let n = r.u16()?;
                    let mut extents = Vec::new();
                    for _ in 0..n {
                        if isz > 0 {
                            r.un(isz)?;
                        }
                        let off = r.un(osz)?;
                        let len = r.un(lsz)?;
                        extents.push((off, len));
                    }
                    h.locs.push(Loc { id, method, base, extents });
                }
            }
            b"idat" => h.idat = Some(body),
            b"iref" => {
                let (v, _, p) = full(fmt, body)?;
                for (rk, rb) in boxes(fmt, p)? {
                    let mut r = Rd::new(fmt, rb);
                    let from = if v == 0 { r.u16()? as u32 } else { r.u32()? };
                    let n = r.u16()?;
                    let mut to = Vec::new();
                    for _ in 0..n {
                        to.push(if v == 0 { r.u16()? as u32 } else { r.u32()? });
                    }
                    h.refs.push((rk, from, to));
                }
            }
            b"iprp" => {
                for (pk, pb) in boxes(fmt, body)? {
                    match &pk {
                        b"ipco" => {
                            for (ck, cb) in boxes(fmt, pb)? {
                                h.props.push(parse_prop(fmt, &ck, cb));
                            }
                        }
                        b"ipma" => {
                            let (v, fl, p) = full(fmt, pb)?;
                            let mut r = Rd::new(fmt, p);
                            let n = r.u32()?;
                            for _ in 0..n {
                                let id = if v < 1 { r.u16()? as u32 } else { r.u32()? };
                                let c = r.u8()?;
                                let mut list = Vec::new();
                                for _ in 0..c {
                                    let idx = if fl & 1 == 1 { (r.u16()? & 0x7fff) as usize } else { (r.u8()? & 0x7f) as usize };
                                    list.push(idx);
                                }
                                h.assoc.insert(id, list);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if !have_primary {
        return Err(bad(fmt, "no primary item (pitm)"));
    }
    Ok(h)
}

fn parse_prop(fmt: Format, kind: &FourCc, b: &[u8]) -> Prop {
    let go = || -> Result<Prop> {
        Ok(match kind {
            b"hvcC" => Prop::Hvcc(b.to_vec()),
            b"av1C" => Prop::Av1c(b.to_vec()),
            b"ispe" => {
                let (_, _, p) = full(fmt, b)?;
                let mut r = Rd::new(fmt, p);
                Prop::Ispe(r.u32()?, r.u32()?)
            }
            b"irot" => Prop::Irot(Rd::new(fmt, b).u8()? & 3),
            b"imir" => Prop::Imir(Rd::new(fmt, b).u8()? & 1),
            b"clap" => {
                let mut r = Rd::new(fmt, b);
                let mut v = [(0i64, 1i64); 4];
                for (i, slot) in v.iter_mut().enumerate() {
                    let n = r.u32()?;
                    let d = r.u32()?;
                    // The two offsets are signed numerators.
                    *slot = (if i >= 2 { n as i32 as i64 } else { n as i64 }, d as i64);
                }
                Prop::Clap(v)
            }
            b"auxC" => {
                let (_, _, p) = full(fmt, b)?;
                Prop::AuxC(Rd::new(fmt, p).cstr()?.to_vec())
            }
            b"colr" => {
                let mut r = Rd::new(fmt, b);
                let t = r.take(4)?;
                if t == b"nclx" {
                    Prop::Nclx { primaries: r.u16()?, transfer: r.u16()?, matrix: r.u16()?, full_range: r.u8()? & 0x80 != 0 }
                } else if t == b"prof" || t == b"rICC" {
                    Prop::Icc(b.get(4..).unwrap_or(&[]).to_vec())
                } else {
                    Prop::Other
                }
            }
            _ => Prop::Other,
        })
    };
    go().unwrap_or(Prop::Other)
}

impl Still<'_> {
    pub(crate) fn item(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Properties of an item in association order.
    pub(crate) fn item_props(&self, id: u32) -> Vec<&Prop> {
        self.assoc.get(&id).map(|l| l.iter().filter_map(|&i| i.checked_sub(1).and_then(|i| self.props.get(i))).collect()).unwrap_or_default()
    }

    pub(crate) fn data(&self, id: u32) -> Result<Vec<u8>> {
        let f = self.fmt;
        let loc = self.locs.iter().find(|l| l.id == id).ok_or_else(|| bad(f, format!("item {id} has no location")))?;
        let src: &[u8] = match loc.method {
            0 => self.file,
            1 => self.idat.ok_or_else(|| bad(f, "idat construction without idat box"))?,
            _ => return Err(Error::Unsupported(f, "iloc construction method")),
        };
        let mut out = Vec::new();
        for &(off, len) in &loc.extents {
            let start = loc.base.checked_add(off).ok_or_else(|| bad(f, "offset overflow"))?;
            let start = usize::try_from(start).map_err(|_| bad(f, "offset too large"))?;
            let end = if len == 0 {
                src.len()
            } else {
                start.checked_add(usize::try_from(len).map_err(|_| bad(f, "length too large"))?).ok_or_else(|| bad(f, "offset overflow"))?
            };
            out.extend_from_slice(src.get(start..end).ok_or_else(|| bad(f, "item data outside the file"))?);
            // Overlapping extents could otherwise multiply a small file into gigabytes.
            if out.len() > src.len() {
                return Err(bad(f, "item larger than the file"));
            }
        }
        Ok(out)
    }

    fn refs_from(&self, kind: &[u8; 4], from: u32) -> Vec<u32> {
        self.refs.iter().filter(|(k, f, _)| k == kind && *f == from).flat_map(|(_, _, t)| t.iter().copied()).collect()
    }

    /// The auxiliary alpha item of `id`, if any.
    fn alpha_item(&self, id: u32) -> Option<u32> {
        const URNS: [&[u8]; 2] = [b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha", b"urn:mpeg:hevc:2015:auxid:1"];
        self.refs
            .iter()
            .filter(|(k, _, to)| k == b"auxl" && to.contains(&id))
            .map(|(_, from, _)| *from)
            .find(|&a| self.item_props(a).iter().any(|p| matches!(p, Prop::AuxC(u) if URNS.iter().any(|x| u.starts_with(x)))))
    }
}

/// A decoded planar picture (or assembled grid) with 16-bit storage. 4:2:0, 4:2:2, 4:4:4 or
/// monochrome (`mono`: `u` / `v` empty).
pub(crate) struct Yuv {
    pub w: usize,
    pub h: usize,
    pub cw: usize,
    pub ch: usize,
    pub ssx: u32,
    pub ssy: u32,
    pub mono: bool,
    pub bit_depth: u32,
    pub y: Vec<u16>,
    pub u: Vec<u16>,
    pub v: Vec<u16>,
    /// Stream colour description: (full range, matrix coefficients, colour primaries).
    pub vui: (bool, u8, u8),
    /// Stream colour primaries and transfer characteristics (CICP), when the codec carries them and
    /// they may be trusted without a `colr` box.
    pub cicp: Option<(u16, u16)>,
    /// Auxiliary alpha plane of this image or grid (a monochrome `Yuv` of the same size).
    pub alpha: Option<Box<Yuv>>,
}

/// Decodes one coded item (or, for a derived item, nothing) to planar YCbCr.
pub(crate) type TileFn = fn(&Still<'_>, u32) -> Result<Yuv>;

/// What differs between the codecs sharing this container.
#[derive(Clone, Copy)]
pub(crate) struct Codec {
    pub fmt: Format,
    pub tile: TileFn,
    /// An alpha item the codec cannot decode (`Error::Unsupported`) is dropped instead of failing the image.
    pub alpha_unsupported_ok: bool,
}

fn copy_plane(src: &[u16], sstride: usize, sw: usize, sh: usize, dst: &mut [u16], dstride: usize, dx: usize, dy: usize, dw: usize, dh: usize) {
    for row in 0..sh {
        let y = dy + row;
        if y >= dh {
            break;
        }
        let n = sw.min(dw.saturating_sub(dx));
        let s = row * sstride;
        let d = y * dstride + dx;
        if let (Some(sr), Some(dr)) = (src.get(s..s + n), dst.get_mut(d..d + n)) {
            dr.copy_from_slice(sr);
        }
    }
}

fn decode_image(c: &Codec, h: &Still<'_>, id: u32, opts: &DecodeOptions, depth: u32) -> Result<Yuv> {
    let f = c.fmt;
    if depth > 2 {
        return Err(bad(f, "derived image nesting too deep"));
    }
    let item = h.item(id).ok_or_else(|| bad(f, format!("missing item {id}")))?;
    match &item.kind {
        b"grid" => {
            let d = h.data(id)?;
            let mut r = Rd::new(f, &d);
            let _ver = r.u8()?;
            let flags = r.u8()?;
            let rows = r.u8()? as usize + 1;
            let cols = r.u8()? as usize + 1;
            let (ow, oh) = if flags & 1 == 1 { (r.u32()? as usize, r.u32()? as usize) } else { (r.u16()? as usize, r.u16()? as usize) };
            check_size(f, ow as u64, oh as u64, opts)?;
            let tiles = h.refs_from(b"dimg", id);
            if tiles.len() < rows * cols {
                return Err(bad(f, "grid references fewer tiles than rows × columns"));
            }
            let mut out: Option<Yuv> = None;
            let (mut tw, mut th) = (0usize, 0usize);
            // Some writers (libheif tiled images) attach an alpha item to every tile instead of to the grid.
            let mut tile_alpha: Option<Yuv> = None;
            let mut n_alpha = 0usize;
            for (i, &tid) in tiles.iter().take(rows * cols).enumerate() {
                let t = decode_image(c, h, tid, opts, depth + 1)?;
                let o = match out.as_mut() {
                    Some(o) => {
                        if t.w != tw || t.h != th {
                            return Err(bad(f, "grid tiles differ in size"));
                        }
                        if (t.ssx, t.ssy, t.mono, t.bit_depth) != (o.ssx, o.ssy, o.mono, o.bit_depth) {
                            return Err(bad(f, "grid tiles differ in chroma format or bit depth"));
                        }
                        o
                    }
                    None => {
                        tw = t.w;
                        th = t.h;
                        if (tw & ((1 << t.ssx) - 1)) != 0 || (th & ((1 << t.ssy) - 1)) != 0 {
                            return Err(Error::Unsupported(f, "grid tiles with odd dimensions"));
                        }
                        let (cw, ch) = if t.mono { (0, 0) } else { (ow.div_ceil(1 << t.ssx), oh.div_ceil(1 << t.ssy)) };
                        out.insert(Yuv {
                            w: ow,
                            h: oh,
                            cw,
                            ch,
                            ssx: t.ssx,
                            ssy: t.ssy,
                            mono: t.mono,
                            bit_depth: t.bit_depth,
                            y: vec![0; ow * oh],
                            u: vec![0; cw * ch],
                            v: vec![0; cw * ch],
                            vui: t.vui,
                            cicp: t.cicp,
                            alpha: None,
                        })
                    }
                };
                let (c_, rr) = (i % cols, i / cols);
                let (x, y) = (c_ * tw, rr * th);
                copy_plane(&t.y, t.w, t.w, t.h, &mut o.y, ow, x, y, ow, oh);
                if !t.mono {
                    let (cx, cy) = (x >> t.ssx, y >> t.ssy);
                    copy_plane(&t.u, t.cw, t.cw, t.ch, &mut o.u, o.cw, cx, cy, o.cw, o.ch);
                    copy_plane(&t.v, t.cw, t.cw, t.ch, &mut o.v, o.cw, cx, cy, o.cw, o.ch);
                }
                if let Some(ta) = t.alpha.as_deref() {
                    if ta.w != t.w || ta.h != t.h {
                        return Err(bad(f, "tile alpha differs in size from its tile"));
                    }
                    let ao = tile_alpha.get_or_insert_with(|| Yuv {
                        w: ow,
                        h: oh,
                        cw: 0,
                        ch: 0,
                        ssx: 0,
                        ssy: 0,
                        mono: true,
                        bit_depth: ta.bit_depth,
                        y: vec![0; ow * oh],
                        u: Vec::new(),
                        v: Vec::new(),
                        vui: ta.vui,
                        cicp: None,
                        alpha: None,
                    });
                    if ao.bit_depth != ta.bit_depth {
                        return Err(bad(f, "grid tile alpha planes differ in bit depth"));
                    }
                    copy_plane(&ta.y, ta.w, ta.w, ta.h, &mut ao.y, ow, x, y, ow, oh);
                    n_alpha += 1;
                }
            }
            let mut out = out.ok_or_else(|| bad(f, "empty grid"))?;
            if n_alpha == rows * cols {
                out.alpha = tile_alpha.map(Box::new);
            } else if n_alpha != 0 {
                return Err(bad(f, "only some grid tiles have an alpha plane"));
            }
            if out.alpha.is_none() {
                out.alpha = decode_alpha(c, h, id, opts, depth)?;
            }
            Ok(out)
        }
        b"iden" => {
            let from = h.refs_from(b"dimg", id);
            let first = from.first().ok_or_else(|| bad(f, "iden without source"))?;
            decode_image(c, h, *first, opts, depth + 1)
        }
        _ => {
            let mut y = (c.tile)(h, id)?;
            y.alpha = decode_alpha(c, h, id, opts, depth)?;
            Ok(y)
        }
    }
}

/// The auxiliary alpha plane of item `id`, if it has one (and the codec can decode it).
fn decode_alpha(c: &Codec, h: &Still<'_>, id: u32, opts: &DecodeOptions, depth: u32) -> Result<Option<Box<Yuv>>> {
    let Some(aid) = h.alpha_item(id) else { return Ok(None) };
    match decode_image(c, h, aid, opts, depth + 1) {
        Ok(a) => Ok(Some(Box::new(a))),
        Err(Error::Unsupported(..)) if c.alpha_unsupported_ok => Ok(None),
        Err(e) => Err(e),
    }
}

/// Orientation (EXIF value) from the primary item's `irot` / `imir`, composed in association order.
fn orientation(props: &[&Prop]) -> u16 {
    // Orientation as a 2×2 matrix on (x right, y down): displayed = M · stored.
    let mut m = [[1i32, 0], [0, 1]];
    let mul = |a: [[i32; 2]; 2], b: [[i32; 2]; 2]| {
        [
            [a[0][0] * b[0][0] + a[0][1] * b[1][0], a[0][0] * b[0][1] + a[0][1] * b[1][1]],
            [a[1][0] * b[0][0] + a[1][1] * b[1][0], a[1][0] * b[0][1] + a[1][1] * b[1][1]],
        ]
    };
    for p in props {
        match p {
            // irot: anticlockwise quarter turns; one turn maps (x, y) → (y, −x).
            Prop::Irot(a) => {
                for _ in 0..*a {
                    m = mul([[0, 1], [-1, 0]], m);
                }
            }
            // imir axis 0: mirror about the vertical axis (x → −x); axis 1: horizontal axis (y → −y).
            Prop::Imir(0) => m = mul([[-1, 0], [0, 1]], m),
            Prop::Imir(_) => m = mul([[1, 0], [0, -1]], m),
            _ => {}
        }
    }
    match m {
        [[1, 0], [0, 1]] => 1,
        [[-1, 0], [0, 1]] => 2,
        [[-1, 0], [0, -1]] => 3,
        [[1, 0], [0, -1]] => 4,
        [[0, 1], [1, 0]] => 5,
        [[0, -1], [1, 0]] => 6,
        [[0, -1], [-1, 0]] => 7,
        [[0, 1], [-1, 0]] => 8,
        _ => 1,
    }
}

fn container_hint(primaries: u16, transfer: u16) -> Option<SourceSpace> {
    let named = match primaries {
        1 => NamedSpace::Srgb,
        9 => NamedSpace::Rec2020,
        12 => NamedSpace::DisplayP3,
        _ => return None,
    };
    let trc = match transfer {
        13 => Trc::Srgb,
        1 | 6 | 14 | 15 => Trc::Rec709,
        4 => Trc::Gamma(2.2),
        _ => return None,
    };
    let mut s = SourceSpace::named(named, SpaceOrigin::Container);
    s.trc = Some([trc.clone(), trc.clone(), trc]);
    Some(s)
}

/// Pixel rectangle `(x, y, w, h)` of a `clap` (clean aperture) on a `w0 × h0` image (ISO/IEC 14496-12
/// 12.1.4: the aperture centre is the image centre plus the offsets).
fn clap_rect(f: Format, c: &[(i64, i64); 4], w0: usize, h0: usize) -> Result<(usize, usize, usize, usize)> {
    if c.iter().any(|&(_, d)| d <= 0) {
        return Err(bad(f, "clap with a zero denominator"));
    }
    let q = |i: usize| c[i].0 as f64 / c[i].1 as f64;
    let (cw, ch) = (q(0), q(1));
    let left = q(2) + (w0 as f64 - 1.0) / 2.0 - (cw - 1.0) / 2.0;
    let top = q(3) + (h0 as f64 - 1.0) / 2.0 - (ch - 1.0) / 2.0;
    let (x, y, w, h) = (left.round(), top.round(), cw.round(), ch.round());
    if x < 0.0 || y < 0.0 || w < 1.0 || h < 1.0 || x + w > w0 as f64 || y + h > h0 as f64 {
        return Err(bad(f, "clap outside the image"));
    }
    Ok((x as usize, y as usize, w as usize, h as usize))
}

fn crop_raw(r: Raw, x: usize, y: usize, w: usize, h: usize) -> Raw {
    let ch = r.model.channels() + r.alpha as usize;
    fn go<T: Copy>(src: &[T], sw: usize, ch: usize, x: usize, y: usize, w: usize, h: usize) -> Vec<T> {
        let mut out = Vec::with_capacity(w * h * ch);
        for row in y..y + h {
            let s = (row * sw + x) * ch;
            out.extend_from_slice(src.get(s..s + w * ch).unwrap_or(&[]));
        }
        out
    }
    let buf = match &r.buf {
        Buf::U8(v) => Buf::U8(go(v, r.width, ch, x, y, w, h)),
        Buf::U16(v) => Buf::U16(go(v, r.width, ch, x, y, w, h)),
        Buf::F32(v) => Buf::F32(go(v, r.width, ch, x, y, w, h)),
    };
    Raw { width: w, height: h, buf, ..r }
}

/// YCbCr (bilinear chroma upsampling, chroma centred between luma samples) → interleaved RGB (or
/// gray for monochrome), with the straight / premultiplied alpha plane of `alpha` as the last channel.
fn to_rgb(y: &Yuv, matrix: u16, full_range: bool, alpha: Option<&Yuv>, premultiplied: bool) -> Raw {
    let (kr, kb) = match matrix {
        1 => (0.2126f32, 0.0722f32),
        9 | 10 => (0.2627, 0.0593),
        4 => (0.30, 0.11),
        7 => (0.212, 0.087),
        _ => (0.299, 0.114), // 5, 6 and unspecified
    };
    // Matrix 0 (identity, GBR) on 4:4:4: the planes are G, B, R directly.
    let identity = matrix == 0 && y.ssx == 0 && y.ssy == 0 && !y.mono;
    let kg = 1.0 - kr - kb;
    let range = |bd: u32, full: bool| -> (f32, f32, f32, f32) {
        let maxv = ((1u32 << bd) - 1) as f32;
        let sh = bd.saturating_sub(8);
        if full {
            (0.0, maxv, (1u32 << (bd - 1)) as f32, maxv)
        } else {
            ((16u32 << sh) as f32, (219u32 << sh) as f32, (128u32 << sh) as f32, (224u32 << sh) as f32)
        }
    };
    let (y_off, y_scale, c_off, c_scale) = range(y.bit_depth, full_range);
    let (w, h) = (y.w, y.h);
    let wide = y.bit_depth > 8;
    let nc = if y.mono { 1 } else { 3 };
    let ch = nc + alpha.is_some() as usize;
    let mut u8buf = if wide { Vec::new() } else { vec![0u8; w * h * ch] };
    let mut u16buf = if wide { vec![0u16; w * h * ch] } else { Vec::new() };
    let (sx, sy) = ((1u32 << y.ssx) as f32, (1u32 << y.ssy) as f32);
    // Chroma sample at luma position x: centre of chroma sample i is at luma s·i + (s − 1)/2.
    let cs = |p: &[u16], cx: f32, cy: f32| -> f32 {
        let fx = (cx - 0.5).max(0.0);
        let fy = (cy - 0.5).max(0.0);
        let x0 = (fx as usize).min(y.cw - 1);
        let y0 = (fy as usize).min(y.ch - 1);
        let x1 = (x0 + 1).min(y.cw - 1);
        let y1 = (y0 + 1).min(y.ch - 1);
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let g = |x: usize, yy: usize| p.get(yy * y.cw + x).copied().unwrap_or(0) as f32;
        let a = g(x0, y0) * (1.0 - tx) + g(x1, y0) * tx;
        let b = g(x0, y1) * (1.0 - tx) + g(x1, y1) * tx;
        a * (1.0 - ty) + b * ty
    };
    let a_range = alpha.map(|a| range(a.bit_depth, a.vui.0));
    for row in 0..h {
        for col in 0..w {
            let lum = (y.y.get(row * w + col).copied().unwrap_or(0) as f32 - y_off) / y_scale;
            let mut px = [0f32; 4];
            if y.mono {
                px[0] = lum;
            } else {
                let (cx, cy) = ((col as f32 + 0.5) / sx, (row as f32 + 0.5) / sy);
                let cb = (cs(&y.u, cx, cy) - c_off) / c_scale;
                let cr = (cs(&y.v, cx, cy) - c_off) / c_scale;
                if identity {
                    // GBR: Y = G, Cb(U) = B, Cr(V) = R, all full-scale samples.
                    let n = |s: f32| (s - y_off) / y_scale;
                    px[0] = n(cs(&y.v, cx, cy));
                    px[1] = lum;
                    px[2] = n(cs(&y.u, cx, cy));
                } else {
                    let r = lum + 2.0 * (1.0 - kr) * cr;
                    let b = lum + 2.0 * (1.0 - kb) * cb;
                    px[0] = r;
                    px[1] = (lum - kr * r - kb * b) / kg;
                    px[2] = b;
                }
            }
            if let (Some(a), Some((a_off, a_scale, _, _))) = (alpha, a_range) {
                px[nc] = (a.y.get(row * a.w + col).copied().unwrap_or(0) as f32 - a_off) / a_scale;
            }
            let i = (row * w + col) * ch;
            for (k, v) in px.into_iter().take(ch).enumerate() {
                let v = v.clamp(0.0, 1.0);
                if wide {
                    u16buf[i + k] = (v * 65535.0 + 0.5) as u16;
                } else {
                    u8buf[i + k] = (v * 255.0 + 0.5) as u8;
                }
            }
        }
    }
    Raw {
        width: w,
        height: h,
        model: if y.mono { Model::Gray } else { Model::Rgb },
        alpha: alpha.is_some(),
        premultiplied: alpha.is_some() && premultiplied,
        buf: if wide { Buf::U16(u16buf) } else { Buf::U8(u8buf) },
        bit_depth: if wide { y.bit_depth as u8 } else { 8 },
    }
}

/// Decode the primary image of an ISO-BMFF still-image file.
pub(crate) fn decode(c: &Codec, bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let f = c.fmt;
    let h = parse(f, bytes)?;
    let pid = h.primary;
    let pprops = h.item_props(pid);
    // Reject absurd sizes before decoding anything.
    if let Some(Prop::Ispe(w, ht)) = pprops.iter().find(|p| matches!(p, Prop::Ispe(..))) {
        check_size(f, *w as u64, *ht as u64, opts)?;
    }
    let img = decode_image(c, &h, pid, opts, 0)?;
    check_size(f, img.w as u64, img.h as u64, opts)?;

    // Auxiliary alpha: same size as the colour image.
    let alpha_img = match img.alpha.as_deref() {
        Some(a) if a.w == img.w && a.h == img.h => Some(a),
        Some(_) => return Err(Error::Unsupported(f, "alpha plane of a different size than the image")),
        None => None,
    };
    let premultiplied = !h.refs_from(b"prem", pid).is_empty();

    let nclx = pprops.iter().find_map(|p| {
        if let Prop::Nclx { primaries, transfer, matrix, full_range } = p { Some((*primaries, *transfer, *matrix, *full_range)) } else { None }
    });
    let icc = pprops.iter().find_map(|p| if let Prop::Icc(c) = p { Some(c.clone()) } else { None });
    let (full_range, matrix, hint) = match nclx {
        Some((pr, tr, m, fr)) => (fr, m, container_hint(pr, tr)),
        // No colr: fall back to the stream's own description, else full-range BT.601 as phone cameras write.
        None => (
            img.vui.0,
            if (img.vui.1 == 0 && (img.ssx, img.ssy) != (0, 0)) || img.vui.1 == 2 { 6 } else { img.vui.1 as u16 },
            img.cicp.and_then(|(p, t)| container_hint(p, t)),
        ),
    };
    let mut raw = to_rgb(&img, matrix, full_range, alpha_img, premultiplied);
    let (mut w, mut ht) = (img.w as u32, img.h as u32);

    // clap (possibly several, in association order): crop in the stored orientation; after an
    // irot / imir it would apply to the rotated image.
    let mut rotated = false;
    for p in &pprops {
        match p {
            Prop::Irot(_) | Prop::Imir(_) => rotated = true,
            Prop::Clap(cl) => {
                if rotated {
                    return Err(Error::Unsupported(f, "clap applied after irot / imir"));
                }
                let (x, y, cw, ch) = clap_rect(f, cl, raw.width, raw.height)?;
                raw = crop_raw(raw, x, y, cw, ch);
                (w, ht) = (cw as u32, ch as u32);
            }
            _ => {}
        }
    }

    let exif =
        h.items.iter().filter(|i| &i.kind == b"Exif").find(|i| h.refs_from(b"cdsc", i.id).contains(&pid)).and_then(|i| h.data(i.id).ok()).and_then(
            |d| {
                let off = u32::from_be_bytes([*d.first()?, *d.get(1)?, *d.get(2)?, *d.get(3)?]) as usize;
                let t = d.get(4usize.checked_add(off)?..)?;
                (t.starts_with(b"II") || t.starts_with(b"MM")).then(|| t.to_vec())
            },
        );
    let xmp = h
        .items
        .iter()
        .filter(|i| &i.kind == b"mime" && i.content_type.starts_with(b"application/rdf+xml"))
        .find(|i| h.refs_from(b"cdsc", i.id).contains(&pid))
        .and_then(|i| h.data(i.id).ok())
        .map(|d| String::from_utf8_lossy(&d).trim_end_matches('\0').to_string());

    let meta = Meta { icc, exif, xmp, orientation: Some(orientation(&pprops)).filter(|o| *o != 1), hint, ..Default::default() };
    finish(f, raw, meta, (w, ht), opts)
}
