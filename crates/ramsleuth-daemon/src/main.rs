//! ramsleuth-daemon — Privileged ramsleuth service (Phase 3).
//!
//! Owns `CAP_SYS_RAWIO` hardware handles, the `/dev/mem` + `/dev/ryzen_smu`
//! drivers, and the Unix-socket RPC server at `/run/ramsleuth/ramsleuth.sock`
//! (mode `0660`), exposing `GetTelemetry`, `RunBenchmark`, `CancelBenchmark`.

fn main() {
    // Scaffold stub: no behavior yet.
}
