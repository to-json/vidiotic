//! User fragment-shader loading: preprocess an OpenGL-3.3-style `.frag` (or a
//! `.wgsl`) into a form the app-injected uniform contract expects, then parse +
//! validate it with naga so compile errors are captured (never a wgpu panic).
//!
//! The preprocessing keeps user line numbers stable: every strip replaces the
//! matched line's content in place (blanking it) rather than deleting the line,
//! so a naga error at combined-line L maps to user-line `L - preamble_lines`.

use std::path::Path;

pub const PREAMBLE: &str = include_str!("../shaders/preamble.frag");

/// Uniform names the preamble already provides — user redeclarations are stripped.
const KNOWN_UNIFORMS: &[&str] = &[
    "time",
    "lvl",
    "mousePos",
    "mouse",
    "resolution",
    "beat",
    "bpm",
    "bar_phase",
    "phrase_phase",
    "freqs1",
    "iTime",
    "iResolution",
];

const GLSL_TYPES: &[&str] = &["float", "int", "vec2", "vec3", "vec4"];

/// Varyings the built-in vertex shader / preamble provide.
const KNOWN_IN_VARYINGS: &[&str] = &["fragTexCoord", "fragColor"];

#[derive(Debug)]
pub enum ShaderError {
    /// Read error (file vanished / mid-write). Caller keeps last-good pipeline.
    Io(String),
    /// naga GLSL/WGSL parse failure. `line` is 1-based in the *user* file when known.
    Parse { msg: String, line: Option<u32> },
    /// naga validation failure.
    Validation { msg: String, line: Option<u32> },
    /// wgpu pipeline-creation validation (interface mismatch).
    Pipeline(String),
}

impl std::fmt::Display for ShaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShaderError::Io(s) => write!(f, "read error: {s}"),
            ShaderError::Parse { msg, line } => match line {
                Some(l) => write!(f, "parse error (line {l}):\n{msg}"),
                None => write!(f, "parse error:\n{msg}"),
            },
            ShaderError::Validation { msg, line } => match line {
                Some(l) => write!(f, "validation error (line {l}):\n{msg}"),
                None => write!(f, "validation error:\n{msg}"),
            },
            ShaderError::Pipeline(s) => write!(f, "pipeline error:\n{s}"),
        }
    }
}

impl std::error::Error for ShaderError {}

pub enum ShaderLang {
    Glsl,
    Wgsl,
}

pub fn lang_of(path: &Path) -> ShaderLang {
    match path.extension().and_then(|e| e.to_str()) {
        Some("wgsl") => ShaderLang::Wgsl,
        _ => ShaderLang::Glsl, // .frag / .fs / .glsl / anything else
    }
}

/// Result of GLSL preprocessing: the full source handed to naga, plus how many
/// lines precede the user's own source (for error-line remapping).
pub struct Preprocessed {
    pub combined: String,
    pub preamble_lines: u32,
}

/// Return the leading-identifier tokens of a line, in order, skipping symbols.
/// e.g. "uniform float freqs1[21];" -> ["uniform","float","freqs1","21"]
fn ident_words(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_alphanumeric() || c == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
            {
                i += 1;
            }
            out.push(&line[start..i]);
        } else {
            i += 1;
        }
    }
    out
}

/// Strip a leading `layout(...)` qualifier from a trimmed line, returning the
/// remainder trimmed. If there's no layout prefix, returns the input unchanged.
fn strip_layout_prefix(s: &str) -> &str {
    let t = s.trim_start();
    if let Some(rest) = t.strip_prefix("layout") {
        let rest = rest.trim_start();
        if rest.starts_with('(') {
            // find matching close paren
            let mut depth = 0i32;
            for (idx, ch) in rest.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            return rest[idx + 1..].trim_start();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    t
}

/// True if the (trimmed) line declares one of the preamble-provided uniforms.
fn is_known_uniform_decl(line: &str) -> bool {
    let w = ident_words(line);
    w.len() >= 3
        && w[0] == "uniform"
        && GLSL_TYPES.contains(&w[1])
        && KNOWN_UNIFORMS.contains(&w[2])
}

/// If the (trimmed) line is `#version ...` or `precision ...;`, blank it.
fn is_version_or_precision(line_trim: &str) -> bool {
    line_trim.starts_with("#version") || {
        let w = ident_words(line_trim);
        w.first() == Some(&"precision")
    }
}

/// Analyze an `in`/`out` varying declaration. Returns:
///   ("in",  name) for a stripped known input varying
///   ("out", name) for a stripped output color
fn varying_decl(line: &str) -> Option<(&'static str, String)> {
    let body = strip_layout_prefix(line);
    let w = ident_words(body);
    // in <type> <name> ;
    if w.len() >= 3 && w[0] == "in" && GLSL_TYPES.contains(&w[1]) {
        if KNOWN_IN_VARYINGS.contains(&w[2]) {
            return Some(("in", w[2].to_string()));
        }
        return None;
    }
    // out vec4 <name> ;
    if w.len() >= 3 && w[0] == "out" && w[1] == "vec4" {
        return Some(("out", w[2].to_string()));
    }
    None
}

/// Replace every `freqs1[EXPR]` with `fftBand((EXPR))`, tracking bracket depth so
/// nested indexing works. Operates on one line (no newline count change).
fn rewrite_freqs1(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        // match identifier "freqs1" with word boundaries
        if line[i..].starts_with("freqs1")
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
        {
            let after = i + "freqs1".len();
            // require '[' possibly after whitespace, and NOT part of a longer ident
            let next_is_ident = after < bytes.len() && is_ident_byte(bytes[after]);
            if !next_is_ident {
                let mut j = after;
                while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'[' {
                    // scan to matching ]
                    let mut depth = 0i32;
                    let mut k = j;
                    while k < bytes.len() {
                        match bytes[k] {
                            b'[' => depth += 1,
                            b']' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                    if k < bytes.len() {
                        let inner = &line[j + 1..k];
                        out.push_str("fftBand((");
                        out.push_str(inner);
                        out.push_str("))");
                        i = k + 1;
                        continue;
                    }
                }
            }
        }
        // default: copy this byte (as char boundary safe: advance by char)
        let ch = line[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Preprocess a GLSL user shader against the app uniform contract.
pub fn preprocess_glsl(user_src: &str) -> Preprocessed {
    let mut generated_defines = String::new();
    let mut saw_frag_color_input = false;

    let mut processed_lines: Vec<String> = Vec::with_capacity(user_src.lines().count());
    for raw in user_src.lines() {
        let trimmed = raw.trim_start();

        if is_version_or_precision(trimmed) {
            processed_lines.push(String::new());
            continue;
        }
        if is_known_uniform_decl(trimmed) {
            processed_lines.push(String::new());
            continue;
        }
        if let Some((kind, name)) = varying_decl(trimmed) {
            match kind {
                "in" => {
                    if name == "fragColor" {
                        saw_frag_color_input = true;
                    }
                    processed_lines.push(String::new());
                    continue;
                }
                "out" => {
                    if name != "FragColor" {
                        generated_defines
                            .push_str(&format!("#define {name} FragColor\n"));
                    }
                    processed_lines.push(String::new());
                    continue;
                }
                _ => {}
            }
        }
        // freqs1[...] rewrite on surviving lines
        processed_lines.push(rewrite_freqs1(raw));
    }

    // If the user declared `in vec4 fragColor;` (raylib vertex color, unused) and
    // did NOT repurpose fragColor as their output name, give it a constant value.
    if saw_frag_color_input && !generated_defines.contains("#define fragColor ") {
        generated_defines.push_str("#define fragColor vec4(1.0, 1.0, 1.0, 1.0)\n");
    }

    // PREAMBLE ends in a newline (and each generated define does too), so the
    // prefix already terminates a line; the user's first line follows directly.
    // preamble_lines therefore equals the number of lines preceding user line 1,
    // which is exactly what remap_line subtracts.
    let mut combined = String::new();
    combined.push_str(PREAMBLE);
    combined.push_str(&generated_defines);
    debug_assert!(combined.ends_with('\n'));
    let preamble_lines = combined.lines().count() as u32;
    combined.push_str(&processed_lines.join("\n"));
    combined.push('\n');

    Preprocessed {
        combined,
        preamble_lines,
    }
}

/// Parse + validate a GLSL user shader, returning a validated naga module ready
/// to hand to wgpu via `ShaderSource::Naga`.
pub fn compile_glsl_to_module(user_src: &str) -> Result<naga::Module, ShaderError> {
    let pre = preprocess_glsl(user_src);
    parse_and_validate_glsl(&pre)
}

fn parse_and_validate_glsl(pre: &Preprocessed) -> Result<naga::Module, ShaderError> {
    let mut frontend = naga::front::glsl::Frontend::default();
    let options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    let module = frontend.parse(&options, &pre.combined).map_err(|errs| {
        let line = errs
            .errors
            .first()
            .map(|e| e.meta.location(&pre.combined))
            .map(|loc| remap_line(loc.line_number, pre.preamble_lines));
        ShaderError::Parse {
            msg: errs.emit_to_string(&pre.combined),
            line,
        }
    })?;

    validate(&module).map_err(|e| {
        let line = e
            .spans()
            .next()
            .map(|(span, _)| span.location(&pre.combined).line_number)
            .map(|l| remap_line(l, pre.preamble_lines));
        ShaderError::Validation {
            msg: format!("{e:?}"),
            line,
        }
    })?;

    Ok(module)
}

/// Parse + validate a WGSL user shader (no preamble; documented binding contract).
pub fn compile_wgsl_to_module(user_src: &str) -> Result<naga::Module, ShaderError> {
    let module = naga::front::wgsl::parse_str(user_src).map_err(|e| {
        let loc = e.location(user_src);
        ShaderError::Parse {
            msg: e.emit_to_string(user_src),
            line: loc.map(|l| l.line_number),
        }
    })?;
    validate(&module).map_err(|e| {
        let line = e
            .spans()
            .next()
            .map(|(span, _)| span.location(user_src).line_number);
        ShaderError::Validation {
            msg: format!("{e:?}"),
            line,
        }
    })?;
    Ok(module)
}

fn validate(
    module: &naga::Module,
) -> Result<naga::valid::ModuleInfo, naga::WithSpan<naga::valid::ValidationError>> {
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(module)
}

fn remap_line(combined_line: u32, preamble_lines: u32) -> u32 {
    combined_line.saturating_sub(preamble_lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_version_and_uniforms() {
        let src = "#version 330 core\nuniform float time;\nuniform float freqs1[21];\nvoid main(){ FragColor = vec4(time); }\n";
        let pre = preprocess_glsl(src);
        // stripped lines are blank, so these tokens must not survive as declarations
        assert!(!pre.combined.contains("#version 330"));
        assert!(!pre.combined.contains("uniform float time;"));
        // preamble still declares Globals { ... time ... }
        assert!(pre.combined.contains("uniform Globals"));
    }

    #[test]
    fn rewrites_freqs1_indexing() {
        assert_eq!(rewrite_freqs1("x = freqs1[band];"), "x = fftBand((band));");
        assert_eq!(rewrite_freqs1("y = freqs1[1] + freqs1[4];"), "y = fftBand((1)) + fftBand((4));");
        // nested
        assert_eq!(rewrite_freqs1("freqs1[clamp(i,0,20)]"), "fftBand((clamp(i,0,20)))");
        // not a false match on a longer identifier
        assert_eq!(rewrite_freqs1("freqs1x[0]"), "freqs1x[0]");
    }

    #[test]
    fn line_count_preserved_by_strips() {
        let src = "#version 330 core\nuniform float time;\nvoid main(){}\n";
        let pre = preprocess_glsl(src);
        // user portion begins after preamble_lines; the user's `void main` was on
        // line 3 of the original, so in combined it is at preamble_lines + 3.
        let combined_lines: Vec<&str> = pre.combined.lines().collect();
        let main_idx = combined_lines
            .iter()
            .position(|l| l.contains("void main"))
            .unwrap() as u32;
        // combined line is 1-based main_idx+1; remap should give 3
        assert_eq!(remap_line(main_idx + 1, pre.preamble_lines), 3);
    }
}
