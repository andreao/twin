//! A minimal, dependency-free WebSocket (RFC 6455) server helper — text frames.
//!
//! In the spirit of the rest of the runtime (raw V8, not Node), the network is just
//! a boundary lens (§10, §11.11): the thin client applies the forward mutation IR
//! (§11.3) and sends back the backward event stream (§11.4).  We hand-roll the small
//! subset we need — the handshake + text/close/ping frames — rather than pull in a
//! crate, keeping deps at v8/sha2/serde_json.

use std::io::{self, Read, Write};

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Read HTTP request headers (up to the blank line) from a stream.
pub fn read_http_headers(stream: &mut impl Read) -> io::Result<String> {
    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    loop {
        let n = stream.read(&mut b)?;
        if n == 0 {
            break;
        }
        buf.push(b[0]);
        if buf.ends_with(b"\r\n\r\n") || buf.len() > 16384 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Given an already-read request, complete the upgrade handshake on `stream`.
/// Returns false if it is not a valid WebSocket upgrade (no key).
pub fn send_handshake(stream: &mut impl Write, request: &str) -> io::Result<bool> {
    let key = request.lines().find_map(|l| {
        let lower = l.to_ascii_lowercase();
        if lower.starts_with("sec-websocket-key:") {
            l.split_once(':').map(|(_, v)| v.trim().to_string())
        } else {
            None
        }
    });
    let key = match key {
        Some(k) => k,
        None => return Ok(false),
    };
    let accept = base64(&sha1(format!("{key}{WS_GUID}").as_bytes()));
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()?;
    Ok(true)
}

/// A decoded inbound frame (only the kinds we care about).
pub enum Frame {
    Text(String),
    Close,
    Ping(Vec<u8>),
    Other,
}

/// Read one frame. Client->server frames are always masked (RFC 6455 §5.3).
pub fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut h = [0u8; 2];
    r.read_exact(&mut h)?;
    let opcode = h[0] & 0x0f;
    let masked = h[1] & 0x80 != 0;
    let mut len = (h[1] & 0x7f) as u64;
    if len == 126 {
        let mut b = [0u8; 2];
        r.read_exact(&mut b)?;
        len = u16::from_be_bytes(b) as u64;
    } else if len == 127 {
        let mut b = [0u8; 8];
        r.read_exact(&mut b)?;
        len = u64::from_be_bytes(b);
    }
    let mut mask = [0u8; 4];
    if masked {
        r.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    if masked {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i & 3];
        }
    }
    Ok(match opcode {
        0x1 => Frame::Text(String::from_utf8_lossy(&payload).into_owned()),
        0x8 => Frame::Close,
        0x9 => Frame::Ping(payload),
        _ => Frame::Other,
    })
}

/// Write a text frame (server->client, unmasked).
pub fn write_text(w: &mut impl Write, s: &str) -> io::Result<()> {
    write_frame(w, 0x1, s.as_bytes())
}

/// Write a pong frame echoing the ping payload.
pub fn write_pong(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    write_frame(w, 0xA, payload)
}

fn write_frame(w: &mut impl Write, opcode: u8, payload: &[u8]) -> io::Result<()> {
    let mut f = vec![0x80 | opcode];
    let n = payload.len();
    if n < 126 {
        f.push(n as u8);
    } else if n < 65536 {
        f.push(126);
        f.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        f.push(127);
        f.extend_from_slice(&(n as u64).to_be_bytes());
    }
    f.extend_from_slice(payload);
    w.write_all(&f)?;
    w.flush()
}

// ---- SHA-1 (RFC 3174) and base64, just enough for the handshake ------------

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let ml = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for i in 0..5 {
        out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

fn base64(data: &[u8]) -> String {
    const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TBL[((n >> 18) & 63) as usize] as char);
        out.push(TBL[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TBL[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TBL[(n & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_known_vector() {
        // RFC 3174 / well-known: sha1("abc")
        let got = sha1(b"abc");
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn handshake_accept_vector() {
        // RFC 6455 §1.3 canonical example
        let accept = base64(&sha1(format!("dGhlIHNhbXBsZSBub25jZQ=={WS_GUID}").as_bytes()));
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn base64_basic() {
        assert_eq!(base64(b"any carnal pleasure."), "YW55IGNhcm5hbCBwbGVhc3VyZS4=");
        assert_eq!(base64(b"any carnal pleasure"), "YW55IGNhcm5hbCBwbGVhc3VyZQ==");
    }
}
