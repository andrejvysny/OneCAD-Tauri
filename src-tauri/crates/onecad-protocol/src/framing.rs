//! OCW1 wire framing.
//!
//! Frame layout (single frame): `magic "OCW1" + u32 jsonLen + u32 binLen + JSON
//! envelope + binary tail`, with a named-section table inside the JSON. All
//! multi-byte integers are little-endian. `stdout` carries frames only; logs go
//! to stderr. There is NO resync after a bad frame — the worker is restarted.
//! See `../../protocol/SCHEMA.md` §1.
//!
//! This module exposes three layers, all sharing one cap/magic check:
//! - pure [`encode_frame`] / [`decode_frame`] over byte slices (no async deps);
//! - blocking [`read_frame_blocking`] / [`write_frame_blocking`] for the
//!   synchronous worker stub;
//! - the async [`OcwCodec`] (`tokio_util::codec::{Decoder, Encoder}`), behind the
//!   `client` feature, used by the [`crate::client::ProtocolClient`].

use crate::error::ProtocolError;

/// Frame magic: the byte sequence `O C W 1` (`0x4F 0x43 0x57 0x31`) on the wire.
/// The BYTE SEQUENCE is normative per SCHEMA.md §1 — always compare bytes, never
/// an endian-decoded integer.
pub const MAGIC_BYTES: [u8; 4] = *b"OCW1";

/// Frame magic read back as a little-endian `u32` (`0x3157434F`). Convenience
/// only; `MAGIC_BYTES` is the normative form. See `../../protocol/SCHEMA.md` §1.
pub const FRAME_MAGIC_LE: u32 = u32::from_le_bytes(MAGIC_BYTES);

/// Maximum JSON envelope length: 16 MiB. See `../../protocol/SCHEMA.md` §1.
pub const MAX_JSON_LEN: u32 = 16 * 1024 * 1024;

/// Maximum binary tail length: 1 GiB. See `../../protocol/SCHEMA.md` §1.
pub const MAX_BIN_LEN: u32 = 1024 * 1024 * 1024;

/// Fixed frame header size: `magic(4) + jsonLen(4) + binLen(4)`.
pub const HEADER_LEN: usize = 12;

/// The fixed-size frame header preceding the JSON envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Length of the JSON envelope in bytes.
    pub json_len: u32,
    /// Length of the binary tail in bytes.
    pub bin_len: u32,
}

/// One decoded OCW1 frame: the raw JSON envelope bytes and the raw binary tail.
///
/// The framing layer is envelope-agnostic — it never parses the JSON. The
/// `bin` field here is the raw tail region; the named-section *table* lives
/// inside the JSON envelope (see [`crate::messages::BinSection`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawFrame {
    /// UTF-8 JSON envelope bytes (no BOM, no trailing NUL).
    pub json: Vec<u8>,
    /// Raw binary tail bytes (may be empty).
    pub bin: Vec<u8>,
}

impl RawFrame {
    /// A frame with a JSON envelope and no binary tail.
    pub fn json_only(json: Vec<u8>) -> Self {
        RawFrame {
            json,
            bin: Vec::new(),
        }
    }
}

/// Read the little-endian header lengths from the first [`HEADER_LEN`] bytes.
///
/// Validates the magic (as bytes) and the caps. Callers that already hold ≥12
/// bytes use this to learn the total frame size.
fn parse_header(head: &[u8]) -> Result<FrameHeader, ProtocolError> {
    debug_assert!(head.len() >= HEADER_LEN);
    if head[0..4] != MAGIC_BYTES {
        return Err(ProtocolError::bad_magic([
            head[0], head[1], head[2], head[3],
        ]));
    }
    let json_len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
    let bin_len = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
    if json_len > MAX_JSON_LEN {
        return Err(ProtocolError::TooLarge {
            what: "json",
            len: json_len,
            cap: MAX_JSON_LEN,
        });
    }
    if bin_len > MAX_BIN_LEN {
        return Err(ProtocolError::TooLarge {
            what: "bin",
            len: bin_len,
            cap: MAX_BIN_LEN,
        });
    }
    Ok(FrameHeader { json_len, bin_len })
}

/// Serialize one frame to a fresh `Vec<u8>`.
///
/// Enforces the section caps. Never emits `NaN`/`Inf` (that is a JSON concern,
/// handled by the envelope serializer). Pure — no async or `bytes` dependency.
pub fn encode_frame(json: &[u8], bin: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let json_len = u32::try_from(json.len()).map_err(|_| ProtocolError::TooLarge {
        what: "json",
        len: MAX_JSON_LEN,
        cap: MAX_JSON_LEN,
    })?;
    if json_len > MAX_JSON_LEN {
        return Err(ProtocolError::TooLarge {
            what: "json",
            len: json_len,
            cap: MAX_JSON_LEN,
        });
    }
    let bin_len = u32::try_from(bin.len()).map_err(|_| ProtocolError::TooLarge {
        what: "bin",
        len: MAX_BIN_LEN,
        cap: MAX_BIN_LEN,
    })?;
    if bin_len > MAX_BIN_LEN {
        return Err(ProtocolError::TooLarge {
            what: "bin",
            len: bin_len,
            cap: MAX_BIN_LEN,
        });
    }
    let mut out = Vec::with_capacity(HEADER_LEN + json.len() + bin.len());
    out.extend_from_slice(&MAGIC_BYTES);
    out.extend_from_slice(&json_len.to_le_bytes());
    out.extend_from_slice(&bin_len.to_le_bytes());
    out.extend_from_slice(json);
    out.extend_from_slice(bin);
    Ok(out)
}

/// Try to decode one frame from the front of `buf`.
///
/// - `Ok(Some((frame, consumed)))` — a full frame; `consumed` bytes may be
///   dropped from the front of the buffer.
/// - `Ok(None)` — not enough bytes yet; the caller should read more.
/// - `Err(_)` — a fatal framing violation (bad magic / over-cap). No resync.
///
/// Caps are checked from the header BEFORE requiring the body, so an over-cap
/// length errors immediately rather than after buffering gigabytes. This
/// function never panics on any input.
pub fn decode_frame(buf: &[u8]) -> Result<Option<(RawFrame, usize)>, ProtocolError> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    let header = parse_header(buf)?;
    let json_len = header.json_len as usize;
    let bin_len = header.bin_len as usize;
    // usize is 64-bit on every target we build for; caps bound each below u32::MAX
    // so this sum cannot overflow.
    let total = HEADER_LEN + json_len + bin_len;
    if buf.len() < total {
        return Ok(None);
    }
    let json = buf[HEADER_LEN..HEADER_LEN + json_len].to_vec();
    let bin = buf[HEADER_LEN + json_len..total].to_vec();
    Ok(Some((RawFrame { json, bin }, total)))
}

// ---------------------------------------------------------------------------
// Blocking helpers (std only) — used by the synchronous worker stub.
// ---------------------------------------------------------------------------

/// Write one frame to a blocking writer and flush.
pub fn write_frame_blocking<W: std::io::Write>(
    w: &mut W,
    json: &[u8],
    bin: &[u8],
) -> Result<(), ProtocolError> {
    let bytes = encode_frame(json, bin)?;
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(())
}

/// Read one frame from a blocking reader.
///
/// - `Ok(Some(frame))` — a full frame.
/// - `Ok(None)` — a clean EOF exactly at a frame boundary (peer closed).
/// - `Err(ConnectionLost)` — EOF part-way through a frame.
/// - `Err(BadMagic | TooLarge)` — fatal framing violation.
pub fn read_frame_blocking<R: std::io::Read>(r: &mut R) -> Result<Option<RawFrame>, ProtocolError> {
    let mut head = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        match r.read(&mut head[filled..]) {
            Ok(0) => {
                if filled == 0 {
                    return Ok(None); // clean EOF at a boundary
                }
                return Err(ProtocolError::ConnectionLost("eof mid-header"));
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let header = parse_header(&head)?;
    let mut json = vec![0u8; header.json_len as usize];
    read_exact_or_lost(r, &mut json)?;
    let mut bin = vec![0u8; header.bin_len as usize];
    read_exact_or_lost(r, &mut bin)?;
    Ok(Some(RawFrame { json, bin }))
}

fn read_exact_or_lost<R: std::io::Read>(r: &mut R, buf: &mut [u8]) -> Result<(), ProtocolError> {
    match r.read_exact(buf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            Err(ProtocolError::ConnectionLost("eof mid-frame"))
        }
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------
// Async codec (tokio_util) — behind the `client` feature.
// ---------------------------------------------------------------------------

/// Custom OCW1 `tokio_util` codec (NOT the stock `LengthDelimitedCodec`).
///
/// `Decoder` yields [`RawFrame`]s and handles partial reads, cap violations
/// (→ [`ProtocolError::TooLarge`]), bad magic (→ [`ProtocolError::BadMagic`],
/// fatal), and EOF mid-frame (→ [`ProtocolError::ConnectionLost`] via
/// `decode_eof`). `Encoder<RawFrame>` writes the header + envelope + tail.
#[cfg(feature = "client")]
#[derive(Debug, Clone, Copy, Default)]
pub struct OcwCodec;

#[cfg(feature = "client")]
impl tokio_util::codec::Decoder for OcwCodec {
    type Item = RawFrame;
    type Error = ProtocolError;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        use bytes::Buf;
        match decode_frame(src)? {
            Some((frame, consumed)) => {
                src.advance(consumed);
                Ok(Some(frame))
            }
            None => {
                // Reserve the remaining bytes we know we need so the framed
                // read grabs the whole frame in as few syscalls as possible.
                if src.len() >= HEADER_LEN {
                    let header = parse_header(src)?; // caps already validated here
                    let total = HEADER_LEN + header.json_len as usize + header.bin_len as usize;
                    if total > src.len() {
                        src.reserve(total - src.len());
                    }
                }
                Ok(None)
            }
        }
    }

    fn decode_eof(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decode(src)? {
            Some(frame) => Ok(Some(frame)),
            None => {
                if src.is_empty() {
                    Ok(None) // clean end of stream
                } else {
                    Err(ProtocolError::ConnectionLost("eof mid-frame"))
                }
            }
        }
    }
}

#[cfg(feature = "client")]
impl tokio_util::codec::Encoder<RawFrame> for OcwCodec {
    type Error = ProtocolError;

    fn encode(&mut self, item: RawFrame, dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        let bytes = encode_frame(&item.json, &item.bin)?;
        dst.extend_from_slice(&bytes);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_bytes(json: &[u8], bin: &[u8]) -> Vec<u8> {
        encode_frame(json, bin).expect("encode")
    }

    #[test]
    fn magic_bytes_are_normative_ocw1() {
        assert_eq!(&MAGIC_BYTES, b"OCW1");
        // "OCW1" read little-endian is 0x3157434F per SCHEMA §1.
        assert_eq!(FRAME_MAGIC_LE, 0x3157_434F);
    }

    #[test]
    fn round_trip_json_and_bin() {
        let bytes = frame_bytes(br#"{"t":"req"}"#, &[1, 2, 3, 4]);
        let (frame, consumed) = decode_frame(&bytes).unwrap().unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(frame.json, br#"{"t":"req"}"#);
        assert_eq!(frame.bin, vec![1, 2, 3, 4]);
    }

    #[test]
    fn round_trip_no_bin() {
        let bytes = frame_bytes(br#"{}"#, &[]);
        let (frame, _) = decode_frame(&bytes).unwrap().unwrap();
        assert!(frame.bin.is_empty());
        assert_eq!(frame.json, b"{}");
    }

    #[test]
    fn partial_header_needs_more() {
        let bytes = frame_bytes(br#"{}"#, &[]);
        for n in 0..HEADER_LEN {
            assert!(matches!(decode_frame(&bytes[..n]), Ok(None)));
        }
    }

    #[test]
    fn partial_body_needs_more() {
        let bytes = frame_bytes(br#"{"x":1}"#, &[9, 9]);
        // Everything up to (but not including) the last byte is incomplete.
        for n in HEADER_LEN..bytes.len() {
            assert!(matches!(decode_frame(&bytes[..n]), Ok(None)), "n={n}");
        }
        assert!(decode_frame(&bytes).unwrap().is_some());
    }

    #[test]
    fn bad_magic_is_fatal() {
        let mut bytes = frame_bytes(br#"{}"#, &[]);
        bytes[0] = b'X';
        match decode_frame(&bytes) {
            Err(ProtocolError::BadMagic { got, expected }) => {
                assert_eq!(expected, MAGIC_BYTES);
                assert_eq!(got[0], b'X');
            }
            other => panic!("expected BadMagic, got {other:?}"),
        }
    }

    #[test]
    fn oversize_json_rejected() {
        let mut head = Vec::new();
        head.extend_from_slice(&MAGIC_BYTES);
        head.extend_from_slice(&(MAX_JSON_LEN + 1).to_le_bytes());
        head.extend_from_slice(&0u32.to_le_bytes());
        match decode_frame(&head) {
            Err(ProtocolError::TooLarge { what, cap, .. }) => {
                assert_eq!(what, "json");
                assert_eq!(cap, MAX_JSON_LEN);
            }
            other => panic!("expected TooLarge json, got {other:?}"),
        }
    }

    #[test]
    fn oversize_bin_rejected() {
        let mut head = Vec::new();
        head.extend_from_slice(&MAGIC_BYTES);
        head.extend_from_slice(&0u32.to_le_bytes());
        head.extend_from_slice(&(MAX_BIN_LEN + 1).to_le_bytes());
        match decode_frame(&head) {
            Err(ProtocolError::TooLarge { what, cap, .. }) => {
                assert_eq!(what, "bin");
                assert_eq!(cap, MAX_BIN_LEN);
            }
            other => panic!("expected TooLarge bin, got {other:?}"),
        }
    }

    #[test]
    fn encode_within_caps_succeeds() {
        // Allocating cap+1 bytes just to prove the over-cap branch is wasteful in
        // CI; the decode side already covers over-cap rejection. Here we assert a
        // normal-sized payload encodes to a header + body of the expected length.
        let out = encode_frame(&[b'a'; 16], &[1, 2]).unwrap();
        assert_eq!(out.len(), HEADER_LEN + 16 + 2);
        let (frame, _) = decode_frame(&out).unwrap().unwrap();
        assert_eq!(frame.json.len(), 16);
        assert_eq!(frame.bin, vec![1, 2]);
    }

    #[test]
    fn blocking_round_trip() {
        let bytes = frame_bytes(br#"{"hello":true}"#, &[7, 8, 9]);
        let mut cursor = std::io::Cursor::new(bytes);
        let frame = read_frame_blocking(&mut cursor).unwrap().unwrap();
        assert_eq!(frame.json, br#"{"hello":true}"#);
        assert_eq!(frame.bin, vec![7, 8, 9]);
        // A second read at EOF yields None (clean boundary).
        assert!(read_frame_blocking(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn blocking_eof_mid_frame_is_connection_lost() {
        let bytes = frame_bytes(br#"{"x":1}"#, &[1, 2, 3]);
        let truncated = &bytes[..bytes.len() - 2];
        let mut cursor = std::io::Cursor::new(truncated.to_vec());
        match read_frame_blocking(&mut cursor) {
            Err(ProtocolError::ConnectionLost(_)) => {}
            other => panic!("expected ConnectionLost, got {other:?}"),
        }
    }

    #[test]
    fn blocking_bad_magic() {
        let mut bytes = frame_bytes(br#"{}"#, &[]);
        bytes[2] = 0xFF;
        let mut cursor = std::io::Cursor::new(bytes);
        assert!(matches!(
            read_frame_blocking(&mut cursor),
            Err(ProtocolError::BadMagic { .. })
        ));
    }

    /// Fuzz-ish: decode must never panic on arbitrary input and must return
    /// Ok(None) / Ok(Some) / Err, never diverge. Uses a cheap xorshift PRNG so
    /// the test has no external dependency.
    #[test]
    fn decode_never_panics_on_random_bytes() {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..5_000 {
            let len = (next() % 64) as usize;
            let mut buf = vec![0u8; len];
            for b in &mut buf {
                *b = (next() & 0xFF) as u8;
            }
            // Must not panic. Any of Ok(None)/Ok(Some)/Err is acceptable.
            let _ = decode_frame(&buf);
            let mut cursor = std::io::Cursor::new(buf);
            let _ = read_frame_blocking(&mut cursor);
        }
    }

    /// Random *valid-magic* headers with random declared lengths: still must not
    /// panic, and over-cap lengths must be rejected rather than buffered.
    #[test]
    fn decode_never_panics_with_valid_magic_random_lengths() {
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            state
        };
        for _ in 0..5_000 {
            let mut buf = Vec::new();
            buf.extend_from_slice(&MAGIC_BYTES);
            let json_len = (next() & 0xFFFF_FFFF) as u32;
            let bin_len = (next() & 0xFFFF_FFFF) as u32;
            buf.extend_from_slice(&json_len.to_le_bytes());
            buf.extend_from_slice(&bin_len.to_le_bytes());
            // maybe append a few random body bytes
            let extra = (next() % 8) as usize;
            buf.extend(std::iter::repeat_n(0xABu8, extra));
            match decode_frame(&buf) {
                Ok(_) => {}
                Err(ProtocolError::TooLarge { .. }) => {}
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        }
    }
}

#[cfg(all(test, feature = "client"))]
mod codec_tests {
    use super::*;
    use bytes::BytesMut;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::codec::{Decoder, Encoder};

    fn encode(json: &[u8], bin: &[u8]) -> Vec<u8> {
        let mut codec = OcwCodec;
        let mut dst = BytesMut::new();
        codec
            .encode(
                RawFrame {
                    json: json.to_vec(),
                    bin: bin.to_vec(),
                },
                &mut dst,
            )
            .unwrap();
        dst.to_vec()
    }

    #[test]
    fn codec_encode_matches_pure_encode() {
        assert_eq!(
            encode(br#"{"a":1}"#, &[5, 6]),
            encode_frame(br#"{"a":1}"#, &[5, 6]).unwrap()
        );
    }

    #[test]
    fn codec_decode_byte_by_byte() {
        let bytes = encode(br#"{"t":"resp","id":1}"#, &[42]);
        let mut codec = OcwCodec;
        let mut buf = BytesMut::new();
        // Feed all but the last byte one at a time — never yields a frame.
        for &b in &bytes[..bytes.len() - 1] {
            buf.extend_from_slice(&[b]);
            assert!(codec.decode(&mut buf).unwrap().is_none());
        }
        // Final byte completes exactly one frame.
        buf.extend_from_slice(&[bytes[bytes.len() - 1]]);
        let frame = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame.json, br#"{"t":"resp","id":1}"#);
        assert_eq!(frame.bin, vec![42]);
        assert!(buf.is_empty());
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn codec_decode_two_frames_in_one_buffer() {
        let mut bytes = encode(br#"{"n":1}"#, &[]);
        bytes.extend(encode(br#"{"n":2}"#, &[]));
        let mut codec = OcwCodec;
        let mut buf = BytesMut::from(&bytes[..]);
        let f1 = codec.decode(&mut buf).unwrap().unwrap();
        let f2 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(f1.json, b"{\"n\":1}");
        assert_eq!(f2.json, b"{\"n\":2}");
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn codec_bad_magic() {
        let mut buf = BytesMut::from(&b"XXXX\x00\x00\x00\x00\x00\x00\x00\x00"[..]);
        let mut codec = OcwCodec;
        assert!(matches!(
            codec.decode(&mut buf),
            Err(ProtocolError::BadMagic { .. })
        ));
    }

    #[tokio::test]
    async fn decode_over_duplex_split_writes() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let bytes = encode(br#"{"t":"hello","seq":0}"#, &[1, 2, 3]);
        // Writer: dribble the frame out in 1-byte chunks with yields so the
        // reader observes many partial reads.
        let writer = tokio::spawn(async move {
            for &b in &bytes {
                client.write_all(&[b]).await.unwrap();
                client.flush().await.unwrap();
                tokio::task::yield_now().await;
            }
            client.shutdown().await.unwrap();
        });
        // Reader: the same read_buf + decode loop the ProtocolClient uses.
        let mut buf = BytesMut::new();
        let frame = loop {
            if let Some((f, consumed)) = decode_frame(&buf).unwrap() {
                use bytes::Buf;
                buf.advance(consumed);
                break f;
            }
            let n = server.read_buf(&mut buf).await.unwrap();
            assert_ne!(n, 0, "eof before full frame");
        };
        assert_eq!(frame.json, br#"{"t":"hello","seq":0}"#);
        assert_eq!(frame.bin, vec![1, 2, 3]);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn eof_mid_frame_via_decode_eof() {
        let bytes = encode(br#"{"x":1}"#, &[9, 9, 9]);
        let mut codec = OcwCodec;
        let mut buf = BytesMut::from(&bytes[..bytes.len() - 2]);
        // Not enough for a frame.
        assert!(codec.decode(&mut buf).unwrap().is_none());
        // At EOF with bytes remaining -> ConnectionLost.
        match codec.decode_eof(&mut buf) {
            Err(ProtocolError::ConnectionLost(_)) => {}
            other => panic!("expected ConnectionLost, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn clean_eof_via_decode_eof() {
        let mut codec = OcwCodec;
        let mut buf = BytesMut::new();
        assert!(codec.decode_eof(&mut buf).unwrap().is_none());
    }
}
