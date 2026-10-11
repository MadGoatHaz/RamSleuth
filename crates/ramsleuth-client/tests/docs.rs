//! Docs-parity confirming tests (CLI-1, CLI-5): read the shipped
//! `Docs/User_Guide.md` §6 (`Running the CLI`) and assert it lists the
//! **full** CLI surface — all five subcommands + the `probe`/`burn` flags,
//! and all five dashboard sections (including `Vcore`). The guide was
//! stale (three subcommands, four sections); these tests pin it to the
//! real `ramsleuth-client` surface so a future regression is a test
//! failure, not a silent doc drift.

use std::path::Path;

/// The repo-root `Docs/User_Guide.md` (the client crate is at
/// `crates/ramsleuth-client`, so two levels up).
fn user_guide_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../Docs/User_Guide.md")
}

/// The §6 section body: from the `## 6.` heading to the next `## ` heading
/// (the `## 7.` that follows).
fn section_6() -> String {
    let text = std::fs::read_to_string(user_guide_path())
        .expect("Docs/User_Guide.md must exist in the repo");
    let start = text
        .find("## 6.")
        .expect("the User_Guide must have a §6 (Running the CLI)");
    let rest = &text[start..];
    let end = rest[8..]
        .find("\n## ")
        .map(|i| i + 8)
        .unwrap_or(rest.len());
    rest[..end].to_owned()
}

/// TC-CLI1 (the §6 usage-block doc-parity confirming test): §6 lists
/// **all five** subcommands (`dump`/`bench`/`status`/`probe`/`burn`) and
/// the `probe`/`burn` flags (`--stdout`/`--json`/`--duration`). Pre-fix the
/// guide listed only three subcommands and omitted all three of these
/// flags (the code was correct; the guide was stale).
#[test]
fn tc_cli1_section_6_lists_all_five_subcommands_and_flags() {
    let s6 = section_6();
    for subcommand in ["dump", "bench", "status", "probe", "burn"] {
        assert!(
            s6.contains(subcommand),
            "§6 must list the `{subcommand}` subcommand"
        );
    }
    for flag in ["--stdout", "--json", "--duration"] {
        assert!(s6.contains(flag), "§6 must document the `{flag}` flag");
    }
}

/// TC-CLI5 (the §6 "five sections" doc-parity confirming test): §6 names
/// **all five** dashboard sections `dump` prints — `CPU`, `Vcore`, `AMD`,
/// `Intel`, and `SPD`. Pre-fix the guide said "four sections" and omitted
/// the `=== Vcore ===` section (which `dump` already prints).
#[test]
fn tc_cli5_section_6_names_the_vcore_section() {
    let s6 = section_6();
    for section in ["CPU", "Vcore", "AMD", "Intel", "SPD"] {
        assert!(
            s6.contains(&format!("=== {section} ===")),
            "§6 must name the `=== {section} ===` dashboard section"
        );
    }
    // The "five sections" framing (not the stale "four sections").
    assert!(
        s6.contains("five sections"),
        "§6 must say the dashboard has five sections"
    );
}
