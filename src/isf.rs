//! ISF (Interactive Shader Format) support.
//!
//! An ISF shader is a fragment shader with a leading JSON header (inside the
//! first `/* … */` comment) that declares typed `INPUTS` (sliders, toggles,
//! colors, dropdowns, points, images) and a `PASSES` array for multi-pass
//! rendering. This module:
//!
//! 1. parses the polymorphic JSON header on nanoserde's public tokenizer (there
//!    is no untyped `Value` type in nanoserde, so [`JVal`] is a tiny hand-rolled
//!    walker — see [`parse_json`]);
//! 2. models the header as [`IsfHeader`] (typed [`IsfInput`]s + [`IsfPass`]es);
//! 3. transpiles the GLSL body onto vidiotic's existing uniform contract
//!    ([`transpile`]) — it reuses the base `PREAMBLE` (sets 0/1/2) and adds a
//!    per-shader parameter UBO at `set = 3`, mapping ISF's `RENDERSIZE`/`TIME`/
//!    `IMG_*`/`inputImage` conventions onto our bindings.
//!
//! The transpiler emits a combined source ready for the same naga parse/validate
//! path used by plain GLSL, plus the [`IsfUbo`] std140 layout used to pack the
//! parameter buffer each frame.

use std::path::PathBuf;

use nanoserde::{DeJsonState, DeJsonTok};

use crate::shader::PREAMBLE;

// ---------------------------------------------------------------------------
// Untyped JSON value (hand-rolled over nanoserde's tokenizer)
// ---------------------------------------------------------------------------

/// An untyped JSON value. ISF headers are polymorphic (a `DEFAULT`/`MIN`/`MAX`
/// may be a number, bool, string, or array), and nanoserde exposes only strict
/// derive-based deserialization plus a public tokenizer — so we walk the tokens
/// into this value ourselves.
#[derive(Debug, Clone, PartialEq)]
pub enum JVal {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Self>),
    Obj(Vec<(String, Self)>),
}

impl JVal {
    fn as_num(&self) -> Option<f64> {
        match self {
            Self::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
    fn as_arr(&self) -> Option<&[Self]> {
        match self {
            Self::Arr(a) => Some(a),
            _ => None,
        }
    }
    /// Truthiness: `true`/`false`, or a nonzero number (ISF uses both for bools).
    fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Num(n) => Some(*n != 0.0),
            _ => None,
        }
    }
    /// Look up a key in an object (case-sensitive).
    fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

type Chars<'a> = std::str::Chars<'a>;

/// Parse a JSON document into a [`JVal`]. Standard JSON only (no comments, no
/// trailing commas) — the ISF header is standard JSON once the enclosing
/// `/* … */` is stripped.
///
/// # Errors
/// Returns a message describing the first malformed or unexpected token.
pub fn parse_json(src: &str) -> Result<JVal, String> {
    let mut st = DeJsonState::default();
    let mut chars = src.chars();
    st.next(&mut chars);
    st.next_tok(&mut chars).map_err(e2s)?;
    let v = parse_value(&mut st, &mut chars)?;
    Ok(v)
}

fn e2s(e: nanoserde::DeJsonErr) -> String {
    format!("{e:?}")
}

fn parse_value(st: &mut DeJsonState, ch: &mut Chars) -> Result<JVal, String> {
    match st.tok.clone() {
        DeJsonTok::CurlyOpen => parse_obj(st, ch),
        DeJsonTok::BlockOpen => parse_arr(st, ch),
        DeJsonTok::Str => {
            let s = st.as_string().map_err(e2s)?;
            st.next_tok(ch).map_err(e2s)?;
            Ok(JVal::Str(s))
        }
        DeJsonTok::U64(_) | DeJsonTok::I64(_) | DeJsonTok::F64(_) => {
            let n = st.as_f64().map_err(e2s)?;
            st.next_tok(ch).map_err(e2s)?;
            Ok(JVal::Num(n))
        }
        DeJsonTok::Bool(b) => {
            st.next_tok(ch).map_err(e2s)?;
            Ok(JVal::Bool(b))
        }
        DeJsonTok::Null => {
            st.next_tok(ch).map_err(e2s)?;
            Ok(JVal::Null)
        }
        other => Err(format!("unexpected JSON token {other:?}")),
    }
}

fn parse_obj(st: &mut DeJsonState, ch: &mut Chars) -> Result<JVal, String> {
    st.next_tok(ch).map_err(e2s)?; // consume '{'
    let mut fields = Vec::new();
    if st.tok == DeJsonTok::CurlyClose {
        st.next_tok(ch).map_err(e2s)?;
        return Ok(JVal::Obj(fields));
    }
    loop {
        let key = st.as_string().map_err(e2s)?; // key must be a string
        st.next_tok(ch).map_err(e2s)?;
        if st.tok != DeJsonTok::Colon {
            return Err(format!("expected ':' in object, got {:?}", st.tok));
        }
        st.next_tok(ch).map_err(e2s)?;
        let val = parse_value(st, ch)?;
        fields.push((key, val));
        match st.tok {
            DeJsonTok::Comma => {
                st.next_tok(ch).map_err(e2s)?;
            }
            DeJsonTok::CurlyClose => {
                st.next_tok(ch).map_err(e2s)?;
                break;
            }
            _ => return Err(format!("expected ',' or '}}' in object, got {:?}", st.tok)),
        }
    }
    Ok(JVal::Obj(fields))
}

fn parse_arr(st: &mut DeJsonState, ch: &mut Chars) -> Result<JVal, String> {
    st.next_tok(ch).map_err(e2s)?; // consume '['
    let mut items = Vec::new();
    if st.tok == DeJsonTok::BlockClose {
        st.next_tok(ch).map_err(e2s)?;
        return Ok(JVal::Arr(items));
    }
    loop {
        let val = parse_value(st, ch)?;
        items.push(val);
        match st.tok {
            DeJsonTok::Comma => {
                st.next_tok(ch).map_err(e2s)?;
            }
            DeJsonTok::BlockClose => {
                st.next_tok(ch).map_err(e2s)?;
                break;
            }
            _ => return Err(format!("expected ',' or ']' in array, got {:?}", st.tok)),
        }
    }
    Ok(JVal::Arr(items))
}

// ---------------------------------------------------------------------------
// Typed header model
// ---------------------------------------------------------------------------

/// A runtime parameter value for an ISF input. Floats are stored as `f32` to
/// match the GPU uniform; integers cover both `long` (dropdown) and `bool`
/// (0/1).
#[derive(Clone, Debug, PartialEq)]
pub enum IsfValue {
    Float(f32),
    Bool(bool),
    Long(i32),
    Color([f32; 4]),
    Point2D([f32; 2]),
}

/// The declared type + bounds of one ISF `INPUT`.
#[derive(Clone, Debug, PartialEq)]
pub enum IsfInputKind {
    Float { min: f32, max: f32, default: f32 },
    Bool { default: bool },
    Long { values: Vec<i32>, labels: Vec<String>, default: i32 },
    Color { default: [f32; 4] },
    Point2D { min: [f32; 2], max: [f32; 2], default: [f32; 2] },
    /// A momentary trigger; treated as a bool in the uniform.
    Event,
    /// An image input (input image / imported texture). Not a UBO field.
    Image,
}

impl IsfInputKind {
    /// The schema default as an [`IsfValue`] (image inputs have no scalar value).
    pub fn default_value(&self) -> Option<IsfValue> {
        match self {
            Self::Float { default, .. } => Some(IsfValue::Float(*default)),
            Self::Bool { default } => Some(IsfValue::Bool(*default)),
            Self::Long { default, .. } => Some(IsfValue::Long(*default)),
            Self::Color { default } => Some(IsfValue::Color(*default)),
            Self::Point2D { default, .. } => Some(IsfValue::Point2D(*default)),
            Self::Event => Some(IsfValue::Bool(false)),
            Self::Image => None,
        }
    }

    fn is_image(&self) -> bool {
        matches!(self, Self::Image)
    }
}

/// One declared `INPUT`.
#[derive(Clone, Debug, PartialEq)]
pub struct IsfInput {
    pub name: String,
    pub label: Option<String>,
    pub kind: IsfInputKind,
}

/// A pass target size. `Full` = the default `RENDERSIZE`; `Expr` is an ISF size
/// expression (e.g. `"$WIDTH/2"`), evaluated by [`eval_size`].
#[derive(Clone, Debug, PartialEq)]
pub enum SizeExpr {
    /// Render at `RENDERSIZE` (the default when `WIDTH`/`HEIGHT` are absent).
    Full,
    /// A raw ISF size expression string (e.g. `"$WIDTH/2"`), evaluated later.
    Expr(String),
}

/// Evaluate a [`SizeExpr`] against the base render size, returning a dimension in
/// pixels (at least 1). `default` is used for [`SizeExpr::Full`] (the base width
/// or height, depending on which axis is being sized). Unparseable expressions
/// fall back to `default` with a warning. Supports numbers, `$WIDTH`/`$HEIGHT`,
/// `+ - * /`, parentheses, and `floor`/`ceil`/`abs`/`min`/`max`.
pub fn eval_size(expr: &SizeExpr, base_w: u32, base_h: u32, default: u32) -> u32 {
    match expr {
        SizeExpr::Full => default.max(1),
        SizeExpr::Expr(s) => match eval_expr(s, base_w as f64, base_h as f64) {
            Some(v) if v >= 1.0 => v.round() as u32,
            Some(_) => 1,
            None => {
                log::warn!("ISF: unparseable size expression {s:?}; using {default}");
                default.max(1)
            }
        },
    }
}

/// One named `PASSES[].TARGET` buffer: which pass writes it, and its properties.
#[derive(Clone, Debug, PartialEq)]
pub struct IsfTarget {
    pub name: String,
    /// Index into `passes` of the pass that renders this target.
    pub writer_pass: usize,
    /// Buffer persists across frames (double-buffered feedback).
    pub persistent: bool,
    pub width: SizeExpr,
    pub height: SizeExpr,
}

/// A minimal arithmetic evaluator for ISF size expressions: numbers,
/// `$WIDTH`/`$HEIGHT`, `+ - * /`, parentheses, and `floor`/`ceil`/`abs`/`min`/`max`.
fn eval_expr(src: &str, w: f64, h: f64) -> Option<f64> {
    let toks = tokenize_expr(src)?;
    let mut p = ExprParser { toks: &toks, pos: 0, w, h };
    let v = p.expr()?;
    (p.pos == p.toks.len()).then_some(v)
}

#[derive(Clone, Debug, PartialEq)]
enum ETok {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Comma,
}

fn tokenize_expr(s: &str) -> Option<Vec<ETok>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '+' => out.push(ETok::Plus),
            '-' => out.push(ETok::Minus),
            '*' => out.push(ETok::Star),
            '/' => out.push(ETok::Slash),
            '(' => out.push(ETok::LParen),
            ')' => out.push(ETok::RParen),
            ',' => out.push(ETok::Comma),
            '$' => {
                let start = i;
                i += 1;
                while i < b.len() && (b[i] as char).is_ascii_alphanumeric() {
                    i += 1;
                }
                out.push(ETok::Ident(s[start..i].to_string()));
                continue;
            }
            _ if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'.') {
                    i += 1;
                }
                out.push(ETok::Num(s[start..i].parse().ok()?));
                continue;
            }
            _ if c.is_ascii_alphabetic() => {
                let start = i;
                while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push(ETok::Ident(s[start..i].to_string()));
                continue;
            }
            _ => return None,
        }
        i += 1;
    }
    Some(out)
}

struct ExprParser<'a> {
    toks: &'a [ETok],
    pos: usize,
    w: f64,
    h: f64,
}

impl ExprParser<'_> {
    fn peek(&self) -> Option<&ETok> {
        self.toks.get(self.pos)
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(t) = self.peek() {
            match t {
                ETok::Plus => {
                    self.pos += 1;
                    v += self.term()?;
                }
                ETok::Minus => {
                    self.pos += 1;
                    v -= self.term()?;
                }
                _ => break,
            }
        }
        Some(v)
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.factor()?;
        while let Some(t) = self.peek() {
            match t {
                ETok::Star => {
                    self.pos += 1;
                    v *= self.factor()?;
                }
                ETok::Slash => {
                    self.pos += 1;
                    let d = self.factor()?;
                    if d == 0.0 {
                        return None;
                    }
                    v /= d;
                }
                _ => break,
            }
        }
        Some(v)
    }
    fn factor(&mut self) -> Option<f64> {
        match self.peek()?.clone() {
            ETok::Num(n) => {
                self.pos += 1;
                Some(n)
            }
            ETok::Minus => {
                self.pos += 1;
                Some(-self.factor()?)
            }
            ETok::LParen => {
                self.pos += 1;
                let v = self.expr()?;
                self.expect(&ETok::RParen)?;
                Some(v)
            }
            ETok::Ident(name) => {
                self.pos += 1;
                if matches!(self.peek(), Some(ETok::LParen)) {
                    self.pos += 1;
                    let mut args = vec![self.expr()?];
                    while matches!(self.peek(), Some(ETok::Comma)) {
                        self.pos += 1;
                        args.push(self.expr()?);
                    }
                    self.expect(&ETok::RParen)?;
                    apply_fn(&name, &args)
                } else {
                    self.var(&name)
                }
            }
            _ => None,
        }
    }
    fn var(&self, name: &str) -> Option<f64> {
        match name.trim_start_matches('$') {
            "WIDTH" => Some(self.w),
            "HEIGHT" => Some(self.h),
            _ => None,
        }
    }
    fn expect(&mut self, t: &ETok) -> Option<()> {
        (self.peek() == Some(t)).then(|| self.pos += 1)
    }
}

fn apply_fn(name: &str, args: &[f64]) -> Option<f64> {
    match (name, args) {
        ("floor", [x]) => Some(x.floor()),
        ("ceil", [x]) => Some(x.ceil()),
        ("abs", [x]) => Some(x.abs()),
        ("min", [a, b]) => Some(a.min(*b)),
        ("max", [a, b]) => Some(a.max(*b)),
        _ => None,
    }
}

/// One entry in the `PASSES` array.
#[derive(Clone, Debug, PartialEq)]
pub struct IsfPass {
    /// Named render target; `None` = the shader's output.
    pub target: Option<String>,
    /// Buffer persists across frames (feedback).
    pub persistent: bool,
    /// Float-precision buffer.
    pub float: bool,
    pub width: SizeExpr,
    pub height: SizeExpr,
}

/// The parsed ISF header.
#[derive(Clone, Debug, PartialEq)]
pub struct IsfHeader {
    pub inputs: Vec<IsfInput>,
    pub passes: Vec<IsfPass>,
    /// Imported images: `name -> path` (relative to the shader file).
    pub imported: Vec<(String, PathBuf)>,
}

impl IsfHeader {
    fn from_jval(v: &JVal) -> Self {
        let inputs = v
            .get("INPUTS")
            .and_then(JVal::as_arr)
            .map(|a| a.iter().filter_map(parse_input).collect())
            .unwrap_or_default();
        let passes = v
            .get("PASSES")
            .and_then(JVal::as_arr)
            .map(|a| a.iter().filter_map(parse_pass).collect())
            .unwrap_or_default();
        let imported = v
            .get("IMPORTED")
            .map(parse_imported)
            .unwrap_or_default();
        Self { inputs, passes, imported }
    }
}

fn parse_input(v: &JVal) -> Option<IsfInput> {
    let name = v.get("NAME").and_then(JVal::as_str)?.to_string();
    let ty = v.get("TYPE").and_then(JVal::as_str)?;
    let label = v.get("LABEL").and_then(JVal::as_str).map(str::to_string);
    let kind = match ty {
        "float" => IsfInputKind::Float {
            min: v.get("MIN").and_then(JVal::as_num).unwrap_or(0.0) as f32,
            max: v.get("MAX").and_then(JVal::as_num).unwrap_or(1.0) as f32,
            default: v.get("DEFAULT").and_then(JVal::as_num).unwrap_or(0.0) as f32,
        },
        "bool" => IsfInputKind::Bool {
            default: v.get("DEFAULT").and_then(JVal::as_bool).unwrap_or(false),
        },
        "long" => {
            let values = v
                .get("VALUES")
                .and_then(JVal::as_arr)
                .map(|a| a.iter().filter_map(|x| x.as_num().map(|n| n as i32)).collect())
                .unwrap_or_default();
            let labels = v
                .get("LABELS")
                .and_then(JVal::as_arr)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            IsfInputKind::Long {
                values,
                labels,
                default: v.get("DEFAULT").and_then(JVal::as_num).unwrap_or(0.0) as i32,
            }
        }
        "color" => IsfInputKind::Color {
            default: parse_vec4(v.get("DEFAULT")).unwrap_or([0.0, 0.0, 0.0, 1.0]),
        },
        "point2D" => IsfInputKind::Point2D {
            min: parse_vec2(v.get("MIN")).unwrap_or([0.0, 0.0]),
            max: parse_vec2(v.get("MAX")).unwrap_or([1.0, 1.0]),
            default: parse_vec2(v.get("DEFAULT")).unwrap_or([0.0, 0.0]),
        },
        "event" => IsfInputKind::Event,
        "image" | "audio" | "audioFFT" => IsfInputKind::Image,
        other => {
            log::warn!("ISF: unsupported input type {other:?} on {name:?}; skipping");
            return None;
        }
    };
    Some(IsfInput { name, label, kind })
}

fn parse_pass(v: &JVal) -> Option<IsfPass> {
    // A PASSES entry is always an object; skip anything malformed.
    if !matches!(v, JVal::Obj(_)) {
        return None;
    }
    Some(IsfPass {
        target: v.get("TARGET").and_then(JVal::as_str).map(str::to_string),
        persistent: v.get("PERSISTENT").and_then(JVal::as_bool).unwrap_or(false),
        float: v.get("FLOAT").and_then(JVal::as_bool).unwrap_or(false),
        width: parse_size(v.get("WIDTH")),
        height: parse_size(v.get("HEIGHT")),
    })
}

fn parse_size(v: Option<&JVal>) -> SizeExpr {
    match v {
        Some(JVal::Str(s)) => SizeExpr::Expr(s.clone()),
        Some(JVal::Num(n)) => SizeExpr::Expr(n.to_string()),
        _ => SizeExpr::Full,
    }
}

fn parse_imported(v: &JVal) -> Vec<(String, PathBuf)> {
    let JVal::Obj(entries) = v else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|(name, spec)| {
            let path = spec.get("PATH").and_then(JVal::as_str)?;
            Some((name.clone(), PathBuf::from(path)))
        })
        .collect()
}

fn parse_vec4(v: Option<&JVal>) -> Option<[f32; 4]> {
    let a = v?.as_arr()?;
    let mut out = [0.0f32; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = a.get(i).and_then(JVal::as_num).unwrap_or(0.0) as f32;
    }
    Some(out)
}

fn parse_vec2(v: Option<&JVal>) -> Option<[f32; 2]> {
    let a = v?.as_arr()?;
    Some([
        a.first().and_then(JVal::as_num).unwrap_or(0.0) as f32,
        a.get(1).and_then(JVal::as_num).unwrap_or(0.0) as f32,
    ])
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Detect and parse an ISF header from a shader source. Returns `None` if the
/// source has no leading `/* … */` JSON header with ISF-identifying keys — a
/// plain `.fs`/`.frag` is not ISF just by extension.
pub fn detect(src: &str) -> Option<IsfHeader> {
    let header_json = extract_header(src)?;
    let v = parse_json(&header_json).ok()?;
    if !matches!(v, JVal::Obj(_)) {
        return None;
    }
    // Require at least one ISF-identifying key so a stray leading comment that
    // happens to be a JSON object doesn't get mistaken for an ISF header.
    let isf_ish = ["ISFVSN", "INPUTS", "PASSES", "CATEGORIES", "DESCRIPTION"]
        .iter()
        .any(|k| v.get(k).is_some());
    if !isf_ish {
        return None;
    }
    Some(IsfHeader::from_jval(&v))
}

/// Extract the JSON text of the leading `/* … */` block comment, or `None` if
/// the source doesn't begin (modulo whitespace) with a block comment.
fn extract_header(src: &str) -> Option<String> {
    let start = src.find("/*")?;
    if !src[..start].trim().is_empty() {
        return None; // header must be the leading content
    }
    let rest = &src[start + 2..];
    let end = rest.find("*/")?;
    Some(rest[..end].to_string())
}

/// Replace the leading `/* … */` header with an equal number of newlines so the
/// GLSL body keeps its original line numbers (for error remapping), and return
/// the body. If there's no header comment the source is returned unchanged.
fn body_after_header(src: &str) -> String {
    let Some(start) = src.find("/*") else {
        return src.to_string();
    };
    let Some(rel_end) = src[start + 2..].find("*/") else {
        return src.to_string();
    };
    let end = start + 2 + rel_end + 2; // past the closing */
    let header = &src[..end];
    let newlines = header.bytes().filter(|&b| b == b'\n').count();
    let mut out = String::with_capacity(src.len());
    for _ in 0..newlines {
        out.push('\n');
    }
    out.push_str(&src[end..]);
    out
}

// ---------------------------------------------------------------------------
// std140 uniform layout + packing
// ---------------------------------------------------------------------------

/// The scalar shape of a UBO field, for std140 offset computation and packing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FieldShape {
    F32,
    I32,
    Vec2,
    Vec4,
}

impl FieldShape {
    fn align(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::Vec2 => 8,
            Self::Vec4 => 16,
        }
    }
    fn size(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::Vec2 => 8,
            Self::Vec4 => 16,
        }
    }
}

/// Fixed built-in fields at the head of every ISF parameter block (offsets are
/// std140 and hold regardless of the user inputs that follow):
/// `vec4 date@0 | vec2 renderSize@16 | int passIndex@24,frameIndex@28 | float timeDelta@32,pad@36`.
const ISF_BUILTIN_SIZE: usize = 40;

/// Per-frame (and per-pass) values for the built-in ISF uniforms. `render_size`
/// and `pass_index` vary per pass in a multi-pass shader.
#[derive(Clone, Copy, Debug, Default)]
pub struct IsfBuiltins {
    pub date: [f32; 4],
    pub render_size: [f32; 2],
    pub pass_index: i32,
    pub frame_index: i32,
    pub time_delta: f32,
}

/// The std140 layout of an ISF shader's `set=3` parameter block: fixed built-ins
/// followed by the user inputs, in declaration order.
#[derive(Clone, Debug)]
pub struct IsfUbo {
    /// std140 byte offset of each non-image input, in schema order.
    offsets: Vec<usize>,
    /// Total std140 size, rounded up to 16 bytes (min 16).
    size: usize,
}

impl IsfUbo {
    /// The buffer size in bytes (always a multiple of 16, at least 16).
    pub fn size(&self) -> usize {
        self.size
    }

    /// Build the layout from the input schema. Image inputs are skipped (they are
    /// textures, not UBO fields). Field order matches `inputs.iter().filter(non-image)`.
    fn from_inputs(inputs: &[IsfInput]) -> Self {
        let mut offsets = Vec::new();
        let mut cursor = ISF_BUILTIN_SIZE;
        for input in inputs {
            let shape = match &input.kind {
                IsfInputKind::Float { .. } => FieldShape::F32,
                IsfInputKind::Bool { .. } | IsfInputKind::Event => FieldShape::I32,
                IsfInputKind::Long { .. } => FieldShape::I32,
                IsfInputKind::Color { .. } => FieldShape::Vec4,
                IsfInputKind::Point2D { .. } => FieldShape::Vec2,
                IsfInputKind::Image => continue,
            };
            let offset = round_up(cursor, shape.align());
            offsets.push(offset);
            cursor = offset + shape.size();
        }
        let size = round_up(cursor.max(16), 16);
        Self { offsets, size }
    }

    /// Pack the parameter buffer. `values` must align 1:1 with the non-image
    /// inputs, in schema order; each is written at its std140 offset. Built-ins
    /// are written at their fixed offsets.
    pub fn pack(&self, values: &[IsfValue], b: &IsfBuiltins) -> Vec<u8> {
        let mut buf = vec![0u8; self.size];
        write_f32x4(&mut buf, 0, b.date);
        write_f32(&mut buf, 16, b.render_size[0]);
        write_f32(&mut buf, 20, b.render_size[1]);
        write_i32(&mut buf, 24, b.pass_index);
        write_i32(&mut buf, 28, b.frame_index);
        write_f32(&mut buf, 32, b.time_delta);
        // offset 36: pad
        for (&o, val) in self.offsets.iter().zip(values.iter()) {
            match val {
                IsfValue::Float(f) => write_f32(&mut buf, o, *f),
                IsfValue::Bool(v) => write_i32(&mut buf, o, i32::from(*v)),
                IsfValue::Long(i) => write_i32(&mut buf, o, *i),
                IsfValue::Color(c) => write_f32x4(&mut buf, o, *c),
                IsfValue::Point2D(p) => {
                    write_f32(&mut buf, o, p[0]);
                    write_f32(&mut buf, o + 4, p[1]);
                }
            }
        }
        buf
    }
}

fn round_up(v: usize, align: usize) -> usize {
    v.div_ceil(align) * align
}

fn write_f32(buf: &mut [u8], off: usize, v: f32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn write_i32(buf: &mut [u8], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn write_f32x4(buf: &mut [u8], off: usize, v: [f32; 4]) {
    for (i, x) in v.iter().enumerate() {
        write_f32(buf, off + i * 4, *x);
    }
}

// ---------------------------------------------------------------------------
// Transpile
// ---------------------------------------------------------------------------

/// A transpiled ISF program: the combined GLSL source ready for naga, the line
/// offset for error remapping, the input schema, and the parameter layout.
pub struct IsfProgram {
    /// Full source: base `PREAMBLE` + ISF additions + body.
    pub combined: String,
    /// Lines preceding the user body (for naga error-line remapping).
    pub preamble_lines: u32,
    pub inputs: Vec<IsfInput>,
    pub passes: Vec<IsfPass>,
    /// Named render targets (from `PASSES[].TARGET`), in binding order. Each is
    /// bound at `set=3, binding = 2 + index`.
    pub targets: Vec<IsfTarget>,
    pub ubo: IsfUbo,
}

impl IsfProgram {
    /// The non-image inputs in schema order — the fields the parameter buffer
    /// holds, and the order [`IsfUbo::pack`] expects `values` in.
    pub fn scalar_inputs(&self) -> impl Iterator<Item = &IsfInput> {
        self.inputs.iter().filter(|i| !i.kind.is_image())
    }
}

/// Derive the named targets from the pass list, in first-appearance order. Each
/// `TARGET` name is written by exactly one pass (the first that names it).
fn targets_of(passes: &[IsfPass]) -> Vec<IsfTarget> {
    let mut out: Vec<IsfTarget> = Vec::new();
    for (i, p) in passes.iter().enumerate() {
        if let Some(name) = &p.target {
            if out.iter().any(|t| t.name == *name) {
                continue;
            }
            out.push(IsfTarget {
                name: name.clone(),
                writer_pass: i,
                persistent: p.persistent,
                width: p.width.clone(),
                height: p.height.clone(),
            });
        }
    }
    out
}

/// Transpile an ISF shader (header + GLSL body) onto vidiotic's uniform
/// contract. Returns `None` if `src` is not an ISF shader.
pub fn transpile(src: &str) -> Option<IsfProgram> {
    let header = detect(src)?;
    let body = body_after_header(src);
    let ubo = IsfUbo::from_inputs(&header.inputs);
    let targets = targets_of(&header.passes);

    let additions = generate_additions(&header, &targets);

    let mut combined = String::new();
    combined.push_str(PREAMBLE);
    debug_assert!(combined.ends_with('\n'));
    combined.push_str(&additions);
    debug_assert!(combined.ends_with('\n'));
    let preamble_lines = combined.lines().count() as u32;
    combined.push_str(&strip_body(&body));
    combined.push('\n');

    Some(IsfProgram {
        combined,
        preamble_lines,
        inputs: header.inputs,
        passes: header.passes,
        targets,
        ubo,
    })
}

/// The ISF prelude appended after the base `PREAMBLE`: built-in aliases, the
/// `IMG_*` accessors, the parameter UBO, per-input `#define`s, and the named
/// pass-target samplers.
fn generate_additions(header: &IsfHeader, targets: &[IsfTarget]) -> String {
    let mut s = String::new();
    s.push_str("// ---- ISF compatibility (auto-generated) ----\n");
    // Built-in aliases. RENDERSIZE is per-pass (from the ISF UBO so a downscaled
    // pass reports its own size); TIME reuses the base Globals block.
    s.push_str("#define RENDERSIZE uISF.renderSize\n");
    s.push_str("#define TIME time\n");
    s.push_str("#define PASSINDEX  uISF.passIndex\n");
    s.push_str("#define FRAMEINDEX uISF.frameIndex\n");
    s.push_str("#define TIMEDELTA  uISF.timeDelta\n");
    s.push_str("#define DATE       uISF.date\n");
    s.push_str("#define gl_FragColor FragColor\n");
    // fragTexCoord is already (0,0)=bottom-left, matching ISF's convention.
    s.push_str("#define isf_FragNormCoord fragTexCoord\n");
    s.push_str("#define vv_FragNormCoord fragTexCoord\n");
    // inputImage = the effect-chain stage input (set=2). Extra image inputs
    // alias to it too until multi-image support (Phase 3).
    s.push_str("#define inputImage sampler2D(inputTex, inputSmp)\n");
    // IMG_* accessors. Our stage buffers are stored top-down, while ISF norm
    // coords are bottom-left, so sampling flips Y.
    s.push_str("#define IMG_NORM_PIXEL(img, nc) texture((img), vec2((nc).x, 1.0 - (nc).y))\n");
    s.push_str(
        "#define IMG_PIXEL(img, pc) texture((img), vec2((pc).x, RENDERSIZE.y - (pc).y) / RENDERSIZE)\n",
    );
    s.push_str("#define IMG_SIZE(img) vec2(textureSize((img), 0))\n");
    s.push_str("#define IMG_THIS_NORM_PIXEL(img) IMG_NORM_PIXEL(img, isf_FragNormCoord)\n");
    s.push_str("#define IMG_THIS_PIXEL(img) IMG_THIS_NORM_PIXEL(img)\n");

    // Parameter UBO at set=3. Field order/offsets mirror `IsfUbo::pack`.
    s.push_str("layout(set = 3, binding = 0) uniform ISFParams {\n");
    s.push_str("    vec4  date;\n");
    s.push_str("    vec2  renderSize;\n");
    s.push_str("    int   passIndex;\n");
    s.push_str("    int   frameIndex;\n");
    s.push_str("    float timeDelta;\n");
    s.push_str("    float _isf_pad;\n");
    for input in &header.inputs {
        match &input.kind {
            IsfInputKind::Float { .. } => {
                s.push_str(&format!("    float in_{};\n", input.name));
            }
            IsfInputKind::Bool { .. } | IsfInputKind::Event | IsfInputKind::Long { .. } => {
                s.push_str(&format!("    int in_{};\n", input.name));
            }
            IsfInputKind::Color { .. } => {
                s.push_str(&format!("    vec4 in_{};\n", input.name));
            }
            IsfInputKind::Point2D { .. } => {
                s.push_str(&format!("    vec2 in_{};\n", input.name));
            }
            IsfInputKind::Image => {}
        }
    }
    s.push_str("} uISF;\n");

    // Named pass targets: one texture each at set=3 binding 2.., sharing one
    // sampler at binding 1. The shader samples them by name via the IMG_* macros.
    if !targets.is_empty() {
        s.push_str("layout(set = 3, binding = 1) uniform sampler isfSmp;\n");
        for (i, t) in targets.iter().enumerate() {
            let b = i + 2;
            s.push_str(&format!(
                "layout(set = 3, binding = {b}) uniform texture2D {}Tex;\n",
                t.name
            ));
        }
        for t in targets {
            s.push_str(&format!("#define {0} sampler2D({0}Tex, isfSmp)\n", t.name));
        }
    }

    // Per-input #defines mapping the bare ISF name onto the UBO field / sampler.
    for input in &header.inputs {
        let n = &input.name;
        match &input.kind {
            IsfInputKind::Bool { .. } | IsfInputKind::Event => {
                s.push_str(&format!("#define {n} (uISF.in_{n} != 0)\n"));
            }
            IsfInputKind::Float { .. }
            | IsfInputKind::Long { .. }
            | IsfInputKind::Color { .. }
            | IsfInputKind::Point2D { .. } => {
                s.push_str(&format!("#define {n} uISF.in_{n}\n"));
            }
            IsfInputKind::Image => {
                // Extra image inputs alias to the stage input for now (Phase 3).
                s.push_str(&format!("#define {n} sampler2D(inputTex, inputSmp)\n"));
            }
        }
    }
    s.push_str("// ---- end ISF compatibility ----\n");
    s
}

/// Blank `#version` / `precision` lines in the ISF body (naga uses the base
/// preamble's `#version 450`), preserving line numbers.
fn strip_body(body: &str) -> String {
    let mut out: Vec<String> = Vec::with_capacity(body.lines().count());
    for raw in body.lines() {
        let t = raw.trim_start();
        if t.starts_with("#version") || t.starts_with("precision ") {
            out.push(String::new());
        } else {
            out.push(raw.to_string());
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalar_json() {
        assert_eq!(parse_json("42").unwrap(), JVal::Num(42.0));
        assert_eq!(parse_json("true").unwrap(), JVal::Bool(true));
        assert_eq!(parse_json("\"hi\"").unwrap(), JVal::Str("hi".into()));
        assert_eq!(parse_json("null").unwrap(), JVal::Null);
    }

    #[test]
    fn parses_nested_json() {
        let v = parse_json(r#"{"a": [1, 2, 3], "b": {"c": true}, "d": -0.5}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_arr().unwrap().len(), 3);
        assert_eq!(v.get("b").unwrap().get("c").unwrap(), &JVal::Bool(true));
        assert_eq!(v.get("d").unwrap().as_num(), Some(-0.5));
    }

    const FLOAT_ISF: &str = r#"/*{
        "DESCRIPTION": "levels",
        "ISFVSN": "2.0",
        "INPUTS": [
            { "NAME": "gain", "TYPE": "float", "MIN": 0.0, "MAX": 2.0, "DEFAULT": 1.0 },
            { "NAME": "invert", "TYPE": "bool", "DEFAULT": false }
        ]
    }*/
void main() {
    vec4 c = IMG_THIS_NORM_PIXEL(inputImage) * gain;
    if (invert) c.rgb = 1.0 - c.rgb;
    gl_FragColor = c;
}
"#;

    #[test]
    fn detects_and_models_float_isf() {
        let h = detect(FLOAT_ISF).expect("is ISF");
        assert_eq!(h.inputs.len(), 2);
        assert_eq!(h.inputs[0].name, "gain");
        assert!(matches!(
            h.inputs[0].kind,
            IsfInputKind::Float { min, max, default } if min == 0.0 && max == 2.0 && default == 1.0
        ));
        assert!(matches!(h.inputs[1].kind, IsfInputKind::Bool { default: false }));
    }

    #[test]
    fn plain_glsl_is_not_isf() {
        assert!(detect("void main(){ FragColor = vec4(1.0); }").is_none());
        // A leading comment that isn't an ISF object must not match.
        assert!(detect("/* just a note */\nvoid main(){}").is_none());
    }

    #[test]
    fn parses_long_and_color_and_point() {
        let src = r#"/*{
            "INPUTS": [
                { "NAME": "mode", "TYPE": "long", "VALUES": [0,1,2], "LABELS": ["a","b","c"], "DEFAULT": 1 },
                { "NAME": "tint", "TYPE": "color", "DEFAULT": [0.1, 0.2, 0.3, 1.0] },
                { "NAME": "center", "TYPE": "point2D", "DEFAULT": [0.5, 0.5], "MAX": [1.0, 1.0] }
            ]
        }*/
void main(){ gl_FragColor = tint; }
"#;
        let h = detect(src).unwrap();
        match &h.inputs[0].kind {
            IsfInputKind::Long { values, labels, default } => {
                assert_eq!(values, &[0, 1, 2]);
                assert_eq!(labels, &["a", "b", "c"]);
                assert_eq!(*default, 1);
            }
            other => panic!("expected long, got {other:?}"),
        }
        assert!(matches!(h.inputs[1].kind, IsfInputKind::Color { default } if default == [0.1, 0.2, 0.3, 1.0]));
        assert!(matches!(h.inputs[2].kind, IsfInputKind::Point2D { .. }));
    }

    #[test]
    fn ubo_layout_std140_offsets() {
        // Built-ins occupy 0..40; gain(float)@40, invert(bool→int)@44; size 48.
        let h = detect(FLOAT_ISF).unwrap();
        let ubo = IsfUbo::from_inputs(&h.inputs);
        assert_eq!(ubo.offsets, vec![40, 44]);
        assert_eq!(ubo.size, 48);
    }

    #[test]
    fn ubo_layout_aligns_vec4_after_float() {
        let src = r#"/*{
            "INPUTS": [
                { "NAME": "a", "TYPE": "float", "DEFAULT": 0.0 },
                { "NAME": "b", "TYPE": "color", "DEFAULT": [0,0,0,1] }
            ]
        }*/
void main(){ gl_FragColor = b; }
"#;
        let h = detect(src).unwrap();
        let ubo = IsfUbo::from_inputs(&h.inputs);
        assert_eq!(ubo.offsets, vec![40, 48]); // float a @40, vec4 b aligned to 48
        assert_eq!(ubo.size, 64);
    }

    #[test]
    fn pack_writes_builtins_and_values() {
        let h = detect(FLOAT_ISF).unwrap();
        let ubo = IsfUbo::from_inputs(&h.inputs);
        let vals = vec![IsfValue::Float(1.5), IsfValue::Bool(true)];
        let b = IsfBuiltins {
            pass_index: 3,
            frame_index: 7,
            time_delta: 0.016,
            date: [1.0, 2.0, 3.0, 4.0],
            render_size: [640.0, 480.0],
        };
        let buf = ubo.pack(&vals, &b);
        assert_eq!(buf.len(), 48);
        assert_eq!(f32::from_le_bytes(buf[0..4].try_into().unwrap()), 1.0); // date.x
        assert_eq!(f32::from_le_bytes(buf[16..20].try_into().unwrap()), 640.0); // renderSize.x
        assert_eq!(i32::from_le_bytes(buf[24..28].try_into().unwrap()), 3); // passIndex
        assert_eq!(i32::from_le_bytes(buf[28..32].try_into().unwrap()), 7); // frameIndex
        assert_eq!(f32::from_le_bytes(buf[40..44].try_into().unwrap()), 1.5); // gain
        assert_eq!(i32::from_le_bytes(buf[44..48].try_into().unwrap()), 1); // invert
    }

    #[test]
    fn eval_size_exprs() {
        assert_eq!(eval_size(&SizeExpr::Full, 1920, 1080, 1920), 1920);
        assert_eq!(eval_size(&SizeExpr::Expr("$WIDTH/2.0".into()), 1920, 1080, 1920), 960);
        assert_eq!(eval_size(&SizeExpr::Expr("floor($WIDTH/4.0)".into()), 1920, 1080, 1920), 480);
        assert_eq!(eval_size(&SizeExpr::Expr("$HEIGHT*0.25".into()), 1920, 1080, 1080), 270);
        assert_eq!(eval_size(&SizeExpr::Expr("256".into()), 1920, 1080, 1920), 256);
        assert_eq!(eval_size(&SizeExpr::Expr("max($WIDTH/8.0, 4.0)".into()), 1920, 1080, 1920), 240);
        // unparseable falls back to default
        assert_eq!(eval_size(&SizeExpr::Expr("$BOGUS +".into()), 1920, 1080, 999), 999);
    }

    #[test]
    fn transpile_multipass_targets() {
        let src = r#"/*{
            "PASSES": [
                { "TARGET": "bufA", "PERSISTENT": true, "WIDTH": "$WIDTH/2.0" },
                {}
            ]
        }*/
void main() { gl_FragColor = IMG_NORM_PIXEL(bufA, isf_FragNormCoord); }
"#;
        let prog = transpile(src).unwrap();
        assert_eq!(prog.targets.len(), 1);
        assert_eq!(prog.targets[0].name, "bufA");
        assert!(prog.targets[0].persistent);
        assert_eq!(prog.targets[0].writer_pass, 0);
        assert_eq!(prog.passes.len(), 2);
        assert!(prog.combined.contains("uniform texture2D bufATex"));
        assert!(prog.combined.contains("#define bufA sampler2D(bufATex, isfSmp)"));
    }

    #[test]
    fn transpile_preserves_body_line_numbers() {
        let prog = transpile(FLOAT_ISF).expect("is ISF");
        // The body's `void main` is on line 8 of the original source.
        let lines: Vec<&str> = prog.combined.lines().collect();
        let main_idx = lines.iter().position(|l| l.contains("void main")).unwrap() as u32;
        assert_eq!(main_idx + 1 - prog.preamble_lines, 9);
    }
}
