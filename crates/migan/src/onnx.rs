//! A reader for the part of the ONNX format the inpainting model uses: the graph's nodes with their
//! attributes, and its initializers (weights and small constants). Protocol Buffers are decoded by
//! hand (varints and length-delimited fields); nothing is generated, nothing is linked.
//!
//! Field numbers are those of `onnx.proto` (ModelProto, GraphProto, NodeProto, AttributeProto,
//! TensorProto; Apache-2.0 / MIT licensed specification).

use std::collections::HashMap;

use crate::{Error, Result};

fn bad(what: &str) -> Error {
    Error::Model(format!("not a usable ONNX file ({what})"))
}

/// One protobuf field: its number and value.
enum Field<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
    Fixed64,
}

struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn varint(&mut self) -> Result<u64> {
        let (mut v, mut shift) = (0u64, 0u32);
        loop {
            let (&byte, rest) = self.b.split_first().ok_or_else(|| bad("truncated varint"))?;
            self.b = rest;
            v |= u64::from(byte & 0x7f).checked_shl(shift).ok_or_else(|| bad("varint too long"))?;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
            if shift > 63 {
                return Err(bad("varint too long"));
            }
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.b.len() {
            return Err(bad("truncated field"));
        }
        let (a, b) = self.b.split_at(n);
        self.b = b;
        Ok(a)
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = Result<(u32, Field<'a>)>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.b.is_empty() {
            return None;
        }
        Some((|| {
            let key = self.varint()?;
            let n = (key >> 3) as u32;
            let f = match key & 7 {
                0 => Field::Varint(self.varint()?),
                1 => {
                    self.take(8)?;
                    Field::Fixed64
                }
                2 => {
                    let len = usize::try_from(self.varint()?).map_err(|_| bad("length"))?;
                    Field::Bytes(self.take(len)?)
                }
                5 => Field::Fixed32(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| bad("fixed32"))?)),
                _ => return Err(bad("unsupported wire type")),
            };
            Ok((n, f))
        })())
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A tensor read from the file: its shape and values (every numeric type widened to `f64`; the
/// graph's integers are small, its weights are `f32`).
#[derive(Clone, Debug)]
pub struct RawTensor {
    pub dims: Vec<usize>,
    pub data: Vec<f64>,
}

fn packed_varints(b: &[u8]) -> Result<Vec<i64>> {
    let mut r = Reader { b };
    let mut out = Vec::new();
    while !r.b.is_empty() {
        out.push(r.varint()? as i64);
    }
    Ok(out)
}

fn f32s(b: &[u8]) -> impl Iterator<Item = f64> + '_ {
    b.as_chunks::<4>().0.iter().map(|c| f64::from(f32::from_le_bytes(*c)))
}

fn f64s(b: &[u8]) -> impl Iterator<Item = f64> + '_ {
    b.as_chunks::<8>().0.iter().map(|c| f64::from_le_bytes(*c))
}

fn i64s(b: &[u8]) -> impl Iterator<Item = f64> + '_ {
    b.as_chunks::<8>().0.iter().map(|c| i64::from_le_bytes(*c) as f64)
}

fn i32s(b: &[u8]) -> impl Iterator<Item = f64> + '_ {
    b.as_chunks::<4>().0.iter().map(|c| f64::from(i32::from_le_bytes(*c)))
}

fn tensor(b: &[u8]) -> Result<(String, RawTensor)> {
    let (mut dims, mut dtype, mut name) = (Vec::new(), 1u64, String::new());
    let (mut raw, mut floats, mut ints, mut doubles): (&[u8], Vec<f64>, Vec<f64>, Vec<f64>) = (&[], Vec::new(), Vec::new(), Vec::new());
    for f in (Reader { b }) {
        match f? {
            (1, Field::Varint(v)) => dims.push(usize::try_from(v).map_err(|_| bad("dim"))?),
            (1, Field::Bytes(p)) => {
                for v in packed_varints(p)? {
                    dims.push(usize::try_from(v).map_err(|_| bad("dim"))?);
                }
            }
            (2, Field::Varint(v)) => dtype = v,
            (4, Field::Bytes(p)) => floats.extend(f32s(p)),
            (4, Field::Fixed32(v)) => floats.push(f64::from(f32::from_bits(v))),
            (5 | 7, Field::Bytes(p)) => ints.extend(packed_varints(p)?.into_iter().map(|v| v as f64)),
            (5 | 7, Field::Varint(v)) => ints.push(v as i64 as f64),
            (8, Field::Bytes(p)) => name = text(p),
            (9, Field::Bytes(p)) => raw = p,
            (10, Field::Bytes(p)) => doubles.extend(f64s(p)),
            _ => {}
        }
    }
    let n: usize = dims.iter().product();
    let data: Vec<f64> = if !raw.is_empty() {
        match dtype {
            1 => f32s(raw).collect(),
            11 => f64s(raw).collect(),
            7 => i64s(raw).collect(),
            6 => i32s(raw).collect(),
            2 => raw.iter().map(|v| f64::from(*v)).collect(),
            3 => raw.iter().map(|v| f64::from(*v as i8)).collect(),
            9 => raw.iter().map(|v| f64::from(*v)).collect(),
            t => return Err(Error::Model(format!("unsupported tensor type {t}"))),
        }
    } else if !floats.is_empty() {
        floats
    } else if !doubles.is_empty() {
        doubles
    } else {
        ints
    };
    if data.len() != n {
        return Err(bad("tensor size does not match its shape"));
    }
    Ok((name, RawTensor { dims, data }))
}

/// A node attribute.
#[derive(Clone, Debug)]
pub enum Attr {
    F(f64),
    I(i64),
    S(String),
    T(RawTensor),
    Ints(Vec<i64>),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub op: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub attrs: HashMap<String, Attr>,
}

impl Node {
    pub fn int(&self, k: &str) -> Option<i64> {
        match self.attrs.get(k) {
            Some(Attr::I(v)) => Some(*v),
            _ => None,
        }
    }
    pub fn float(&self, k: &str) -> Option<f64> {
        match self.attrs.get(k) {
            Some(Attr::F(v)) => Some(*v),
            _ => None,
        }
    }
    pub fn string(&self, k: &str) -> Option<&str> {
        match self.attrs.get(k) {
            Some(Attr::S(v)) => Some(v),
            _ => None,
        }
    }
    pub fn ints(&self, k: &str) -> Option<&[i64]> {
        match self.attrs.get(k) {
            Some(Attr::Ints(v)) => Some(v),
            _ => None,
        }
    }
    pub fn tensor(&self, k: &str) -> Option<&RawTensor> {
        match self.attrs.get(k) {
            Some(Attr::T(v)) => Some(v),
            _ => None,
        }
    }
}

fn attribute(b: &[u8]) -> Result<(String, Attr)> {
    let (mut name, mut f, mut i, mut s, mut t) = (String::new(), None, None, None, None);
    let mut ints = Vec::new();
    for fld in (Reader { b }) {
        match fld? {
            (1, Field::Bytes(p)) => name = text(p),
            (2, Field::Fixed32(v)) => f = Some(f64::from(f32::from_bits(v))),
            (3, Field::Varint(v)) => i = Some(v as i64),
            (4, Field::Bytes(p)) => s = Some(text(p)),
            (5, Field::Bytes(p)) => t = Some(tensor(p)?.1),
            (8, Field::Bytes(p)) => ints.extend(packed_varints(p)?),
            (8, Field::Varint(v)) => ints.push(v as i64),
            _ => {}
        }
    }
    let a = if let Some(t) = t {
        Attr::T(t)
    } else if let Some(s) = s {
        Attr::S(s)
    } else if let Some(f) = f {
        Attr::F(f)
    } else if let Some(i) = i {
        Attr::I(i)
    } else {
        Attr::Ints(ints)
    };
    Ok((name, a))
}

fn node(b: &[u8]) -> Result<Node> {
    let mut n = Node { op: String::new(), inputs: Vec::new(), outputs: Vec::new(), attrs: HashMap::new() };
    for f in (Reader { b }) {
        match f? {
            (1, Field::Bytes(p)) => n.inputs.push(text(p)),
            (2, Field::Bytes(p)) => n.outputs.push(text(p)),
            (4, Field::Bytes(p)) => n.op = text(p),
            (5, Field::Bytes(p)) => {
                let (k, v) = attribute(p)?;
                n.attrs.insert(k, v);
            }
            _ => {}
        }
    }
    Ok(n)
}

/// The graph of a model file.
pub struct Graph {
    pub nodes: Vec<Node>,
    pub initializers: HashMap<String, RawTensor>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

fn value_name(b: &[u8]) -> Result<String> {
    for f in (Reader { b }) {
        if let (1, Field::Bytes(p)) = f? {
            return Ok(text(p));
        }
    }
    Ok(String::new())
}

pub fn parse(bytes: &[u8]) -> Result<Graph> {
    let mut graph: Option<&[u8]> = None;
    for f in (Reader { b: bytes }) {
        if let (7, Field::Bytes(p)) = f? {
            graph = Some(p);
        }
    }
    let graph = graph.ok_or_else(|| bad("no graph"))?;
    let mut g = Graph { nodes: Vec::new(), initializers: HashMap::new(), inputs: Vec::new(), outputs: Vec::new() };
    for f in (Reader { b: graph }) {
        match f? {
            (1, Field::Bytes(p)) => g.nodes.push(node(p)?),
            (5, Field::Bytes(p)) => {
                let (name, t) = tensor(p)?;
                g.initializers.insert(name, t);
            }
            (11, Field::Bytes(p)) => g.inputs.push(value_name(p)?),
            (12, Field::Bytes(p)) => g.outputs.push(value_name(p)?),
            _ => {}
        }
    }
    // inputs that are really initializers (older exporters list them) are not inputs
    g.inputs.retain(|i| !g.initializers.contains_key(i));
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_and_fields_decode() {
        let mut r = Reader { b: &[0xac, 0x02, 0x01] };
        assert_eq!(r.varint().unwrap(), 300);
        assert_eq!(r.varint().unwrap(), 1);
        assert!(r.varint().is_err());
        // field 1, length-delimited "abc"; field 2, varint 7
        let fields: Vec<_> = Reader { b: &[0x0a, 3, b'a', b'b', b'c', 0x10, 7] }.collect::<Result<Vec<_>>>().unwrap();
        assert!(matches!(fields[0], (1, Field::Bytes(b"abc"))) && matches!(fields[1], (2, Field::Varint(7))));
        // truncated and hostile input is an error, not a panic
        assert!(parse(&[0x3a, 0xff, 0xff, 0xff, 0xff, 0x0f]).is_err());
        assert!(parse(&[]).is_err());
    }
}
