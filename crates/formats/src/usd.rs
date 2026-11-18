//! USD in its text form (`.usda`): a reader and a writer for the part of the scene description
//! that carries models: the prim hierarchy (`Xform`, `Scope`, untyped prims), `Mesh` geometry
//! (`points`, `faceVertexCounts`, `faceVertexIndices`, per-corner or per-vertex `primvars:st`),
//! transform operations (`xformOp:transform`, `translate`, `scale`, `rotateX/Y/Z` and
//! `rotateXYZ` in every order, `orient`, with `xformOpOrder` and `!invert!`), materials
//! (`UsdPreviewSurface` colour, metalness and roughness, and `primvars:displayColor`) and the
//! stage's `upAxis` and `metersPerUnit`.
//!
//! Written from the public USD documentation: the ASCII (`usda`) file format and its grammar, the
//! `UsdGeom` schemas (`Xform`, `Mesh`, `xformOp`), `UsdShade` and `UsdPreviewSurface`. It reads
//! one layer. Layer composition (references, payloads, sublayers, variants), time samples beyond
//! the first, instancing, USD's other geometry types, skeletons, lights and cameras are left out
//! and each is reported as a warning. Binary crate files (`.usdc`) and `.usdz` packages are not
//! read: they are named in the error so the user can save a `.usda` from the other application.
//!
//! The reader is bounded: the input size, the number of values read, the prim count and depth and
//! every face and index are checked, so a hostile file is an error, not an allocation.

use std::collections::HashMap;
use std::fmt::Write as _;

use craft_mesh as cm;
use glam::{DMat4, DQuat, DVec3, Mat4};

use crate::exchange::Item;
use crate::{FormatError, Result};

/// Most numbers one file may hold in arrays and tuples, in all.
pub const MAX_NUMBERS: usize = 64_000_000;
/// Most prims one file may hold.
pub const MAX_PRIMS: usize = 100_000;
/// Deepest prim nesting, and deepest value nesting.
pub const MAX_DEPTH: usize = 64;
/// Longest string, identifier or path the reader keeps.
const MAX_TEXT: usize = 4096;

fn bad<T>(m: impl Into<String>) -> Result<T> {
    Err(FormatError::Malformed(format!("USD: {}", m.into())))
}

/// What the stage says about its axes and units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stage {
    pub z_up: bool,
    pub metres_per_unit: Option<f64>,
}

// ---------------------------------------------------------------------------------- values

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Num(f64),
    Text(String),
    /// An identifier such as `None` or a token written bare.
    Ident(String),
    Path(String),
    Tuple(Vec<Value>),
    /// An array of numbers.
    Nums(Vec<f64>),
    /// An array of equal-length numeric tuples, flattened.
    Tuples {
        width: usize,
        data: Vec<f64>,
    },
    /// Any other array.
    List(Vec<Value>),
    /// A dictionary or the time samples of an attribute (the first sample is kept).
    Dict(Vec<(String, Value)>),
}

impl Value {
    fn nums(&self) -> Option<Vec<f64>> {
        match self {
            Value::Num(n) => Some(vec![*n]),
            Value::Nums(v) => Some(v.clone()),
            Value::Tuple(t) => t.iter().map(|v| if let Value::Num(n) = v { Some(*n) } else { None }).collect(),
            _ => None,
        }
    }
    fn text(&self) -> Option<&str> {
        match self {
            Value::Text(s) | Value::Ident(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct Attr {
    name: String,
    value: Option<Value>,
    meta: Vec<(String, Value)>,
}

#[derive(Debug, Default)]
struct Prim {
    kind: String,
    name: String,
    attrs: Vec<Attr>,
    rels: Vec<(String, Option<Value>)>,
    children: Vec<Prim>,
}

impl Prim {
    fn attr(&self, name: &str) -> Option<&Attr> {
        self.attrs.iter().find(|a| a.name == name)
    }
    fn value(&self, name: &str) -> Option<&Value> {
        self.attr(name).and_then(|a| a.value.as_ref())
    }
}

// ---------------------------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Num(f64),
    Str(String),
    Asset(String),
    Path(String),
    Punct(char),
    End,
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
    peeked: Option<Tok>,
    numbers: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a [u8]) -> Self {
        Lexer { s, i: 0, peeked: None, numbers: 0 }
    }

    fn peek(&mut self) -> Result<&Tok> {
        if self.peeked.is_none() {
            let t = self.lex()?;
            self.peeked = Some(t);
        }
        Ok(self.peeked.as_ref().unwrap_or(&Tok::End))
    }

    fn next(&mut self) -> Result<Tok> {
        match self.peeked.take() {
            Some(t) => Ok(t),
            None => self.lex(),
        }
    }

    fn at(&self, k: usize) -> u8 {
        self.s.get(self.i + k).copied().unwrap_or(0)
    }

    fn lex(&mut self) -> Result<Tok> {
        loop {
            match self.at(0) {
                b' ' | b'\t' | b'\r' | b'\n' | b',' => self.i += 1,
                b'#' => {
                    while self.i < self.s.len() && self.at(0) != b'\n' {
                        self.i += 1;
                    }
                }
                b'/' if self.at(1) == b'*' => {
                    self.i += 2;
                    while self.i < self.s.len() && !(self.at(0) == b'*' && self.at(1) == b'/') {
                        self.i += 1;
                    }
                    self.i += 2;
                }
                _ => break,
            }
        }
        if self.i >= self.s.len() {
            return Ok(Tok::End);
        }
        let c = self.at(0);
        match c {
            b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'=' | b';' | b':' => {
                self.i += 1;
                Ok(Tok::Punct(c as char))
            }
            b'"' | b'\'' => self.string(c),
            b'@' => self.asset(),
            b'<' => {
                let start = self.i + 1;
                while self.i < self.s.len() && self.at(0) != b'>' {
                    self.i += 1;
                }
                let text = String::from_utf8_lossy(self.s.get(start..self.i).unwrap_or_default()).chars().take(MAX_TEXT).collect();
                self.i += 1;
                Ok(Tok::Path(text))
            }
            b'0'..=b'9' | b'+' | b'-' | b'.' if c != b'.' || self.at(1).is_ascii_digit() => self.number(),
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'.' || c >= 0x80 => {
                let start = self.i;
                while self.i < self.s.len()
                    && (self.at(0).is_ascii_alphanumeric() || matches!(self.at(0), b'_' | b':' | b'.' | b'!' | b'-') || self.at(0) >= 0x80)
                {
                    self.i += 1;
                }
                // `a:b` names are one identifier, but a lone trailing ':' (a time sample key) is punctuation.
                let mut end = self.i;
                while end > start && self.s.get(end - 1) == Some(&b':') {
                    end -= 1;
                }
                self.i = end;
                let text: String = String::from_utf8_lossy(self.s.get(start..end).unwrap_or_default()).chars().take(MAX_TEXT).collect();
                match text.as_str() {
                    "inf" => Ok(Tok::Num(f64::INFINITY)),
                    "nan" => Ok(Tok::Num(f64::NAN)),
                    _ => Ok(Tok::Ident(text)),
                }
            }
            other => bad(format!("unexpected character {:?} at byte {}", other as char, self.i)),
        }
    }

    fn number(&mut self) -> Result<Tok> {
        let start = self.i;
        if matches!(self.at(0), b'+' | b'-') {
            self.i += 1;
            // -inf
            if self.at(0).is_ascii_alphabetic() {
                let s = self.i;
                while self.at(0).is_ascii_alphabetic() {
                    self.i += 1;
                }
                let word = self.s.get(s..self.i).unwrap_or_default();
                let neg = self.s.get(start) == Some(&b'-');
                return match word {
                    b"inf" => Ok(Tok::Num(if neg { f64::NEG_INFINITY } else { f64::INFINITY })),
                    b"nan" => Ok(Tok::Num(f64::NAN)),
                    _ => bad("a sign must be followed by a number"),
                };
            }
        }
        while self.at(0).is_ascii_digit()
            || self.at(0) == b'.'
            || (matches!(self.at(0), b'e' | b'E') && self.i > start)
            || (matches!(self.at(0), b'+' | b'-') && matches!(self.s.get(self.i.wrapping_sub(1)), Some(b'e' | b'E')))
        {
            self.i += 1;
        }
        let text = std::str::from_utf8(self.s.get(start..self.i).unwrap_or_default()).unwrap_or("");
        self.numbers += 1;
        if self.numbers > MAX_NUMBERS {
            return Err(FormatError::Unsupported(format!("USD: more than {MAX_NUMBERS} numbers")));
        }
        match text.parse::<f64>() {
            Ok(v) => Ok(Tok::Num(v)),
            Err(_) => bad(format!("{text:?} is not a number")),
        }
    }

    fn string(&mut self, quote: u8) -> Result<Tok> {
        let triple = self.at(1) == quote && self.at(2) == quote;
        self.i += if triple { 3 } else { 1 };
        let mut out = Vec::new();
        loop {
            if self.i >= self.s.len() {
                return bad("a string is not closed");
            }
            let c = self.at(0);
            if c == b'\\' {
                let n = self.at(1);
                out.push(match n {
                    b'n' => b'\n',
                    b't' => b'\t',
                    o => o,
                });
                self.i += 2;
                continue;
            }
            if c == quote && (!triple || (self.at(1) == quote && self.at(2) == quote)) {
                self.i += if triple { 3 } else { 1 };
                break;
            }
            if out.len() < MAX_TEXT * 4 {
                out.push(c);
            }
            self.i += 1;
        }
        Ok(Tok::Str(String::from_utf8_lossy(&out).chars().take(MAX_TEXT).collect()))
    }

    fn asset(&mut self) -> Result<Tok> {
        let triple = self.at(1) == b'@' && self.at(2) == b'@';
        self.i += if triple { 3 } else { 1 };
        let start = self.i;
        loop {
            if self.i >= self.s.len() {
                return bad("an asset path is not closed");
            }
            if self.at(0) == b'@' && (!triple || (self.at(1) == b'@' && self.at(2) == b'@')) {
                break;
            }
            self.i += 1;
        }
        let text = String::from_utf8_lossy(self.s.get(start..self.i).unwrap_or_default()).chars().take(MAX_TEXT).collect();
        self.i += if triple { 3 } else { 1 };
        Ok(Tok::Asset(text))
    }
}

// ---------------------------------------------------------------------------------- parser

struct Parser<'a> {
    lx: Lexer<'a>,
    prims: usize,
    warnings: Vec<String>,
    stage: Stage,
}

impl Parser<'_> {
    fn expect(&mut self, c: char) -> Result<()> {
        match self.lx.next()? {
            Tok::Punct(p) if p == c => Ok(()),
            other => bad(format!("expected `{c}`, found {other:?}")),
        }
    }

    fn punct(&mut self, c: char) -> Result<bool> {
        if matches!(self.lx.peek()?, Tok::Punct(p) if *p == c) {
            self.lx.next()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return bad("values are nested too deeply");
        }
        match self.lx.next()? {
            Tok::Num(n) => Ok(Value::Num(n)),
            Tok::Str(s) => Ok(Value::Text(s)),
            // `@file.usda@</Prim>`: an asset with the prim in it that is meant.
            Tok::Asset(s) => {
                if matches!(self.lx.peek()?, Tok::Path(_)) {
                    self.lx.next()?;
                }
                Ok(Value::Text(s))
            }
            Tok::Path(p) => Ok(Value::Path(p)),
            Tok::Ident(i) => Ok(match i.as_str() {
                "inf" => Value::Num(f64::INFINITY),
                "nan" => Value::Num(f64::NAN),
                _ => Value::Ident(i),
            }),
            Tok::Punct('(') => {
                let mut items = Vec::new();
                while !self.punct(')')? {
                    if matches!(self.lx.peek()?, Tok::End) {
                        return bad("a tuple is not closed");
                    }
                    items.push(self.value(depth + 1)?);
                    if items.len() > 1_000_000 {
                        return bad("a tuple is too long");
                    }
                }
                Ok(Value::Tuple(items))
            }
            Tok::Punct('[') => self.array(depth),
            Tok::Punct('{') => {
                // A dictionary (`type name = value`, or `name: value` pairs) or time samples
                // (`frame: value`). Only the first entry's value matters here.
                let mut entries = Vec::new();
                while !self.punct('}')? {
                    let key = match self.lx.next()? {
                        Tok::Num(n) => n.to_string(),
                        Tok::Ident(i) | Tok::Str(i) => i,
                        Tok::End => return bad("a dictionary is not closed"),
                        other => return bad(format!("unexpected {other:?} in a dictionary")),
                    };
                    // `dictionary name = { … }` and `string name = "x"` carry a type before the name.
                    let mut key = key;
                    while !matches!(self.lx.peek()?, Tok::Punct(':') | Tok::Punct('=')) {
                        key = match self.lx.next()? {
                            Tok::Ident(i) | Tok::Str(i) => i,
                            Tok::Punct('[') => {
                                self.expect(']')?;
                                key
                            }
                            _ => return bad("unexpected token in a dictionary"),
                        };
                    }
                    self.lx.next()?;
                    let v = self.value(depth + 1)?;
                    self.punct(';')?;
                    if entries.len() < 4096 {
                        entries.push((key, v));
                    }
                }
                Ok(Value::Dict(entries))
            }
            other => bad(format!("unexpected {other:?} where a value belongs")),
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value> {
        let mut nums: Vec<f64> = Vec::new();
        let mut width = 0usize;
        let mut tuples = true;
        let mut list: Vec<Value> = Vec::new();
        let mut kind = 0u8; // 0 unknown, 1 numbers, 2 tuples, 3 other
        loop {
            match self.lx.peek()? {
                Tok::Punct(']') => {
                    self.lx.next()?;
                    break;
                }
                Tok::End => return bad("an array is not closed"),
                Tok::Num(n) if kind <= 1 => {
                    nums.push(*n);
                    kind = 1;
                    self.lx.next()?;
                }
                Tok::Punct('(') if kind == 0 || kind == 2 => {
                    // A tuple of numbers goes straight into the flat buffer.
                    self.lx.next()?;
                    let mut n = 0usize;
                    loop {
                        match self.lx.next()? {
                            Tok::Num(v) => {
                                nums.push(v);
                                n += 1;
                            }
                            Tok::Punct(')') => break,
                            _ => {
                                tuples = false;
                                break;
                            }
                        }
                    }
                    if !tuples || (width != 0 && width != n) {
                        return bad("an array holds tuples of different lengths or kinds");
                    }
                    width = n;
                    kind = 2;
                }
                _ => {
                    if kind == 1 || kind == 2 {
                        return bad("an array mixes numbers, tuples and other values");
                    }
                    kind = 3;
                    list.push(self.value(depth + 1)?);
                }
            }
        }
        Ok(match kind {
            1 => Value::Nums(nums),
            2 => Value::Tuples { width, data: nums },
            3 => Value::List(list),
            _ => Value::Nums(Vec::new()),
        })
    }

    /// `( key = value … )`: metadata, with bare strings (documentation) left out.
    fn meta(&mut self) -> Result<Vec<(String, Value)>> {
        let mut out = Vec::new();
        if !self.punct('(')? {
            return Ok(out);
        }
        while !self.punct(')')? {
            match self.lx.next()? {
                Tok::Ident(mut key) => {
                    // `prepend references = …`, `add apiSchemas = …`
                    while !matches!(self.lx.peek()?, Tok::Punct('=') | Tok::Punct(')')) {
                        match self.lx.next()? {
                            Tok::Ident(k) => key = k,
                            _ => return bad("unexpected token in metadata"),
                        }
                    }
                    if self.punct('=')? {
                        let v = self.value(0)?;
                        if out.len() < 256 {
                            out.push((key, v));
                        }
                    }
                }
                Tok::Str(_) => {}
                Tok::End => return bad("metadata is not closed"),
                other => return bad(format!("unexpected {other:?} in metadata")),
            }
        }
        Ok(out)
    }

    /// One statement of a prim body or the layer: a child prim, an attribute, a relationship or
    /// something skipped.
    fn prim(&mut self, specifier: &str, depth: usize) -> Result<Option<Prim>> {
        if depth >= MAX_DEPTH {
            return Err(FormatError::Unsupported(format!("USD: prims nest deeper than {MAX_DEPTH}")));
        }
        self.prims += 1;
        if self.prims > MAX_PRIMS {
            return Err(FormatError::Unsupported(format!("USD: more than {MAX_PRIMS} prims")));
        }
        let mut prim = Prim::default();
        // `def Type "name"`, `def "name"`
        match self.lx.next()? {
            Tok::Ident(t) => {
                prim.kind = t;
                match self.lx.next()? {
                    Tok::Str(n) => prim.name = n,
                    _ => return bad("a prim needs a name in quotes"),
                }
            }
            Tok::Str(n) => prim.name = n,
            other => return bad(format!("a prim needs a type or a name, found {other:?}")),
        }
        let meta = self.meta()?;
        for (k, _) in &meta {
            if matches!(k.as_str(), "references" | "payload" | "inherits" | "specializes" | "variantSets" | "variants") {
                self.warn(format!("`{}` on prim `{}` is not composed", k, prim.name));
            }
        }
        self.expect('{')?;
        while !self.punct('}')? {
            if matches!(self.lx.peek()?, Tok::End) {
                return bad(format!("prim `{}` is not closed", prim.name));
            }
            self.statement(&mut prim, depth)?;
        }
        Ok(if specifier == "def" { Some(prim) } else { None })
    }

    fn warn(&mut self, m: String) {
        if self.warnings.len() < 64 && !self.warnings.contains(&m) {
            self.warnings.push(m);
        }
    }

    fn statement(&mut self, parent: &mut Prim, depth: usize) -> Result<()> {
        let first = match self.lx.next()? {
            Tok::Ident(i) => i,
            other => return bad(format!("unexpected {other:?} in a prim")),
        };
        match first.as_str() {
            "def" | "over" | "class" => {
                if let Some(child) = self.prim(&first, depth + 1)? {
                    parent.children.push(child);
                }
            }
            "variantSet" => {
                self.warn("variant sets are not read".into());
                self.lx.next()?; // name
                self.expect('=')?;
                self.value(0)?;
            }
            "prepend" | "append" | "add" | "delete" | "reorder" => {
                // A list edit of a relationship or attribute: parse what follows as a statement.
                return self.statement(parent, depth);
            }
            _ => {
                // [custom] [uniform] type[] name [= value] [(meta)]
                let mut words = vec![first];
                while let Tok::Ident(_) = self.lx.peek()? {
                    if let Tok::Ident(w) = self.lx.next()? {
                        words.push(w);
                    }
                    // `type[]`
                    if self.punct('[')? {
                        self.expect(']')?;
                    }
                    if words.len() > 6 {
                        return bad("an attribute declaration has too many words");
                    }
                }
                // `float3[]` is `float3` then `[` `]`, which the loop above handles after a word;
                // a first word followed directly by `[` is handled here.
                if self.punct('[')? {
                    self.expect(']')?;
                    if let Tok::Ident(w) = self.lx.next()? {
                        words.push(w);
                    }
                }
                let name = words.last().cloned().unwrap_or_default();
                let is_rel = words.iter().any(|w| w == "rel");
                let value = if self.punct('=')? { Some(self.value(0)?) } else { None };
                let meta = self.meta()?;
                if is_rel {
                    parent.rels.push((name, value));
                } else {
                    // `foo.timeSamples = {…}` keeps the first sample; `foo.connect` is a connection.
                    let (base, suffix) = match name.rsplit_once('.') {
                        Some((b, s)) => (b.to_string(), Some(s.to_string())),
                        None => (name.clone(), None),
                    };
                    match (suffix.as_deref(), value) {
                        (Some("timeSamples"), Some(Value::Dict(mut d))) => {
                            self.warn("animated attributes are read at their first time sample".into());
                            let first = if d.is_empty() { None } else { Some(d.remove(0).1) };
                            parent.attrs.push(Attr { name: base, value: first, meta });
                        }
                        (Some(_), _) => {}
                        (None, value) => parent.attrs.push(Attr { name, value, meta }),
                    }
                }
            }
        }
        Ok(())
    }

    fn layer(&mut self) -> Result<Vec<Prim>> {
        let meta = self.meta()?;
        for (k, v) in &meta {
            match (k.as_str(), v) {
                ("upAxis", v) => self.stage.z_up = v.text() == Some("Z"),
                ("metersPerUnit", Value::Num(n)) if *n > 0.0 && n.is_finite() => self.stage.metres_per_unit = Some(*n),
                ("subLayers", _) => self.warn("sublayers are not read".into()),
                _ => {}
            }
        }
        let mut roots = Vec::new();
        loop {
            match self.lx.next()? {
                Tok::End => break,
                Tok::Ident(s) if matches!(s.as_str(), "def" | "over" | "class") => {
                    if let Some(p) = self.prim(&s, 0)? {
                        roots.push(p);
                    }
                }
                other => return bad(format!("unexpected {other:?} at the top level")),
            }
        }
        Ok(roots)
    }
}

// ---------------------------------------------------------------------------------- scene

fn dmat_from_rows(rows: &[f64]) -> Option<DMat4> {
    if rows.len() != 16 || rows.iter().any(|v| !v.is_finite()) {
        return None;
    }
    // USD writes row-vector matrices: row i is the image of axis i. glam's columns are those rows.
    let c = |i: usize| glam::DVec4::new(rows[i * 4], rows[i * 4 + 1], rows[i * 4 + 2], rows[i * 4 + 3]);
    Some(DMat4::from_cols(c(0), c(1), c(2), c(3)))
}

fn matrix_value(v: &Value) -> Option<DMat4> {
    match v {
        Value::Tuple(rows) => {
            let mut flat = Vec::new();
            for r in rows {
                flat.extend(r.nums()?);
            }
            dmat_from_rows(&flat)
        }
        _ => None,
    }
}

/// The local matrix of a prim from its `xformOpOrder`.
fn local_matrix(p: &Prim, warnings: &mut Vec<String>) -> DMat4 {
    let order: Vec<String> = match p.value("xformOpOrder") {
        Some(Value::List(l)) => l.iter().filter_map(|v| v.text().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    let mut m = DMat4::IDENTITY;
    for op in order.iter().take(64) {
        let (inverse, name) = match op.strip_prefix("!invert!") {
            Some(n) => (true, n),
            None => (false, op.as_str()),
        };
        let Some(value) = p.value(name) else { continue };
        let kind = name.strip_prefix("xformOp:").unwrap_or(name);
        let base = kind.split(':').next().unwrap_or(kind);
        let nums = value.nums().unwrap_or_default();
        let rot = |axis: DVec3, deg: f64| DQuat::from_axis_angle(axis, deg.to_radians());
        let one = match base {
            "transform" => matrix_value(value),
            "translate" if nums.len() == 3 => Some(DMat4::from_translation(DVec3::new(nums[0], nums[1], nums[2]))),
            "scale" if nums.len() == 3 => Some(DMat4::from_scale(DVec3::new(nums[0], nums[1], nums[2]))),
            "rotateX" if nums.len() == 1 => Some(DMat4::from_quat(rot(DVec3::X, nums[0]))),
            "rotateY" if nums.len() == 1 => Some(DMat4::from_quat(rot(DVec3::Y, nums[0]))),
            "rotateZ" if nums.len() == 1 => Some(DMat4::from_quat(rot(DVec3::Z, nums[0]))),
            "orient" if nums.len() == 4 => {
                // (real, i, j, k)
                let q = DQuat::from_xyzw(nums[1], nums[2], nums[3], nums[0]);
                (q.length() > 1e-9).then(|| DMat4::from_quat(q.normalize()))
            }
            b if b.starts_with("rotate") && b.len() == 9 && nums.len() == 3 => {
                // rotateXYZ: about X first, then Y, then Z.
                let mut r = DQuat::IDENTITY;
                for letter in b[6..].chars() {
                    // The tuple always reads (x, y, z); the name gives the order they apply in.
                    let (axis, deg) = match letter {
                        'X' => (DVec3::X, nums[0]),
                        'Y' => (DVec3::Y, nums[1]),
                        _ => (DVec3::Z, nums[2]),
                    };
                    r = rot(axis, deg) * r;
                }
                Some(DMat4::from_quat(r))
            }
            _ => None,
        };
        let Some(one) = one.filter(|o| o.is_finite()) else {
            if warnings.len() < 64 {
                warnings.push(format!("transform operation `{name}` on `{}` was not understood", p.name));
            }
            continue;
        };
        let one = if inverse { one.inverse() } else { one };
        if !one.is_finite() {
            continue;
        }
        m *= one;
    }
    m
}

struct Build<'a> {
    scene: cm::Scene,
    limits: &'a cm::Limits,
    /// Material prim path → index.
    materials: HashMap<String, usize>,
    bindings: Vec<(usize, String)>,
    names: Vec<String>,
}

fn paths(p: &Prim, at: &str, out: &mut Vec<String>) {
    let here = format!("{at}/{}", p.name);
    out.push(here.clone());
    for c in &p.children {
        paths(c, &here, out);
    }
}

fn colour(v: &Value) -> Option<[f32; 3]> {
    let n = v.nums()?;
    (n.len() == 3).then(|| [n[0] as f32, n[1] as f32, n[2] as f32])
}

/// A `Material` prim: the surface shader below it, as the suite's PBR parameters.
fn read_material(m: &Prim) -> cm::Material {
    let mut out = cm::Material { name: m.name.clone(), ..Default::default() };
    for sh in &m.children {
        if let Some(c) = sh.value("inputs:diffuseColor").and_then(colour) {
            out.base_color = [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), 1.0];
        }
        if let Some(Value::Num(v)) = sh.value("inputs:metallic") {
            out.metallic = (*v as f32).clamp(0.0, 1.0);
        }
        if let Some(Value::Num(v)) = sh.value("inputs:roughness") {
            out.roughness = (*v as f32).clamp(0.0, 1.0);
        }
        if let Some(Value::Num(v)) = sh.value("inputs:opacity") {
            out.base_color[3] = (*v as f32).clamp(0.0, 1.0);
        }
    }
    out
}

fn read_mesh(p: &Prim, limits: &cm::Limits) -> Result<Option<cm::Mesh>> {
    let Some(Value::Tuples { width: 3, data: pts }) = p.value("points") else { return Ok(None) };
    let n = pts.len() / 3;
    if n > limits.max_vertices {
        return Err(FormatError::Unsupported(format!("USD: mesh `{}` has more than {} points", p.name, limits.max_vertices)));
    }
    let counts = match p.value("faceVertexCounts") {
        Some(Value::Nums(c)) => c,
        _ => return Ok(None),
    };
    let idx = match p.value("faceVertexIndices") {
        Some(Value::Nums(c)) => c,
        _ => return Ok(None),
    };
    let total: f64 = counts.iter().sum();
    if total != idx.len() as f64 {
        return bad(format!("mesh `{}`: the face counts add up to {total} but there are {} indices", p.name, idx.len()));
    }
    if idx.len() > limits.max_triangles.saturating_mul(3) {
        return Err(FormatError::Unsupported(format!("USD: mesh `{}` has more than {} corners", p.name, limits.max_triangles * 3)));
    }
    let mut mesh = cm::Mesh { name: p.name.clone(), positions: pts.as_chunks::<3>().0.to_vec(), ..Default::default() };
    // Texture coordinates: per corner (faceVarying) or per point (vertex), through optional indices.
    let st = p.attr("primvars:st").or_else(|| p.attr("primvars:st0"));
    let uv_values: Vec<[f32; 2]> = match st.and_then(|a| a.value.as_ref()) {
        Some(Value::Tuples { width: 2, data }) => data.as_chunks::<2>().0.iter().map(|c| [c[0] as f32, 1.0 - c[1] as f32]).collect(),
        _ => Vec::new(),
    };
    let interp = st.and_then(|a| a.meta.iter().find(|(k, _)| k == "interpolation")).and_then(|(_, v)| v.text()).unwrap_or("vertex");
    let uv_indices: Option<&Vec<f64>> = match p.value(&format!("{}:indices", st.map_or("primvars:st", |a| a.name.as_str()))) {
        Some(Value::Nums(i)) => Some(i),
        _ => None,
    };
    let mut part = cm::Part { name: p.name.clone(), ..Default::default() };
    let mut uvs_used = Vec::new();
    let mut corner = 0usize;
    for c in counts {
        let k = *c as usize;
        if !(*c >= 0.0 && c.fract() == 0.0) || k > limits.max_face_corners {
            return bad(format!("mesh `{}`: a face has {c} corners", p.name));
        }
        let end = corner + k;
        if k >= 3 {
            for i in corner..end {
                let v = idx[i];
                if !(v >= 0.0 && (v as usize) < n && v.fract() == 0.0) {
                    return bad(format!("mesh `{}`: point index {v} is out of range", p.name));
                }
                let position = v as u32;
                let uv_slot = match (interp, uv_indices) {
                    ("faceVarying", Some(ix)) => ix.get(i).map(|x| *x as usize),
                    ("faceVarying", None) => Some(i),
                    ("vertex", Some(ix)) => ix.get(position as usize).map(|x| *x as usize),
                    ("vertex", None) => Some(position as usize),
                    _ => None,
                };
                let uv = match uv_slot.and_then(|s| uv_values.get(s)) {
                    Some(value) => {
                        uvs_used.push(*value);
                        (uvs_used.len() - 1) as u32
                    }
                    None => cm::NONE,
                };
                part.corners.push(cm::Corner { position, normal: cm::NONE, uv });
            }
            part.face_sizes.push(k as u32);
        }
        corner = end;
    }
    // Either every corner has a UV or none is kept.
    if part.corners.iter().any(|c| c.uv == cm::NONE) {
        for c in &mut part.corners {
            c.uv = cm::NONE;
        }
    } else if !uvs_used.is_empty() {
        mesh.uv_sets.push(uvs_used);
    }
    mesh.parts.push(part);
    Ok(Some(mesh))
}

impl Build<'_> {
    fn node(&mut self, p: &Prim, at: &str, parent: Option<usize>, warnings: &mut Vec<String>) -> Result<()> {
        if self.scene.nodes.len() >= self.limits.max_nodes {
            return Err(FormatError::Unsupported(format!("USD: more than {} nodes", self.limits.max_nodes)));
        }
        let path = format!("{at}/{}", p.name);
        let lm = local_matrix(p, warnings);
        let mut node = cm::Node { name: p.name.clone(), transform: lm.to_cols_array(), ..Default::default() };
        match p.kind.as_str() {
            "Mesh" => {
                if let Some(mesh) = read_mesh(p, self.limits)? {
                    if self.scene.meshes.len() >= self.limits.max_meshes {
                        return Err(FormatError::Unsupported("USD: too many meshes".into()));
                    }
                    self.scene.meshes.push(mesh);
                    node.mesh = Some(self.scene.meshes.len() - 1);
                    let bound = p.rels.iter().find(|(n, _)| n == "material:binding").and_then(|(_, v)| match v {
                        Some(Value::Path(s)) => Some(s.clone()),
                        _ => None,
                    });
                    match bound {
                        Some(b) => self.bindings.push((self.scene.meshes.len() - 1, b)),
                        None => {
                            // primvars:displayColor: one colour for the whole mesh.
                            if let Some(Value::Tuples { width: 3, data }) = p.value("primvars:displayColor")
                                && data.len() >= 3
                            {
                                self.scene.materials.push(cm::Material {
                                    name: format!("{} colour", p.name),
                                    base_color: [data[0] as f32, data[1] as f32, data[2] as f32, 1.0],
                                    metallic: 0.0,
                                    roughness: 0.6,
                                    ..Default::default()
                                });
                                let mi = self.scene.materials.len() - 1;
                                if let Some(m) = self.scene.meshes.last_mut().and_then(|m| m.parts.first_mut()) {
                                    m.material = Some(mi);
                                }
                            }
                        }
                    }
                }
            }
            "" | "Xform" | "Scope" | "Material" | "Shader" | "NodeGraph" => {}
            other => warnings.push(format!("prim `{}` of type {other} is not read as geometry", p.name)),
        }
        self.scene.nodes.push(node);
        let me = self.scene.nodes.len() - 1;
        match parent {
            Some(par) => {
                if let Some(n) = self.scene.nodes.get_mut(par) {
                    n.children.push(me);
                }
            }
            None => self.scene.roots.push(me),
        }
        self.names.push(path.clone());
        // Material prims are resources, not part of the scene tree.
        // A scope that only holds materials is a resource folder, not part of the scene tree.
        let resources = |c: &Prim| {
            matches!(c.kind.as_str(), "Material" | "Shader" | "NodeGraph")
                || (c.kind == "Scope" && !c.children.is_empty() && c.children.iter().all(|k| k.kind == "Material"))
        };
        for c in p.children.iter().filter(|c| !resources(c)) {
            self.node(c, &path, Some(me), warnings)?;
        }
        Ok(())
    }
}

/// Read a `.usda` file into the neutral scene, and what the stage says about axes and units.
pub fn read(bytes: &[u8], limits: &cm::Limits) -> Result<(cm::Scene, Stage)> {
    if bytes.starts_with(b"PXR-USDC") {
        return Err(FormatError::Unsupported("USD: this is a binary crate file (.usdc); save it as .usda from the application that made it".into()));
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return Err(FormatError::Unsupported("USD: .usdz packages are not read; unpack it and open the .usda inside".into()));
    }
    if bytes.len() > limits.max_data_bytes {
        return Err(FormatError::Unsupported(format!("USD: file larger than {} bytes", limits.max_data_bytes)));
    }
    let head = String::from_utf8_lossy(bytes.get(..bytes.len().min(32)).unwrap_or_default()).to_ascii_lowercase();
    if !head.starts_with("#usda") {
        return bad("the file does not start with `#usda`");
    }
    let mut p = Parser { lx: Lexer::new(bytes), prims: 0, warnings: Vec::new(), stage: Stage::default() };
    let roots = p.layer()?;
    let mut warnings = std::mem::take(&mut p.warnings);
    let stage = p.stage;
    // Material prims by path.
    let mut all = Vec::new();
    for r in &roots {
        paths(r, "", &mut all);
    }
    let mut build = Build { scene: cm::Scene::default(), limits, materials: HashMap::new(), bindings: Vec::new(), names: Vec::new() };
    for path in &all {
        let prim = find(&roots, path);
        if let Some(m) = prim.filter(|m| m.kind == "Material") {
            if build.scene.materials.len() >= limits.max_materials {
                return Err(FormatError::Unsupported("USD: too many materials".into()));
            }
            build.scene.materials.push(read_material(m));
            build.materials.insert(path.clone(), build.scene.materials.len() - 1);
        }
    }
    for r in &roots {
        build.node(r, "", None, &mut warnings)?;
    }
    for (mesh, path) in std::mem::take(&mut build.bindings) {
        if let Some(mi) = build.materials.get(&path).copied()
            && let Some(part) = build.scene.meshes.get_mut(mesh).and_then(|m| m.parts.first_mut())
        {
            part.material = Some(mi);
        }
    }
    if build.scene.meshes.is_empty() {
        warnings.push("the file holds no mesh geometry".into());
    }
    build.scene.warnings = warnings.into_iter().map(|w| format!("USD: {w}")).collect();
    Ok((build.scene, stage))
}

fn find<'a>(roots: &'a [Prim], path: &str) -> Option<&'a Prim> {
    let mut parts = path.split('/').filter(|s| !s.is_empty());
    let mut level = roots;
    let mut found = None;
    for part in parts.by_ref() {
        let next = level.iter().find(|p| p.name == part)?;
        found = Some(next);
        level = &next.children;
    }
    found
}

// ---------------------------------------------------------------------------------- writer

/// A USD prim name: letters, digits and underscores, not starting with a digit.
fn ident(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).take(100).collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

fn f(v: f32) -> String {
    if v.is_finite() { format!("{v}") } else { "0".into() }
}

fn matrix_text(m: Mat4) -> String {
    let c = m.to_cols_array_2d();
    let row = |i: usize| format!("({}, {}, {}, {})", f(c[i][0]), f(c[i][1]), f(c[i][2]), f(c[i][3]));
    format!("( {}, {}, {}, {} )", row(0), row(1), row(2), row(3))
}

/// The items as USD text (`.usda`), whose positions are already in the file's axes (`z_up`) and
/// unit. One `Xform` per item with its transform, a `Mesh` child where the item has a mesh, and
/// a `UsdPreviewSurface` material per mesh under `/Materials`.
pub fn write_in(items: &[Item], z_up: bool, metres_per_unit: f32) -> Result<Vec<u8>> {
    let mut o = String::new();
    let _ = write!(
        o,
        "#usda 1.0\n(\n    defaultPrim = \"World\"\n    metersPerUnit = {}\n    upAxis = \"{}\"\n    doc = \"Written by ModelCraft\"\n)\n\n",
        f(metres_per_unit),
        if z_up { "Z" } else { "Y" }
    );
    // Materials first.
    let mut mat_names: HashMap<usize, String> = HashMap::new();
    let mut mats = String::new();
    for (i, it) in items.iter().enumerate() {
        if it.mesh.as_ref().is_none_or(|m| m.face_count() == 0) {
            continue;
        }
        let name = format!("M{}_{}", i, ident(&it.name));
        let c = it.material.base_color;
        let _ = write!(
            mats,
            "        def Material \"{name}\"\n        {{\n            token outputs:surface.connect = </World/Materials/{name}/Surface.outputs:surface>\n            def Shader \"Surface\"\n            {{\n                uniform token info:id = \"UsdPreviewSurface\"\n                color3f inputs:diffuseColor = ({}, {}, {})\n                float inputs:metallic = {}\n                float inputs:roughness = {}\n                float inputs:opacity = {}\n                token outputs:surface\n            }}\n        }}\n",
            f(c[0]),
            f(c[1]),
            f(c[2]),
            f(it.material.metallic),
            f(it.material.roughness),
            f(c[3])
        );
        mat_names.insert(i, name);
    }
    o.push_str("def Xform \"World\"\n{\n");
    if !mats.is_empty() {
        let _ = write!(o, "    def Scope \"Materials\"\n    {{\n{mats}    }}\n");
    }
    let mut children: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, it) in items.iter().enumerate() {
        let parent = it.parent.filter(|p| *p < items.len() && *p != i);
        children.entry(parent).or_default().push(i);
    }
    // Depth-first with an explicit stack: (item, depth, closing pending).
    enum Step {
        Open(usize, usize),
        Close(usize),
    }
    let mut stack: Vec<Step> = children.get(&None).map(|v| v.iter().rev().map(|i| Step::Open(*i, 1)).collect()).unwrap_or_default();
    let mut visited = vec![false; items.len()];
    let mut used: HashMap<(Option<usize>, String), usize> = HashMap::new();
    while let Some(step) = stack.pop() {
        match step {
            Step::Close(d) => {
                let _ = writeln!(o, "{}}}", "    ".repeat(d));
            }
            Step::Open(i, d) => {
                if d > MAX_DEPTH || std::mem::replace(&mut visited[i], true) {
                    continue;
                }
                let it = &items[i];
                let pad = "    ".repeat(d);
                let base = ident(&it.name);
                let n = used.entry((it.parent, base.clone())).or_insert(0);
                let name = if *n == 0 { base } else { format!("{base}_{n}") };
                *n += 1;
                let _ = writeln!(o, "{pad}def Xform \"{name}\"\n{pad}{{");
                let _ = writeln!(o, "{pad}    matrix4d xformOp:transform = {}", matrix_text(it.transform.matrix()));
                let _ = writeln!(o, "{pad}    uniform token[] xformOpOrder = [\"xformOp:transform\"]");
                if let Some(mesh) = it.mesh.as_ref().filter(|m| m.face_count() > 0) {
                    mesh_prim(&mut o, mesh, mat_names.get(&i).map(String::as_str), d + 1);
                }
                stack.push(Step::Close(d));
                if let Some(kids) = children.get(&Some(i)) {
                    stack.extend(kids.iter().rev().map(|k| Step::Open(*k, d + 1)));
                }
            }
        }
    }
    o.push_str("}\n");
    Ok(o.into_bytes())
}

fn mesh_prim(o: &mut String, mesh: &modelcraft_mesh::PolyMesh, material: Option<&str>, d: usize) {
    let pad = "    ".repeat(d);
    let api = if material.is_some() { " (\n" } else { "" };
    let _ = write!(o, "{pad}def Mesh \"Mesh\"{api}");
    if material.is_some() {
        let _ = write!(o, "{pad}    prepend apiSchemas = [\"MaterialBindingAPI\"]\n{pad})\n");
    } else {
        o.push('\n');
    }
    let _ = writeln!(o, "{pad}{{");
    let _ = writeln!(o, "{pad}    uniform token subdivisionScheme = \"none\"");
    let counts: Vec<String> = (0..mesh.face_count() as u32).map(|fc| mesh.face_len(fc).to_string()).collect();
    let _ = writeln!(o, "{pad}    int[] faceVertexCounts = [{}]", counts.join(", "));
    let mut indices = Vec::new();
    let mut uvs = Vec::new();
    for fc in 0..mesh.face_count() as u32 {
        for h in mesh.face_half_edges(fc) {
            indices.push(mesh.origin(h).to_string());
            if mesh.has_uvs() {
                let uv = mesh.uv(h);
                uvs.push(format!("({}, {})", f(uv.x), f(1.0 - uv.y)));
            }
        }
    }
    let _ = writeln!(o, "{pad}    int[] faceVertexIndices = [{}]", indices.join(", "));
    let pts: Vec<String> = mesh.positions().iter().map(|p| format!("({}, {}, {})", f(p.x), f(p.y), f(p.z))).collect();
    let _ = writeln!(o, "{pad}    point3f[] points = [{}]", pts.join(", "));
    let normals: Vec<String> = mesh.vertex_normals().iter().map(|n| format!("({}, {}, {})", f(n.x), f(n.y), f(n.z))).collect();
    let _ = writeln!(o, "{pad}    normal3f[] normals = [{}] (\n{pad}        interpolation = \"vertex\"\n{pad}    )", normals.join(", "));
    if !uvs.is_empty() {
        let _ = writeln!(o, "{pad}    texCoord2f[] primvars:st = [{}] (\n{pad}        interpolation = \"faceVarying\"\n{pad}    )", uvs.join(", "));
    }
    if let Some(m) = material {
        let _ = writeln!(o, "{pad}    rel material:binding = </World/Materials/{m}>");
    }
    let _ = writeln!(o, "{pad}}}");
}

#[cfg(test)]
#[path = "usd_tests.rs"]
mod tests;
