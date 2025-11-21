# RamSleuth Implementation Plan

This document outlines the plan to fix, enhance, and optimize the RamSleuth codebase. The plan is divided into three phases: Fixes (Critical), Enhancements (Features), and Optimizations (Refactoring).

## Phase 1: Fixes (Critical Bugs & Maintenance)

The primary goal of this phase is to eliminate code duplication, improve security, and ensure consistent behavior.

### 1.1. Consolidate Configuration Logic
**Overview:** `ramsleuth.py` currently contains its own `load_config` and `save_config` functions (lines 226-376) which duplicate the logic in the robust `settings_service.py`. This violates the DRY principle and bypasses the validation logic in the service.
**Action Items:**
- Remove `load_config` and `save_config` functions from `ramsleuth.py`.
- Import the `SettingsService` (or the backward-compatible `load_config`/`save_config` wrappers) from `settings_service.py`.
- Update all calls in `ramsleuth.py` to use the imported functions.
- Ensure the global `DEBUG` flag is correctly passed to the service initialization.

### 1.2. Secure & Efficient Device Registration
**Overview:** The `register_devices` function (lines 735-806) uses `subprocess.run(["sudo", "tee", ...])` to write to sysfs. Since the script enforces root execution via `check_root()`, using `sudo` is redundant, inefficient, and potentially less secure than direct file I/O.
**Action Items:**
- Refactor `register_devices` to use Python's built-in `open(path, 'w')` context manager.
- Remove the `subprocess` call.
- Implement `try/except` blocks to handle `PermissionError` and `OSError` (e.g., if the device is already registered or the driver is missing), preserving the "best-effort/non-fatal" behavior.

### 1.3. Refine Root & Dependency Logic
**Overview:** `get_current_memory_settings` checks `os.geteuid()` and conditionally targets `sudo dmidecode`. Since the main entry point enforces root (except in test mode), this check is largely redundant. Additionally, `load_modules` uses a bare `except Exception`.
**Action Items:**
- In `get_current_memory_settings`, remove the conditional `sudo` prefixing. Rely on the main script's root enforcement.
- Ensure `dmidecode` execution fails gracefully (or is skipped) if running in `--test-data` mode without root privileges.
- Refine exception handling in `load_modules` to catch specific exceptions (`OSError`, `subprocess.SubprocessError`) where possible, though the current defensive coding is acceptable.

---

## Phase 2: Enhancements (New Features)

This phase focuses on improving the user experience and adding standard CLI features.

### 2.1. Add Version Flag
**Overview:** The CLI lacks a standard `--version` flag.
**Action Items:**
- Define a `__version__` constant in `ramsleuth.py`.
- Add `--version` argument to `parse_arguments()`.
- Print version and exit in `main()` if the flag is present.

### 2.2. TUI Enhancements
**Overview:** The TUI is currently static; it loads data at launch and cannot refresh. It also lacks export functionality.
**Action Items:**
- **Rescan Capability:**
    - Add a "Rescan" binding (e.g., `Ctrl+R`) to `RamSleuthApp`.
    - This requires Phase 3.1 (Refactoring Scanning Logic) to be completed or implemented concurrently.
    - The action should re-run the hardware scan and update the `DataTable` and `raw_data` without restarting the application.
- **Export Functionality:**
    - Add an "Export" binding (e.g., `Ctrl+E`).
    - Allow saving the current DIMM data to a JSON file (e.g., `ramsleuth_report.json`) from within the TUI.

---

## Phase 3: Optimizations (Refactoring)

This phase improves code structure and maintainability.

### 3.1. Refactor Scanning Logic
**Overview:** The core logic for discovering busses, scanning addresses, running the decoder, and parsing output is currently embedded directly in the `main()` function. This makes it difficult to reuse for the TUI "Rescan" feature or integration tests.
**Action Items:**
- Extract the scanning workflow into a dedicated function, e.g., `perform_system_scan() -> Tuple[List[Dict], Dict[str, str]]`.
    - This function should handle `find_smbus`, `scan_bus`, `register_devices`, `run_decoder`, `parse_output`, and `deduplicate_dimms`.
    - It should handle the `--test-data` logic as a parameter or internal check.
- Update `main()` to call this function.
- Update `RamSleuthApp` to call this function for its initial load and the Rescan feature.

### 3.2. Code Cleanup
**Overview:** `ramsleuth.py` is a large file. Cleaning up imports and organizing helper functions will improve readability.
**Action Items:**
- Group imports (standard library, third-party, local).
- Ensure consistent type hinting.
- Remove any unused legacy code or commented-out blocks.
