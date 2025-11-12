# RamSleuth Development Guide

## 1. Overview

RamSleuth is structured as a small, testable toolkit for safe SPD inspection and DRAM die identification.

Core components:

- `ramsleuth.py`:
  - Orchestrator and CLI/TUI entrypoint.
  - Handles root/dependency checks, SMBus discovery, `decode-dimms` integration, parsing, normalization, heuristic resolution, and output modes.
- `RamSleuth_DB.py`:
  - Pure heuristic engine:
    - Loads and validates `die_database.json`.
    - Normalizes DIMM metadata.
    - Evaluates `is_match()` constraints.
    - Selects die types via `find_die_type()` using priority-ordered rules.
- `dependency_engine.py`:
  - Autonomous dependency management system:
    - Detects 15+ Linux distributions with appropriate package managers.
    - Checks for missing system tools and Python packages.
    - Performs autonomous installation with user approval (interactive mode).
    - Provides clear guidance for manual installation (non-interactive mode).
- `settings_service.py`:
  - Centralized configuration management:
    - XDG Base Directory specification compliance.
    - Sudo context awareness with proper file ownership handling.
    - Settings validation and persistence.
    - Backward compatibility with existing config format.
- `die_database.json`:
  - Source of heuristic rules.
  - Entries are prioritized (`priority`) and consumed by `RamSleuth_DB`.
- `tests/`:
  - `tests/test_parse_output.py`:
    - Validates `parse_output()` against a stable side-by-side fixture.
  - `tests/test_RamSleuth_DB_is_match.py`:
    - Validates `is_match()` semantics for all constraint types.
  - `tests/test_dependency_engine.py`:
    - Validates dependency detection and installation logic.
  - `tests/test_settings_service.py`:
    - Validates configuration management and persistence.

This document describes how these pieces fit together, expected workflows, and the current development track.

## 2. Architecture

### 2.1 Orchestrator (`ramsleuth.py`)

Responsibilities (high-level):

- Environment and dependency handling:
  - `check_root()`:
    - Enforces that RamSleuth runs with sufficient privileges for SMBus/SPD access.
  - `dependency_engine.check_and_install_dependencies()`:
    - Detects missing dependencies including system tools (i2c-tools, dmidecode) and Python libraries (textual, linkify-it-py).
    - Asks for user permission before attempting any installation.
    - Uses the system's native package manager (apt, pacman, dnf, etc.) based on distro detection.
    - Only proceeds with installation after explicit user approval.
    - Provides a "run-and-it-works" experience while maintaining user control.
  - **Removed legacy functions**: `check_distro()`, `_get_pkg_cmds()`, `check_dependencies()`, and `prompt_install()` have been completely removed in favor of the centralized `dependency_engine` approach.

- Kernel module and SMBus handling:
  - `load_modules()`:
    - Best-effort, idempotent `modprobe` calls for:
      - Core: `i2c-dev`, `ee1004`, `at24`.
      - Vendor SMBus drivers (e.g., `i2c-amd-mp2-pci`, `i2c-i801`) based on detected CPU vendor.
    - Failures are non-fatal and only surfaced via the lightweight debug logger.
  - `find_smbus()`:
    - Runs `i2cdetect -l` (if available).
    - Filters candidate adapters based on SMBus-related markers.
    - Excludes GPU/graphics adapters.
    - Returns a sorted list of bus IDs.
  - `scan_bus(bus_id)`:
    - Runs `i2cdetect -y <bus_id>`.
    - Collects addresses in `0x50-0x57` as SPD candidates.
    - Returns sorted unique addresses on success; tolerant of errors.

- Sysfs registration:
  - `register_devices(bus_id, addresses)`:
    - Always attempts best-effort registration of SPD EEPROM devices for the given bus/addresses.
    - Uses `/sys/bus/i2c/devices/i2c-<bus_id>/new_device` with an `ee1004` driver hint for DDR4/DDR5 SPD EEPROMs.
    - Must not abort the main flow on errors:
      - Missing paths, permission issues, or existing devices are non-fatal.
      - Detailed failures are only surfaced via the lightweight debug logger when DEBUG is enabled.
    - There is no feature-flag gate in Phase 3; any future opt-out mechanism must be introduced as an explicit design change.

- Decoder integration:
  - `run_decoder()`:
    - Resolves `decode-dimms` (required).
    - Invokes plain `decode-dimms` as the primary source of structured per-DIMM data.
    - Invokes `decode-dimms --side-by-side` as a best-effort supplementary source; failures here must not mask a successful plain invocation.
    - If both plain and side-by-side variants fail to produce usable output, raises `RuntimeError` (handled by `main()` based on mode).
    - Returns:
      - `combined_output`: plain output, optionally followed by side-by-side output (consumed by `parse_output()`).
      - `raw_individual`: mapping of `dimm_N` keys to raw plain decode-dimms blocks used by `--full`/TUI.

- Parsing:
  - `parse_output(raw_output)`:
    - Accepts the combined output from `run_decoder()`.
    - If a matrix-style `"Field | DIMM ..."` header is present:
      - Builds one DIMM dict per column.
    - Otherwise:
      - Parses plain `Decoding EEPROM` / `SPD data for` blocks and their key/value lines.
      - Extracts canonical fields (generation, manufacturer, `module_part_number`, `dram_mfg`, `module_gb`, `module_ranks`, `SDRAM Device Width`, `JEDEC_voltage`, `JEDEC Timings`, slot from `Guessing DIMM is in`, DDR5-specific fields).
      - Splits grouped slot labels into one logical record per slot; does not emit aggregate `"bank 3 bank 4"` style rows.
    - Returns a list of DIMM dicts representing one logical record per physical DIMM before de-duplication.

- Heuristic DB integration:
  - `load_die_database()`:
    - Wrapper around `RamSleuth_DB.load_database("die_database.json")`.
    - On failure (not found, parse errors, structural errors):
      - Emits clear fatal messages to stderr.
      - Exits with specific non-zero codes.

- Interactive lootbox/sticker prompts:
  - `prompt_for_sticker_code(dimm_index, brand)`:
    - Brand-specific instructions (Corsair, G.Skill, Crucial Lootbox, Hynix IC).
    - Returns user-provided codes.
  - `apply_lootbox_prompts(dimms, interactive)`:
    - Only runs when explicitly in an interactive flow.
    - Adds:
      - `corsair_version`
      - `gskill_sticker_code`
      - `crucial_sticker_suffix`
      - `hynix_ic_part_number`
    - Used to refine heuristic matches for supported kits.

- CLI / mode handling:
  - `parse_arguments()`:
    - Modes (mutually exclusive):
      - `--summary`
      - `--full`
      - `--json`
      - `--tui`
    - Flags:
      - `--no-interactive`
      - `--ci` (alias)
      - `--debug`
  - `output_summary(dimms)`:
    - Concise, one-line-per-DIMM summary.
  - `output_full(dimms, raw_individual)`:
    - Detailed per-DIMM view plus raw `decode-dimms` block where available.
  - `output_json(dimms)`:
    - JSON-only output on stdout (no extra noise).
  - `launch_tui(dimms, raw_individual)`:
    - Textual-based TUI with enhanced features:
      - Left: DIMM list with sortable columns and frozen first column.
      - Right: Summary/Full views for selected DIMM.
      - Current memory settings pane showing live system data.
      - Theme management (Ctrl+T toggle, Ctrl+P command palette).
      - Tab persistence across sessions.
      - Pane focus toggling for keyboard navigation.
    - If Textual is missing:
      - Warns on stderr and falls back to `output_summary()`.

- Main flow:
  - `main()`:
    - Parses arguments.
    - Derives `non_interactive` based on flags and modes.
    - Enforces root check.
    - Validates dependencies; guides or exits if missing.
    - Loads modules (best-effort).
    - Discovers SMBus busses and SPD addresses; calls `register_devices()` as best-effort Phase 3 behavior.
    - Calls `run_decoder()` → `combined_output`, `raw_individual`.
    - Calls `parse_output(combined_output)` to obtain initial DIMM candidates.
    - Loads `die_database.json` via `load_die_database()`.
    - Normalizes each DIMM with `RamSleuth_DB.normalize_dimm_data()`.
    - Resolves `die_type` and `notes` via `RamSleuth_DB.find_die_type()`.
    - Applies conservative de-duplication:
      - Treats entries as duplicates when slot (if present), `module_part_number`, manufacturer, and generation all match.
      - Keeps one record per physical DIMM.
      - Avoids phantom aggregate entries.
    - In interactive default/TUI-capable flows without non-interactive flags:
      - Runs lootbox/sticker prompts after initial parsing, re-normalizes, and re-resolves `die_type`.
    - Dispatches output:
      - Precedence: `--json` > `--full` > `--summary` > `--tui` > default behavior.
    - Maintains strict separation:
      - Non-interactive modes never trigger interactive prompts.

### 2.2 Dependency Engine (`dependency_engine.py`)

The dependency engine provides autonomous dependency management using system-native package managers only, completely eliminating pip-based installation and handling 15+ Linux distributions.

**Key Components:**

- **Distribution Detection** (`detect_distribution()`):
  - Enhanced OS detection using `/etc/os-release` as primary source.
  - Falls back to `lsb_release` and platform detection.
  - Supports 15+ distributions: Arch, Debian, Ubuntu, Fedora, RHEL, CentOS, Rocky, AlmaLinux, openSUSE, Gentoo, and derivatives.
  - Returns structured information including package manager commands.

- **Dependency Checking** (`get_missing_dependencies()`):
  - Checks for core tools: `i2cdetect`, `decode-dimms`, `dmidecode`.
  - Checks for Python packages: `textual`, `linkify_it`.
  - Uses multiple verification methods: `shutil.which()`, `--version`, `--help`.
  - Categorizes missing dependencies as "system" or "python".

- **Installation Command Construction** (`construct_install_command()`):
  - Maps abstract tool names to distribution-specific package names.
  - Builds appropriate install commands for each package manager:
    - `pacman -S --noconfirm --needed` (Arch)
    - `apt install -y` (Debian/Ubuntu)
    - `dnf install -y` (Fedora/RHEL)
    - `zypper install -y` (openSUSE)
    - `emerge --ask n` (Gentoo)

- **Autonomous Installation** (`auto_install_dependencies()`):
  - Executes installation commands with timeout protection (10 minutes).
  - Provides real-time feedback on installation progress.
  - Verifies installation success and reports failures.

- **Error Handling**:
  - `handle_unknown_distro()`: Fatal error with manual installation instructions.
  - `handle_installation_failure()`: Clear error messages for installation failures.

- **Main Integration** (`check_and_install_dependencies()`):
  - Orchestrates the entire dependency flow.
  - Interactive mode: Attempts autonomous installation with user approval.
  - Non-interactive mode: Fails fast with clear guidance and installation commands.
  - Handles unknown distributions gracefully.

**Behavior:**
- **Interactive mode**: Prompts user for permission, then attempts automatic installation.
- **Non-interactive mode**: Provides exact commands for manual installation and exits.
- **User control**: No automatic installation without explicit user approval.
- **Distribution coverage**: Comprehensive support for major Linux distributions.

### 2.3 Heuristic Engine (`RamSleuth_DB.py`)

Key responsibilities:

- `load_database(filepath="die_database.json")`:
  - Resolves path relative to this module.
  - Loads JSON and enforces:
    - Top-level list.
    - Each entry:
      - `priority`: int
      - `die_type`: non-empty string
  - Sorts entries by descending `priority`.
  - Raises exceptions for callers to handle.

- Normalization helpers:
  - Normalize:
    - Generation: `DDR1`/`DDR2`/`DDR3`/`DDR4`/`DDR5`.
    - Manufacturers: conservative string normalization.
    - `dram_mfg`: standardized for SK Hynix, Samsung, Micron, etc.
    - Capacities (`module_gb`): from MB/GB strings or numerics.
    - Ranks (`module_ranks`): into forms like `1R`, `2R`, `4R`.
    - Chip organization (`chip_org`): e.g., `x8`, `x16`.
    - Part numbers and sticker codes.
    - Timings and voltages.
  - All helpers are side-effect-free.

- `normalize_dimm_data(dimm)`:
  - Consumes raw DIMM dict (e.g., from `parse_output()`).
  - Produces a new dict with normalized keys:
    - `generation`
    - `manufacturer`
    - `dram_mfg`
    - `module_gb`
    - `module_ranks`
    - `chip_org`
    - `module_part_number`
    - `timings_xmp`
    - `timings_jdec`
    - `voltage_xmp`
    - `voltage_jdec`
    - Sticker/IC fields:
      - `corsair_version`
      - `gskill_sticker_code`
      - `crucial_sticker_suffix`
      - `hynix_ic_part_number`
  - Intended to be merged back into the caller's dict:
    - Callers typically do `dimm.update(normalize_dimm_data(dimm))`.

- `is_match(dimm, entry)`:
  - Deterministic rule evaluation:
    - Only recognized keys influence matching.
    - All constraints are combined with logical AND.
  - Implements:
    - `generation`: exact.
    - `manufacturer`: case-insensitive substring.
    - `dram_mfg`: exact, case-insensitive.
    - `module_gb`: numeric equality.
    - `module_ranks`: exact.
    - `chip_org`: exact.
    - `part_number_contains`: substring.
    - `part_number_exact`: case-insensitive exact.
    - `timings_xmp`: exact or substring.
    - `timings_jdec`: exact.
    - `voltage_xmp`: string equality.
    - `corsair_version`:
      - If DB value ends with `.` → prefix match.
      - Otherwise exact.
    - `gskill_sticker_code`: substring.
    - `crucial_sticker_suffix`: case-insensitive exact.
    - `hynix_ic_parse_8th`:
      - Matches 8th character (index 7) of `hynix_ic_part_number`.
  - Ignores unknown keys in the DB entry.

- `find_die_type(dimm, db)`:
  - Iterates all DB entries in priority-sorted order.
  - Tracks matches at the highest priority encountered.
  - Resolution:
    - No matches:
      - `("Unknown", "No heuristic match found in database.")`
    - Single best match:
      - Returns its `die_type` and optional `notes`.
    - Multiple matches at same best priority:
      - If all share same `die_type`:
        - Returns that `die_type` and combined notes.
      - Otherwise:
        - Returns `("Ambiguous", "Multiple matching heuristics at same priority: ...")`.

This module is pure: no printing, no exits, deterministic behavior only.

### 2.4 Settings Service (`settings_service.py`)

The SettingsService provides centralized configuration management with sophisticated path resolution and sudo awareness.

**Core Class: `SettingsService`**

- **Initialization** (`__init__`):
  - Accepts debug flag for verbose logging.
  - Automatically initializes paths and loads settings.
  - Provides backward-compatible API.

- **Path Management**:
  - `_initialize_paths()`: Determines config directory and file paths.
  - `_get_config_base_dir()`: Implements XDG Base Directory specification.
  - **Sudo awareness**: When running with sudo, uses original user's home directory.
  - **XDG compliance**: Respects `XDG_CONFIG_HOME` environment variable.

- **Settings Management**:
  - `load_settings()`: Loads configuration from JSON file with error handling.
  - `save_settings()`: Saves configuration with automatic directory creation.
  - `_handle_sudo_ownership()`: Changes file ownership when running with sudo.
  - **Validation**: Built-in validation rules for known settings.
  - **Defaults**: Provides sensible defaults for all settings.

- **API Methods**:
  - `get_setting(key, default)`: Retrieve individual settings.
  - `set_setting(key, value)`: Set and persist individual settings.
  - `get_all_settings()`: Retrieve complete settings dictionary.
  - `validate_setting(key, value)`: Validate settings against rules.
  - `reset_to_defaults()`: Reset all settings to defaults.

**Backward Compatibility**:

- `load_config(debug)`: Legacy function that uses SettingsService internally.
- `save_config(config, debug)`: Legacy function that uses SettingsService internally.
- `get_default_settings(debug)`: Module-level singleton accessor.

**Configuration Keys**:
- `theme`: "dark" or "light" (validated)
- `active_tab`: "summary_tab" or "full_tab" (validated)
- `default_view_tab`: "summary" or "full" (validated)

**Behavior**:
- **Normal execution**: Config saved to `~/.config/ramsleuth/ramsleuth_config.json`
- **Sudo execution**: Config saved to original user's home directory, not root's
- **File ownership**: When running with sudo, config file ownership is changed to the original user
- **Directory creation**: Config directory is created automatically with `parents=True`
- **Error handling**: Graceful fallback to defaults on any errors

### 2.5 Enhanced TUI Features

The Textual-based TUI includes several advanced features beyond basic display:

**Widget Enhancements**:
- **DataTable**: Sortable columns with frozen first column for better horizontal scrolling.
- **Current Settings Pane**: Live display of system memory settings from `dmidecode`.
- **Dual-pane layout**: Independent summary and full views with tab persistence.
- **Keyboard navigation**: Vim-style keys (j/k) plus arrow keys, with pane focus toggling.

**Theme Management**:
- **Ctrl+T Toggle**: Immediate theme switching with persistence.
- **Command Palette** (Ctrl+P): Textual 0.20.0+ feature providing "dark" and "light" commands.
- **watch_dark()**: Automatic persistence of theme changes from any source.
- **watch_theme()**: Support for custom theme names beyond basic dark/light.
- **Theme validation**: Falls back to "dark" if requested theme is unavailable.

**Command Palette System**:
- **ThemeCommandProvider**: Implements Textual's Provider interface.
- **Commands**: "dark" and "light" with descriptive help text.
- **Integration**: Registered via `COMMAND_PROVIDERS` set in TUI app.
- **Actions**: `action_set_theme_dark()` and `action_set_theme_light()` methods.

**Tab Persistence**:
- **Automatic saving**: Active tab saved immediately on change.
- **Config key**: Uses `"active_tab"` in settings.
- **Restoration**: Tab state restored on TUI launch.

**Pane Focus Management**:
- **action_toggle_pane_focus()**: Switches focus between DIMM selector and detail pane.
- **Visual feedback**: Focused pane indicated by Textual's default styling.
- **Keyboard accessibility**: Full navigation without mouse.

**Current Memory Settings Integration**:
- **Live data**: Parses `dmidecode -t memory` for actual system settings.
- **JEDEC Speed**: Extracted from SPD via "Maximum module speed" or "JEDEC Timings".
- **XMP Profile**: Parsed from `timings_xmp` or part number.
- **Complete metadata**: Includes manufacturer, part number, size, voltage, and timings.

### 2.6 Tests & Fixtures

- `tests/test_parse_output.py`:
  - Imports `parse_output` from `ramsleuth`.
  - Uses `test_data.txt` to:
    - Validate DIMM count.
    - Ensure core fields are parsed and preserved.
    - Ensure slot derivation from `Guessing DIMM is in`.
    - Ensure JEDEC timings and DDR5-specific fields are preserved.
    - Validate tolerance for malformed and extra lines.
    - Assert deterministic results across runs.

- `tests/test_RamSleuth_DB_is_match.py`:
  - Exercises `is_match()` with a synthetic normalized DIMM template.
  - Validates:
    - Each constraint type independently (positive/negative cases).
    - Prefix/substr semantics for sticker-based fields.
    - Handling of unknown keys in DB entries.
    - Tight all-constraints match scenario.

- `tests/test_dependency_engine.py`:
  - Validates distribution detection for supported distributions.
  - Tests dependency checking logic.
  - Verifies installation command construction.
  - Tests error handling for unknown distributions.

- `tests/test_settings_service.py`:
  - Validates configuration loading and saving.
  - Tests sudo-aware path resolution.
  - Verifies settings validation rules.
  - Tests backward compatibility functions.

These tests encode the expected semantics of parsing, heuristic matching, dependency management, and configuration.

## 3. Development Setup

Recommended workflow from repository root:

1. Create a virtual environment:
   - `python -m venv .venv`
2. Activate it:
   - `. .venv/bin/activate`
3. Install dependencies:
   - Preferred (if available):
     - `pip install -r requirements-dev.txt`
   - Minimal test/dev setup:
     - `pip install pytest textual`
4. Run tests:
   - `PYTHONPATH=. pytest -q`

Notes:

- Always run commands from the repository root so imports resolve (`ramsleuth.py`, `RamSleuth_DB.py`, tests).
- Hardware/SPD access:
  - Unit tests rely on `test_data.txt` and synthetic DIMMs; they do not require root.
  - Real hardware integration (running `ramsleuth.py` end-to-end) should be done as root with `i2c-tools` / `decode-dimms` installed.

## 4. Current Development Track

### 4.1 Completed (Phase 1–3)

The following work is implemented and reflected in the current codebase and tests:

- **Specification ingestion**:
  - Architecture and behavior aligned with the design documents in `Doc/`.
- **Discrepancy analysis**:
  - Prior spec vs implementation mismatches have been addressed in the orchestrator and heuristic engine.
- **Dependency engine**:
  - Fully autonomous dependency management with system-native package managers.
  - Support for 15+ Linux distributions.
  - Interactive and non-interactive modes with appropriate user control.
  - Comprehensive error handling and fallback guidance.
- **Settings service**:
  - Centralized configuration management with XDG compliance.
  - Sudo-aware path resolution and file ownership handling.
  - Settings validation and persistence.
  - Backward compatibility with existing code.
- **Test harness**:
  - `test_data.txt` added as a deterministic side-by-side fixture.
  - `tests/test_parse_output.py` covers:
    - Column mapping.
    - Capacity/rank/width fields.
    - JEDEC voltage mapping.
    - Slot derivation.
    - DDR5 PMIC and IC fields.
    - Robust handling of malformed/extra lines.
  - `tests/test_RamSleuth_DB_is_match.py` covers:
    - All supported constraint types.
    - Sticker/lootbox/IC semantics.
    - Deterministic, AND-based matching.
  - `tests/test_dependency_engine.py` covers:
    - Distribution detection accuracy.
    - Dependency checking logic.
    - Installation command construction.
  - `tests/test_settings_service.py` covers:
    - Configuration persistence.
    - Sudo context handling.
    - Settings validation.
- **Core orchestrator behaviors**:
  - `parse_output()` aligned with side-by-side expectations and fixture.
  - `load_modules()` implemented as best-effort, idempotent, non-fatal.
  - `register_devices()`:
    - Performs best-effort SPD EEPROM registration via the sysfs `new_device` interface on detected candidate busses/addresses.
    - Non-fatal on failure, with details only visible under DEBUG.
  - **Dependency integration**:
    - `check_and_install_dependencies()` called from main flow.
    - Respects interactive/non-interactive modes.
    - Provides clear guidance for manual installation when needed.
  - **Settings integration**:
    - Configuration loaded via `load_config()` (uses SettingsService internally).
    - Theme management integrated into TUI.
    - Tab persistence across sessions.
  - **Lootbox/sticker prompting**:
    - Implemented via `prompt_for_sticker_code()` and `apply_lootbox_prompts()`.
    - Only active in interactive flows (no prompts in `--summary/--full/--json`).
    - Brand-specific triggers for Corsair, G.Skill, Crucial Lootbox, SK Hynix DDR5.
  - **CLI modes**:
    - `--summary`, `--full`, `--json`, `--tui` implemented.
    - Precedence rules enforced in `main()`.
    - Non-interactive flags (`--no-interactive`, `--ci`) respected.
  - **TUI enhancements**:
    - Textual-based TUI implemented via `launch_tui()`.
    - Clear layout with DIMM list and detail views.
    - Enhanced DataTable with frozen columns and sorting.
    - Current memory settings pane with live system data.
    - Theme management (Ctrl+T toggle, Ctrl+P command palette).
    - Tab persistence and pane focus management.
    - Graceful fallback to summary if Textual is unavailable.
- **Validation**:
  - Behavior and expectations are captured via:
    - The test suite.
    - The orchestrator and heuristic module design.
    - This DEV.md and the top-level README as alignment references.

### 4.2 Known Work / Future Development

Planned and recommended enhancements:

- **Test realism and coverage**:
  - Add more `decode-dimms --side-by-side` fixtures from varied platforms and DIMM vendors.
  - Introduce integration tests for `ramsleuth.main()`:
    - Non-root execution paths.
    - Missing-tool scenarios.
    - Handling of partial/failed `decode-dimms` runs.
  - Expand dependency engine tests for additional distributions.
  - Add performance benchmarks for large DIMM inventories.

- **Database evolution**:
  - Grow `die_database.json` with:
    - Additional SKUs and bins.
    - More sticker-driven rules that align with existing `is_match()` semantics.
  - Consider database versioning for backward compatibility.

- **Extended heuristics**:
  - Explore additional vendor-specific rules:
    - SK Hynix, Micron, Samsung patterns.
    - DDR5 PMIC / IC interpretations as upstream `decode-dimms` stabilizes fields.
  - Add voltage-based heuristics for overclocking identification.

- **Enhanced TUI features**:
  - Search/filter functionality in DIMM list.
  - Export capabilities (save JSON/summary from TUI).
  - Real-time monitoring of memory settings.
  - Custom theme support beyond dark/light.

- **Packaging**:
  - Introduce a proper project layout:
    - `pyproject.toml` / packaging metadata.
    - Console entry points instead of `python ramsleuth.py`.
  - Distribution packages for major Linux distributions.

- **CI integration**:
  - Configure CI to run on pushes/PRs:
    - `PYTHONPATH=.`.
    - Unit tests.
    - Optionally linting and type checking (e.g., `ruff`, `mypy`).
  - Add distribution detection tests in CI containers.

- **Documentation**:
  - Keep `README.md` and `DEV.md` synchronized with implementation.
  - Add usage examples for:
    - Ambiguous matches.
    - Unknown/unsupported modules.
    - JSON consumption patterns.
  - Create user guide for TUI features.
  - Document distribution-specific installation nuances.

## 5. Contribution Guidelines

Lightweight rules to keep the project coherent and safe:

- **Design and code structure**:
  - Keep `ramsleuth.py` as the orchestrator:
    - Environment checks, subprocess calls, I/O, CLI/TUI, and high-level flow.
  - Keep `RamSleuth_DB.py`:
    - Pure, deterministic logic.
    - No direct user interaction, printing, or exits.
  - Keep `dependency_engine.py`:
    - Self-contained dependency management.
    - No side effects without user approval.
  - Keep `settings_service.py`:
    - Centralized configuration logic.
    - Backward compatibility maintained.

- **Tests**:
  - Any change to parsing (`parse_output`) must:
    - Update or add fixtures as needed.
    - Include/adjust tests in `tests/test_parse_output.py`.
  - Any change to heuristics (`is_match`, `find_die_type`, normalization, or `die_database.json` schema/semantics) must:
    - Include/adjust tests in `tests/test_RamSleuth_DB_is_match.py`.
  - Any change to dependency handling must:
    - Include tests in `tests/test_dependency_engine.py`.
  - Any change to configuration management must:
    - Include tests in `tests/test_settings_service.py`.

- **Safety**:
  - Sysfs SPD registration is implemented as best-effort Phase 3 behavior. Any modifications (including adding opt-out flags) must be treated as explicit design changes and documented.
  - **User approval requirement**: The dependency_engine asks for permission before installing any packages. This maintains user control while providing a smooth setup experience.
  - **No automatic package installation**: Never bypass the user approval step in interactive mode.
  - **Non-fatal failures**: Module loading, SMBus discovery, and sysfs registration must never cause hard failures.

- **Style and behavior**:
  - Follow existing patterns and naming conventions.
  - Preserve deterministic, explicit error handling and clear exit codes.
  - Maintain compatibility with non-interactive environments (CI, automation).
  - Keep functions pure where possible; I/O and side effects should be explicit.

- **Documentation**:
  - Update DEV.md and README.md for any architectural changes.
  - Ensure line number references in documentation remain accurate.
  - Add docstrings for new public functions and classes.
  - Keep examples in documentation current with implementation.