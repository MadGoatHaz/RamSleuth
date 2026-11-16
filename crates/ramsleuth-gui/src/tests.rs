use std::fs;

use ramsleuth_bench::BenchmarkGrid;
use ramsleuth_gui::{BenchState, CapacityUnit, ClockUnit, DEFAULT_POLL_INTERVAL_MS};
use ramsleuth_telemetry::amd_pm::{AmdPmCadBus, AmdPmSnapshot, AmdPmTimings, AmdPmVoltages};
use ramsleuth_telemetry::amd_readout::EccStatus;
use ramsleuth_telemetry::amd_readout::{
    map_amd, AmdReadout, CadBus, ClockReadout, DivMode, GearMode, TimingSet, VoltageSet,
};
use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor, IntelGen};
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::intel_readout::{ChannelMode, IntelChannel, IntelReadout};
use ramsleuth_telemetry::spd_decode::SpdModule;
use ramsleuth_telemetry::{ProbeSystem, SystemMemoryTelemetry, SystemPlatform};

use super::*;

/// A unique temp out dir per test (pid-scoped — the style / client
/// temp-path precedent); created now, removed by the test.
fn temp_out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ramsleuth-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("the test out dir must be created");
    dir
}

/// A representative benchmark grid (the Phase 1 reference
/// magnitudes — the style test fixture).
fn fixture_grid() -> BenchmarkGrid {
    BenchmarkGrid {
        read_gbps: [26.35, 35.10, 30.40, 12.11],
        write_gbps: [43.63, 38.20, 33.05, 14.02],
        copy_gbps: [12.11, 36.40, 31.80, 11.05],
        latency_ns: [86.84, 1.12, 4.20, 13.90],
    }
}

/// (a) No flags → the default socket (the protocol's frozen value).
#[test]
fn parse_args_no_flags_defaults() {
    let args = parse_args(Vec::<String>::new()).expect("no flags must parse");
    assert_eq!(args, GuiArgs::default());
    assert_eq!(args.socket, PathBuf::from(DEFAULT_SOCKET_PATH));
}

/// (a) `--socket /tmp/x` → that socket (a repeat wins — last
/// value).
#[test]
fn parse_args_socket_flag() {
    let args = parse_args(["--socket", "/tmp/x"]).expect("--socket /tmp/x must parse");
    assert_eq!(args.socket, PathBuf::from("/tmp/x"));

    let args =
        parse_args(["--socket", "/tmp/a", "--socket", "/tmp/b"]).expect("repeats must parse");
    assert_eq!(args.socket, PathBuf::from("/tmp/b"));
}

/// (a) An unknown flag → an error naming it.
#[test]
fn parse_args_unknown_flag_errors() {
    let err = parse_args(["--bogus"]).expect_err("an unknown flag must error");
    assert!(
        err.contains("--bogus"),
        "the error must name the flag, got: {err}"
    );
}

/// (a) A positional argument / a missing `--socket` value → an
/// error.
#[test]
fn parse_args_positional_and_missing_value_error() {
    assert!(parse_args(["/tmp/x"]).is_err(), "a positional must error");
    assert!(
        parse_args(["--socket"]).is_err(),
        "a missing --socket value must error"
    );
}

/// (b) `ExportJson` with telemetry set → a JSON file is written to
/// the out dir, `Ok(Some(path))`, the file exists + parses as a
/// JSON object carrying the terminal grid under the `bench` key
/// (C6-29 — the grid is exported alongside the telemetry).
#[test]
fn perform_export_json_writes_and_parses() {
    let dir = temp_out_dir("export");
    let data = TelemetryData {
        telemetry: Some(ramsleuth_telemetry::collect()),
        bench: BenchState {
            grid: Some(fixture_grid()),
            ..Default::default()
        },
        ..Default::default()
    };

    let path = perform_export(GuiAction::ExportJson, &data, &dir)
        .expect("the export must not fail")
        .expect("telemetry set must produce a file");
    assert!(
        path.starts_with(&dir),
        "the file must land in the out dir: {path:?}"
    );
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .expect("the file must be named");
    assert!(name.starts_with("ramsleuth-export-"), "got: {name}");
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("json"));

    let text = fs::read_to_string(&path).expect("the exported file must be readable");
    let value: serde_json::Value =
        serde_json::from_str(&text).expect("the export must be valid JSON");
    assert!(
        value.is_object(),
        "a snapshot must serialize to a JSON object"
    );
    assert!(
        value["bench"].is_object(),
        "a completed run must export the bench grid object, got: {}",
        value["bench"]
    );

    fs::remove_dir_all(&dir).expect("cleanup");
}

/// (c) `SnapshotPng` with a grid set → a PNG file is written to
/// the out dir, `Ok(Some(path))`, and its first 8 bytes are the
/// PNG signature.
#[test]
fn perform_export_png_writes_valid_png() {
    let dir = temp_out_dir("snapshot");
    let data = TelemetryData {
        bench: BenchState {
            grid: Some(fixture_grid()),
            ..Default::default()
        },
        ..Default::default()
    };

    let path = perform_export(GuiAction::SnapshotPng, &data, &dir)
        .expect("the snapshot must not fail")
        .expect("a grid set must produce a file");
    assert!(
        path.starts_with(&dir),
        "the file must land in the out dir: {path:?}"
    );
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .expect("the file must be named");
    assert!(name.starts_with("ramsleuth-snapshot-"), "got: {name}");
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));

    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let bytes = fs::read(&path).expect("the snapshot file must be readable");
    assert_eq!(
        &bytes[..8],
        &PNG_SIG,
        "the file must start with the PNG signature"
    );

    fs::remove_dir_all(&dir).expect("cleanup");
}

/// (d) `SnapshotPng` with no grid → `Ok(None)` and no file is
/// written.
#[test]
fn perform_export_png_without_grid_is_none() {
    let dir = temp_out_dir("snapshot-none");
    let data = TelemetryData::default(); // no grid yet

    let result =
        perform_export(GuiAction::SnapshotPng, &data, &dir).expect("no grid must not fail");
    assert_eq!(result, None, "no grid must yield Ok(None)");
    assert!(
        dir.read_dir().expect("the out dir must exist").count() == 0,
        "no file must be written"
    );

    fs::remove_dir_all(&dir).expect("cleanup");
}

/// (d) mirror: `ExportJson` with no telemetry → `Ok(None)` and no
/// file is written.
#[test]
fn perform_export_json_without_telemetry_is_none() {
    let dir = temp_out_dir("export-none");
    let data = TelemetryData::default(); // no snapshot yet

    let result =
        perform_export(GuiAction::ExportJson, &data, &dir).expect("no telemetry must not fail");
    assert_eq!(result, None, "no telemetry must yield Ok(None)");
    assert!(
        dir.read_dir().expect("the out dir must exist").count() == 0,
        "no file must be written"
    );

    fs::remove_dir_all(&dir).expect("cleanup");
}

/// (e) `Quit` / `None` → `Ok(None)` and no file is written — even
/// with data present.
#[test]
fn perform_export_quit_and_none_are_noops() {
    let dir = temp_out_dir("noop");
    let data = TelemetryData {
        telemetry: Some(ramsleuth_telemetry::collect()),
        bench: BenchState {
            grid: Some(fixture_grid()),
            ..Default::default()
        },
        ..Default::default()
    };

    for action in [GuiAction::Quit, GuiAction::None] {
        let result = perform_export(action, &data, &dir).expect("a noop action must not fail");
        assert_eq!(result, None, "{action:?} must yield Ok(None)");
    }
    assert!(
        dir.read_dir().expect("the out dir must exist").count() == 0,
        "a noop action must write no file"
    );

    fs::remove_dir_all(&dir).expect("cleanup");
}

/// A failed export (a missing out dir) is a structured
/// `GuiError::Io`, never a panic (the no-panic contract, plan D5 —
/// the style test precedent).
#[test]
fn perform_export_missing_dir_is_io_error() {
    let data = TelemetryData {
        telemetry: Some(ramsleuth_telemetry::collect()),
        ..Default::default()
    };
    let missing = std::env::temp_dir()
        .join(format!("ramsleuth-nodir-{}", std::process::id()))
        .join("no-such-subdir");
    let err = perform_export(GuiAction::ExportJson, &data, &missing)
        .expect_err("a missing dir must fail");
    assert!(
        matches!(err, GuiError::Io(_)),
        "a missing out dir must map to GuiError::Io, got: {err:?}"
    );
}

/// `daemon_status_indicator`: GREEN + `Daemon OK` only when
/// connected + healthy; RED + `Daemon` for an error, a
/// disconnected status, or a never-polled (empty) status.
#[test]
fn daemon_status_indicator_semantics() {
    let healthy = TelemetryData {
        daemon_status: "connected: /tmp/x".to_owned(),
        error: None,
        ..Default::default()
    };
    assert_eq!(
        daemon_status_indicator(&healthy),
        (DAEMON_OK_GREEN, "Daemon OK")
    );

    let errored = TelemetryData {
        daemon_status: "connected: /tmp/x".to_owned(),
        error: Some("boom".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        daemon_status_indicator(&errored),
        (DAEMON_DOWN_RED, "Daemon")
    );

    let disconnected = TelemetryData {
        daemon_status: "disconnected".to_owned(),
        ..Default::default()
    };
    assert_eq!(
        daemon_status_indicator(&disconnected),
        (DAEMON_DOWN_RED, "Daemon")
    );

    assert_eq!(
        daemon_status_indicator(&TelemetryData::default()),
        (DAEMON_DOWN_RED, "Daemon")
    );
}

/// `notice_color`: a written file reads CYAN, a failed export
/// CRIMSON, the informational hint AMBER.
#[test]
fn notice_color_semantics() {
    assert_eq!(notice_color("wrote /home/x/ramsleuth-export-1.json"), CYAN);
    assert_eq!(notice_color("! export I/O error: gone"), CRIMSON);
    assert_eq!(notice_color("nothing to export yet (no data)"), AMBER);
}

/// `unix_timestamp`: a plausible wall-clock second count (2020 –
/// 2100), never a panic.
#[test]
fn unix_timestamp_is_plausible() {
    let ts = unix_timestamp();
    assert!(
        ts > 1_577_836_800,
        "expected a post-2020 timestamp, got: {ts}"
    );
    assert!(
        ts < 4_102_444_800,
        "expected a pre-2100 timestamp, got: {ts}"
    );
}

// ------------------------------------------------------------------
// C6-20: the header's pure line builders (no egui context, no
// display — the live window run is the QA gate).
// ------------------------------------------------------------------

/// A header-test snapshot: a known AMD Zen 3 CPU + a populated
/// platform (one Na cell — AGESA, the common case), a
/// configurable capacity tail, an Na AMD / Intel branch by
/// default, and a configurable SPD list.
fn fixture_telemetry(
    total_capacity: Section<f64>,
    dimm_sizes: Vec<Section<f64>>,
    spd: Vec<SpdModule>,
) -> SystemMemoryTelemetry {
    SystemMemoryTelemetry {
        cpu: CpuInfo {
            vendor: CpuVendor::Amd(AmdZen::Zen3),
            brand: "Ryzen 9 5950X".to_owned(),
        },
        amd: Section::na(NaReason::NotApplicable),
        intel: Section::na(NaReason::NotApplicable),
        spd,
        platform: SystemPlatform {
            cpu_clock_mhz: Section::Value(3600.0),
            motherboard: Section::Value("ProArt X570-CREATOR".to_owned()),
            bios: Section::Value("F60 + 09/15/2024".to_owned()),
            agesa: Section::na(NaReason::NotApplicable),
            smu_version: Section::na(NaReason::NotApplicable),
        },
        total_capacity,
        dimm_sizes,
        vcore: Section::na(NaReason::NotApplicable),
    }
}

/// A minimal SPD module fixture (every cell Na except the speed).
fn fixture_spd_module(speed_mts: Option<u16>) -> SpdModule {
    SpdModule {
        index: 0x52,
        is_ddr5: false,
        maker: Section::na(NaReason::NotApplicable),
        die_maker: Section::na(NaReason::NotApplicable),
        die_type: Section::na(NaReason::NotApplicable),
        devices: Section::na(NaReason::NotApplicable),
        part: Section::na(NaReason::NotApplicable),
        serial: Section::na(NaReason::NotApplicable),
        rank: Section::na(NaReason::NotApplicable),
        density_mbit: Section::na(NaReason::NotApplicable),
        speed_mts: match speed_mts {
            Some(value) => Section::Value(value),
            None => Section::na(NaReason::NotApplicable),
        },
        profiles: Vec::new(),
    }
}

/// A clock-readout fixture (every cell Na except the two the mode
/// segment consumes).
fn fixture_clocks(div_mode: Option<DivMode>, mclk_mhz: Option<f64>) -> ClockReadout {
    ClockReadout {
        mclk_mhz: mclk_mhz
            .map(Section::Value)
            .unwrap_or_else(|| Section::na(NaReason::NotApplicable)),
        uclk_mhz: Section::na(NaReason::NotApplicable),
        fclk_mhz: Section::na(NaReason::NotApplicable),
        div_mode: div_mode
            .map(Section::Value)
            .unwrap_or_else(|| Section::na(NaReason::NotApplicable)),
        gear_mode: Section::na(NaReason::NotApplicable),
        gdm: Section::na(NaReason::NotApplicable),
        pdm: Section::na(NaReason::NotApplicable),
        command_rate: Section::na(NaReason::NotApplicable),
    }
}

/// The MCLK-derived MT/s is a **single source** across reporters:
/// the GUI's `mclk_derived_speed` mirrors the probe's `derived_mts`
/// (MCLK × 2, DDR double-pumping) so the GUI summary line renders
/// the SAME value. The 12600KF fixture (the corrected 1800 MHz
/// MCLK from the Alder Lake half-step decode of the real
/// 0x79E00124 word) renders **3600 MT/s** — not the pre-fix 7200.
#[test]
fn mclk_mt_s_is_single_source_across_reporters() {
    fn na<T>() -> Section<T> {
        Section::na(NaReason::NotApplicable)
    }
    let t = SystemMemoryTelemetry {
        cpu: CpuInfo {
            vendor: CpuVendor::Intel(IntelGen::AlderLake),
            brand: "Intel(R) Core(TM) i5-12600KF CPU @ 3.50GHz".to_owned(),
        },
        amd: na(),
        intel: Section::Value(IntelReadout {
            channels: vec![IntelChannel {
                index: 0,
                clocks: ClockReadout {
                    mclk_mhz: Section::Value(1800.0),
                    uclk_mhz: Section::Value(1800.0),
                    fclk_mhz: na(),
                    div_mode: na(),
                    gear_mode: Section::Value(GearMode::One),
                    gdm: na(),
                    pdm: na(),
                    command_rate: na(),
                },
                timings: TimingSet {
                    cl: na(),
                    rcwdwr: na(),
                    rcdrd: na(),
                    rp: na(),
                    ras: na(),
                    rc: na(),
                    rrds: na(),
                    rrld: na(),
                    faw: na(),
                    wtrs: na(),
                    wtrl: na(),
                    wr: na(),
                    rfc1: na(),
                    rfc2: na(),
                    rfcsb: na(),
                    cwl: na(),
                    rtp: na(),
                    rdwr: na(),
                    wrrd: na(),
                    rdrd_sd: na(),
                    rdrd_dd: na(),
                    rdrd_scl: na(),
                    rdrd_sc: na(),
                    wrwr_sd: na(),
                    wrwr_dd: na(),
                    wrwr_scl: na(),
                    wrwr_sc: na(),
                },
                cad_bus: CadBus {
                    proc_odt: na(),
                    rtt_nom: na(),
                    rtt_wr: na(),
                    rtt_park: na(),
                    clk_drv: na(),
                    addr_cmd_drv: na(),
                    cs_odt_drv: na(),
                    cke_drv: na(),
                },
                voltages: VoltageSet {
                    vddcr_soc_mv: na(),
                    vddio_mem_mv: na(),
                    vdd_misc_mv: na(),
                    vpp_mv: na(),
                    vcore_mv: na(),
                },
                rtl: na(),
            }],
            channel_mode: None,
            ecc_status: EccStatus::Unknown,
            memory_tech: None,
            channel_trust: None,
            channel_annotation: String::new(),
            dimm_count: 2,
        }),
        spd: Vec::new(),
        platform: SystemPlatform {
            cpu_clock_mhz: na(),
            motherboard: na(),
            bios: na(),
            agesa: na(),
            smu_version: na(),
        },
        total_capacity: na(),
        dimm_sizes: vec![Section::Value(16.0), Section::Value(16.0)],
        vcore: na(),
    };
    assert_eq!(super::mclk_derived_speed(&t), Some("3600 MT/s".to_owned()));
}

/// (h1) `platform_badge`: the vendor's short word — any AMD Zen
/// → `AMD`, any Intel → `Intel`, the unrecognized vendor →
/// `Unknown`.
#[test]
fn platform_badge_maps_every_vendor() {
    for zen in [
        AmdZen::Zen1,
        AmdZen::Zen2,
        AmdZen::Zen3,
        AmdZen::Zen4,
        AmdZen::Zen5,
        AmdZen::Zen6,
    ] {
        assert_eq!(platform_badge(&CpuVendor::Amd(zen)), "AMD");
    }
    assert_eq!(
        platform_badge(&CpuVendor::Intel(IntelGen::AlderLake)),
        "Intel"
    );
    assert_eq!(platform_badge(&CpuVendor::Unknown), "Unknown");
}

/// (h3) `cpu_line_text`: the spec's line 2 — brand + the clock in
/// the selected clock unit (C7-11: `format_clock` — the default
/// MHz keeps the carried wire value, the GHz arm ÷1000) +
/// motherboard / BIOS + the provenance fragment (C8-11, D-1) —
/// with both cells Na it degrades to `AGESA N/A`; a fully Na
/// platform degrades every segment.
#[test]
fn cpu_line_text_populated_and_na_degraded() {
    let t = fixture_telemetry(
        Section::Value(32.0),
        vec![Section::Value(16.0), Section::Value(16.0)],
        Vec::new(),
    );
    assert_eq!(
            cpu_line_text(&t, &Units::default()),
            "CPU: Ryzen 9 5950X @ 3600 MHz | Motherboard: ProArt X570-CREATOR (BIOS: F60 + 09/15/2024, AGESA N/A)"
        );

    // The GHz knob (C7-11): the same carried clock renders ÷1000
    // (3600 MHz → 3.6 GHz).
    let ghz = Units {
        clock: ClockUnit::GHz,
        ..Units::default()
    };
    assert_eq!(
            cpu_line_text(&t, &ghz),
            "CPU: Ryzen 9 5950X @ 3.6 GHz | Motherboard: ProArt X570-CREATOR (BIOS: F60 + 09/15/2024, AGESA N/A)"
        );

    let all_na = SystemMemoryTelemetry {
        platform: SystemPlatform {
            cpu_clock_mhz: Section::na(NaReason::NotApplicable),
            motherboard: Section::na(NaReason::NotApplicable),
            bios: Section::na(NaReason::NotApplicable),
            agesa: Section::na(NaReason::NotApplicable),
            smu_version: Section::na(NaReason::NotApplicable),
        },
        ..t
    };
    assert_eq!(
        cpu_line_text(&all_na, &Units::default()),
        "CPU: Ryzen 9 5950X @ N/A | Motherboard: N/A (BIOS: N/A, AGESA N/A)"
    );
}

/// (h3′) `age_fragment` (C8-11, D-1): the provenance arms — a
/// true AGESA (the BIOS-string scan) wins and suppresses the SMU
/// version (D-1's precedence); else the shape-checked `ryzen_smu`
/// firmware version renders under its own `SMU` label; else the
/// honest `AGESA N/A`. No `AGESA <smu value>` hybrid, never
/// fabricated.
#[test]
fn age_fragment_by_provenance() {
    let base = SystemPlatform {
        cpu_clock_mhz: Section::Value(3600.0),
        motherboard: Section::Value("ProArt X570-CREATOR".to_owned()),
        bios: Section::Value("5601".to_owned()),
        agesa: Section::na(NaReason::NotApplicable),
        smu_version: Section::na(NaReason::NotApplicable),
    };
    assert_eq!(age_fragment(&base), "AGESA N/A");

    let smu = SystemPlatform {
        smu_version: Section::Value("56.78.0".to_owned()),
        ..base.clone()
    };
    assert_eq!(age_fragment(&smu), "SMU 56.78.0");

    let agesa = SystemPlatform {
        agesa: Section::Value("ComboAm4v2 PI 1.2.0.12".to_owned()),
        ..base.clone()
    };
    assert_eq!(age_fragment(&agesa), "AGESA ComboAm4v2 PI 1.2.0.12");

    // The precedence (D-1): a true AGESA suppresses the SMU value.
    let both = SystemPlatform {
        agesa: Section::Value("ComboAm4v2 PI 1.2.0.12".to_owned()),
        smu_version: Section::Value("56.78.0".to_owned()),
        ..base.clone()
    };
    assert_eq!(age_fragment(&both), "AGESA ComboAm4v2 PI 1.2.0.12");
}

/// (h3″) `cpu_line_text`'s provenance fragment in the header
/// (C8-11, D-1): the live host shape — `bios = 5601` (a bare OEM
/// build code, no AGESA token), no true AGESA (the real string is
/// root-gated), the `ryzen_smu` firmware `56.78.0` → `… (BIOS:
/// 5601, SMU 56.78.0)` — the reported `AGESA 56.78.0` mislabel
/// pinned as a regression; a true AGESA renders `AGESA <v>` with
/// the SMU value suppressed (D-1's precedence).
#[test]
fn cpu_line_text_provenance_fragment() {
    // The live host shape: the SMU firmware under its true label.
    let mut t = fixture_telemetry(Section::Value(32.0), Vec::new(), Vec::new());
    t.platform.bios = Section::Value("5601".to_owned());
    t.platform.smu_version = Section::Value("56.78.0".to_owned());
    assert_eq!(
            cpu_line_text(&t, &Units::default()),
            "CPU: Ryzen 9 5950X @ 3600 MHz | Motherboard: ProArt X570-CREATOR (BIOS: 5601, SMU 56.78.0)"
        );

    // A true AGESA (the BIOS-string scan) wins over the SMU value.
    let mut t = fixture_telemetry(Section::Value(32.0), Vec::new(), Vec::new());
    t.platform.agesa = Section::Value("ComboAm4v2 PI 1.2.0.12".to_owned());
    t.platform.smu_version = Section::Value("56.78.0".to_owned());
    assert_eq!(
            cpu_line_text(&t, &Units::default()),
            "CPU: Ryzen 9 5950X @ 3600 MHz | Motherboard: ProArt X570-CREATOR (BIOS: F60 + 09/15/2024, AGESA ComboAm4v2 PI 1.2.0.12)"
        );
}

/// (h4) `dimm_summary`: distinct-size grouping in the selected
/// capacity unit (C7-11: `format_capacity` — default GiB, the GB
/// knob converts × 1.073741824) — 2×16 → `2x16 GiB`, a mixed kit
/// → `1x16 GiB + 1x32 GiB` (the Na entry contributes nothing),
/// all-Na / empty → `N/A`, a non-whole size keeps one decimal;
/// C9-01 (D-1): the parallel SPD list's rank word groups with
/// the size — same-size same-rank stays one `<count>x` group
/// with the word (`2x16 GiB Single-Rank`), same-size
/// different-rank splits into two groups, and a `Na`/`0` rank
/// omits the word (the bare form; the mixed-kit case keeps
/// `1x16 GiB + 1x32 GiB` when the ranks are absent).
#[test]
fn dimm_summary_groups_and_degrades() {
    let default_units = Units::default();
    assert_eq!(
        dimm_summary(
            &[Section::Value(16.0), Section::Value(16.0)],
            &[],
            &default_units
        ),
        "2x16 GiB"
    );
    assert_eq!(
        dimm_summary(
            &[
                Section::Value(16.0),
                Section::na(NaReason::NotApplicable),
                Section::Value(32.0),
            ],
            &[],
            &default_units,
        ),
        "1x16 GiB + 1x32 GiB"
    );
    assert_eq!(
        dimm_summary(&[Section::na(NaReason::NotApplicable)], &[], &default_units),
        "N/A"
    );
    assert_eq!(dimm_summary(&[], &[], &default_units), "N/A");
    assert_eq!(
        dimm_summary(&[Section::Value(4.5)], &[], &default_units),
        "1x4.5 GiB"
    );

    // The GB knob (C7-11): 16 GiB → 17.2 GB per group.
    let gb = Units {
        capacity: CapacityUnit::GB,
        ..Units::default()
    };
    assert_eq!(
        dimm_summary(&[Section::Value(16.0), Section::Value(16.0)], &[], &gb),
        "2x17.2 GB"
    );

    // C9-01 (D-1): the rank word from the parallel SPD list
    // groups with the size (single-rank fixture modules).
    let single_rank = || {
        let mut module = fixture_spd_module(Some(3200));
        module.rank = Section::Value(1);
        module
    };
    let dual_rank = || {
        let mut module = fixture_spd_module(Some(3200));
        module.rank = Section::Value(2);
        module
    };
    assert_eq!(
        dimm_summary(
            &[Section::Value(16.0), Section::Value(16.0)],
            &[single_rank(), single_rank()],
            &default_units,
        ),
        "2x16 GiB Single-Rank"
    );
    assert_eq!(
        dimm_summary(
            &[Section::Value(16.0), Section::Value(16.0)],
            &[single_rank(), dual_rank()],
            &default_units,
        ),
        "1x16 GiB Single-Rank + 1x16 GiB Dual-Rank"
    );
    assert_eq!(
        dimm_summary(
            &[Section::Value(16.0), Section::Value(32.0)],
            &[single_rank(), dual_rank()],
            &default_units,
        ),
        "1x16 GiB Single-Rank + 1x32 GiB Dual-Rank"
    );
    // A `Na` rank omits the word: the ranked + unranked pair
    // splits into two groups (first-seen order).
    assert_eq!(
        dimm_summary(
            &[Section::Value(16.0), Section::Value(16.0)],
            &[single_rank(), fixture_spd_module(None)],
            &default_units,
        ),
        "1x16 GiB Single-Rank + 1x16 GiB"
    );
}

/// (h4b) `rank_word` (the shared `ramsleuth-display` form, C9-01,
/// D-1): the SPD cards' rank label for the compact header — `1` →
/// `Single-Rank`, `2` → `Dual-Rank`, `n > 0` → `<n>-Rank` — with a
/// `Na`/`0` rank yielding `None` (the word is omitted, never `N/A`).
#[test]
fn rank_word_arms() {
    assert_eq!(
        ramsleuth_display::rank_word(&Section::Value(1)),
        Some("Single-Rank".to_owned())
    );
    assert_eq!(
        ramsleuth_display::rank_word(&Section::Value(2)),
        Some("Dual-Rank".to_owned())
    );
    assert_eq!(
        ramsleuth_display::rank_word(&Section::Value(4)),
        Some("4-Rank".to_owned())
    );
    assert_eq!(ramsleuth_display::rank_word(&Section::Value(0)), None);
    assert_eq!(
        ramsleuth_display::rank_word(&Section::na(NaReason::NotApplicable)),
        None
    );
}

/// (h4c) `slot_note` (C9-01, D-1): the total-vs-breakdown
/// note — total > SPD sum with a uniform module size
/// near-dividing the total (the live host: 62.68 GiB over
/// 2×16 GiB, the OS reserves a fraction) → `2 of 4 slots
/// SPD-visible`; a non-integer quotient → the generic note;
/// total ≤ sum, a `Na` total, or no visible modules → `None`
/// (no false alarm).
#[test]
fn slot_note_arms() {
    // The live host shape: 4×16 GiB installed (62.68 GiB OS
    // total), 2 bound to the SPD bus (32 GiB sum).
    assert_eq!(
        slot_note(
            &Section::Value(62.68),
            &[Section::Value(16.0), Section::Value(16.0)]
        ),
        Some("2 of 4 slots SPD-visible".to_owned())
    );
    // An exact multiple (no reserved fraction): 3 slots, 2
    // visible.
    assert_eq!(
        slot_note(
            &Section::Value(48.0),
            &[Section::Value(16.0), Section::Value(16.0)]
        ),
        Some("2 of 3 slots SPD-visible".to_owned())
    );
    // A non-integer quotient (50/16 = 3.125, more than a tenth
    // of a module off) → the generic note.
    assert_eq!(
        slot_note(
            &Section::Value(50.0),
            &[Section::Value(16.0), Section::Value(16.0)]
        ),
        Some("SPD sees 2 of the installed capacity".to_owned())
    );
    // total ≤ sum → no note (the breakdown accounts for the
    // whole total).
    assert_eq!(
        slot_note(
            &Section::Value(32.0),
            &[Section::Value(16.0), Section::Value(16.0)]
        ),
        None
    );
    assert_eq!(
        slot_note(
            &Section::Value(16.0),
            &[Section::Value(16.0), Section::Value(16.0)]
        ),
        None
    );
    // A `Na` total → no note.
    assert_eq!(
        slot_note(
            &Section::na(NaReason::NotApplicable),
            &[Section::Value(16.0)]
        ),
        None
    );
    // No visible modules (all-Na sizes) → no note, even with a
    // `Value` total.
    assert_eq!(
        slot_note(
            &Section::Value(62.68),
            &[Section::na(NaReason::NotApplicable)]
        ),
        None
    );
}

/// (h5) `channel_mode`: 1 / 2 / 4 → Single / Dual / Quad, every
/// other count (0, odd) → `N/A` (D-C8).
#[test]
fn channel_mode_from_dimm_count() {
    assert_eq!(channel_mode(1), "Single-Channel");
    assert_eq!(channel_mode(2), "Dual-Channel");
    assert_eq!(channel_mode(4), "Quad-Channel");
    assert_eq!(channel_mode(0), "N/A");
    assert_eq!(channel_mode(3), "N/A");
}

/// (h5b) `intel_mode`: the hardware `MAD_INTER_CHANNEL` mode takes
/// the "Mode:" slot — DualSymmetric → `Interleaved` (CYAN),
/// DualFlex → `Flex` (AMBER), Single → `N/A` (the default color);
/// a non-Intel / no-mode readout → `None` (the caller falls back
/// to the AMD sync mode).
#[test]
fn intel_mode_matrix() {
    // Helper: a telemetry with the given intel section (AMD Na).
    fn intel_t(intel: Section<IntelReadout>) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Intel(IntelGen::Skylake),
                brand: "synthetic".to_owned(),
            },
            amd: Section::na(NaReason::NotApplicable),
            intel,
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::na(NaReason::NotApplicable),
            dimm_sizes: vec![Section::Value(16.0)],
            vcore: Section::na(NaReason::NotApplicable),
        }
    }
    assert_eq!(
        intel_mode(&intel_t(Section::Value(IntelReadout {
            channels: Vec::new(),
            channel_mode: Some(ChannelMode::DualSymmetric),
            ecc_status: EccStatus::Unknown,
            memory_tech: None,
            channel_trust: None,
            channel_annotation: String::new(),
            dimm_count: 0,
        }))),
        Some(("Interleaved".to_owned(), Some(CYAN)))
    );
    assert_eq!(
        intel_mode(&intel_t(Section::Value(IntelReadout {
            channels: Vec::new(),
            channel_mode: Some(ChannelMode::DualFlex),
            ecc_status: EccStatus::Unknown,
            memory_tech: None,
            channel_trust: None,
            channel_annotation: String::new(),
            dimm_count: 0,
        }))),
        Some(("Flex".to_owned(), Some(AMBER)))
    );
    assert_eq!(
        intel_mode(&intel_t(Section::Value(IntelReadout {
            channels: Vec::new(),
            channel_mode: Some(ChannelMode::Single),
            ecc_status: EccStatus::Unknown,
            memory_tech: None,
            channel_trust: None,
            channel_annotation: String::new(),
            dimm_count: 0,
        }))),
        Some(("N/A".to_owned(), None))
    );
    // No mode (the /dev/mem fallback / pre-24-attr module) → None.
    assert_eq!(
        intel_mode(&intel_t(Section::Value(IntelReadout {
            channels: Vec::new(),
            channel_mode: None,
            ecc_status: EccStatus::Unknown,
            memory_tech: None,
            channel_trust: None,
            channel_annotation: String::new(),
            dimm_count: 0,
        }))),
        None
    );
}

/// (h5c) The header channel label is hardware-preferred: a Flex-Mode
/// box (one bound SPD, Dual-Flex) shows `Dual-Channel (Flex)` — not
/// the SPD-count `Single-Channel`; no-mode Intel and the
/// no-readout states keep the SPD-count label.
#[test]
fn channel_label_hardware_preferred() {
    // Helper: a telemetry with the given intel section + one bound
    // 16 GiB DIMM (the Flex-Box shape the fix targets).
    fn one_dimm(intel: Section<IntelReadout>) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Intel(IntelGen::Skylake),
                brand: "synthetic".to_owned(),
            },
            amd: Section::na(NaReason::NotApplicable),
            intel,
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(16.0),
            dimm_sizes: vec![Section::Value(16.0)],
            vcore: Section::na(NaReason::NotApplicable),
        }
    }

    // The Flex-Box: one bound DIMM, hardware Dual-Flex → the
    // hardware label overrides the SPD-count "Single-Channel".
    let flex = one_dimm(Section::Value(IntelReadout {
        channels: Vec::new(),
        channel_mode: Some(ChannelMode::DualFlex),
        ecc_status: EccStatus::Unknown,
        memory_tech: None,
        channel_trust: None,
        channel_annotation: String::new(),
        dimm_count: 0,
    }));
    assert_eq!(
        ram_line_prefix(&flex, &Units::default()),
        "RAM: 16 GiB (1x16 GiB) | Dual-Channel (Flex) | N/A | Mode: "
    );

    // The same Flex-Box with the ECC capability decode (the
    // CAPID0_A value) → the enum label rides the ECC segment.
    let flex_ecc = one_dimm(Section::Value(IntelReadout {
        channels: Vec::new(),
        channel_mode: Some(ChannelMode::DualFlex),
        ecc_status: EccStatus::CapableButDisabled,
        memory_tech: None,
        channel_trust: None,
        channel_annotation: String::new(),
        dimm_count: 0,
    }));
    assert_eq!(
        ram_line_prefix(&flex_ecc, &Units::default()),
        "RAM: 16 GiB (1x16 GiB) | Dual-Channel (Flex) | ECC: Capable (disabled) | Mode: "
    );

    // No hardware mode (the /dev/mem fallback) → the SPD-count label.
    let no_mode = one_dimm(Section::Value(IntelReadout {
        channels: Vec::new(),
        channel_mode: None,
        ecc_status: EccStatus::Unknown,
        memory_tech: None,
        channel_trust: None,
        channel_annotation: String::new(),
        dimm_count: 0,
    }));
    assert_eq!(
        ram_line_prefix(&no_mode, &Units::default()),
        "RAM: 16 GiB (1x16 GiB) | Single-Channel | N/A | Mode: "
    );

    // No readout at all (AMD Na + Intel Na) → the SPD-count
    // label, unchanged.
    let amd = one_dimm(Section::na(NaReason::NotApplicable));
    assert_eq!(
        ram_line_prefix(&amd, &Units::default()),
        "RAM: 16 GiB (1x16 GiB) | Single-Channel | N/A | Mode: "
    );
}

/// (h5d) The header channel label is hardware-preferred on AMD
/// too: the UMC-synthesized [`MemoryChannelMode`] beats the
/// one-bound-DIMM SPD-count label (the live 5950X box: one bound
/// SPD, dual-channel silicon), and the platform [`EccStatus`]
/// label rides the ECC segment (`N/A` for the unreadable / absent
/// state — the non-root daemon's all-`Unknown` decode).
#[test]
fn amd_channel_mode_hardware_preferred() {
    // A PM-snapshot-mapped readout (the MCLK at 0 → Na, so no
    // derived-speed segment rides the line) with the channel mode
    // + ECC state overridden to the case under test.
    fn amd_readout(mode: MemoryChannelMode, ecc: EccStatus) -> AmdReadout {
        let mut readout = map_amd(&AmdPmSnapshot {
            version: 0x0007_0B02,
            pm_exposed: true,
            smn_setpoint_unverified: false,
            threadripper: false,
            siod: false,
            mclk_mhz: 0,
            uclk_mhz: 1600,
            fclk_mhz: 1600,
            div_mode: 0,
            gdm: 1,
            pdm: 0,
            command_rate: 0,
            timings: AmdPmTimings {
                cl: 16,
                rcwdwr: 16,
                rcdrd: 16,
                rp: 16,
                ras: 32,
                rc: 48,
                rrds: 4,
                rrld: 4,
                faw: 16,
                wtrs: 8,
                wtrl: 8,
                wr: 8,
                rfc1: 160,
                rfc2: 160,
                rfcsb: 160,
                cwl: 16,
                rtp: 8,
                rdwr: 8,
                wrrd: 4,
                rdrd_sd: 100,
                rdrd_dd: 101,
                rdrd_scl: 102,
                rdrd_sc: 103,
                wrwr_sd: 104,
                wrwr_dd: 105,
                wrwr_scl: 106,
                wrwr_sc: 107,
            },
            cad_bus: AmdPmCadBus {
                proc_odt: 5,
                rtt_nom: 2,
                rtt_wr: 0,
                rtt_park: 4,
                clk_drv: 6,
                addr_cmd_drv: 8,
                cs_odt_drv: 10,
                cke_drv: 12,
            },
            voltages: AmdPmVoltages {
                vddcr_soc_mv: 1150,
                vddio_mem_mv: 1350,
                vdd_misc_mv: 1000,
                vpp_mv: 1800,
                vcore_mv: 1150,
            },
        });
        readout.channel_mode = mode;
        readout.ecc_status = ecc;
        readout
    }
    // Helper: an AMD telemetry with the given readout + one bound
    // 16 GiB DIMM (the SPD-count label would read
    // "Single-Channel").
    fn amd_t(mode: MemoryChannelMode, ecc: EccStatus) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "synthetic".to_owned(),
            },
            amd: Section::Value(amd_readout(mode, ecc)),
            intel: Section::na(NaReason::UnsupportedHardware),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(16.0),
            dimm_sizes: vec![Section::Value(16.0)],
            vcore: Section::na(NaReason::NotApplicable),
        }
    }
    // The hardware mode beats the one-bound-DIMM SPD count.
    assert_eq!(
        ram_line_prefix(
            &amd_t(
                MemoryChannelMode::DualSymmetric,
                EccStatus::CapableButDisabled
            ),
            &Units::default(),
        ),
        "RAM: 16 GiB (1x16 GiB) | Dual-Channel (Symmetric) | ECC: Capable (disabled) | Mode: "
    );
    // The all-`Unknown` decode (the non-root daemon) → the honest
    // N/A in both the channel and the ECC segment.
    assert_eq!(
        ram_line_prefix(
            &amd_t(MemoryChannelMode::Unknown, EccStatus::Unknown),
            &Units::default(),
        ),
        "RAM: 16 GiB (1x16 GiB) | N/A | N/A | Mode: "
    );
}

/// (h6) `ram_line_prefix`: the spec's line 3 minus the mode —
/// the total in the selected capacity unit (C7-11:
/// `format_capacity` — default GiB, the GB knob converts
/// × 1.073741824), the per-DIMM breakdown (C9-01, D-1: with
/// the rank word — `2x16 GiB Single-Rank`), the max SPD speed
/// (omitted when no module carries one), the channel mode, the
/// total-vs-breakdown slot note when the OS total strictly
/// exceeds the SPD sum (C9-01, D-1: `2 of 4 slots
/// SPD-visible`), and the `Mode: ` lead-in; the degraded tail
/// renders the honest N/A segments (no note — total ≤ sum / Na).
#[test]
fn ram_line_prefix_populated_and_degraded() {
    let t = fixture_telemetry(
        Section::Value(32.0),
        vec![Section::Value(16.0), Section::Value(16.0)],
        vec![fixture_spd_module(Some(3200))],
    );
    assert_eq!(
        ram_line_prefix(&t, &Units::default()),
        "RAM: 32 GiB (2x16 GiB) 3200 MT/s | Dual-Channel | N/A | Mode: "
    );

    let no_speed = fixture_telemetry(
        Section::Value(32.0),
        vec![Section::Value(16.0), Section::Value(16.0)],
        vec![fixture_spd_module(None)],
    );
    assert_eq!(
        ram_line_prefix(&no_speed, &Units::default()),
        "RAM: 32 GiB (2x16 GiB) | Dual-Channel | N/A | Mode: "
    );

    // A single Na DIMM still counts as one bound module (the
    // channel is the count, not the sizes): Single-Channel.
    let degraded = fixture_telemetry(
        Section::na(NaReason::NotApplicable),
        vec![Section::na(NaReason::NotApplicable)],
        Vec::new(),
    );
    assert_eq!(
        ram_line_prefix(&degraded, &Units::default()),
        "RAM: N/A (N/A) | Single-Channel | N/A | Mode: "
    );

    // No bound modules at all: every segment degrades to N/A.
    let empty = fixture_telemetry(Section::na(NaReason::NotApplicable), Vec::new(), Vec::new());
    assert_eq!(
        ram_line_prefix(&empty, &Units::default()),
        "RAM: N/A (N/A) | N/A | N/A | Mode: "
    );

    // The GB knob (C7-11): 32 GiB → 34.4 GB total, 16 GiB →
    // 17.2 GB per group.
    let gb = Units {
        capacity: CapacityUnit::GB,
        ..Units::default()
    };
    assert_eq!(
        ram_line_prefix(&t, &gb),
        "RAM: 34.4 GB (2x17.2 GB) 3200 MT/s | Dual-Channel | N/A | Mode: "
    );

    // C9-01 (D-1): the live host shape — single-rank modules
    // + the OS total (62.68 GiB) above the SPD sum (32 GiB):
    // the rank word in the breakdown + the slot note segment.
    let single_rank = || {
        let mut module = fixture_spd_module(Some(3200));
        module.rank = Section::Value(1);
        module
    };
    let ranked = fixture_telemetry(
        Section::Value(62.68),
        vec![Section::Value(16.0), Section::Value(16.0)],
        vec![single_rank(), single_rank()],
    );
    assert_eq!(
            ram_line_prefix(&ranked, &Units::default()),
            "RAM: 62.7 GiB (2x16 GiB Single-Rank) 3200 MT/s | Dual-Channel | 2 of 4 slots SPD-visible | N/A | Mode: "
        );

    // The rank word with total ≤ the SPD sum: no note (no
    // false alarm).
    let balanced = fixture_telemetry(
        Section::Value(32.0),
        vec![Section::Value(16.0), Section::Value(16.0)],
        vec![single_rank(), single_rank()],
    );
    assert_eq!(
        ram_line_prefix(&balanced, &Units::default()),
        "RAM: 32 GiB (2x16 GiB Single-Rank) 3200 MT/s | Dual-Channel | N/A | Mode: "
    );
}

/// (h6b) `ram_line_prefix` speed fallback: no bound module
/// carries a speed, so the line derives the rate from the
/// platform's MCLK × 2 — the AMD readout's MCLK (1200 MHz →
/// 2400 MT/s); an out-of-band MCLK (0 → Na) keeps the
/// pre-fallback omission.
#[test]
fn ram_line_prefix_speed_falls_back_to_mclk_x2() {
    fn amd_snapshot(mclk_mhz: u16) -> AmdPmSnapshot {
        AmdPmSnapshot {
            version: 0x0007_0B02,
            pm_exposed: true,
            smn_setpoint_unverified: false,
            threadripper: false,
            siod: false,
            mclk_mhz,
            uclk_mhz: 1600,
            fclk_mhz: 1600,
            div_mode: 0,
            gdm: 1,
            pdm: 0,
            command_rate: 0,
            timings: AmdPmTimings {
                cl: 16,
                rcwdwr: 16,
                rcdrd: 16,
                rp: 16,
                ras: 32,
                rc: 48,
                rrds: 4,
                rrld: 4,
                faw: 16,
                wtrs: 8,
                wtrl: 8,
                wr: 8,
                rfc1: 160,
                rfc2: 160,
                rfcsb: 160,
                cwl: 16,
                rtp: 8,
                rdwr: 8,
                wrrd: 4,
                rdrd_sd: 100,
                rdrd_dd: 101,
                rdrd_scl: 102,
                rdrd_sc: 103,
                wrwr_sd: 104,
                wrwr_dd: 105,
                wrwr_scl: 106,
                wrwr_sc: 107,
            },
            cad_bus: AmdPmCadBus {
                proc_odt: 5,
                rtt_nom: 2,
                rtt_wr: 0,
                rtt_park: 4,
                clk_drv: 6,
                addr_cmd_drv: 8,
                cs_odt_drv: 10,
                cke_drv: 12,
            },
            voltages: AmdPmVoltages {
                vddcr_soc_mv: 1150,
                vddio_mem_mv: 1350,
                vdd_misc_mv: 1000,
                vpp_mv: 1800,
                vcore_mv: 1150,
            },
        }
    }
    // The AMD readout's MCLK (1200 MHz → 2400 MT/s derived).
    let mut t = fixture_telemetry(
        Section::Value(32.0),
        vec![Section::Value(16.0), Section::Value(16.0)],
        vec![fixture_spd_module(None)],
    );
    t.amd = Section::Value(map_amd(&amd_snapshot(1200)));
    assert_eq!(
        ram_line_prefix(&t, &Units::default()),
        "RAM: 32 GiB (2x16 GiB) 2400 MT/s | N/A | N/A | Mode: "
    );
    // An out-of-band MCLK (0 → Na) keeps the pre-fallback omission.
    let mut na = fixture_telemetry(
        Section::Value(32.0),
        vec![Section::Value(16.0), Section::Value(16.0)],
        vec![fixture_spd_module(None)],
    );
    na.amd = Section::Value(map_amd(&amd_snapshot(0)));
    assert_eq!(
        ram_line_prefix(&na, &Units::default()),
        "RAM: 32 GiB (2x16 GiB) | N/A | N/A | Mode: "
    );
}

/// (h7) `sync_mode_from_clocks`: D-C8's ratio segment — 1:1 with
/// an MCLK (AMBER; C7-11: the MCLK in the selected clock unit —
/// default MHz, the GHz knob ÷1000), 1:1 without (AMBER, the bare
/// text), 1:2 (CRIMSON), and a Na ratio (the honest N/A, default
/// color).
#[test]
fn sync_mode_from_clocks_arms() {
    let default_units = Units::default();
    assert_eq!(
        sync_mode_from_clocks(
            &fixture_clocks(Some(DivMode::OneToOne), Some(1800.0)),
            &default_units,
        ),
        (
            "Synchronous 1:1 (UCLK = MCLK = 1800 MHz)".to_owned(),
            Some(AMBER)
        )
    );
    assert_eq!(
        sync_mode_from_clocks(
            &fixture_clocks(Some(DivMode::OneToOne), None),
            &default_units
        ),
        ("Synchronous 1:1".to_owned(), Some(AMBER))
    );
    assert_eq!(
        sync_mode_from_clocks(
            &fixture_clocks(Some(DivMode::OneToTwo), Some(1800.0)),
            &default_units,
        ),
        ("Asynchronous 1:2".to_owned(), Some(CRIMSON))
    );
    assert_eq!(
        sync_mode_from_clocks(&fixture_clocks(None, Some(1800.0)), &default_units),
        ("N/A".to_owned(), None)
    );

    // The GHz knob (C7-11): 1800 MHz → 1.8 GHz.
    let ghz = Units {
        clock: ClockUnit::GHz,
        ..Units::default()
    };
    assert_eq!(
        sync_mode_from_clocks(&fixture_clocks(Some(DivMode::OneToOne), Some(1800.0)), &ghz),
        (
            "Synchronous 1:1 (UCLK = MCLK = 1.8 GHz)".to_owned(),
            Some(AMBER)
        )
    );
}

/// (h8) `sync_mode`: an Na AMD branch (Intel silicon / the
/// driver missing) degrades the whole segment to an honest `N/A`
/// — never a panic (independent of the clock unit — the Na
/// branch renders no clock at all).
#[test]
fn sync_mode_degrades_on_na_amd_branch() {
    let t = fixture_telemetry(Section::Value(32.0), Vec::new(), Vec::new());
    assert_eq!(sync_mode(&t, &Units::default()), ("N/A".to_owned(), None));
    let ghz = Units {
        clock: ClockUnit::GHz,
        ..Units::default()
    };
    assert_eq!(sync_mode(&t, &ghz), ("N/A".to_owned(), None));
}

// ------------------------------------------------------------------
// C6-30: the keyboard key map, the CLI socket seed, the settings
// panel render.
// ------------------------------------------------------------------

/// (k1) `key_action`: the legend's three keys map to their
/// actions; every other key (other function keys, letters,
/// modifiers, navigation) yields `None` (no action this frame).
#[test]
fn key_action_maps_only_the_legend_keys() {
    assert_eq!(key_action(egui::Key::F2), Some(GuiAction::SnapshotPng));
    assert_eq!(key_action(egui::Key::F3), Some(GuiAction::ExportJson));
    assert_eq!(key_action(egui::Key::Q), Some(GuiAction::Quit));

    for key in [
        egui::Key::F1,
        egui::Key::F4,
        egui::Key::F12,
        egui::Key::A,
        egui::Key::Escape,
        egui::Key::Enter,
        egui::Key::Space,
        egui::Key::Num0,
        egui::Key::ArrowUp,
    ] {
        assert_eq!(key_action(key), None, "{key:?} must not fire an action");
    }
}

/// (k2) `seed_settings`: the CLI `--socket` becomes
/// `settings.socket` (the panel shows the active socket, the
/// poller reads it live), every other knob stays at its default
/// (2 s poll, refresh off by default — one baseline fetch on
/// connect; enable in Settings to poll); the no-flag default
/// seeds the protocol's default socket.
#[test]
fn seed_settings_from_the_cli_socket() {
    let args = GuiArgs {
        socket: PathBuf::from("/tmp/ramsleuth-dev.sock"),
    };
    let settings = seed_settings(&args);
    assert_eq!(settings.socket, "/tmp/ramsleuth-dev.sock");
    assert_eq!(settings.poll_interval_ms, DEFAULT_POLL_INTERVAL_MS);
    assert!(!settings.refresh_enabled, "refresh must default off");
    assert_eq!(
        settings,
        GuiSettings {
            socket: "/tmp/ramsleuth-dev.sock".to_owned(),
            ..Default::default()
        }
    );

    let settings = seed_settings(&GuiArgs::default());
    assert_eq!(settings.socket, DEFAULT_SOCKET_PATH);
}

/// (k3) The settings panel renders headless without panicking
/// (the `begin_frame` idiom, the history test precedent): the
/// default settings + a mutated one (every widget — the socket
/// edit, the drag, the combos, the checkbox — executes).
#[test]
fn settings_panel_renders_headless_without_panicking() {
    for settings in [
        GuiSettings::default(),
        GuiSettings {
            socket: "/tmp/x".to_owned(),
            poll_interval_ms: 5_000,
            ..Default::default()
        },
    ] {
        let mut settings = settings;
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput::default());
        egui::CentralPanel::default().show(&ctx, |ui| {
            render_settings_panel(ui, &mut settings);
        });
    }
}

// ------------------------------------------------------------------
// C7-21 (D-3): the Graphs button + the deferred child viewport
// (headless: no display, no daemon — the eframe spawn is
// compile-checked; the live Wayland window is this chunk's gate).
// ------------------------------------------------------------------

/// A synthesized primary-button click at `pos` (press + release
/// in one frame — egui's interaction resolution confirms a
/// `Released { click: Some(..) }` over the widget's previous-
/// frame rect).
fn click_events(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

/// (g1) The button's click-toggle semantics over the shared
/// `Arc<AtomicBool>`: no click leaves the flag closed, a click
/// on the button opens the window, and a click while open
/// closes it (both toggle directions, headless — the eframe
/// spawn itself is the live gate).
#[test]
fn graphs_button_click_toggles_the_shared_flag() {
    let flag = Arc::new(AtomicBool::new(false));
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1200.0, 300.0));

    let frame = |events: Vec<egui::Event>| -> egui::Rect {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        ctx.begin_frame(input);
        let mut rect = egui::Rect::NOTHING;
        egui::CentralPanel::default().show(&ctx, |ui| {
            ui.horizontal(|ui| {
                rect = graphs_button(ui, &flag).rect;
            });
        });
        let _ = ctx.end_frame();
        rect
    };

    // No click: stays closed; the button is laid out.
    let rect = frame(Vec::new());
    assert!(
        !flag.load(Ordering::Relaxed),
        "no click must not open the window"
    );
    assert!(
        rect.area() > 0.0,
        "the button must be laid out, got {rect:?}"
    );

    // A click at the button's center: opens.
    frame(click_events(rect.center()));
    assert!(
        flag.load(Ordering::Relaxed),
        "a click on the button must open the window"
    );

    // A click while open: closes.
    let rect = frame(Vec::new());
    frame(click_events(rect.center()));
    assert!(
        !flag.load(Ordering::Relaxed),
        "a click while open must close the window"
    );
}

/// (g2) The full header renders the Graphs button headless
/// without panicking in both open states (the k3 headless
/// idiom — the button's `selected`-while-open rendering is
/// compile-checked + live-gated), and a render never toggles
/// the flag (only a click does).
#[test]
fn render_header_renders_the_graphs_button_headless_without_panicking() {
    for open in [false, true] {
        let flag = Arc::new(AtomicBool::new(open));
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput::default());
        let mut settings_open = false;
        let mut requirements_open = true;
        RamSleuthApp::render_header(
            &ctx,
            &TelemetryData::default(),
            &mut settings_open,
            &mut requirements_open,
            &flag,
            &None,
        );
        let _ = ctx.end_frame();
        assert_eq!(
            flag.load(Ordering::Relaxed),
            open,
            "a render must not toggle the flag"
        );
    }
}

/// (g3) The spawn / keep-alive / GC path over the real egui
/// context (the D-3 mechanism, headless): a root frame with the
/// flag open registers the deferred child (`ViewportClass::
/// Deferred` in the frame's viewport output), a child frame
/// renders the graph window over the shared state without
/// closing it, the next root frame's re-registration keeps the
/// child alive, and the first root frame without a re-
/// registration GCs it (that IS the close).
#[test]
fn graphs_viewport_spawn_keep_alive_and_gc_headless() {
    let state = Arc::new(RwLock::new(TelemetryData::default()));
    let flag = Arc::new(AtomicBool::new(true));
    // The eframe desktop runtime disables embed mode (a bare
    // `Context::default()` leaves it on — the no-integration
    // fallback) so the deferred viewport is registered, not run
    // inline.
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let child = graphs_viewport_id();

    // Root frame 1: the spawn — the deferred child is registered.
    ctx.begin_frame(egui::RawInput::default());
    show_graphs_viewport(&ctx, &state, &flag, &None);
    let out = ctx.end_frame();
    let registered = out.viewport_output.get(&child).cloned();
    assert!(
        registered.is_some(),
        "the deferred child must be registered on spawn"
    );
    assert!(
        registered.expect("registered").class == egui::ViewportClass::Deferred,
        "the child must be a deferred (independent-window) viewport"
    );

    // The child's own frame: renders the window over the shared
    // state (a pure reader, D6) on its own cadence, without
    // closing it.
    let mut child_input = egui::RawInput {
        viewport_id: child,
        ..Default::default()
    };
    child_input.viewports.insert(
        child,
        egui::ViewportInfo {
            parent: Some(egui::ViewportId::ROOT),
            ..Default::default()
        },
    );
    ctx.begin_frame(child_input);
    run_graphs_child_frame(&ctx, &state, &flag);
    let child_out = ctx.end_frame();
    assert!(
        !child_out.shapes.is_empty(),
        "the child frame must paint the graph window"
    );
    assert!(
        flag.load(Ordering::Relaxed),
        "a plain child frame must not close the window"
    );

    // Root frame 2 (flag still open): the re-registration keeps
    // the child alive (the keep-alive).
    ctx.begin_frame(egui::RawInput::default());
    show_graphs_viewport(&ctx, &state, &flag, &None);
    let out = ctx.end_frame();
    assert!(
        out.viewport_output.contains_key(&child),
        "the keep-alive re-registration must retain the child"
    );

    // Root frame 3 (flag closed): no re-registration — the
    // child is GC'd (the D-3 close).
    flag.store(false, Ordering::Relaxed);
    ctx.begin_frame(egui::RawInput::default());
    let out = ctx.end_frame();
    assert!(
        !out.viewport_output.contains_key(&child),
        "a child the root stops registering must be GC'd (the close)"
    );
}

/// (g4) The WM close path (the D-3 close via the window's own
/// close button): a child frame carrying `ViewportEvent::Close`
/// sees `close_requested()` and clears the shared open flag —
/// the next root frame stops re-registering and the window is
/// GC'd (behaviorally identical to the header button's close,
/// D-C7).
#[test]
fn graphs_viewport_close_requested_clears_the_flag_headless() {
    let state = Arc::new(RwLock::new(TelemetryData::default()));
    let flag = Arc::new(AtomicBool::new(true));
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let child = graphs_viewport_id();

    // Register the child once (the context learns its parent).
    ctx.begin_frame(egui::RawInput::default());
    show_graphs_viewport(&ctx, &state, &flag, &None);
    let _ = ctx.end_frame();

    // The WM close: the child's frame carries `ViewportEvent::
    // Close` (eframe's glow runtime pushes it on the window's
    // CloseRequested event).
    let mut info = egui::ViewportInfo {
        parent: Some(egui::ViewportId::ROOT),
        ..Default::default()
    };
    info.events.push(egui::ViewportEvent::Close);
    let mut child_input = egui::RawInput {
        viewport_id: child,
        ..Default::default()
    };
    child_input.viewports.insert(child, info);
    ctx.begin_frame(child_input);
    run_graphs_child_frame(&ctx, &state, &flag);
    let _ = ctx.end_frame();

    assert!(
        !flag.load(Ordering::Relaxed),
        "close_requested must clear the open flag (the D-3 close)"
    );
}

/// (g5) The C9-02 transition detector's pure edge logic: the
/// opening edge saves the current gate and forces it on (even
/// with auto-refresh off), the closing edge restores the saved
/// original (a pre-enabled original stays enabled) and consumes
/// the save, a no-edge frame writes nothing, and a close edge
/// with no saved value (defensive) writes nothing.
#[test]
fn refresh_transition_open_forces_on_and_close_restores() {
    // Opening with auto-refresh OFF (the default): the detector
    // saves the `false` original and forces the gate on.
    let mut saved = None;
    let mut last = false;
    assert_eq!(
        apply_refresh_transition(&mut saved, &mut last, true, false),
        Some(true),
        "the opening edge must force the gate on"
    );
    assert_eq!(saved, Some(false), "the pre-open original must be saved");
    assert!(last, "the edge memory must now read open");

    // A held-open frame is a no-edge: no write.
    assert_eq!(
        apply_refresh_transition(&mut saved, &mut last, true, true),
        None
    );
    assert_eq!(saved, Some(false));

    // Closing (either path — both clear the same flag, D-C7):
    // restore the saved `false` + consume the save.
    assert_eq!(
        apply_refresh_transition(&mut saved, &mut last, false, true),
        Some(false),
        "the closing edge must restore the saved original"
    );
    assert_eq!(saved, None, "the save must be consumed on the close edge");
    assert!(!last, "the edge memory must now read closed");

    // A held-closed frame is a no-edge: no write.
    assert_eq!(
        apply_refresh_transition(&mut saved, &mut last, false, false),
        None
    );
    assert_eq!(saved, None);

    // Opening with the gate already ON: the detector saves the
    // `true` original and re-asserts on.
    assert_eq!(
        apply_refresh_transition(&mut saved, &mut last, true, true),
        Some(true)
    );
    assert_eq!(saved, Some(true));

    // Closing: a pre-enabled original stays enabled.
    assert_eq!(
        apply_refresh_transition(&mut saved, &mut last, false, true),
        Some(true),
        "a pre-enabled original must stay enabled after the close"
    );
    assert_eq!(saved, None);

    // Defensive: a close edge with no saved value writes
    // nothing (the flag cannot be open without the app having
    // seen the opening edge, but the detector must stay no-op
    // safe).
    let mut last = true;
    assert_eq!(
        apply_refresh_transition(&mut saved, &mut last, false, false),
        None
    );
    assert!(!last);
}

/// (g6) The C9-02 lifecycle through the app itself (the
/// per-frame detector's wiring, headless — `update` calls
/// `apply_graphs_lifecycle`): opening the window force-enables
/// the shared refresh gate even with auto-refresh off, and
/// closing it (either path — both store the same flag `false`,
/// D-C7: the g1 button toggle and the g4 WM close) reverts the
/// gate to the saved original; a pre-enabled original stays
/// enabled after the close.
#[test]
fn graphs_lifecycle_force_enables_and_reverts_the_refresh_gate() {
    let out_dir = temp_out_dir("graphs-lifecycle");
    for original in [false, true] {
        let (bench_tx, _bench_rx) = std::sync::mpsc::channel::<BenchCmd>();
        let mut app = RamSleuthApp {
            state: Arc::new(RwLock::new(TelemetryData {
                settings: GuiSettings {
                    refresh_enabled: original,
                    ..Default::default()
                },
                ..Default::default()
            })),
            bench_tx,
            stop: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            settings_open: false,
            requirements_open: false,
            setup: Arc::new(RwLock::new(SetupOutcome::default())),
            icon: None,
            graphs_open: Arc::new(AtomicBool::new(false)),
            saved_refresh: None,
            last_graphs_open: false,
            out_dir: out_dir.clone(),
            poller: None,
            notice: None,
            probe_state: ProbeState::default(),
            probe_title: None,
            about_open: false,
        };
        let gate = |app: &RamSleuthApp| state_read(&app.state).settings.refresh_enabled;

        // The opening edge (the header button's click stores
        // `true`, g1): the detector force-enables the gate.
        app.graphs_open.store(true, Ordering::Relaxed);
        app.apply_graphs_lifecycle();
        assert!(
            gate(&app),
            "open must force the gate on (original: {original})"
        );
        assert_eq!(
            app.saved_refresh,
            Some(original),
            "the pre-open original must be saved"
        );

        // A held-open frame: no edge, the gate stays on.
        app.apply_graphs_lifecycle();
        assert!(gate(&app), "a held-open frame must not touch the gate");

        // The closing edge (either path stores `false` — the
        // g1 button toggle / the g4 WM close): the detector
        // reverts to the saved original.
        app.graphs_open.store(false, Ordering::Relaxed);
        app.apply_graphs_lifecycle();
        assert_eq!(
            gate(&app),
            original,
            "close must restore the saved original (original: {original})"
        );
        assert_eq!(
            app.saved_refresh, None,
            "the save must be consumed on the close edge"
        );

        // A held-closed frame: no edge, the original stands.
        app.apply_graphs_lifecycle();
        assert_eq!(
            gate(&app),
            original,
            "a held-closed frame must not touch the gate"
        );
    }
    fs::remove_dir_all(&out_dir).expect("cleanup");
}
/// (u1) The C9-05 right-column split (D-4b): the right column
/// renders the bench and the status as two stacked slices — the
/// bench at its natural height over the status, the 8 pt gap
/// preserved — and degrades to the `ScrollArea` overflow
/// fallback in a small window (the bench's natural height alone
/// exceeds the column, the status slice takes its zero budget,
/// and the content scrolls below). No panic at either size, no
/// action without a click, one CYAN frame per zone.
#[test]
fn render_zones_right_column_split_two_stacked_slices() {
    let out_dir = temp_out_dir("right-split");
    // (w, h): the default 968×600 (the non-scrolling dashboard,
    // C7-19 — both slices fit the column) and a small window
    // (the scroll fallback path — the bench alone overflows).
    for (case, (w, h)) in [
        (DEFAULT_WINDOW_SIZE[0], DEFAULT_WINDOW_SIZE[1]),
        (900.0, 220.0),
    ]
    .into_iter()
    .enumerate()
    {
        let (bench_tx, _bench_rx) = std::sync::mpsc::channel::<BenchCmd>();
        let app = RamSleuthApp {
            state: Arc::new(RwLock::new(TelemetryData::default())),
            bench_tx,
            stop: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            settings_open: false,
            requirements_open: false,
            setup: Arc::new(RwLock::new(SetupOutcome::default())),
            icon: None,
            graphs_open: Arc::new(AtomicBool::new(false)),
            saved_refresh: None,
            last_graphs_open: false,
            out_dir: out_dir.clone(),
            poller: None,
            notice: None,
            probe_state: ProbeState::default(),
            probe_title: None,
            about_open: false,
        };
        let data = TelemetryData::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(w, h));
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        });
        let action = app.render_zones(&ctx, &data);
        let out = ctx.end_frame();

        assert_eq!(
            action,
            GuiAction::None,
            "a no-click frame must report no action"
        );

        // The zone frames: the CYAN-stroked rects of frame size
        // (the header / settings strips use `HEADER_STROKE`, not
        // CYAN; glyphs are `Shape::Text`, not `Shape::Rect`).
        let frames: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.color == CYAN && rect.rect.width() >= 100.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect();
        // The right-column frames: those sharing the rightmost
        // column's left edge (the telemetry frame is the lone
        // left one).
        let total = frames.len();
        // The outer margins (D-15.1): the leftmost frame edge's
        // margin from the screen's left edge, and the rightmost
        // frame edge's margin from the screen's right edge.
        let left_x = frames.iter().fold(f32::INFINITY, |mn, r| mn.min(r.min.x));
        let right_x = frames.iter().fold(0.0_f32, |mx, r| mx.max(r.min.x));
        let rightmost_x = frames.iter().fold(0.0_f32, |mx, r| mx.max(r.max.x));
        let mut right: Vec<egui::Rect> = frames
            .into_iter()
            .filter(|r| (r.min.x - right_x).abs() < 2.0)
            .collect();
        if case == 0 {
            // Default window: all three frames are visible —
            // one per zone (the placeholder status draws no SPD
            // cards).
            assert_eq!(
                right.len(),
                2,
                "the right column must hold the bench + status slices (frames: {total})"
            );
            right.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
            let (bench, status) = (right[0], right[1]);
            assert!(
                bench.max.y < status.min.y,
                "the bench slice must sit above the status slice"
            );
            let gap = status.min.y - bench.max.y;
            assert!(
                gap >= 7.9,
                "the preserved 8 pt gap must separate the slices (got {gap})"
            );
            assert!(
                gap <= 25.0,
                "the gap is 8 pt + one item spacing, not a layout blow-up (got {gap})"
            );
            // The bench slice keeps its natural height — far
            // from stretched to the column (filling the
            // remainder is the status slice's job, C9-07).
            let bench_h = bench.height();
            assert!(
                bench_h < 0.6 * (h - 100.0),
                "the bench slice must stay at its natural height (got {bench_h} of {h})"
            );
            // Both slices fit the column — the fallback stays
            // dormant (the non-scrolling dashboard, C7-19).
            assert!(
                status.max.y <= h - 3.9,
                "the status frame must fit the column (bottom {} of {})",
                status.max.y,
                h
            );
            // The symmetric outer margins (D-15.1): the left
            // margin == the right margin (both = OUTER_MARGIN),
            // and the right margin clears the frame stroke's
            // 0.5 pt outer half — the missing right border is
            // gone and stays gone.
            let left_margin = left_x;
            let right_margin = w - rightmost_x;
            assert!(
                    (left_margin - right_margin).abs() <= 1.0,
                    "the left and right outer margins must be symmetric (left {left_margin}, right {right_margin})"
                );
            assert!(
                (left_margin - OUTER_MARGIN).abs() <= 1.0,
                "the left margin must be OUTER_MARGIN (got {left_margin})"
            );
            assert!(
                (right_margin - OUTER_MARGIN).abs() <= 1.0,
                "the right margin must be OUTER_MARGIN (got {right_margin})"
            );
            assert!(
                    right_margin >= 0.5,
                    "the right margin must clear the frame stroke's 0.5 pt outer half (got {right_margin})"
                );
        } else {
            // Small window: the bench's natural height alone
            // exceeds the column — the scroll fallback is
            // active: the bench overflows the window bottom and
            // the status (stacked below it) is clipped off-
            // screen until scrolled into view.
            assert_eq!(
                right.len(),
                1,
                "the visible right-column slice is the bench (frames: {total})"
            );
            assert!(
                right[0].max.y > h - 3.9,
                "the bench must overflow the small column (the scroll fallback), bottom {} of {}",
                right[0].max.y,
                h
            );
        }
    }
    fs::remove_dir_all(&out_dir).expect("cleanup");
}

/// TC-FE5 (the GUI layout confirming test): the bench zone receives a
/// **natural, non-zero** allocation (its frame rect's height > 0 and
/// equals its content — NOT the old zero-height
/// `allocate_ui_with_layout(Vec2(w, 0.0))` read-back footgun), and the
/// status zone fills the **constrained remainder** below it; a resize
/// to a small window takes the `ScrollArea` overflow fallback **without**
/// re-entering a zero-height measure path (no panic). Pre-FE-5 the bench
/// was measured through a zero rect and the status through an
/// arithmetic `row_h − bench_h − 8` remainder slice.
#[test]
fn tc_fe5_bench_natural_status_fills_remainder() {
    let out_dir = temp_out_dir("fe5-zones");
    let (bench_tx, _bench_rx) = std::sync::mpsc::channel::<BenchCmd>();
    let app = RamSleuthApp {
        state: Arc::new(RwLock::new(TelemetryData::default())),
        bench_tx,
        stop: Arc::new(AtomicBool::new(false)),
        cancel: Arc::new(AtomicBool::new(false)),
        settings_open: false,
        requirements_open: false,
        setup: Arc::new(RwLock::new(SetupOutcome::default())),
        icon: None,
        graphs_open: Arc::new(AtomicBool::new(false)),
        saved_refresh: None,
        last_graphs_open: false,
        out_dir: out_dir.clone(),
        poller: None,
        notice: None,
        probe_state: ProbeState::default(),
        probe_title: None,
        about_open: false,
    };
    let data = TelemetryData::default();

    // Extract the CYAN-stroked zone frame rects (the bench + status
    // share the right column's left edge).
    let frame_rects = |w: f32, h: f32| -> Vec<egui::Rect> {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(w, h));
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        });
        let action = app.render_zones(&ctx, &data);
        let out = ctx.end_frame();
        assert_eq!(
            action,
            GuiAction::None,
            "a no-click frame reports no action"
        );
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.color == CYAN && rect.rect.width() >= 100.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect()
    };

    // (1) Default window: the bench is a natural, non-zero height and
    // sits above the status, which fills the constrained remainder
    // (down to the column's bottom).
    let frames = frame_rects(DEFAULT_WINDOW_SIZE[0], DEFAULT_WINDOW_SIZE[1]);
    assert!(
        frames.len() >= 2,
        "the bench + status frames must render ({frames:?})"
    );
    let right_x = frames.iter().fold(0.0_f32, |mx, r| mx.max(r.min.x));
    let mut right: Vec<egui::Rect> = frames
        .into_iter()
        .filter(|r| (r.min.x - right_x).abs() < 2.0)
        .collect();
    right.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
    let (bench, status) = (right[0], right[1]);
    // The bench's allocation is natural + non-zero (not a zero-rect
    // read-back): a bench grid is a substantial, content-sized height.
    assert!(
        bench.height() > 40.0,
        "the bench zone must get a natural, non-zero height (got {})",
        bench.height()
    );
    // The status fills the constrained remainder: below the bench,
    // reaching the column's bottom (no gap left unfilled at the
    // default size).
    assert!(
        bench.max.y < status.min.y,
        "the bench must sit above the status"
    );
    assert!(
        status.max.y > DEFAULT_WINDOW_SIZE[1] - 60.0,
        "the status must fill down toward the column bottom (bottom {} of {})",
        status.max.y,
        DEFAULT_WINDOW_SIZE[1]
    );

    // (2) A small window: the bench's natural height overflows the
    // column, the `ScrollArea` fallback takes over — no panic, and the
    // bench is STILL a non-zero (natural) allocation (the zero-height
    // measure path is gone).
    let small = frame_rects(900.0, 220.0);
    assert!(
        !small.is_empty(),
        "the small window must still render the zone frames (no panic)"
    );
    fs::remove_dir_all(&out_dir).expect("cleanup");
}

/// (s1) The exit-code → [`SetupOutcome`] mapping (the frozen
/// C21-01 contract, headless): exit 0 = done with no failure;
/// exit 1 = the structured tail — the FIRST stderr line
/// containing `ERROR:` (any helper tag: `[ramsleuth-setup]`,
/// `[ryzen-smu-dkms]`, `[intel-dkms]`), else the last non-empty
/// line, else the generic `sudo` pointer — with the process exit
/// code appended; exit 2 = the usage note; a signal death (the
/// worker's `-1`) degrades to the tail — no panic.
#[test]
fn setup_exit_code_maps_to_the_outcome() {
    // Exit 0: success / no-op.
    let outcome = map_setup_exit(0, "");
    assert!(
        !outcome.running,
        "the worker's terminal state clears running"
    );
    assert!(outcome.done, "exit 0 must set done");
    assert!(outcome.failure.is_none(), "exit 0 must carry no failure");

    // Exit 1: the FIRST structured "ERROR:" line wins (a later
    // line must not mask it) + the process exit code.
    let stderr = "step 1: daemon enabled\n[ramsleuth-setup] ERROR: usermod failed\n\
                      [ramsleuth-setup] ERROR: step 4 failed";
    let outcome = map_setup_exit(1, stderr);
    assert!(!outcome.done, "exit 1 must not set done");
    assert_eq!(
        outcome.failure.as_deref(),
        Some(with_setup_log_pointer("[ramsleuth-setup] ERROR: usermod failed (exit 1)").as_str()),
        "exit 1 must carry the first structured ERROR line + the exit code \
             + the durable-log pointer"
    );

    // Exit 1: the vendor-helper tags match too (the `--with-dkms`
    // delegation is `exec`-replaced, so the helper's line is the
    // structured reason).
    let outcome = map_setup_exit(
        1,
        "info\n[ryzen-smu-dkms] ERROR: dkms build failed — compile break\n\
              tail line",
    );
    assert_eq!(
        outcome.failure.as_deref(),
        Some(
            with_setup_log_pointer(
                "[ryzen-smu-dkms] ERROR: dkms build failed — compile break (exit 1)"
            )
            .as_str(),
        ),
    );
    let outcome = map_setup_exit(1, "info\n[intel-dkms] ERROR: modprobe failed\n");
    assert_eq!(
        outcome.failure.as_deref(),
        Some(with_setup_log_pointer("[intel-dkms] ERROR: modprobe failed (exit 1)").as_str()),
    );

    // Exit 1 without the marker: the last non-empty line + the
    // exit code.
    let outcome = map_setup_exit(
        1,
        "info line\nmodprobe: FATAL: Module ryzen_smu_drv not found\n",
    );
    assert_eq!(
        outcome.failure.as_deref(),
        Some(
            with_setup_log_pointer("modprobe: FATAL: Module ryzen_smu_drv not found (exit 1)")
                .as_str()
        ),
    );

    // Exit 1 with empty stderr: the generic `sudo` pointer + the
    // exit code.
    assert_eq!(
        map_setup_exit(1, "").failure.as_deref(),
        Some(with_setup_log_pointer(&format!("{GENERIC_FAILURE_POINTER} (exit 1)")).as_str()),
        "a marker-less empty stderr must point at the manual floor + the log"
    );

    // Exit 2: usage (should not happen via the GUI).
    let outcome = map_setup_exit(2, "whatever");
    assert!(
        outcome
            .failure
            .as_deref()
            .is_some_and(|message| message.contains("usage, exit 2")),
        "exit 2 must name the usage error: {:?}",
        outcome.failure
    );

    // A signal death (the worker maps it to -1): hard failure,
    // no panic.
    let outcome = map_setup_exit(-1, "killed");
    assert!(!outcome.done);
    assert_eq!(
        outcome.failure.as_deref(),
        Some(with_setup_log_pointer("killed (exit -1)").as_str())
    );
}

/// (s4) The durable-log pointer: every setup failure message the GUI
/// shows — an exit-1 structured tail, an exit-2 usage, a signal
/// death (the worker's `-1`), and a pkexec spawn failure — ends
/// with the known log path as a trailing line, so a failure on a
/// test machine is diagnosable via `cat /var/lib/ramsleuth/setup.log`
/// (the existing diagnostic + exit code stay the first line).
#[test]
fn setup_failure_messages_carry_the_log_path() {
    let pointer = format!("\nFull log: {SETUP_LOG_PATH}");
    for code in [1, 2, -1] {
        let outcome = map_setup_exit(code, "some stderr line");
        let message = outcome
            .failure
            .as_deref()
            .expect("a non-zero exit must carry a failure");
        assert!(
            message.ends_with(pointer.as_str()),
            "the exit-{code} failure message must end with the log pointer: {message:?}"
        );
    }
    let spawn = pkexec_spawn_failure_message(
        &std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory"),
        "alice",
        true,
    );
    assert!(
        spawn.ends_with(pointer.as_str()),
        "the pkexec spawn failure must end with the log pointer: {spawn}"
    );
    assert!(
        spawn.contains("sudo ramsleuth-setup --user alice --with-dkms"),
        "the spawn failure keeps the manual floor: {spawn}"
    );
    // A success carries no failure (and no pointer): the log pointer
    // is only relevant to a broken run.
    assert!(map_setup_exit(0, "").failure.is_none());
}

/// (s3) The Secure Boot one-time-step exit (10): NOT a failure —
/// the `secure_boot_pending` state carries the helper's own
/// single-line guidance (the first `ERROR:` stderr line + the
/// exit code), else the built-in MOK guidance; `done` stays false
/// (no auto-reconnect edge) and `running` is cleared.
#[test]
fn secure_boot_pending_exit_is_actionable_not_failure() {
    // With the helper's own guidance line on stderr.
    let stderr = "[ryzen-smu-dkms] ERROR: Secure Boot is on — ONE-TIME: reboot, \
                      at the blue MOK screen choose Enroll MOK key(s) → Continue → Yes, \
                      then re-click Setup";
    let outcome = map_setup_exit(10, stderr);
    assert!(!outcome.done, "exit 10 must not set done");
    assert!(!outcome.running, "exit 10 must clear running");
    assert!(
        outcome.failure.is_none(),
        "exit 10 must not be a failure: {:?}",
        outcome.failure
    );
    let expected = format!("{stderr} (exit 10)");
    assert_eq!(
        outcome.secure_boot_pending.as_deref(),
        Some(expected.as_str()),
        "the helper's own single-line guidance wins"
    );

    // Without a usable stderr line: the built-in MOK guidance.
    let outcome = map_setup_exit(10, "");
    assert!(outcome.failure.is_none());
    assert_eq!(
        outcome.secure_boot_pending.as_deref(),
        Some(SECURE_BOOT_PENDING_GUIDANCE),
        "an empty stderr falls back to the built-in guidance"
    );
}

/// (s2) The manual-floor command (plan risk 1): the `sudo`
/// equivalent of the one click, the `--with-dkms` flag on the
/// AMD/Intel vendor decision.
#[test]
fn sudo_setup_command_is_the_manual_floor() {
    assert_eq!(
        sudo_setup_command("alice", false),
        "sudo ramsleuth-setup --user alice"
    );
    assert_eq!(
        sudo_setup_command("alice", true),
        "sudo ramsleuth-setup --user alice --with-dkms"
    );
}

/// (C21-26) The window-icon decode path: a small in-memory RGBA
/// PNG (the `snapshot_png` encoder precedent) round-trips through
/// the same decoder the icon uses — the dimensions + a pixel
/// value survive; corrupt bytes degrade to `None` (no-panic,
/// D5); the committed C21-25 256×256 master decodes at the
/// advertised size.
#[test]
fn window_icon_png_decode() {
    // A 3×2 RGBA PNG, one distinct pixel per cell.
    let rgba: Vec<u8> = (0..6)
        .flat_map(|i| [i * 4 + 1, i * 4 + 2, i * 4 + 3, 255])
        .collect();
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 3, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a valid header encodes");
        writer
            .write_image_data(&rgba)
            .expect("a valid RGBA8 frame encodes");
    }
    let icon = decode_icon_png(&png_bytes).expect("a valid RGBA8 PNG decodes");
    assert_eq!((icon.width, icon.height), (3, 2));
    // The pixel at (x=2, y=1) survives the round-trip (row-major).
    let idx = 5 * 4;
    assert_eq!(&icon.rgba[idx..idx + 4], &rgba[idx..idx + 4]);

    // Corrupt payload: `None`, never a panic (no-panic, D5).
    assert!(decode_icon_png(&[0, 1, 2, 3]).is_none());

    // The committed C21-25 window-embed master: 256×256 RGBA8.
    let embedded = window_icon().expect("the committed 256×256 master decodes");
    assert_eq!((embedded.width, embedded.height), (256, 256));
    assert_eq!(embedded.rgba.len(), 256 * 256 * 4);
}
// -----------------------------------------------------------------
// The consent-gated "Submit Probe Report" flow (chunk probe-3).
// -----------------------------------------------------------------

/// A minimal probe-report fixture (the identity the title + the
/// markdown body the preview / URL tests consume).
fn fixture_probe_report() -> ProbeReport {
    ProbeReport {
        telemetry: fixture_telemetry(Section::na(NaReason::NotApplicable), Vec::new(), Vec::new()),
        raw: None,
        system: ProbeSystem {
            cpu_brand: "Test CPU".to_owned(),
            cpu_vendor: "unknown".to_owned(),
            cpu_gen: "Unknown".to_owned(),
            pci_host_bridge: None,
            kernel: "6.6.0-test".to_owned(),
            os: "Linux / Test".to_owned(),
            arch: "x86_64".to_owned(),
            ramsleuth_version: "2.4.13".to_owned(),
            telemetry_source: "unavailable".to_owned(),
        },
    }
}

/// A fully-constructed app in the probe-flow idle state (the
/// dialog + state-machine test fixture; the caller cleans up
/// `out_dir`).
fn probe_test_app(name: &str) -> RamSleuthApp {
    let (bench_tx, _bench_rx) = std::sync::mpsc::channel::<BenchCmd>();
    RamSleuthApp {
        state: Arc::new(RwLock::new(TelemetryData::default())),
        bench_tx,
        stop: Arc::new(AtomicBool::new(false)),
        cancel: Arc::new(AtomicBool::new(false)),
        settings_open: false,
        requirements_open: false,
        setup: Arc::new(RwLock::new(SetupOutcome::default())),
        icon: None,
        graphs_open: Arc::new(AtomicBool::new(false)),
        saved_refresh: None,
        last_graphs_open: false,
        out_dir: temp_out_dir(name),
        poller: None,
        notice: None,
        probe_state: ProbeState::default(),
        probe_title: None,
        about_open: false,
    }
}

/// (q1) The probe-flow state machine: the full
/// `Idle → Consent → Preview → Idle` chain — the header's Probe
/// button opens the consent, the poller's landed report renders
/// the preview (the markdown + the title, the slot consumed), and
/// [Cancel] closes — plus the `Submit` → `Idle` arm, the
/// `Copy` → `Idle` arm, the `CancelConsent` → `Idle` arm, and
/// the `Err` → `Idle` + notice arm. Every closing path returns
/// to `Idle` (no terminal state — the flow is re-openable);
/// out-of-state events are no-ops.
#[test]
fn probe_state_machine_transitions() {
    // The full consent → preview → idle chain, driven through the
    // app's `advance_probe` (the result consumer).
    let mut app = probe_test_app("probe-chain");
    let out_dir = app.out_dir.clone();
    assert_eq!(app.probe_state, ProbeState::Idle);

    // Open (the header's Probe button): Idle → Consent.
    app.probe_state.apply(ProbeEvent::Open);
    assert_eq!(app.probe_state, ProbeState::Consent);

    // The poller lands the report: Consent → Preview (the
    // markdown is rendered, the title is set, the slot consumed).
    app.state.write().unwrap().probe_result = ProbeResult::Ok(fixture_probe_report());
    app.advance_probe();
    match &app.probe_state {
        ProbeState::Preview(md) => {
            assert!(
                md.contains("# RamSleuth Probe Report"),
                "the preview must carry the rendered markdown: {md}"
            );
        }
        other => panic!("expected Preview, got {other:?}"),
    }
    assert!(app.probe_title.is_some(), "the title must be set");
    assert_eq!(
        state_read(&app.state).probe_result,
        ProbeResult::None,
        "the result slot must be consumed"
    );

    // [Cancel]: Preview → Idle.
    app.probe_state.apply(ProbeEvent::CancelPreview);
    assert_eq!(app.probe_state, ProbeState::Idle);

    fs::remove_dir_all(out_dir).expect("cleanup");

    // The `Submit` arm: Preview → Idle (the flow is re-openable
    // — no terminal state).
    let mut app = probe_test_app("probe-submit");
    let out_dir = app.out_dir.clone();
    app.probe_state = ProbeState::Preview("md".to_owned());
    app.probe_state.apply(ProbeEvent::Submit);
    assert_eq!(app.probe_state, ProbeState::Idle);
    fs::remove_dir_all(out_dir).expect("cleanup");

    // The `Copy` arm: Preview → Idle.
    let mut app = probe_test_app("probe-copy");
    let out_dir = app.out_dir.clone();
    app.probe_state = ProbeState::Preview("md".to_owned());
    app.probe_state.apply(ProbeEvent::Copy);
    assert_eq!(app.probe_state, ProbeState::Idle);
    fs::remove_dir_all(out_dir).expect("cleanup");

    // The `CancelConsent` arm: Consent → Idle.
    let mut app = probe_test_app("probe-cancelconsent");
    let out_dir = app.out_dir.clone();
    app.probe_state = ProbeState::Consent;
    app.probe_state.apply(ProbeEvent::CancelConsent);
    assert_eq!(app.probe_state, ProbeState::Idle);
    fs::remove_dir_all(out_dir).expect("cleanup");

    // The `Err` arm: Consent + a failed request → Idle + notice
    // (the slot consumed).
    let mut app = probe_test_app("probe-err");
    let out_dir = app.out_dir.clone();
    app.probe_state = ProbeState::Consent;
    app.state.write().unwrap().probe_result = ProbeResult::Err("daemon down".to_owned());
    app.advance_probe();
    assert_eq!(app.probe_state, ProbeState::Idle);
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(text, _)| text.contains("daemon down")),
        "the failure must flash a notice"
    );
    assert_eq!(
        state_read(&app.state).probe_result,
        ProbeResult::None,
        "the result slot must be consumed"
    );
    fs::remove_dir_all(out_dir).expect("cleanup");

    // Out-of-state events are no-ops (the machine is total).
    let mut s = ProbeState::Idle;
    s.apply(ProbeEvent::CancelPreview);
    assert_eq!(s, ProbeState::Idle);
    s.apply(ProbeEvent::Copy);
    assert_eq!(s, ProbeState::Idle);
    s.apply(ProbeEvent::Submit);
    assert_eq!(s, ProbeState::Idle);
    let mut s = ProbeState::Consent;
    s.apply(ProbeEvent::Open);
    assert_eq!(s, ProbeState::Consent);
    s.apply(ProbeEvent::CancelPreview);
    assert_eq!(s, ProbeState::Consent);
    s.apply(ProbeEvent::Copy);
    assert_eq!(s, ProbeState::Consent);
    s.apply(ProbeEvent::Submit);
    assert_eq!(s, ProbeState::Consent);
}

/// (q1b) The probe modal's Esc dismiss (the TUI's close
/// contract): Consent and Preview both close to `Idle`; with no
/// probe dialog open the handler is a no-op.
#[test]
fn handle_probe_esc_closes_the_open_modal() {
    let mut app = probe_test_app("probe-esc-consent");
    let out_dir = app.out_dir.clone();
    app.probe_state = ProbeState::Consent;
    app.handle_probe_esc();
    assert_eq!(app.probe_state, ProbeState::Idle);
    fs::remove_dir_all(out_dir).expect("cleanup");

    let mut app = probe_test_app("probe-esc-preview");
    let out_dir = app.out_dir.clone();
    app.probe_state = ProbeState::Preview("md".to_owned());
    app.handle_probe_esc();
    assert_eq!(app.probe_state, ProbeState::Idle);
    fs::remove_dir_all(out_dir).expect("cleanup");

    let mut app = probe_test_app("probe-esc-idle");
    let out_dir = app.out_dir.clone();
    app.handle_probe_esc();
    assert_eq!(app.probe_state, ProbeState::Idle);
    fs::remove_dir_all(out_dir).expect("cleanup");
}

/// (q2) The GitHub issue URL construction: the title form
/// (`[Probe] <brand> · <gen> · <os> · v<version>`) + the
/// `issues/new` URL with the percent-encoded title only (the
/// body deliberately absent — a full report exceeds GitHub's
/// issue-form limit; the clipboard carries it).
#[test]
fn probe_issue_title_and_url_construction() {
    let report = fixture_probe_report();
    // The title (the template's [Probe] prefix + the identity).
    let title = probe_issue_title(&report);
    assert_eq!(
        title,
        "[Probe] Test CPU \u{00b7} Unknown \u{00b7} Linux / Test \u{00b7} v2.4.13"
    );
    // The title's percent-encoding (the brackets, the spaces, the
    // middot, the slash).
    assert_eq!(
            url_encode(&title),
            "%5BProbe%5D%20Test%20CPU%20%C2%B7%20Unknown%20%C2%B7%20Linux%20%2F%20Test%20%C2%B7%20v2.4.13"
        );
    // The URL (the issues/new form, title only — the body must
    // NOT ride the query string: it would exceed GitHub's limit).
    let url = probe_issue_url(&title);
    assert_eq!(
            url,
            "https://github.com/MadGoatHaz/RamSleuth/issues/new?title=%5BProbe%5D%20Test%20CPU%20%C2%B7%20Unknown%20%C2%B7%20Linux%20%2F%20Test%20%C2%B7%20v2.4.13"
        );
    assert!(
        !url.contains("body="),
        "the body must not ride the URL: {url}"
    );
}

/// (q2b) The percent-encoder: the RFC 3986 unreserved set passes
/// through; every other byte is `%XX` (space → `%20`, never `+`).
#[test]
fn url_encode_percent_encoding() {
    assert_eq!(url_encode("ABCdef019-_.~"), "ABCdef019-_.~");
    assert_eq!(url_encode("a b"), "a%20b");
    assert_eq!(url_encode("[x]"), "%5Bx%5D");
    assert_eq!(url_encode("\u{00b7}"), "%C2%B7");
    assert_eq!(url_encode("#"), "%23");
    assert_eq!(url_encode("a/b"), "a%2Fb");
    assert_eq!(url_encode("a=b&c"), "a%3Db%26c");
}

/// (q3) The consent dialog renders headless — the title, the
/// body, and both buttons paint — and a two-frame press / release
/// on [Allow] sets the poller's `probe_requested` edge (the
/// consent hand-off) while the flow stays `Consent` (in flight).
#[test]
fn consent_dialog_paints_and_allow_sets_the_request_edge() {
    let mut app = probe_test_app("consent-allow");
    app.probe_state = ProbeState::Consent;
    let out_dir = app.out_dir.clone();
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(968.0, 600.0));
    let frame_input = |events: Vec<egui::Event>| egui::RawInput {
        screen_rect: Some(screen),
        events,
        ..Default::default()
    };
    let frame = |events: Vec<egui::Event>, app: &mut RamSleuthApp| {
        ctx.run(frame_input(events), |ctx| {
            app.render_probe_consent_dialog(ctx);
        })
    };

    // Frame 1: new areas — hidden for one frame (the
    // first-frame placement heuristic).
    frame(Vec::new(), &mut app);
    // Frame 2: the dialog paints (the title, the body, both
    // buttons).
    let first = frame(Vec::new(), &mut app);
    let texts: Vec<&str> = first
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect();
    assert!(
        texts.contains(&"Submit Probe Report"),
        "the title must paint: {texts:?}"
    );
    assert!(
        texts.contains(&PROBE_CONSENT_BODY),
        "the body must paint: {texts:?}"
    );
    assert!(
        texts.contains(&"Allow"),
        "the Allow button must paint: {texts:?}"
    );
    assert!(
        texts.contains(&"Cancel"),
        "the Cancel button must paint: {texts:?}"
    );
    assert!(
        !state_read(&app.state).probe_requested,
        "no click must not set the edge"
    );

    // The [Allow] label's center (the graph.rs text-click idiom).
    let allow = first
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Allow" => Some(egui::pos2(
                text.pos.x + text.galley.size().x / 2.0,
                text.pos.y + text.galley.size().y / 2.0,
            )),
            _ => None,
        })
        .expect("the Allow button's label must be painted");

    let click = |pressed: bool| egui::Event::PointerButton {
        pos: allow,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    // Frames 3 + 4: press, then release, on [Allow] → the consent
    // edge is set (the poller will do the I/O); the flow stays
    // `Consent` (in flight).
    frame(vec![click(true)], &mut app);
    frame(vec![click(false)], &mut app);
    assert!(
        state_read(&app.state).probe_requested,
        "a press + release on Allow must set the request edge"
    );
    assert_eq!(
        app.probe_state,
        ProbeState::Consent,
        "Allow must stay in Consent (in flight)"
    );

    fs::remove_dir_all(out_dir).expect("cleanup");
}

/// (q4) The consent dialog's [Cancel] returns the flow to `Idle`.
#[test]
fn consent_dialog_cancel_returns_to_idle() {
    let mut app = probe_test_app("consent-cancel");
    app.probe_state = ProbeState::Consent;
    let out_dir = app.out_dir.clone();
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(968.0, 600.0));
    let frame_input = |events: Vec<egui::Event>| egui::RawInput {
        screen_rect: Some(screen),
        events,
        ..Default::default()
    };
    let frame = |events: Vec<egui::Event>, app: &mut RamSleuthApp| {
        ctx.run(frame_input(events), |ctx| {
            app.render_probe_consent_dialog(ctx);
        })
    };

    frame(Vec::new(), &mut app);
    let first = frame(Vec::new(), &mut app);
    let cancel = first
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Cancel" => Some(egui::pos2(
                text.pos.x + text.galley.size().x / 2.0,
                text.pos.y + text.galley.size().y / 2.0,
            )),
            _ => None,
        })
        .expect("the Cancel button's label must be painted");

    let click = |pressed: bool| egui::Event::PointerButton {
        pos: cancel,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![click(true)], &mut app);
    frame(vec![click(false)], &mut app);
    assert_eq!(
        app.probe_state,
        ProbeState::Idle,
        "a press + release on Cancel must return to Idle"
    );
    assert!(
        !state_read(&app.state).probe_requested,
        "Cancel must not set the request edge"
    );

    fs::remove_dir_all(out_dir).expect("cleanup");
}

/// (q5) The preview dialog renders headless — the title + the
/// markdown body + all three buttons paint — and [Cancel] returns
/// the flow to `Idle` (the dialog's buttons are the only
/// interactive content, the modal precedent).
#[test]
fn preview_dialog_paints_and_cancel_returns_to_idle() {
    let mut app = probe_test_app("preview-cancel");
    let report = fixture_probe_report();
    let md = render_probe_report_md(&report);
    app.probe_state = ProbeState::Preview(md);
    app.probe_title = Some(probe_issue_title(&report));
    let out_dir = app.out_dir.clone();
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(968.0, 600.0));
    let frame_input = |events: Vec<egui::Event>| egui::RawInput {
        screen_rect: Some(screen),
        events,
        ..Default::default()
    };
    let frame = |events: Vec<egui::Event>, app: &mut RamSleuthApp| {
        ctx.run(frame_input(events), |ctx| {
            app.render_probe_preview_dialog(ctx);
        })
    };

    frame(Vec::new(), &mut app);
    let first = frame(Vec::new(), &mut app);
    let texts: Vec<&str> = first
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect();
    assert!(
        texts.contains(&"Probe Report Preview"),
        "the title must paint: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("# RamSleuth Probe Report")),
        "the markdown body must paint: {texts:?}"
    );
    assert!(
        texts.contains(&"Open GitHub Issue"),
        "the Open GitHub Issue button must paint: {texts:?}"
    );
    assert!(
        texts.contains(&"Copy to Clipboard"),
        "the Copy to Clipboard button must paint: {texts:?}"
    );
    assert!(
        texts.contains(&"Cancel"),
        "the Cancel button must paint: {texts:?}"
    );

    // [Cancel] → Idle.
    let cancel = first
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Cancel" => Some(egui::pos2(
                text.pos.x + text.galley.size().x / 2.0,
                text.pos.y + text.galley.size().y / 2.0,
            )),
            _ => None,
        })
        .expect("the Cancel button's label must be painted");
    let click = |pressed: bool| egui::Event::PointerButton {
        pos: cancel,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![click(true)], &mut app);
    frame(vec![click(false)], &mut app);
    assert_eq!(
        app.probe_state,
        ProbeState::Idle,
        "a press + release on Cancel must return to Idle"
    );

    fs::remove_dir_all(out_dir).expect("cleanup");
}

/// (q5b) The preview dialog's [Open GitHub Issue] copies the
/// full markdown report to the clipboard (a 4–5 KB body would
/// exceed GitHub's issue-form URL limit — the 404), opens the
/// title-only `issues/new` URL, flashes the paste-into-the-body
/// notice in the header, and returns the flow to `Idle` (the
/// dialog closes — the Probe button is re-openable without a
/// restart).
#[test]
fn preview_dialog_open_issue_copies_report_and_notices() {
    let mut app = probe_test_app("preview-issue");
    let report = fixture_probe_report();
    let md = render_probe_report_md(&report);
    app.probe_state = ProbeState::Preview(md.clone());
    app.probe_title = Some(probe_issue_title(&report));
    let out_dir = app.out_dir.clone();
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(968.0, 600.0));
    let frame_input = |events: Vec<egui::Event>| egui::RawInput {
        screen_rect: Some(screen),
        events,
        ..Default::default()
    };
    let frame = |events: Vec<egui::Event>, app: &mut RamSleuthApp| {
        ctx.run(frame_input(events), |ctx| {
            app.render_probe_preview_dialog(ctx);
        })
    };

    frame(Vec::new(), &mut app);
    let first = frame(Vec::new(), &mut app);
    // The [Open GitHub Issue] label's center (the graph.rs
    // text-click idiom).
    let open = first
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Open GitHub Issue" => {
                Some(egui::pos2(
                    text.pos.x + text.galley.size().x / 2.0,
                    text.pos.y + text.galley.size().y / 2.0,
                ))
            }
            _ => None,
        })
        .expect("the Open GitHub Issue button's label must be painted");
    let click = |pressed: bool| egui::Event::PointerButton {
        pos: open,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    // Press + release → the clipboard copy + the notice + the
    // title-only URL open + the Idle transition (the click
    // completes on the release frame).
    frame(vec![click(true)], &mut app);
    let out = frame(vec![click(false)], &mut app);
    assert_eq!(
        app.probe_state,
        ProbeState::Idle,
        "a press + release on Open GitHub Issue must return the flow to Idle"
    );
    // The full report is in the clipboard (egui's copied_text
    // output — eframe forwards it to the OS clipboard).
    assert_eq!(
        out.platform_output.copied_text.as_str(),
        md.as_str(),
        "the full markdown report must be copied to the clipboard"
    );
    // The title-only issues/new URL opens (no body param).
    let Some(open_url) = out.platform_output.open_url else {
        panic!("the title-only issues/new URL must open");
    };
    assert!(
        !open_url.new_tab,
        "the URL must open in the same tab (the same_tab call)"
    );
    assert!(
        open_url
            .url
            .starts_with("https://github.com/MadGoatHaz/RamSleuth/issues/new?title="),
        "the URL must be the issues/new form: {}",
        open_url.url
    );
    assert!(
        !open_url.url.contains("body="),
        "the body must not ride the URL: {}",
        open_url.url
    );
    // The header notice tells the user to paste it into the body.
    assert_eq!(
        app.notice.as_ref().map(|(text, _)| text.as_str()),
        Some(PROBE_ISSUE_COPY_NOTICE),
        "the paste-into-the-body notice must be set"
    );

    fs::remove_dir_all(out_dir).expect("cleanup");
}

/// (q5c) The preview dialog's [Copy to Clipboard] copies the
/// full markdown report to the clipboard (egui's copied_text
/// output — eframe forwards it to the OS clipboard) and returns
/// the flow to `Idle` (the dialog closes — the Probe button is
/// re-openable without a restart).
#[test]
fn preview_dialog_copy_copies_and_returns_to_idle() {
    let mut app = probe_test_app("preview-copy");
    let report = fixture_probe_report();
    let md = render_probe_report_md(&report);
    app.probe_state = ProbeState::Preview(md.clone());
    app.probe_title = Some(probe_issue_title(&report));
    let out_dir = app.out_dir.clone();
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(968.0, 600.0));
    let frame_input = |events: Vec<egui::Event>| egui::RawInput {
        screen_rect: Some(screen),
        events,
        ..Default::default()
    };
    let frame = |events: Vec<egui::Event>, app: &mut RamSleuthApp| {
        ctx.run(frame_input(events), |ctx| {
            app.render_probe_preview_dialog(ctx)
        })
    };

    frame(Vec::new(), &mut app);
    let first = frame(Vec::new(), &mut app);
    let copy = first
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Copy to Clipboard" => {
                Some(egui::pos2(
                    text.pos.x + text.galley.size().x / 2.0,
                    text.pos.y + text.galley.size().y / 2.0,
                ))
            }
            _ => None,
        })
        .expect("the Copy to Clipboard button's label must be painted");
    let click = |pressed: bool| egui::Event::PointerButton {
        pos: copy,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![click(true)], &mut app);
    let out = frame(vec![click(false)], &mut app);
    assert_eq!(
        out.platform_output.copied_text.as_str(),
        md.as_str(),
        "the full markdown report must be copied to the clipboard"
    );
    assert_eq!(
        app.probe_state,
        ProbeState::Idle,
        "a press + release on Copy to Clipboard must return the flow to Idle"
    );

    fs::remove_dir_all(out_dir).expect("cleanup");
}

// -----------------------------------------------------------------
// The About/Help modal (chunk gui-about).
// -----------------------------------------------------------------

/// (a1) The About body: the version line carries the live
/// `CARGO_PKG_VERSION`; the daemon model, the capabilities, the
/// Intel/AMD split, and the full-guide pointer are all present.
#[test]
fn about_body_carries_the_version_and_capabilities() {
    let sections = about_body();
    let bodies: Vec<&str> = sections.iter().map(|(_, body)| body.as_str()).collect();
    let full = bodies.join("\n");
    assert!(
        full.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))),
        "the version line must carry the package version: {full}"
    );
    for needle in [
        "CAP_SYS_RAWIO",
        "Memory & Latency Benchmark",
        "XMP / EXPO",
        "ECC",
        "burn-in",
        "Probe report",
        "MCHBAR",
        "UCLK / MCLK",
        "Docs/User_Guide.md",
    ] {
        assert!(
            full.contains(needle),
            "the body must mention {needle:?}: {full}"
        );
    }
}

/// (a2) The About modal renders headless — the title, the
/// version line, the daemon-model line, and the [Close] button
/// paint — and a two-frame press / release on [Close] dismisses
/// it (the graph.rs text-click idiom; the app's gate, verbatim
/// from `update`, is `about_open`).
#[test]
fn about_dialog_paints_and_close_dismisses_headless() {
    let mut app = probe_test_app("about-dialog");
    app.about_open = true;
    let out_dir = app.out_dir.clone();
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(968.0, 600.0));
    let frame_input = |events: Vec<egui::Event>| egui::RawInput {
        screen_rect: Some(screen),
        events,
        ..Default::default()
    };
    let frame = |events: Vec<egui::Event>, app: &mut RamSleuthApp| {
        ctx.run(frame_input(events), |ctx| {
            // The app's per-frame gate, verbatim from `update`:
            // the dialog renders only while `about_open`.
            if app.about_open {
                app.render_about_dialog(ctx);
            }
        })
    };

    // Frame 1: the two areas are new — hidden for exactly one
    // frame (the first-frame placement heuristic).
    frame(Vec::new(), &mut app);
    // Frame 2 (no events): the modal paints (the title, the
    // body, the button).
    let first = frame(Vec::new(), &mut app);
    let texts: Vec<&str> = first
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect();
    assert!(
        texts.contains(&"About RamSleuth"),
        "the title must paint: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains(&format!("v{}", env!("CARGO_PKG_VERSION")))),
        "the version line must paint: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("CAP_SYS_RAWIO")),
        "the daemon-model line must paint: {texts:?}"
    );
    assert!(
        texts.contains(&"Close"),
        "the Close button must paint: {texts:?}"
    );
    assert!(app.about_open, "no click must not dismiss");

    // The [Close] label's center (the graph.rs text-click idiom).
    let close = first
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Close" => Some(egui::pos2(
                text.pos.x + text.galley.size().x / 2.0,
                text.pos.y + text.galley.size().y / 2.0,
            )),
            _ => None,
        })
        .expect("the Close button's label must be painted");

    // Frames 3 + 4: press, then release, on [Close] → the
    // dialog closes.
    let click = |pressed: bool| egui::Event::PointerButton {
        pos: close,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![click(true)], &mut app);
    frame(vec![click(false)], &mut app);
    assert!(
        !app.about_open,
        "a press + release on Close must dismiss the modal"
    );

    // Frame 5 (the gate): a dismissed modal repaints no dialog.
    let last = frame(Vec::new(), &mut app);
    let texts: Vec<&str> = last
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect();
    assert!(
        !texts.contains(&"About RamSleuth"),
        "a dismissed modal must not repaint the dialog: {texts:?}"
    );

    fs::remove_dir_all(out_dir).expect("cleanup");
}

/// (a3) The header's About button: the full header renders
/// headless, and a two-frame press / release on the "About"
/// label reports the `HeaderAction::About` edge (the app shell
/// opens the modal); a no-click frame reports `None` (the
/// graph.rs click idiom).
#[test]
fn render_header_about_button_reports_the_click_edge() {
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(968.0, 600.0));
    let flag = Arc::new(AtomicBool::new(false));
    let mut settings_open = false;
    let mut requirements_open = false;
    let mut render = |events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        ctx.begin_frame(input);
        let action = RamSleuthApp::render_header(
            &ctx,
            &TelemetryData::default(),
            &mut settings_open,
            &mut requirements_open,
            &flag,
            &None,
        );
        let out = ctx.end_frame();
        (action, out)
    };

    // Frame 1 (no click): the header renders the About button
    // and reports no edge.
    let (action, out) = render(Vec::new());
    assert_eq!(
        action,
        HeaderAction::None,
        "a no-click frame must report no edge"
    );
    let about = out
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "About" => Some(egui::pos2(
                text.pos.x + text.galley.size().x / 2.0,
                text.pos.y + text.galley.size().y / 2.0,
            )),
            _ => None,
        })
        .expect("the About button's label must be painted");

    // Frames 2 + 3: press, then release, on "About" → the click
    // edge is reported.
    let click = |pressed: bool| egui::Event::PointerButton {
        pos: about,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    render(vec![click(true)]);
    let (action, _) = render(vec![click(false)]);
    assert_eq!(
        action,
        HeaderAction::About,
        "a press + release on About must report the click edge"
    );
}
