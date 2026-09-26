//! End-to-end `--version` / `--help` behavior of the `ramsleuth-client` binary
//! (spawned via `CARGO_BIN_EXE_ramsleuth-client` — the real bin, not the test
//! harness): both short-circuit before any app work and exit `0`, the
//! version line carries the package version, and the usage text lists the
//! five subcommands (the `probe` / `burn` parity pair included).

use std::process::Command;

fn bin_path() -> std::path::PathBuf {
    // Prefer cargo's bin pointer (set by toolchains that provide it);
    // otherwise derive the bin from this test exe's location:
    // `<target>/debug/deps/cli-<hash>` -> sibling `<target>/debug/ramsleuth-client>`.
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_ramsleuth-client") {
        return std::path::PathBuf::from(path);
    }
    let exe = std::env::current_exe().expect("current test exe");
    // `<target>/debug/deps/cli-<hash>`: the bin is one level up.
    let debug_dir = exe.parent().and_then(|d| d.parent()).expect("target/debug");
    debug_dir.join("ramsleuth-client")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin_path())
        .args(args)
        .output()
        .expect("the ramsleuth-client binary must spawn")
}

/// (a) `--version` / `-V`: exit 0, exactly the version line on stdout.
#[test]
fn version_prints_the_package_version_and_exits_zero() {
    for flag in ["--version", "-V"] {
        let out = run(&[flag]);
        assert!(out.status.success(), "{flag} must exit 0, got {:?}", out.status);
        let expected = format!("ramSleuth ramsleuth-client v{}", env!("CARGO_PKG_VERSION"));
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(stdout, expected + "\n", "{flag} must print the version line");
    }
}

/// (b) `--help` / `-h`: exit 0, the usage text on stdout.
#[test]
fn help_prints_the_usage_and_exits_zero() {
    for flag in ["--help", "-h"] {
        let out = run(&[flag]);
        assert!(out.status.success(), "{flag} must exit 0, got {:?}", out.status);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("Usage: ramsleuth-client ["),
            "{flag} prints the usage, got: {stdout}"
        );
        assert!(
            stdout.contains("-V, --version"),
            "{flag} lists --version in the usage, got: {stdout}"
        );
        // The five subcommands are all listed (the probe / burn parity
        // pair included).
        for sub in ["dump", "bench", "status", "probe", "burn"] {
            assert!(
                stdout.contains(sub),
                "{flag} lists the `{sub}` subcommand in the usage, got: {stdout}"
            );
        }
    }
}

/// (c) `<subcommand> --help` short-circuits before parsing / connecting
/// (exit 0, the usage on stdout) — the `probe` / `burn` parity pair
/// included.
#[test]
fn subcommand_help_exits_zero() {
    for args in [
        vec!["probe", "--help"],
        vec!["probe", "-h"],
        vec!["burn", "--help"],
        vec!["burn", "-h"],
    ] {
        let out = run(&args);
        assert!(
            out.status.success(),
            "{args:?} must exit 0, got {:?}",
            out.status
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("Usage: ramsleuth-client ["),
            "{args:?} prints the usage, got: {stdout}"
        );
    }
}
