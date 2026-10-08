//! HEIC / HEIF still images: ISO-BMFF item parsing (`meta`: `pitm`, `iinf`, `iloc`, `iref`, `iprp`),
//! HEVC intra tiles decoded by the clean-room pure-Rust `filmcraft-hevc` (ITU-T H.265, Main / Main 10,
//! 4:2:0), `grid` assembly in the YCbCr domain, then one YCbCr → RGB conversion.
//!
//! Supported: primary item `hvc1` or a `grid` of `hvc1` tiles; `colr` (`nclx` / ICC), `irot` / `imir`
//! (reported as an EXIF orientation, not applied), EXIF and XMP items. Not supported (reported as
//! errors, never panics): `iovl` overlays, 4:2:2 / 4:4:4 / monochrome, auxiliary alpha / depth, HDR
//! transfer functions beyond a plain sRGB / Rec.709 reading.

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::space::{NamedSpace, SourceSpace, SpaceOrigin, Trc};
use crate::{DecodeOptions, Decoded, Error, Format, Result};
use filmcraft_hevc::{Decoder, Plane};
use std::collections::HashMap;

const F: Format = Format::Heif;

fn bad(m: impl Into<String>) -> Error {
    Error::Malformed(F, m.into())
}

/// Bounds-checked big-endian reader.
struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8]) -> Self {
        Rd { b, p: 0 }
    }
    fn left(&self) -> usize {
        self.b.len().saturating_sub(self.p)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).ok_or_else(|| bad("length overflow"))?;
        let s = self.b.get(self.p..end).ok_or_else(|| bad("truncated box"))?;
        self.p = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32> {
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
            _ => Err(bad("unsupported iloc field size")),
        }
    }
    fn cstr(&mut self) -> Result<&'a [u8]> {
        let rest = self.b.get(self.p..).ok_or_else(|| bad("truncated string"))?;
        let n = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
        let s = &rest[..n];
        self.p += (n + 1).min(rest.len());
        Ok(s)
    }
}

type FourCc = [u8; 4];

/// Split a run of boxes into `(type, payload)`.
fn boxes(data: &[u8]) -> Result<Vec<(FourCc, &[u8])>> {
    let mut out = Vec::new();
    let mut r = Rd::new(data);
    while r.left() >= 8 {
        let size = r.u32()? as u64;
        let t = r.take(4)?;
        let kind = [t[0], t[1], t[2], t[3]];
        let (body_len, hdr) = match size {
            0 => (r.left() as u64, 8u64),
            1 => (r.u64()?.checked_sub(16).ok_or_else(|| bad("bad box size"))?, 16),
            n => (n.checked_sub(8).ok_or_else(|| bad("bad box size"))?, 8),
        };
        let _ = hdr;
        let body = r.take(usize::try_from(body_len).map_err(|_| bad("box too large"))?)?;
        out.push((kind, body));
    }
    Ok(out)
}

/// FullBox header: (version, flags, payload).
fn full(body: &[u8]) -> Result<(u8, u32, &[u8])> {
    let mut r = Rd::new(body);
    let v = r.u8()?;
    let f = r.take(3)?;
    Ok((v, u32::from_be_bytes([0, f[0], f[1], f[2]]), body.get(4..).unwrap_or(&[])))
}

struct Item {
    id: u32,
    kind: FourCc,
    content_type: Vec<u8>,
}

struct Loc {
    id: u32,
    method: u8,
    base: u64,
    extents: Vec<(u64, u64)>,
}

#[derive(Clone)]
enum Prop {
    Hvcc(Vec<u8>),
    Ispe(u32, u32),
    Irot(u8),
    Imir(u8),
    Nclx { primaries: u16, transfer: u16, matrix: u16, full_range: bool },
    Icc(Vec<u8>),
    Other,
}

struct Heif<'a> {
    file: &'a [u8],
    idat: Option<&'a [u8]>,
    primary: u32,
    items: Vec<Item>,
    locs: Vec<Loc>,
    props: Vec<Prop>,
    assoc: HashMap<u32, Vec<usize>>,
    refs: Vec<(FourCc, u32, Vec<u32>)>,
}

fn parse(file: &[u8]) -> Result<Heif<'_>> {
    let top = boxes(file)?;
    let meta = top.iter().find(|(k, _)| k == b"meta").ok_or_else(|| bad("no meta box"))?.1;
    let (_, _, meta_body) = full(meta)?;
    let mut h = Heif { file, idat: None, primary: 0, items: Vec::new(), locs: Vec::new(), props: Vec::new(), assoc: HashMap::new(), refs: Vec::new() };
    let mut have_primary = false;
    for (k, body) in boxes(meta_body)? {
        match &k {
            b"pitm" => {
                let (v, _, p) = full(body)?;
                let mut r = Rd::new(p);
                h.primary = if v == 0 { r.u16()? as u32 } else { r.u32()? };
                have_primary = true;
            }
            b"iinf" => {
                let (v, _, p) = full(body)?;
                let mut r = Rd::new(p);
                let _count = if v == 0 { r.u16()? as u32 } else { r.u32()? };
                for (ik, ib) in boxes(p.get(r.p..).unwrap_or(&[]))? {
                    if &ik != b"infe" {
                        continue;
                    }
                    let (iv, _, ip) = full(ib)?;
                    if iv < 2 {
                        continue;
                    }
                    let mut ir = Rd::new(ip);
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
                let (v, _, p) = full(body)?;
                let mut r = Rd::new(p);
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
                let (v, _, p) = full(body)?;
                for (rk, rb) in boxes(p)? {
                    let mut r = Rd::new(rb);
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
                for (pk, pb) in boxes(body)? {
                    match &pk {
                        b"ipco" => {
                            for (ck, cb) in boxes(pb)? {
                                h.props.push(parse_prop(&ck, cb));
                            }
                        }
                        b"ipma" => {
                            let (v, fl, p) = full(pb)?;
                            let mut r = Rd::new(p);
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
        return Err(bad("no primary item (pitm)"));
    }
    Ok(h)
}

fn parse_prop(kind: &FourCc, b: &[u8]) -> Prop {
    let go = || -> Result<Prop> {
        Ok(match kind {
            b"hvcC" => Prop::Hvcc(b.to_vec()),
            b"ispe" => {
                let (_, _, p) = full(b)?;
                let mut r = Rd::new(p);
                Prop::Ispe(r.u32()?, r.u32()?)
            }
            b"irot" => Prop::Irot(Rd::new(b).u8()? & 3),
            b"imir" => Prop::Imir(Rd::new(b).u8()? & 1),
            b"colr" => {
                let mut r = Rd::new(b);
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

impl Heif<'_> {
    fn item(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Properties of an item in association order.
    fn item_props(&self, id: u32) -> Vec<&Prop> {
        self.assoc.get(&id).map(|l| l.iter().filter_map(|&i| i.checked_sub(1).and_then(|i| self.props.get(i))).collect()).unwrap_or_default()
    }

    fn data(&self, id: u32) -> Result<Vec<u8>> {
        let loc = self.locs.iter().find(|l| l.id == id).ok_or_else(|| bad(format!("item {id} has no location")))?;
        let src: &[u8] = match loc.method {
            0 => self.file,
            1 => self.idat.ok_or_else(|| bad("idat construction without idat box"))?,
            _ => return Err(Error::Unsupported(F, "iloc construction method")),
        };
        let mut out = Vec::new();
        for &(off, len) in &loc.extents {
            let start = loc.base.checked_add(off).ok_or_else(|| bad("offset overflow"))?;
            let start = usize::try_from(start).map_err(|_| bad("offset too large"))?;
            let end = if len == 0 { src.len() } else { start.checked_add(usize::try_from(len).map_err(|_| bad("length too large"))?).ok_or_else(|| bad("offset overflow"))? };
            out.extend_from_slice(src.get(start..end).ok_or_else(|| bad("item data outside the file"))?);
        }
        Ok(out)
    }

    fn refs_from(&self, kind: &[u8; 4], from: u32) -> Vec<u32> {
        self.refs.iter().filter(|(k, f, _)| k == kind && *f == from).flat_map(|(_, _, t)| t.iter().copied()).collect()
    }
}

/// A decoded 4:2:0 picture (or assembled grid) with 16-bit storage.
struct Yuv {
    w: usize,
    h: usize,
    cw: usize,
    ch: usize,
    bit_depth: u32,
    y: Vec<u16>,
    u: Vec<u16>,
    v: Vec<u16>,
    vui: (bool, u8, u8),
}

fn plane16(p: &Plane) -> Vec<u16> {
    match p {
        Plane::U8(v) => v.iter().map(|&x| x as u16).collect(),
        Plane::U16(v) => v.clone(),
    }
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

fn decode_tile(h: &Heif<'_>, id: u32) -> Result<Yuv> {
    let item = h.item(id).ok_or_else(|| bad(format!("missing item {id}")))?;
    if &item.kind != b"hvc1" && &item.kind != b"hev1" {
        return Err(Error::Unsupported(F, "tile is not an HEVC image"));
    }
    let hvcc = h.item_props(id).into_iter().find_map(|p| if let Prop::Hvcc(c) = p { Some(c.clone()) } else { None }).ok_or_else(|| bad("tile has no hvcC"))?;
    let data = h.data(id)?;
    let mut dec = Decoder::from_hvcc(&hvcc).map_err(|e| bad(format!("hvcC: {e}")))?;
    let mut pics = dec.decode(&data, 0).map_err(|e| bad(format!("HEVC: {e}")))?;
    pics.extend(dec.flush());
    if let Some(e) = dec.take_error() {
        return Err(bad(format!("HEVC: {e}")));
    }
    let p = pics.into_iter().next().ok_or_else(|| bad("HEVC stream produced no picture"))?;
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
        bit_depth: p.bit_depth,
        y: tight(&y16, p.y_stride, w, ht),
        u: tight(&u16v, p.uv_stride, cw, ch),
        v: tight(&v16, p.uv_stride, cw, ch),
        vui: (p.color.full_range, p.color.matrix, p.color.primaries),
    })
}

fn decode_image(h: &Heif<'_>, id: u32, opts: &DecodeOptions, depth: u32) -> Result<Yuv> {
    if depth > 2 {
        return Err(bad("derived image nesting too deep"));
    }
    let item = h.item(id).ok_or_else(|| bad(format!("missing item {id}")))?;
    match &item.kind {
        b"hvc1" | b"hev1" => decode_tile(h, id),
        b"grid" => {
            let d = h.data(id)?;
            let mut r = Rd::new(&d);
            let _ver = r.u8()?;
            let flags = r.u8()?;
            let rows = r.u8()? as usize + 1;
            let cols = r.u8()? as usize + 1;
            let (ow, oh) = if flags & 1 == 1 { (r.u32()? as usize, r.u32()? as usize) } else { (r.u16()? as usize, r.u16()? as usize) };
            check_size(F, ow as u64, oh as u64, opts)?;
            let tiles = h.refs_from(b"dimg", id);
            if tiles.len() < rows * cols {
                return Err(bad("grid references fewer tiles than rows × columns"));
            }
            let (cw, ch) = (ow.div_ceil(2), oh.div_ceil(2));
            let mut out = Yuv { w: ow, h: oh, cw, ch, bit_depth: 8, y: vec![0; ow * oh], u: vec![0; cw * ch], v: vec![0; cw * ch], vui: (true, 6, 1) };
            let (mut tw, mut th) = (0usize, 0usize);
            for (i, &tid) in tiles.iter().take(rows * cols).enumerate() {
                let t = decode_tile(h, tid)?;
                if i == 0 {
                    tw = t.w;
                    th = t.h;
                    out.bit_depth = t.bit_depth;
                    out.vui = t.vui;
                    if tw % 2 != 0 || th % 2 != 0 {
                        return Err(Error::Unsupported(F, "grid tiles with odd dimensions"));
                    }
                } else if t.w != tw || t.h != th {
                    return Err(bad("grid tiles differ in size"));
                }
                let (c, rr) = (i % cols, i / cols);
                let (x, y) = (c * tw, rr * th);
                copy_plane(&t.y, t.w, t.w, t.h, &mut out.y, ow, x, y, ow, oh);
                copy_plane(&t.u, t.cw, t.cw, t.ch, &mut out.u, cw, x / 2, y / 2, cw, ch);
                copy_plane(&t.v, t.cw, t.cw, t.ch, &mut out.v, cw, x / 2, y / 2, cw, ch);
            }
            Ok(out)
        }
        b"iden" => {
            let from = h.refs_from(b"dimg", id);
            let first = from.first().ok_or_else(|| bad("iden without source"))?;
            decode_image(h, *first, opts, depth + 1)
        }
        _ => Err(Error::Unsupported(F, "primary item is not an HEVC image or grid (overlay / AV1 / other)")),
    }
}

/// Orientation (EXIF value) from the primary item's `irot` / `imir`, composed in association order.
fn orientation(props: &[&Prop]) -> u16 {
    // Orientation as a 2×2 matrix on (x right, y down): displayed = M · stored.
    let mut m = [[1i32, 0], [0, 1]];
    let mul = |a: [[i32; 2]; 2], b: [[i32; 2]; 2]| [[a[0][0] * b[0][0] + a[0][1] * b[1][0], a[0][0] * b[0][1] + a[0][1] * b[1][1]], [a[1][0] * b[0][0] + a[1][1] * b[1][0], a[1][0] * b[0][1] + a[1][1] * b[1][1]]];
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

/// YCbCr (bilinear 4:2:0 upsampling, chroma centred between luma rows / columns) → interleaved RGB.
fn to_rgb(y: &Yuv, matrix: u16, full_range: bool) -> Raw {
    let (kr, kb) = match matrix {
        1 => (0.2126f32, 0.0722f32),
        9 | 10 => (0.2627, 0.0593),
        4 => (0.30, 0.11),
        7 => (0.212, 0.087),
        _ => (0.299, 0.114), // 5, 6 and unspecified
    };
    let kg = 1.0 - kr - kb;
    let maxv = ((1u32 << y.bit_depth) - 1) as f32;
    let sh = y.bit_depth.saturating_sub(8);
    let (y_off, y_scale, c_off, c_scale) = if full_range { (0.0, maxv, (1u32 << (y.bit_depth - 1)) as f32, maxv) } else { ((16u32 << sh) as f32, (219u32 << sh) as f32, (128u32 << sh) as f32, (224u32 << sh) as f32) };
    let (w, h) = (y.w, y.h);
    let wide = y.bit_depth > 8;
    let mut u8buf = if wide { Vec::new() } else { vec![0u8; w * h * 3] };
    let mut u16buf = if wide { vec![0u16; w * h * 3] } else { Vec::new() };
    // Chroma sample at luma position x: centre of chroma sample i is at luma 2i + 0.5.
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
    for row in 0..h {
        for col in 0..w {
            let lum = (y.y.get(row * w + col).copied().unwrap_or(0) as f32 - y_off) / y_scale;
            let (cx, cy) = ((col as f32 + 0.5) / 2.0, (row as f32 + 0.5) / 2.0);
            let cb = (cs(&y.u, cx, cy) - c_off) / c_scale;
            let cr = (cs(&y.v, cx, cy) - c_off) / c_scale;
            let r = lum + 2.0 * (1.0 - kr) * cr;
            let b = lum + 2.0 * (1.0 - kb) * cb;
            let g = (lum - kr * r - kb * b) / kg;
            let i = (row * w + col) * 3;
            for (k, v) in [r, g, b].into_iter().enumerate() {
                let v = v.clamp(0.0, 1.0);
                if wide {
                    u16buf[i + k] = (v * 65535.0 + 0.5) as u16;
                } else {
                    u8buf[i + k] = (v * 255.0 + 0.5) as u8;
                }
            }
        }
    }
    Raw { width: w, height: h, model: Model::Rgb, alpha: false, premultiplied: false, buf: if wide { Buf::U16(u16buf) } else { Buf::U8(u8buf) }, bit_depth: if wide { y.bit_depth as u8 } else { 8 } }
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let h = parse(bytes)?;
    let pid = h.primary;
    let pprops = h.item_props(pid);
    // Reject absurd sizes before decoding anything.
    if let Some(Prop::Ispe(w, ht)) = pprops.iter().find(|p| matches!(p, Prop::Ispe(..))) {
        check_size(F, *w as u64, *ht as u64, opts)?;
    }
    let img = decode_image(&h, pid, opts, 0)?;
    check_size(F, img.w as u64, img.h as u64, opts)?;

    let nclx = pprops.iter().find_map(|p| if let Prop::Nclx { primaries, transfer, matrix, full_range } = p { Some((*primaries, *transfer, *matrix, *full_range)) } else { None });
    let icc = pprops.iter().find_map(|p| if let Prop::Icc(c) = p { Some(c.clone()) } else { None });
    let (full_range, matrix, hint) = match nclx {
        Some((pr, tr, m, fr)) => (fr, m, container_hint(pr, tr)),
        // No colr: fall back to the stream's VUI, else full-range BT.601 as phone cameras write.
        None => (img.vui.0, if img.vui.1 == 0 || img.vui.1 == 2 { 6 } else { img.vui.1 as u16 }, None),
    };
    let raw = to_rgb(&img, matrix, full_range);
    let (w, ht) = (img.w as u32, img.h as u32);

    let exif = h
        .items
        .iter()
        .filter(|i| &i.kind == b"Exif")
        .find(|i| h.refs_from(b"cdsc", i.id).contains(&pid))
        .and_then(|i| h.data(i.id).ok())
        .and_then(|d| {
            let off = u32::from_be_bytes([*d.first()?, *d.get(1)?, *d.get(2)?, *d.get(3)?]) as usize;
            let t = d.get(4usize.checked_add(off)?..)?;
            (t.starts_with(b"II") || t.starts_with(b"MM")).then(|| t.to_vec())
        });
    let xmp = h
        .items
        .iter()
        .filter(|i| &i.kind == b"mime" && i.content_type.starts_with(b"application/rdf+xml"))
        .find(|i| h.refs_from(b"cdsc", i.id).contains(&pid))
        .and_then(|i| h.data(i.id).ok())
        .map(|d| String::from_utf8_lossy(&d).trim_end_matches('\0').to_string());

    let meta = Meta { icc, exif, xmp, orientation: Some(orientation(&pprops)).filter(|o| *o != 1), hint, ..Default::default() };
    finish(F, raw, meta, (w, ht), opts)
}
