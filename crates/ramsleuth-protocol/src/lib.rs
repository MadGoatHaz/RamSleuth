//! ramsleuth-protocol — the shared wire protocol between the ramsleuth
//! daemon (the only privileged process) and its unprivileged clients
//! (CLI, TUI, GUI).
//!
//! **Phase 3 crate (P3-10).** One length-prefixed Bincode frame per
//! message (plan D3: u32-LE length prefix + `bincode` 1.3 payload; the
//! codec lands in the `frame` module, P3-11). The frame payload is the
//! [`Message`] enum — one [`Request`] or one [`Response`] per frame,
//! per direction.
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

pub use frame::{decode_frame, encode_frame, Frame, FrameError, MAX_FRAME_SIZE};
pub use messages::{BenchMode, DEFAULT_SOCKET_PATH, Message, Request, Response};
