# RamSleuth Implementation Plan v3 (Refined)

This plan incorporates refinements to handle "Manual/OC" detection and display "Max XMP/EXPO" capabilities even when they don't match the current speed.

## Phase 1: Real Timings & Dependencies

 **Goal:** Display the *actual* active timings (XMP/EXPO/Tweaked) and show "Max Rated" capabilities.

### 1.1. Dependency Management (`dependency_engine.py`)
*   **Objective:** Ensure `decode-dimms` is available to read detailed SPD data.
*   **Action Items:**
    *   Verify `i2c-tools` is correctly mapped for all distros in `dependency_engine.py`.
    *   Ensure the auto-installer works as expected.

### 1.2. Kernel Module Loading
*   **Objective:** Ensure necessary kernel modules are loaded for `decode-dimms` to function.
*   **Action Items:**
    *   Update `scanner.load_modules()` to explicitly include `eeprom` (sometimes required by `decode-dimms` legacy modes) in addition to `ee1004` (DDR4/5) and `at24`.
    *   Sequence: `modprobe eeprom` -> `modprobe ee1004` -> `register_devices` -> `decode-dimms`.

### 1.3. Active Profile Inference & Capabilities Logic
*   **Challenge:** We cannot directly read current CAS Latency from userspace (requires root + direct hardware access/PCI scraping).
*   **Refined Strategy (Inference):**
    1.  **Get Configured Speed:** Use `dmidecode -t memory` to get "Configured Memory Speed" (e.g., "3800 MT/s").
    2.  **Get Capabilities:** Use `decode-dimms` to parse the full SPD table, extracting all JEDEC and XMP/EXPO profiles.
    3.  **Identify Max XMP:** Find the XMP/EXPO profile with the highest speed (e.g., "3600 MT/s") and store it as "Rated XMP".
    4.  **Match:** Compare "Configured Speed" against the profiles.
        *   If Configured Speed matches an XMP profile speed -> Assume XMP is active. Display XMP timings.
        *   If Configured Speed matches a JEDEC profile -> Assume JEDEC.
        *   If Configured Speed != any profile:
            *   Display "Active Profile: Manual/OC".
            *   Display "Rated XMP: [Max Speed] (Timings)" to let the user compare.
*   **Action Items:**
    *   Enhance `scanner.get_current_memory_settings()`:
        *   Parse XMP/EXPO tables from `decode-dimms` output more robustly.
        *   Implement logic to find the "Max" profile regardless of current speed.
        *   Implement the "Manual/OC" detection logic.

## Phase 2: TUI Redesign

**Goal:** A more information-dense and vertically oriented interface.

### 2.1. Layout Restructuring (`ramsleuth_pkg/tui.py`)
*   **Grid Split:**
    *   **Left Pane:** 25% width.
    *   **Right Pane:** 75% width.
*   **CSS Implementation:**
    ```css
    Screen { layout: horizontal; }
    #left_pane { width: 25%; height: 100%; dock: left; }
    #right_pane { width: 75%; height: 100%; dock: right; }
    ```

### 2.2. Left Pane: Vertical Stack of Cards
*   **Requirement:** "Left pane information must be displayed vertically (stacked), not horizontally."
*   **Implementation:**
    *   Replace `DataTable` with a `VerticalScroll` container.
    *   Create a custom Widget `DIMMCard(Static)` or `Button`.
    *   **Card Content (Stacked per DIMM):**
        ```text
        [ Slot: DIMM_A1 ]
        [ Mfg: Corsair  ]
        [ Part: CMK32... ]
        [ Config: 3800MT/s @ 1.35V ]
        --------------------------
        ```
    *   **Interactivity:** Clicking a card or using Up/Down keys updates the Right Pane.

### 2.3. Right Pane: Detailed Views
*   **Summary Tab:**
    *   Show the "Real" timings (inferred matches) prominently.
    *   **New:** Show "Max Rated XMP" if running Manual/OC.
    *   Show Voltage comparison (Configured vs XMP).
*   **Full Tab:**
    *   Raw `decode-dimms` output.

## Phase 3: Validation

*   **Test Case 1 (Dependencies):** Run on a clean VM/environment. Verify `i2c-tools` is installed automatically.
*   **Test Case 2 (TUI):** Verify 1/4 vs 3/4 split. Verify navigation between DIMM cards.
*   **Test Case 3 (Timings):** Mock `dmidecode` output (3800MT/s) and `decode-dimms` (XMP 3600) to verify:
    *   Active Profile shows "Manual/OC".
    *   Capabilities show "Rated XMP: 3600 MT/s".