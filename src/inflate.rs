//! DEFLATE decoder (design_doc §9.9 boundary readers) — PDF FlateDecode streams and
//! xlsx zip entries arrive compressed, and mounting them means decoding here, with
//! no external crates.  `inflate` handles raw RFC 1951 streams; `zlib` peels the
//! RFC 1950 wrapper (2-byte header, Adler-32 trailer) around one.
//!
//! Decode-only, whole-buffer in / whole-buffer out; errors are human-readable and
//! garbage input can never panic or loop — the bit reader bounds-checks every read
//! and output is capped at 256 MiB.

/// Refuse to inflate past this (a corrupt stream must not eat all memory).
const MAX_OUTPUT: usize = 256 * 1024 * 1024;

/// Decompress a raw DEFLATE (RFC 1951) stream.
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    inflate_from(&mut BitReader::new(data))
}

/// Decompress a zlib (RFC 1950) stream: 2-byte header, DEFLATE body, Adler-32 trailer.
/// The checksum is verified when present; a truncated trailer is tolerated.
pub fn zlib(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 2 {
        return Err("zlib: stream shorter than 2-byte header".into());
    }
    let (cmf, flg) = (data[0], data[1]);
    if cmf & 0x0f != 8 {
        return Err(format!("zlib: compression method {} is not deflate", cmf & 0x0f));
    }
    if (cmf as u32 * 256 + flg as u32) % 31 != 0 {
        return Err("zlib: header check failed (not a zlib stream?)".into());
    }
    if flg & 0x20 != 0 {
        return Err("zlib: preset dictionary not supported".into());
    }
    let mut r = BitReader::new(&data[2..]);
    let out = inflate_from(&mut r)?;
    // Trailer: big-endian Adler-32 of the decompressed data, right after the last
    // block (byte-aligned). Verify only if all 4 bytes survived.
    if let Ok(trailer) = r.bytes_after_align(4) {
        let stored = u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
        let computed = adler32(&out);
        if stored != computed {
            return Err(format!("zlib: Adler-32 mismatch (stored {stored:08x}, computed {computed:08x})"));
        }
    }
    Ok(out)
}

fn inflate_from(r: &mut BitReader) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let last = r.bits(1)? == 1;
        match r.bits(2)? {
            0 => stored_block(r, &mut out)?,
            1 => {
                let (lit, dist) = fixed_tables();
                compressed_block(r, &lit, &dist, &mut out)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(r)?;
                compressed_block(r, &lit, &dist, &mut out)?;
            }
            _ => return Err("deflate: reserved block type 11".into()),
        }
        if last {
            return Ok(out);
        }
    }
}

/// Stored block: align to a byte, then LEN, ~LEN (both little-endian u16), raw bytes.
fn stored_block(r: &mut BitReader, out: &mut Vec<u8>) -> Result<(), String> {
    let hdr = r.bytes_after_align(4)?;
    let len = u16::from_le_bytes([hdr[0], hdr[1]]);
    let nlen = u16::from_le_bytes([hdr[2], hdr[3]]);
    if len != !nlen {
        return Err("deflate: stored block length check failed".into());
    }
    let bytes = r.bytes_after_align(len as usize)?;
    if out.len() + bytes.len() > MAX_OUTPUT {
        return Err("deflate: output exceeds 256 MiB cap".into());
    }
    out.extend_from_slice(bytes);
    Ok(())
}

// Length codes 257..=285 map to base lengths plus this many extra bits.
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

// Distance codes 0..=29 likewise.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];

/// Decode one Huffman-compressed block (fixed or dynamic) into `out`.
fn compressed_block(r: &mut BitReader, lit: &Huffman, dist: &Huffman, out: &mut Vec<u8>) -> Result<(), String> {
    loop {
        let sym = lit.decode(r)?;
        match sym {
            0..=255 => {
                if out.len() >= MAX_OUTPUT {
                    return Err("deflate: output exceeds 256 MiB cap".into());
                }
                out.push(sym as u8);
            }
            256 => return Ok(()), // end of block
            257..=285 => {
                let i = sym as usize - 257;
                let len = LEN_BASE[i] as usize + r.bits(LEN_EXTRA[i] as u32)? as usize;
                let dsym = dist.decode(r)? as usize;
                if dsym >= 30 {
                    return Err(format!("deflate: invalid distance code {dsym}"));
                }
                let d = DIST_BASE[dsym] as usize + r.bits(DIST_EXTRA[dsym] as u32)? as usize;
                if d > out.len() {
                    return Err(format!("deflate: distance {d} reaches before output start"));
                }
                if out.len() + len > MAX_OUTPUT {
                    return Err("deflate: output exceeds 256 MiB cap".into());
                }
                // Byte-by-byte so overlapping copies (d < len) repeat correctly.
                let start = out.len() - d;
                for j in 0..len {
                    let b = out[start + j];
                    out.push(b);
                }
            }
            _ => return Err(format!("deflate: invalid literal/length code {sym}")),
        }
    }
}

/// A canonical Huffman code, stored zlib-style as per-length symbol counts plus the
/// symbols in code order.  Decoding walks lengths 1..=15 consuming one bit at a
/// time — bounded, so hostile input can't loop.
struct Huffman {
    counts: [u16; 16], // counts[l] = number of codes of bit-length l
    symbols: Vec<u16>, // symbols ordered by (length, symbol) = canonical code order
}

impl Huffman {
    /// Build from per-symbol code lengths (0 = symbol unused).
    fn new(lengths: &[u8]) -> Result<Huffman, String> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l > 15 {
                return Err(format!("deflate: code length {l} exceeds 15"));
            }
            counts[l as usize] += 1;
        }
        // Reject over-subscribed codes (more codes than the tree has room for);
        // incomplete codes are legal (e.g. a single distance code) and simply make
        // some bit patterns undecodable.
        let mut left = 1i32;
        for l in 1..16 {
            left = (left << 1) - counts[l] as i32;
            if left < 0 {
                return Err("deflate: over-subscribed Huffman code".into());
            }
        }
        // Canonical order: symbols sorted by length, ties by symbol value.
        let mut offsets = [0u16; 16];
        for l in 1..15 {
            offsets[l + 1] = offsets[l] + counts[l];
        }
        let mut symbols = vec![0u16; lengths.iter().filter(|&&l| l > 0).count()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l > 0 {
                symbols[offsets[l as usize] as usize] = sym as u16;
                offsets[l as usize] += 1;
            }
        }
        Ok(Huffman { counts, symbols })
    }

    fn decode(&self, r: &mut BitReader) -> Result<u16, String> {
        let mut code = 0u32; // code accumulated so far (MSB-first, per RFC 1951)
        let mut first = 0u32; // first canonical code of this length
        let mut index = 0u32; // index of that code's symbol
        for len in 1..=15 {
            code |= r.bits(1)?;
            let count = self.counts[len] as u32;
            if code < first + count {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err("deflate: invalid Huffman code".into())
    }
}

/// The fixed tables of block type 01 (RFC 1951 §3.2.6).
fn fixed_tables() -> (Huffman, Huffman) {
    let mut lit = [0u8; 288];
    for (sym, l) in lit.iter_mut().enumerate() {
        *l = match sym {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let dist = [5u8; 30];
    // Both are complete by construction; new() cannot fail on them.
    (Huffman::new(&lit).unwrap(), Huffman::new(&dist).unwrap())
}

/// Read the dynamic-block table definitions (RFC 1951 §3.2.7): a code-length code,
/// then the literal/length and distance code lengths encoded with it.
fn dynamic_tables(r: &mut BitReader) -> Result<(Huffman, Huffman), String> {
    let hlit = r.bits(5)? as usize + 257; // literal/length codes
    let hdist = r.bits(5)? as usize + 1; // distance codes
    let hclen = r.bits(4)? as usize + 4; // code-length codes
    if hlit > 286 || hdist > 30 {
        return Err(format!("deflate: bad table sizes (hlit {hlit}, hdist {hdist})"));
    }
    // Code-length code lengths arrive in this fixed order.
    const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
    let mut cl_lengths = [0u8; 19];
    for &i in ORDER.iter().take(hclen) {
        cl_lengths[i] = r.bits(3)? as u8;
    }
    let cl_code = Huffman::new(&cl_lengths)?;

    // One length list covers literals then distances; repeats may span the seam.
    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = cl_code.decode(r)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                // repeat previous length 3-6 times
                if i == 0 {
                    return Err("deflate: repeat code with no previous length".into());
                }
                let prev = lengths[i - 1];
                let n = 3 + r.bits(2)? as usize;
                if i + n > lengths.len() {
                    return Err("deflate: length repeat overruns table".into());
                }
                lengths[i..i + n].fill(prev);
                i += n;
            }
            17 | 18 => {
                // 17: 3-10 zeros (3 extra bits); 18: 11-138 zeros (7 extra bits)
                let n = if sym == 17 { 3 + r.bits(3)? as usize } else { 11 + r.bits(7)? as usize };
                if i + n > lengths.len() {
                    return Err("deflate: zero repeat overruns table".into());
                }
                i += n; // already zero
            }
            _ => return Err(format!("deflate: invalid code-length symbol {sym}")),
        }
    }
    if lengths[256] == 0 {
        return Err("deflate: dynamic block has no end-of-block code".into());
    }
    let lit = Huffman::new(&lengths[..hlit])?;
    let dist = Huffman::new(&lengths[hlit..])?;
    Ok((lit, dist))
}

/// LSB-first bit reader over a byte slice; every read is bounds-checked and errors
/// on truncation instead of wrapping or panicking.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // next byte to load bits from
    buf: u32,   // bit buffer, LSB = next bit
    have: u32,  // valid bits in buf
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> BitReader<'a> {
        BitReader { data, pos: 0, buf: 0, have: 0 }
    }

    /// Take `n` bits (0..=16), LSB-first as DEFLATE packs them.
    fn bits(&mut self, n: u32) -> Result<u32, String> {
        while self.have < n {
            let b = *self.data.get(self.pos).ok_or("deflate: unexpected end of input")?;
            self.buf |= (b as u32) << self.have;
            self.have += 8;
            self.pos += 1;
        }
        let v = self.buf & ((1u32 << n) - 1);
        self.buf >>= n;
        self.have -= n;
        Ok(v)
    }

    /// Discard bits to the next byte boundary, then take `n` whole bytes.
    fn bytes_after_align(&mut self, n: usize) -> Result<&'a [u8], String> {
        // Dropping have % 8 bits leaves whole buffered bytes; give them back to the
        // slice by rewinding pos, which keeps the return a plain borrow of the input.
        self.buf = 0;
        self.pos -= (self.have / 8) as usize;
        self.have = 0;
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        let end = end.ok_or("deflate: unexpected end of input")?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }
}

/// Adler-32 (RFC 1950 §8): two mod-65521 sums; the classic 5552-byte batching keeps
/// the u32 accumulators from overflowing between reductions.
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ground truth generated with python3 zlib and embedded here (the tests never
    // shell out).  Raw-deflate vectors used wbits=-15; ZLIB used zlib.compress.

    // zlib.compressobj(0, zlib.DEFLATED, -15) of b"stored block payload"
    const STORED: &[u8] = &[
        1, 20, 0, 235, 255, 115, 116, 111, 114, 101, 100, 32, 98, 108, 111, 99,
        107, 32, 112, 97, 121, 108, 111, 97, 100,
    ];

    // zlib.compressobj(6, zlib.DEFLATED, -15) of b"Hello, twin! Hello, twin!"
    // (first block header bits: BFINAL=1, BTYPE=01 fixed Huffman)
    const FIXED: &[u8] = &[
        243, 72, 205, 201, 201, 215, 81, 40, 41, 207, 204, 83, 84, 240, 64, 226,
        0, 0,
    ];

    // zlib.compressobj(9, zlib.DEFLATED, -15) of dynamic_text() — 4340 bytes of
    // repetitive pangrams (BFINAL=1, BTYPE=10 dynamic Huffman, 100 bytes compressed)
    const DYNAMIC: &[u8] = &[
        237, 203, 209, 17, 64, 48, 20, 68, 209, 86, 182, 15, 213, 36, 4, 33,
        60, 34, 65, 82, 189, 55, 74, 48, 62, 247, 115, 231, 158, 77, 163, 195,
        158, 125, 59, 195, 70, 185, 86, 244, 114, 99, 202, 203, 118, 64, 78, 23,
        145, 52, 7, 83, 11, 58, 25, 154, 119, 17, 19, 19, 19, 19, 19, 19,
        19, 19, 19, 255, 132, 55, 163, 110, 41, 176, 138, 46, 159, 70, 244, 254,
        116, 154, 170, 91, 17, 252, 158, 37, 234, 119, 56, 8, 9, 9, 9, 9,
        9, 63, 193, 7,
    ];

    fn dynamic_text() -> String {
        "the quick brown fox jumps over the lazy dog; ".repeat(60)
            + &"pack my box with five dozen liquor jugs; ".repeat(40)
    }

    // zlib.compress(b"zlib wrapped payload with checksum", 9)
    const ZLIB: &[u8] = &[
        120, 218, 171, 202, 201, 76, 82, 40, 47, 74, 44, 40, 72, 77, 81, 40,
        72, 172, 204, 201, 79, 76, 81, 40, 207, 44, 201, 80, 72, 206, 72, 77,
        206, 46, 46, 205, 5, 0, 228, 208, 13, 30,
    ];

    #[test]
    fn stored_block_roundtrip() {
        assert_eq!(inflate(STORED).unwrap(), b"stored block payload");
    }

    #[test]
    fn fixed_huffman_roundtrip() {
        assert_eq!(inflate(FIXED).unwrap(), b"Hello, twin! Hello, twin!");
    }

    #[test]
    fn dynamic_huffman_roundtrip() {
        let out = inflate(DYNAMIC).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), dynamic_text());
    }

    #[test]
    fn zlib_wrapper_with_checksum() {
        assert_eq!(zlib(ZLIB).unwrap(), b"zlib wrapped payload with checksum");
    }

    #[test]
    fn zlib_bad_checksum_is_error() {
        let mut bad = ZLIB.to_vec();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        assert!(zlib(&bad).unwrap_err().contains("Adler-32"));
    }

    #[test]
    fn zlib_truncated_trailer_is_tolerated() {
        // Checksum can't be verified, but the payload is intact — accept it.
        let cut = &ZLIB[..ZLIB.len() - 3];
        assert_eq!(zlib(cut).unwrap(), b"zlib wrapped payload with checksum");
    }

    #[test]
    fn truncated_input_is_error() {
        for vec in [STORED, FIXED, DYNAMIC] {
            for cut in [0, 1, vec.len() / 2, vec.len() - 1] {
                assert!(inflate(&vec[..cut]).is_err(), "truncation at {cut} not caught");
            }
        }
        assert!(zlib(&ZLIB[..1]).is_err());
    }

    #[test]
    fn garbage_input_errors_and_terminates() {
        // A deterministic junk buffer: no panic, no hang, just Err (or, for byte
        // patterns that happen to parse, bounded Ok — never a runaway).
        let mut x = 0x12345678u32;
        for len in [1usize, 7, 64, 512] {
            let junk: Vec<u8> = (0..len)
                .map(|_| {
                    x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                    (x >> 24) as u8
                })
                .collect();
            let _ = inflate(&junk); // must return, not loop
            assert!(zlib(&junk).is_err());
        }
        // Reserved block type 11 is a hard error.
        assert!(inflate(&[0b0000_0111, 0, 0]).unwrap_err().contains("11"));
        // All-0xff drives the fixed-table path with absurd back-references.
        assert!(inflate(&[0xff; 32]).is_err());
    }

    #[test]
    fn overlapping_copy_repeats() {
        // "abcabcabc..." compresses to a literal run plus an overlapping match
        // (distance 3, length > 3); byte-by-byte copying must unfold it.
        // zlib.compressobj(9, zlib.DEFLATED, -15) of b"abc" * 30
        const ABC: &[u8] = &[75, 76, 74, 78, 164, 13, 2, 0];
        assert_eq!(inflate(ABC).unwrap(), "abc".repeat(30).into_bytes());
    }
}
