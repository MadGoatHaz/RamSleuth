//! End-to-end `--version` / `--help` behavior of the `ramsleuth-tui` binary
//! (spawned via `CARGO_BIN_EXE_ramsleuth-tui` — the real bin, not the test
//! harness): both short-circuit before any app work and exit `0`, and
//! the version line carries the package version.

use std::process::Command;

fn bin_path() -> std::path::PathBuf {
    // Prefer cargo's bin pointer (set by toolchains that provide it);
    // otherwise derive the bin from this test exe's location:
    // `<target>/debug/deps/cli-<hash>` -> sibling `<target>/debug/ramsleuth-tui>`.
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_ramsleuth-tui") {
        return std::path::PathBuf::from(path);
    }
    let exe = std::env::current_exe().expect("current test exe");
    // `<target>/debug/deps/cli-<hash>`: the bin is one level up.
    let debug_dir = exe.parent().and_then(|d| d.parent()).expect("target/debug");
    debug_dir.join("ramsleuth-tui")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin_path())
        .args(args)
        .output()
        .expect("the ramsleuth-tui binary must spawn")
}

/// (a) `--version` / `-V`: exit 0, exactly the version line on stdout.
#[test]
fn version_prints_the_package_version_and_exits_zero() {
    for flag in ["--version", "-V"] {
        let out = run(&[flag]);
        assert!(out.status.success(), "{flag} must exit 0, got {:?}", out.status);
        let expected = format!("ramSleuth ramsleuth-tui v{}", env!("CARGO_PKG_VERSION"));
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
            stdout.contains("Usage: ramsleuth-tui ["),
            "{flag} prints the usage, got: {stdout}"
        );
        assert!(
            stdout.contains("-V, --version"),
            "{flag} lists --version in the usage, got: {stdout}"
        );
    }
}
