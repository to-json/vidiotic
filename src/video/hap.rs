//! HAP frame parsing: a demuxed HAP packet is one frame = a sequence of
//! sections. Each section header is a 24-bit little-endian size + 1 type byte
//! (or, if the 24-bit size is zero, an 8-byte header with a 32-bit size). The
//! type byte's low nibble is the texture format, high nibble the second-stage
//! compressor (none / Snappy / chunked). This module is wgpu-free so it parses
//! and unit-tests without a GPU; `render.rs` maps `HapTextureFormat` to a wgpu
//! `TextureFormat`.
//!
//! Reference: <https://github.com/Vidvox/hap/blob/master/documentation/HapVideoDRAFT.md>

/// Second-stage compressor (high nibble of the section type byte).
const COMP_NONE: u8 = 0xA0;
const COMP_SNAPPY: u8 = 0xB0;
const COMP_COMPLEX: u8 = 0xC0; // chunked

/// Texture format (low nibble of the section type byte).
const FMT_RGB_DXT1: u8 = 0x0B; // BC1
const FMT_RGBA_DXT5: u8 = 0x0E; // BC3
const FMT_YCOCG_DXT5: u8 = 0x0F; // scaled-YCoCg BC3
const FMT_RGTC1: u8 = 0x01; // BC4 (single-channel; HAP alpha plane / HapA)
const FMT_BC7: u8 = 0x0C;

/// Decode-instructions child section types (inside a chunked section).
const ST_DECODE_INSTRUCTIONS: u8 = 0x01;
const ST_COMPRESSOR_TABLE: u8 = 0x02;
const ST_SIZE_TABLE: u8 = 0x03;
const ST_OFFSET_TABLE: u8 = 0x04;

/// Per-chunk second-stage compressor byte in the compressor table.
const CHUNK_COMP_NONE: u8 = 0x0A;
const CHUNK_COMP_SNAPPY: u8 = 0x0B;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HapTextureFormat {
    Bc1,      // Hap        — RGB DXT1
    Bc3,      // Hap Alpha  — RGBA DXT5
    Bc3YCoCg, // Hap Q      — scaled YCoCg DXT5 (needs shader unswizzle)
    Bc4,      // Hap Alpha-Only / the alpha plane of HapM
    Bc7,      // Hap R
}

impl HapTextureFormat {
    /// Bytes per 4x4 block for the compressed format.
    pub fn block_bytes(self) -> u32 {
        match self {
            Self::Bc1 | Self::Bc4 => 8,
            Self::Bc3 | Self::Bc3YCoCg | Self::Bc7 => 16,
        }
    }

    /// `videoMode` uniform value for the composite shader's `video()` helper.
    /// Overridden to 2 by the caller when a `HapM` alpha plane accompanies it.
    pub fn video_mode(self) -> i32 {
        match self {
            Self::Bc3YCoCg => 1,
            Self::Bc4 => 3,
            _ => 0,
        }
    }

    fn from_nibble(low: u8) -> Result<Self, HapErr> {
        match low {
            FMT_RGB_DXT1 => Ok(Self::Bc1),
            FMT_RGBA_DXT5 => Ok(Self::Bc3),
            FMT_YCOCG_DXT5 => Ok(Self::Bc3YCoCg),
            FMT_RGTC1 => Ok(Self::Bc4),
            FMT_BC7 => Ok(Self::Bc7),
            other => Err(HapErr::UnknownFormat(other)),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HapErr {
    Truncated,
    UnknownFormat(u8),
    UnknownCompressor(u8),
    BadChunkTables,
    Snappy,
    /// `HapM` second texture wasn't the expected BC4 alpha plane.
    UnexpectedAlpha,
}

impl std::fmt::Display for HapErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "HAP frame truncated"),
            Self::UnknownFormat(b) => write!(f, "unknown HAP texture format nibble {b:#04x}"),
            Self::UnknownCompressor(b) => write!(f, "unknown HAP compressor {b:#04x}"),
            Self::BadChunkTables => write!(f, "malformed HAP chunk tables"),
            Self::Snappy => write!(f, "snappy decompression failed"),
            Self::UnexpectedAlpha => write!(f, "HapM alpha plane was not BC4"),
        }
    }
}
impl std::error::Error for HapErr {}

/// Metadata returned alongside the decoded texture bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HapMeta {
    pub format: HapTextureFormat,
    /// True when an alpha plane (BC4) was decoded into the alpha buffer (`HapM`).
    pub has_alpha: bool,
    /// The `videoMode` to feed the shader (accounts for the alpha plane).
    pub video_mode: i32,
}

/// Parse a section header at the start of `b`.
/// Returns (`payload_len`, `type_byte`, `header_len`).
fn read_section(b: &[u8]) -> Result<(usize, u8, usize), HapErr> {
    if b.len() < 4 {
        return Err(HapErr::Truncated);
    }
    let s24 = u32::from_le_bytes([b[0], b[1], b[2], 0]);
    let ty = b[3];
    if s24 != 0 {
        Ok((s24 as usize, ty, 4))
    } else {
        if b.len() < 8 {
            return Err(HapErr::Truncated);
        }
        Ok((u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize, ty, 8))
    }
}

/// Decompress one complete texture section (header included) into `out`.
/// Returns the texture format from the section's low nibble.
fn decode_texture_section(b: &[u8], out: &mut Vec<u8>) -> Result<HapTextureFormat, HapErr> {
    let (len, ty, hdr) = read_section(b)?;
    let payload = b.get(hdr..hdr + len).ok_or(HapErr::Truncated)?;
    let format = HapTextureFormat::from_nibble(ty & 0x0F)?;

    out.clear();
    match ty & 0xF0 {
        COMP_NONE => out.extend_from_slice(payload),
        COMP_SNAPPY => snappy_into(payload, out)?,
        COMP_COMPLEX => decode_chunked(payload, out)?,
        other => return Err(HapErr::UnknownCompressor(other)),
    }
    Ok(format)
}

fn snappy_into(input: &[u8], out: &mut Vec<u8>) -> Result<(), HapErr> {
    let n = snap::raw::decompress_len(input).map_err(|_| HapErr::Snappy)?;
    out.resize(n, 0);
    let written = snap::raw::Decoder::new()
        .decompress(input, out)
        .map_err(|_| HapErr::Snappy)?;
    out.truncate(written);
    Ok(())
}

/// Decode a chunked (`COMP_COMPLEX`) section body: a Decode Instructions Container
/// followed by the frame data. Reassembles chunks (each raw or Snappy) into `out`.
fn decode_chunked(body: &[u8], out: &mut Vec<u8>) -> Result<(), HapErr> {
    // First child section must be the Decode Instructions Container.
    let (di_len, di_ty, di_hdr) = read_section(body)?;
    if di_ty != ST_DECODE_INSTRUCTIONS {
        return Err(HapErr::BadChunkTables);
    }
    let di = body.get(di_hdr..di_hdr + di_len).ok_or(HapErr::Truncated)?;
    let frame_data = body.get(di_hdr + di_len..).ok_or(HapErr::Truncated)?;

    let mut compressors: Option<&[u8]> = None;
    let mut sizes: Option<&[u8]> = None;
    let mut offsets: Option<&[u8]> = None;

    // Walk the child sections of the instructions container.
    let mut p = 0usize;
    while p < di.len() {
        let (len, ty, hdr) = read_section(&di[p..])?;
        let seg = di
            .get(p + hdr..p + hdr + len)
            .ok_or(HapErr::Truncated)?;
        match ty {
            ST_COMPRESSOR_TABLE => compressors = Some(seg),
            ST_SIZE_TABLE => sizes = Some(seg),
            ST_OFFSET_TABLE => offsets = Some(seg),
            _ => {} // ignore unknown instruction sections
        }
        p += hdr + len;
    }

    let compressors = compressors.ok_or(HapErr::BadChunkTables)?;
    let sizes = sizes.ok_or(HapErr::BadChunkTables)?;
    if sizes.len() % 4 != 0 {
        return Err(HapErr::BadChunkTables);
    }
    let chunk_count = sizes.len() / 4;
    if compressors.len() != chunk_count {
        return Err(HapErr::BadChunkTables);
    }
    let chunk_size = |i: usize| -> usize {
        u32::from_le_bytes([
            sizes[i * 4],
            sizes[i * 4 + 1],
            sizes[i * 4 + 2],
            sizes[i * 4 + 3],
        ]) as usize
    };
    // Chunk offsets within frame_data: from the offset table if present, else the
    // running sum of the (compressed) chunk sizes.
    let chunk_offset = |i: usize| -> usize {
        if let Some(off) = offsets {
            u32::from_le_bytes([
                off[i * 4],
                off[i * 4 + 1],
                off[i * 4 + 2],
                off[i * 4 + 3],
            ]) as usize
        } else {
            (0..i).map(chunk_size).sum()
        }
    };

    out.clear();
    for (i, &comp) in compressors.iter().enumerate() {
        let start = chunk_offset(i);
        let end = start + chunk_size(i);
        let chunk = frame_data.get(start..end).ok_or(HapErr::Truncated)?;
        match comp {
            CHUNK_COMP_NONE => out.extend_from_slice(chunk),
            CHUNK_COMP_SNAPPY => {
                let n = snap::raw::decompress_len(chunk).map_err(|_| HapErr::Snappy)?;
                let base = out.len();
                out.resize(base + n, 0);
                let written = snap::raw::Decoder::new()
                    .decompress(chunk, &mut out[base..])
                    .map_err(|_| HapErr::Snappy)?;
                out.truncate(base + written);
            }
            other => return Err(HapErr::UnknownCompressor(other)),
        }
    }
    Ok(())
}

/// Decode a full HAP frame packet. `texture_count` comes from the codec `FourCC`
/// (1 for Hap/HapA/HapY/HapAOnly, 2 for `HapM`). For 2 textures the second is the
/// BC4 alpha plane, decoded into `alpha`.
///
/// # Errors
/// Returns [`HapErr`] if the packet is truncated or malformed, or if a two-plane
/// frame's alpha section is not the expected BC4 format.
pub fn decode_frame(
    packet: &[u8],
    texture_count: u8,
    main: &mut Vec<u8>,
    alpha: &mut Vec<u8>,
) -> Result<HapMeta, HapErr> {
    let format = decode_texture_section(packet, main)?;
    if texture_count >= 2 {
        // Second complete section follows the first in the packet.
        let (len0, _ty0, hdr0) = read_section(packet)?;
        let rest = packet.get(hdr0 + len0..).ok_or(HapErr::Truncated)?;
        let alpha_fmt = decode_texture_section(rest, alpha)?;
        // HapM: main is YCoCg BC3, alpha is BC4 → composite mode 2.
        if alpha_fmt != HapTextureFormat::Bc4 {
            return Err(HapErr::UnexpectedAlpha);
        }
        Ok(HapMeta {
            format,
            has_alpha: true,
            video_mode: 2,
        })
    } else {
        Ok(HapMeta {
            format,
            has_alpha: false,
            video_mode: format.video_mode(),
        })
    }
}

/// Encode BC1 (DXT1) texture bytes as a single-section Snappy-compressed HAP1
/// frame (the inverse of `decode_frame` for the Hap1 path). Used by the
/// transcode helper to produce `.mov` clips this app can play back.
///
/// # Panics
/// Panics if Snappy compression fails, which cannot happen for valid input.
pub fn encode_hap1_frame(bc1: &[u8]) -> Vec<u8> {
    let compressed = snap::raw::Encoder::new()
        .compress_vec(bc1)
        .expect("snappy compression is infallible for valid input");
    let ty = COMP_SNAPPY | FMT_RGB_DXT1;
    let n = compressed.len();
    let mut out = Vec::with_capacity(n + 8);
    if n < (1 << 24) {
        out.extend_from_slice(&[n as u8, (n >> 8) as u8, (n >> 16) as u8, ty]);
    } else {
        out.extend_from_slice(&[0, 0, 0, ty]);
        out.extend_from_slice(&(n as u32).to_le_bytes());
    }
    out.extend_from_slice(&compressed);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let bc1: Vec<u8> = (0..(64u32 * 8)).map(|i| (i * 5) as u8).collect(); // 64 blocks
        let frame = encode_hap1_frame(&bc1);
        let (mut main, mut alpha) = (Vec::new(), Vec::new());
        let meta = decode_frame(&frame, 1, &mut main, &mut alpha).unwrap();
        assert_eq!(meta.format, HapTextureFormat::Bc1);
        assert_eq!(main, bc1);
    }

    /// Build a section: 3-byte LE size + type byte + payload.
    fn section(ty: u8, payload: &[u8]) -> Vec<u8> {
        let n = payload.len() as u32;
        let mut v = vec![n as u8, (n >> 8) as u8, (n >> 16) as u8, ty];
        v.extend_from_slice(payload);
        v
    }

    /// Build a section with the 8-byte (zero-24) header form.
    fn section_long(ty: u8, payload: &[u8]) -> Vec<u8> {
        let n = payload.len() as u32;
        let mut v = vec![0, 0, 0, ty];
        v.extend_from_slice(&n.to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn uncompressed_bc1() {
        let payload: Vec<u8> = (0..64).collect(); // one 8-byte BC1 block worth * 8
        let frame = section(COMP_NONE | FMT_RGB_DXT1, &payload);
        let (mut main, mut alpha) = (Vec::new(), Vec::new());
        let meta = decode_frame(&frame, 1, &mut main, &mut alpha).unwrap();
        assert_eq!(meta.format, HapTextureFormat::Bc1);
        assert_eq!(meta.video_mode, 0);
        assert!(!meta.has_alpha);
        assert_eq!(main, payload);
    }

    #[test]
    fn snappy_bc3() {
        let payload: Vec<u8> = (0..256).map(|i| (i * 7) as u8).collect();
        let compressed = snap::raw::Encoder::new().compress_vec(&payload).unwrap();
        let frame = section(COMP_SNAPPY | FMT_RGBA_DXT5, &compressed);
        let (mut main, mut alpha) = (Vec::new(), Vec::new());
        let meta = decode_frame(&frame, 1, &mut main, &mut alpha).unwrap();
        assert_eq!(meta.format, HapTextureFormat::Bc3);
        assert_eq!(main, payload);
    }

    #[test]
    fn ycocg_video_mode() {
        let payload = vec![0u8; 16];
        let frame = section(COMP_NONE | FMT_YCOCG_DXT5, &payload);
        let (mut main, mut alpha) = (Vec::new(), Vec::new());
        let meta = decode_frame(&frame, 1, &mut main, &mut alpha).unwrap();
        assert_eq!(meta.format, HapTextureFormat::Bc3YCoCg);
        assert_eq!(meta.video_mode, 1);
    }

    #[test]
    fn long_header_form() {
        let payload: Vec<u8> = (0..100).collect();
        let frame = section_long(COMP_NONE | FMT_RGB_DXT1, &payload);
        let (mut main, mut alpha) = (Vec::new(), Vec::new());
        let meta = decode_frame(&frame, 1, &mut main, &mut alpha).unwrap();
        assert_eq!(meta.format, HapTextureFormat::Bc1);
        assert_eq!(main, payload);
    }

    #[test]
    fn chunked_two_chunks_mixed_compressors() {
        // Two chunks: chunk0 raw, chunk1 snappy. Reassembled = c0 ++ c1_plain.
        let c0: Vec<u8> = (0..32).collect();
        let c1_plain: Vec<u8> = (100..164).collect();
        let c1 = snap::raw::Encoder::new().compress_vec(&c1_plain).unwrap();

        let compressor_table = section(ST_COMPRESSOR_TABLE, &[CHUNK_COMP_NONE, CHUNK_COMP_SNAPPY]);
        let mut size_bytes = Vec::new();
        size_bytes.extend_from_slice(&(c0.len() as u32).to_le_bytes());
        size_bytes.extend_from_slice(&(c1.len() as u32).to_le_bytes());
        let size_table = section(ST_SIZE_TABLE, &size_bytes);

        let mut di_body = Vec::new();
        di_body.extend_from_slice(&compressor_table);
        di_body.extend_from_slice(&size_table);
        let di = section(ST_DECODE_INSTRUCTIONS, &di_body);

        let mut body = Vec::new();
        body.extend_from_slice(&di);
        body.extend_from_slice(&c0); // frame data follows the instructions container
        body.extend_from_slice(&c1);

        let frame = section(COMP_COMPLEX | FMT_RGB_DXT1, &body);
        let (mut main, mut alpha) = (Vec::new(), Vec::new());
        let meta = decode_frame(&frame, 1, &mut main, &mut alpha).unwrap();
        assert_eq!(meta.format, HapTextureFormat::Bc1);

        let mut expected = c0.clone();
        expected.extend_from_slice(&c1_plain);
        assert_eq!(main, expected);
    }

    #[test]
    fn chunked_with_offset_table() {
        // Offsets deliberately place chunk1 before chunk0 in the frame data to
        // prove the offset table is honored (not just running sums).
        let c0: Vec<u8> = (0..16).collect();
        let c1: Vec<u8> = (200..216).collect();
        // frame data layout: [c1][c0]
        let off0 = c1.len() as u32; // c0 starts after c1
        let off1 = 0u32;

        let compressor_table = section(ST_COMPRESSOR_TABLE, &[CHUNK_COMP_NONE, CHUNK_COMP_NONE]);
        let mut size_bytes = Vec::new();
        size_bytes.extend_from_slice(&(c0.len() as u32).to_le_bytes());
        size_bytes.extend_from_slice(&(c1.len() as u32).to_le_bytes());
        let size_table = section(ST_SIZE_TABLE, &size_bytes);
        let mut off_bytes = Vec::new();
        off_bytes.extend_from_slice(&off0.to_le_bytes());
        off_bytes.extend_from_slice(&off1.to_le_bytes());
        let offset_table = section(ST_OFFSET_TABLE, &off_bytes);

        let mut di_body = Vec::new();
        di_body.extend_from_slice(&compressor_table);
        di_body.extend_from_slice(&size_table);
        di_body.extend_from_slice(&offset_table);
        let di = section(ST_DECODE_INSTRUCTIONS, &di_body);

        let mut body = Vec::new();
        body.extend_from_slice(&di);
        body.extend_from_slice(&c1); // physical order per offsets
        body.extend_from_slice(&c0);

        let frame = section(COMP_COMPLEX | FMT_RGBA_DXT5, &body);
        let (mut main, mut alpha) = (Vec::new(), Vec::new());
        decode_frame(&frame, 1, &mut main, &mut alpha).unwrap();

        // reassembly is in chunk index order: c0 then c1
        let mut expected = c0.clone();
        expected.extend_from_slice(&c1);
        assert_eq!(main, expected);
    }

    #[test]
    fn hapm_two_textures() {
        let ycocg: Vec<u8> = (0..64).collect();
        let bc4_alpha: Vec<u8> = (0..32).map(|i| 255 - i as u8).collect();
        let mut frame = section(COMP_NONE | FMT_YCOCG_DXT5, &ycocg);
        frame.extend_from_slice(&section(COMP_NONE | FMT_RGTC1, &bc4_alpha));

        let (mut main, mut alpha) = (Vec::new(), Vec::new());
        let meta = decode_frame(&frame, 2, &mut main, &mut alpha).unwrap();
        assert_eq!(meta.format, HapTextureFormat::Bc3YCoCg);
        assert!(meta.has_alpha);
        assert_eq!(meta.video_mode, 2);
        assert_eq!(main, ycocg);
        assert_eq!(alpha, bc4_alpha);
    }

    #[test]
    fn truncated_is_error() {
        assert_eq!(read_section(&[1, 2]).unwrap_err(), HapErr::Truncated);
        let (mut m, mut a) = (Vec::new(), Vec::new());
        // claims 100-byte payload but supplies none
        let frame = vec![100, 0, 0, COMP_NONE | FMT_RGB_DXT1];
        assert!(decode_frame(&frame, 1, &mut m, &mut a).is_err());
    }
}
