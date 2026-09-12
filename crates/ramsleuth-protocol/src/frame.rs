//! The length-prefixed Bincode frame codec (P3-11, plan D3).
//!
//! One frame = one [`Message`]: a 4-byte little-endian `u32` length
//! prefix (the byte length of the bincode payload that follows) plus
//! the bincode-1.3 serialization of the message. The codec is
//! synchronous and format-only — the daemon (P3-16) wraps it with tokio
//! I/O and the clients (P3-18+) with std I/O; neither side
//! reimplements the layout.
//!
//! **Incremental-reader contract (wire layout, frozen):** callers
//! append every received byte to a buffer and call [`decode_frame`]
//! repeatedly. [`FrameError::Incomplete`] means "not enough bytes yet —
//! read more": it is a transient signal, not a broken stream.
//! [`Frame::consumed`] is the exact number of bytes one frame used, so
//! the caller can drop it and keep decoding what follows.
//!
//! **16 MiB guard:** any frame whose declared payload length exceeds
//! [`MAX_FRAME_SIZE`] is rejected as [`FrameError::Oversized`] *before*
//! the payload is awaited, so a hostile or corrupted length prefix can
//! never make a peer allocate an absurd buffer.
//!
//! **No panics:** every failure path is a [`FrameError`] — a bincode
//! rejection surfaces as [`FrameError::Decode`], never a panic (the
//! protocol's no-panic contract, plan D3).

use std::error::Error;
use std::fmt;

use crate::messages::Message;

/// The maximum allowed length of a frame's bincode payload, in bytes
/// (16 MiB). The length prefix counts payload bytes only, so the
/// total on-the-wire size of a frame is at most `4 + MAX_FRAME_SIZE`.
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// The size of the little-endian `u32` length prefix, in bytes.
const LENGTH_PREFIX_SIZE: usize = 4;

/// Errors the frame codec can return.
///
/// All three arms are stream-level (not application-level) failures:
/// [`Incomplete`] is a transient "read more" signal, while
/// [`Oversized`] / [`Decode`] mean the byte stream is invalid and the
/// peer should drop the connection.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameError {
    /// The buffer does not yet hold a complete frame (fewer than four
    /// prefix bytes, or fewer than the declared payload bytes). Callers
    /// should append more bytes and retry.
    Incomplete,
    /// The 4-byte length prefix declares a payload longer than
    /// [`MAX_FRAME_SIZE`]. The stream is treated as hostile/corrupt.
    Oversized,
    /// The payload bytes were present but bincode rejected them (`msg`
    /// carries the bincode error text).
    Decode(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Incomplete => {
                write!(f, "incomplete frame: not enough buffered bytes yet")
            }
            FrameError::Oversized => {
                write!(f, "frame payload length exceeds the {MAX_FRAME_SIZE}-byte maximum")
            }
            FrameError::Decode(msg) => write!(f, "frame payload failed to decode: {msg}"),
        }
    }
}

impl Error for FrameError {}

/// One decoded frame: the [`Message`] payload plus `consumed`, the
/// exact number of bytes (length prefix + payload) the frame used from
/// the buffer passed to [`decode_frame`].
#[derive(Debug, PartialEq)]
pub struct Frame {
    /// The decoded message (one request, or one response).
    pub message: Message,
    /// The number of bytes this frame consumed — advance/slice the
    /// buffer by exactly this amount to reach the next frame.
    pub consumed: usize,
}

/// Encode one [`Message`] into a full wire frame: the 4-byte
/// little-endian length of the bincode payload, followed by the
/// payload itself.
///
/// The length prefix counts payload bytes only (never itself). Returns
/// [`FrameError::Oversized`] if the serialized payload exceeds
/// [`MAX_FRAME_SIZE`] (unreachable in practice for [`Message`], which
/// serializes to at most a few hundred bytes).
pub fn encode_frame(message: &Message) -> Result<Vec<u8>, FrameError> {
    let payload = bincode::serialize(message).map_err(|e| FrameError::Decode(e.to_string()))?;
    if payload.len() > MAX_FRAME_SIZE {
        return Err(FrameError::Oversized);
    }
    // The size check above guarantees `payload.len() <= MAX_FRAME_SIZE`
    // (16 MiB), far below `u32::MAX`, so this cast is lossless.
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(LENGTH_PREFIX_SIZE + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode one frame from the front of `buf`, which may hold a partial
/// frame, exactly one frame, or several back-to-back frames (the
/// incremental-reader contract).
///
/// Outcomes:
/// * `buf.len() < 4` → [`FrameError::Incomplete`];
/// * declared length > [`MAX_FRAME_SIZE`] → [`FrameError::Oversized`];
/// * fewer than `4 + length` bytes buffered → [`FrameError::Incomplete`];
/// * bincode rejects the payload → [`FrameError::Decode`];
/// * otherwise → `Ok(Frame { message, consumed: 4 + length })`.
///
/// Safe to call repeatedly on a growing buffer: it neither mutates nor
/// allocates beyond the decoded [`Message`].
pub fn decode_frame(buf: &[u8]) -> Result<Frame, FrameError> {
    if buf.len() < LENGTH_PREFIX_SIZE {
        return Err(FrameError::Incomplete);
    }
    // `u32` → `usize` is lossless on every supported (32/64-bit)
    // target.
    let length = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if length > MAX_FRAME_SIZE {
        return Err(FrameError::Oversized);
    }
    let end = LENGTH_PREFIX_SIZE + length;
    if buf.len() < end {
        return Err(FrameError::Incomplete);
    }
    let message: Message = bincode::deserialize(&buf[LENGTH_PREFIX_SIZE..end])
        .map_err(|e| FrameError::Decode(e.to_string()))?;
    Ok(Frame { message, consumed: end })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{BenchMode, Request, Response};
    use ramsleuth_bench::{BenchmarkGrid, BenchOp, StreamProgress, StreamTarget, Tier};

    fn grid() -> BenchmarkGrid {
        BenchmarkGrid {
            read_gbps: [512.0, 897.5, 402.0, 198.5],
            write_gbps: [410.0, 823.0, 311.5, 152.0],
            copy_gbps: [455.0, 851.5, 349.0, 176.5],
            latency_ns: [88.0, 1.1, 3.4, 12.7],
        }
    }

    /// (a) encode→decode round-trip returns the original `Message` —
    /// both directions, representative payloads — and `consumed`
    /// accounts for the whole buffer.
    #[test]
    fn encode_decode_round_trip() {
        let messages = vec![
            Message::Request(Request::GetTelemetry),
            Message::Request(Request::StartBenchmark {
                target: StreamTarget::Tier(Tier::L2),
                mode: BenchMode::Full,
            }),
            Message::Request(Request::StartBenchmark {
                target: StreamTarget::Cell(Tier::Memory, BenchOp::Read),
                mode: BenchMode::MemoryOnly,
            }),
            Message::Request(Request::CancelBenchmark { run_id: 7 }),
            Message::Response(Response::BenchStarted { run_id: 42 }),
            Message::Response(Response::BenchProgress(StreamProgress {
                cell_index: 5,
                total_cells: 12,
                tier: Tier::Memory,
                op: BenchOp::Copy,
                value: 349.0,
                label: "Memory · Copy (GB/s)".to_owned(),
            })),
            Message::Response(Response::BenchResult { run_id: 9, grid: grid() }),
            Message::Response(Response::BenchCancelled { run_id: 9 }),
            Message::Response(Response::Error("boom".to_owned())),
        ];
        for msg in messages {
            let frame = encode_frame(&msg).expect("Message must encode");
            let decoded = decode_frame(&frame).expect("encoded frame must decode");
            assert_eq!(decoded.message, msg);
            assert_eq!(decoded.consumed, frame.len());
        }
    }

    /// (b) The encoded layout is exactly `[4-byte LE length][payload]`:
    /// the first 4 bytes are the LE length of the bincode payload,
    /// followed by the payload verbatim.
    #[test]
    fn frame_layout_is_le_length_prefix_plus_payload() {
        let msg = Message::Request(Request::GetTelemetry);
        let payload = bincode::serialize(&msg).expect("Message must serialize");
        let frame = encode_frame(&msg).expect("Message must encode");
        assert_eq!(frame.len(), LENGTH_PREFIX_SIZE + payload.len());
        assert_eq!(&frame[0..LENGTH_PREFIX_SIZE], &(payload.len() as u32).to_le_bytes());
        assert_eq!(&frame[LENGTH_PREFIX_SIZE..], &payload);
    }

    /// (c) A truncated buffer is `Incomplete` — never a false success:
    /// both the <4-byte case and every partial-payload cut short of a
    /// full frame.
    #[test]
    fn truncated_buffers_return_incomplete() {
        assert_eq!(decode_frame(b""), Err(FrameError::Incomplete));
        assert_eq!(decode_frame(b"\x04\x00"), Err(FrameError::Incomplete));
        assert_eq!(decode_frame(b"\x04\x00\x00"), Err(FrameError::Incomplete));

        let frame = encode_frame(&Message::Response(Response::BenchResult {
            run_id: 3,
            grid: grid(),
        }))
        .expect("Message must encode");
        assert!(frame.len() > LENGTH_PREFIX_SIZE + 1);
        for cut in 0..frame.len() {
            assert_eq!(
                decode_frame(&frame[..cut]),
                Err(FrameError::Incomplete),
                "cut {cut} of {} bytes must be Incomplete",
                frame.len()
            );
        }
        // The fully-buffered frame decodes.
        assert!(decode_frame(&frame).is_ok());
    }

    /// (d) A hostile length prefix (far beyond the 16 MiB guard) is
    /// `Oversized`, detected *before* the payload is awaited — even
    /// when no payload bytes are buffered yet.
    #[test]
    fn hostile_length_returns_oversized() {
        let mut buf = u32::MAX.to_le_bytes().to_vec();
        assert_eq!(decode_frame(&buf), Err(FrameError::Oversized));
        buf.extend_from_slice(&[0u8; 8]);
        assert_eq!(decode_frame(&buf), Err(FrameError::Oversized));
    }

    /// The guard is a strict `>`: a length exactly at `MAX_FRAME_SIZE`
    /// is not `Oversized`, but without its payload it is `Incomplete`.
    #[test]
    fn length_at_the_cap_is_incomplete_not_oversized() {
        let mut buf = (MAX_FRAME_SIZE as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(&[0u8; 4]);
        assert_eq!(decode_frame(&buf), Err(FrameError::Incomplete));
    }

    /// (e) A payload that is present but bincode rejects is
    /// `Decode(_)` carrying the bincode error text — never a panic.
    #[test]
    fn corrupt_payload_returns_decode_error() {
        let frame =
            encode_frame(&Message::Request(Request::GetTelemetry)).expect("Message must encode");
        // `Message::Request(GetTelemetry)` serializes to the two variant
        // indices; the `Message` variant byte at offset 4 must stay
        // `0` or `1`. 0xFF is not a valid variant, so bincode rejects.
        let mut bad = frame;
        bad[LENGTH_PREFIX_SIZE] = 0xFF;
        let err = decode_frame(&bad).expect_err("corrupt payload must not decode");
        assert!(matches!(err, FrameError::Decode(_)), "expected Decode, got {err:?}");
        assert!(!err.to_string().is_empty());
    }

    /// (f) Two back-to-back frames in one buffer: the first
    /// `decode_frame` returns frame 1 with `consumed` equal to its
    /// exact length, and decoding `buf[consumed..]` returns frame 2.
    #[test]
    fn two_back_to_back_frames_decode_in_order() {
        let first = Message::Request(Request::CancelBenchmark { run_id: 1 });
        let second = Message::Response(Response::BenchStarted { run_id: 1 });
        let f1 = encode_frame(&first).expect("first must encode");
        let f2 = encode_frame(&second).expect("second must encode");
        let mut stream = f1.clone();
        stream.extend_from_slice(&f2);

        let decoded1 = decode_frame(&stream).expect("first frame must decode");
        assert_eq!(decoded1.message, first);
        assert_eq!(decoded1.consumed, f1.len());

        let decoded2 = decode_frame(&stream[decoded1.consumed..]).expect("second frame must decode");
        assert_eq!(decoded2.message, second);
        assert_eq!(decoded2.consumed, f2.len());
        assert_eq!(decoded1.consumed + decoded2.consumed, stream.len());
    }

    /// `Display` + `Error` render every arm without panicking.
    #[test]
    fn display_and_error_impls() {
        let e: &dyn Error = &FrameError::Incomplete;
        assert!(!e.to_string().is_empty());
        let e: &dyn Error = &FrameError::Oversized;
        assert!(e.to_string().contains("maximum"));
        let e: &dyn Error = &FrameError::Decode("bad bytes".to_owned());
        assert!(e.to_string().contains("bad bytes"));
    }
}
