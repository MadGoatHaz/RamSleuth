# RamSleuth Implementation Plan v2

This updated plan incorporates new requirements for identifying current RAM timings, architectural optimizations, and UI enhancements.

## Phase 1: Enhanced Memory Telemetry (Timings & Speed)

The goal requires distinguishing between *rated* specifications (SPD) and *current* operating parameters.

### 1.1. Live Speed & Timing Extraction
**Objective:** Retrieve "Configured Speed" and attempt to infer or retrieve active timings.
*   **Strategy:**
    *   **Speed:** `dmidecode -t memory` is the most reliable standard source for "Configured Memory Speed" (DDR4/5) or "Speed" (DDR3).
    *   **Timings (Direct):** Linux user-space tools rarely expose live CAS Latency (tCL) without direct memory controller access (PCI scraping), which is hardware-specific (Intel vs AMD vs generations) and high-risk.
    *   **Timings (Inferred):**
        1.  Get "Configured Speed" from `dmidecode`.
        2.  Scan SPD data (`decode-dimms`) for a generic JEDEC or XMP profile that matches this speed.
        3.  Display "Inferred Timings" with a clear label (e.g., "Active Profile (Inferred): 3200 MT/s @ 16-18-18-38").
*   **Action Items:**
    *   Refine `get_current_memory_settings()` in `ramsleuth.py`:
        *   Capture `Configured Clock Speed` and `Minimum/Maximum Voltage`.
    *   Create a new helper `match_speed_to_profile(current_speed, spd_data)`:
        *   Parses the raw SPD text to find JEDEC/XMP tables.
        *   Returns the timings for the entry matching `current_speed`.

### 1.2. Voltage Telemetry
**Objective:** Display current voltage vs rated voltage.
*   **Action Items:**
    *   Parse `Configured Voltage` from `dmidecode`.
    *   Compare against `Voltage` (SPD) to show if the user is undervolting/overvolting.

## Phase 2: Architectural Refactoring & Optimization

`ramsleuth.py` has grown too large (~2500 lines). We will modularize it to improve maintainability and enable features like non-blocking UI updates.

### 2.1. Module Split
**Objective:** Decompose `ramsleuth.py` into logical modules.
*   **Structure:**
    *   `ramsleuth.py` (Entry point, CLI parsing, Orchestrator)
    *   `scanner.py` (Hardware detection: `find_smbus`, `scan_bus`, `run_decoder`, `perform_system_scan`)
    *   `parser.py` (Text parsing: `parse_output`, `deduplicate_dimms`)
    *   `tui.py` (Textual App: `RamSleuthApp`, `launch_tui`)
    *   `utils.py` (Helpers: `check_root`, `_debug_print`, `load_modules`)
*   **Action Items:**
    *   Create files and migrate code.
    *   Ensure circular imports are avoided.
    *   Update `ramsleuth.py` to import from these new modules.

### 2.2. Non-Blocking "Rescan" (Threading)
**Objective:** Prevent the TUI from freezing during a hardware scan.
*   **Action Items:**
    *   In `tui.py`, refactor `action_rescan` to run `perform_system_scan` in a generic `threading.Thread`.
    *   Use Textual's `call_from_thread` or `post_message` to update the UI once the scan completes.
    *   Add a `LoadingIndicator` or "Scanning..." modal during the operation.

## Phase 3: UI & Feature Enhancements

### 3.1. Detailed "Timings" View
**Objective:** A dedicated view for memory enthusiasts.
*   **UI Changes:**
    *   Add a "Timings" tab in the TUI right pane.
    *   Display a matrix comparison:
        *   **Column 1: JEDEC (Standard)**
        *   **Column 2: XMP/EXPO (Rated)**
        *   **Column 3: Current (Active/Inferred)**
    *   Highlight discrepancies (e.g., if running JEDEC speed but XMP is available).

### 3.2. Export Capability
**Objective:** Save findings to disk.
*   **Action Items:**
    *   Implement `Ctrl+E` (Export).
    *   Formats: JSON (machine readable) and Markdown (human readable report).
    *   Include "System Summary" (Kernel, relevant hardware) in the report.

### 3.3. Database Updater (Optional/Future)
*   *Deferred for now to focus on core stability, but architecture should allow swapping `die_database.json` easily.*

## Execution Order

1.  **Refactor (Phase 2.1):** Split the files first. It's hard to add features to a 2500-line file safely.
2.  **Timings Logic (Phase 1):** Implement the new parsing logic in `parser.py` / `scanner.py`.
3.  **TUI Upgrades (Phase 2.2 & 3):** Implement threading and the new Timings view in `tui.py`.