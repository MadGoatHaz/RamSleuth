//! ramsleuth-protocol — the shared wire protocol between the ramsleuth
//! daemon (the only privileged process) and its unprivileged clients
//! (CLI, TUI, GUI).
//!
//! **Phase 3 crate (P3-10).** One versioned, length-prefixed Bincode
//! frame per message (plan D3: u32-LE length prefix + a u32-LE
//! [`PROTOCOL_VERSION`] + `bincode` 1.3 payload; the codec lands in the
//! `frame` module, P3-11). The frame payload is the [`Message`] enum —
//! one [`Request`] or one [`Response`] per frame, per direction.
//!
//! **Forward compatibility (C6 / DAEMON-1):** every frame carries
//! [`PROTOCOL_VERSION`], the schema generation of the payload layout.
//! Payload structs are bincode-positional and **append-only-at-the-tail**
//! (the byte-pinning tests in [`messages`] enforce it); any layout change
//! bumps the version, and [`frame::decode_frame`] rejects a cross-
//! generation frame with [`frame::FrameError::VersionMismatch`] rather
//! than silently mis-parsing it.
//!
//! **Single source of truth (plan D2):** the payload types are REUSED
//! from the telemetry and bench crates — `SystemMemoryTelemetry`,
//! `StreamTarget`, `StreamProgress`, `BenchmarkGrid` — and duplicated
//! nowhere. This crate owns only the wire enums, the socket-path
//! constant, and the frame codec.
//!
//! **Socket:** [`DEFAULT_SOCKET_PATH`] is the single socket-path source
//! for the daemon and every client (plan D5: the daemon accepts a
//! `--socket` override so it can run unprivileged for local dev).

pub mod frame;
pub mod messages;

pub use frame::{
    decode_frame, encode_frame, Frame, FrameError, MAX_FRAME_SIZE, PROTOCOL_VERSION,
};
pub use messages::{BenchMode, DEFAULT_SOCKET_PATH, Message, Request, Response};
