# RamSleuth - RAM SPD Inspector and Die Identifier

RamSleuth is a non-destructive memory analysis tool that reads SPD (Serial Presence Detect) data from RAM modules to identify DRAM die types and provide detailed hardware information. Built around the industry-standard `decode-dimms` tool, it offers both command-line and interactive TUI interfaces for analyzing memory modules.

![RamSleuth TUI Interface](Doc/Screenshot.png?v=20251121071956)

## Overview

### What RamSleuth Does

RamSleuth safely inspects your computer's RAM modules to answer key questions:
- What DRAM die type is used in each memory module?
- What are the specifications and capabilities of your RAM?
- What are the detailed SPD timings, including XMP/EXPO profiles?
- Are your modules running at their rated speeds?

The tool primarily reads detailed SPD data from hardware using `i2c-tools` and `decode-dimms`. It applies a curated heuristic database to identify DRAM die manufacturers and types. It supports DDR3, DDR4, and DDR5 memory modules from major vendors including Samsung, SK Hynix, Micron, Corsair, G.Skill, and Crucial.

**Important Note on Timings (Transparency Principle):**
RamSleuth prioritizes reading full XMP/EXPO and JEDEC timings (CL-RCD-RP-RAS) directly from the SPD chip using `decode-dimms`. However, if the hardware SPD read fails (e.g., due to chipset limitations or locked memory), the tool employs a **Part Number Fallback** mechanism. In this fallback scenario, only the nominal Speed and CAS Latency (CL) are extracted from the module's part number string, and full timing data will be unavailable. This limitation is clearly indicated in the output.

### Key Features

- **DRAM Die Detection**: Identifies memory die types using a comprehensive heuristic database
- **Multiple Output Modes**: Summary, detailed, JSON, and interactive TUI views
- **Interactive TUI**: Text-based interface with theme support and keyboard navigation
- **Persistent Configuration**: Remembers your preferences across sessions
- **Automatic Dependency Management**: Detects and helps install required tools
- **Safe Operation**: Non-destructive, read-only analysis with no hardware modification
- **Cross-Distribution Support**: Works on 15+ Linux distributions with native package management

### Quick Start

```bash
# Clone and run (requires root for hardware access)
git clone <repository-url>
cd RamSleuth
sudo python ramsleuth.py

# Or use specific modes
sudo python ramsleuth.py --summary    # Concise one-line per DIMM
sudo python ramsleuth.py --full       # Detailed view with raw SPD data
sudo python ramsleuth.py --json       # Machine-readable JSON output
sudo python ramsleuth.py --tui        # Interactive TUI interface
```

### Usage Examples

**Default Mode** (interactive TUI if available):
```bash
sudo python ramsleuth.py
```

**Summary Mode** (scripting friendly):
```bash
sudo python ramsleuth.py --summary
# Output: DIMM_A1: DDR4 16GB Corsair CMK16GX4M2B3200C16 -> Samsung B-die
```

**JSON Mode** (automation):
```bash
sudo python ramsleuth.py --json > memory_report.json
```

**Test Data Mode** (validation without hardware):
```bash
python ramsleuth.py --test-data
```

### Theme Usage

The TUI supports theme persistence with two methods:

**Ctrl+T**: Toggle between dark and light themes instantly. Your preference is saved immediately.

**Ctrl+P**: Open the command palette (requires Textual 0.20.0+) and type "dark" or "light" to select.

Configuration is stored at `~/.config/ramsleuth/ramsleuth_config.json` and works correctly when running with `sudo` (saves to your user directory, not root's).

### System Requirements

- **Python**: 3.8 or newer
- **Permissions**: Root access (or equivalent) for SPD hardware access
- **Dependencies**: 
  - `i2c-tools` (provides `i2cdetect` and `decode-dimms`)
  - `dmidecode`
  - `python-textual` (optional, for TUI mode)
  - `python-linkify-it-py` (optional, for TUI help)

RamSleuth will detect missing dependencies and offer to install them using your system's native package manager (apt, pacman, dnf, etc.).

---

## Technical Details

### Architecture

RamSleuth is structured as a Python package (`ramsleuth_pkg`) with a dedicated entry point (`ramsleuth.py`) for simplified orchestration. This modular approach enhances testability and maintainability.

**Core Components (within `ramsleuth_pkg`):**
- `tui.py`: Interactive Text-based User Interface.
- `parser.py`: Handles data parsing, normalization, and heuristic resolution.
- `dependency_engine.py`: System-native dependency detection and installation.
- `scanner.py`: Handles hardware discovery (SMBus/I2C bus scanning) and SPD data collection.
- `utils.py`: Contains utility functions and settings integration.

**Top-Level Components:**
- `ramsleuth.py`: Main orchestrator and CLI entry point.
- `RamSleuth_DB.py`: Pure heuristic engine for die type identification.
- `settings_service.py`: Centralized configuration management with XDG compliance.
- `die_database.json`: Curated heuristic rules database.

**Execution Flow:**
1. Environment validation (root check, dependency detection)
2. Hardware discovery (SMBus/I2C bus scanning)
3. SPD data collection via `i2c-tools` / `decode-dimms`
4. Data parsing and normalization
5. Heuristic matching against die database
6. Output formatting based on selected mode

### Configuration Management

The `SettingsService` class provides centralized configuration management:

- **XDG Base Directory Compliance**: Config stored at `~/.config/ramsleuth/`
- **Sudo Awareness**: When run with sudo, uses original user's home directory
- **Automatic Ownership Management**: Recursive chown ensures proper file permissions
- **Validation**: Built-in validation for all settings with clear error handling
- **Persistence**: Immediate save on change, not deferred until exit

**Configuration API:**
```python
from settings_service import SettingsService

settings = SettingsService(debug=True)
theme = settings.get_setting("theme", "dark")
settings.set_setting("theme", "light")
```

### Dependency Engine

The dependency engine provides autonomous package management across 15+ Linux distributions:

**Supported Distributions:**
- Arch Linux, Artix, EndeavourOS, Manjaro
- Debian, Ubuntu, Linux Mint, Pop!_OS
- Fedora, RHEL, CentOS, Rocky Linux, AlmaLinux
- openSUSE (Leap and Tumbleweed)
- Gentoo

**Behavior:**
- Detects distribution and appropriate package manager automatically
- Checks for missing core tools (`i2cdetect`, `decode-dimms`, `dmidecode`)
- Checks for optional Python packages (`textual`, `linkify-it-py`)
- In interactive mode, asks permission before installing anything
- Constructs and executes correct installation commands for each distribution
- Never installs packages without explicit user approval

### TUI Implementation

The interactive Textual-based TUI has been redesigned for improved usability and information density:

**Layout and Navigation:**
- **25/75 Split Layout**: A fixed vertical sidebar (25%) on the left for the DIMM inventory list, and a main content area (75%) on the right for detailed information.
- **DIMM Cards**: The left sidebar uses a `DataTable` to present a concise list of detected DIMMs, acting as navigation cards.
- **Detail Panes**: The main content area uses tabs (`Summary` and `Full`) to display detailed information for the currently selected DIMM.
- **Current Settings**: A dedicated pane displays live system memory settings (e.g., current frequency, motherboard timings).

**Key Features:**
- **Dual Theme Methods**: Both Ctrl+T toggle and Ctrl+P command palette
- **Focus Management**: Tab key switches between DIMM list and detail panes
- **Persistent State**: Active tab and theme preferences saved automatically
- **Rich Text Support**: Full Rich markup for formatted output
- **Keyboard Navigation**: Vim-style keys (j/k) and arrow keys

**Enhanced TUI Widgets:**
Recent enhancements ensure detail panes support:
- Line-by-line keyboard navigation
- Mouse text selection for copying to clipboard
- Visual focus indicators
- Improved scrolling performance

### Heuristic Database

The `die_database.json` contains prioritized rules for DRAM die identification:

**Rule Structure:**
```json
{
  "priority": 100,
  "die_type": "Samsung B-die",
  "generation": "DDR4",
  "manufacturer": "corsair",
  "part_number_contains": "CMK",
  "timings_xmp": "3200-16-18-18"
}
```

**Matching Logic:**
- Rules evaluated in priority order (highest first)
- All constraints must match (logical AND)
- Supports substring, exact, and prefix matching
- Handles sticker codes and IC part numbers for enhanced detection

### Interactive Features

**Lootbox/Sticker Prompts:**
In interactive mode, RamSleuth can prompt for additional information to improve detection accuracy:

- **Corsair**: "Ver X.XX" version codes from module labels
- **G.Skill**: Small alphanumeric sticker codes on heatspreaders
- **Crucial**: Suffix codes like ".M8FE1" from BL2K16G36C16U4B kits
- **SK Hynix**: IC part numbers from DDR5 DRAM packages

These prompts only appear in interactive TUI mode and are never shown in non-interactive modes (`--summary`, `--full`, `--json`).

### Debug Mode

Enable debug logging to troubleshoot issues:

```bash
sudo python ramsleuth.py --debug
```

Debug output includes:
- Configuration file operations and path resolution
- Hardware discovery details (SMBus scanning, device registration)
- Parser decision logic and intermediate data
- Theme change events and state management
- Dependency detection and installation steps

### Test Mode

Use test data for validation without hardware:

```bash
python ramsleuth.py --test-data
```

This mode uses the included `test_data.txt` fixture to demonstrate parsing and heuristic matching without requiring actual hardware access.

### Development

**Setup:**
```bash
python -m venv .venv
. .venv/bin/activate
pip install pytest textual
```

**Run Tests:**
```bash
PYTHONPATH=. pytest -q
```

**Test Coverage:**
- `test_parse_output.py`: Parser validation with side-by-side fixtures
- `test_RamSleuth_DB_is_match.py`: Heuristic matching logic
- `test_settings_service.py`: Configuration management
- `test_dependency_engine.py`: Dependency detection across distributions
- `test_command_palette.py`: TUI command palette functionality
- `test_theme_persistence.py`: Theme saving and loading

### Safety and Security

**Design Principles:**
- **Read-Only Operation**: Never modifies hardware or firmware
- **Best-Effort Registration**: SPD EEPROM registration via sysfs is non-fatal
- **Explicit User Consent**: No automatic package installation without permission
- **Input Validation**: All user input sanitized to prevent injection
- **Error Isolation**: Failures in one module don't crash the entire application
- **Clear Error Messages**: Human-readable errors with actionable guidance

**Sysfs Registration:**
When run as root, RamSleuth may attempt to register SPD EEPROM devices via `/sys/bus/i2c/devices/i2c-*/new_device`. These operations are:
- Best-effort only (failures don't stop execution)
- Non-destructive (no data modification)
- Debug-logged only (not shown to users unless `--debug` is used)
- Appropriate for DDR4/DDR5 SPD EEPROMs using the `ee1004` driver

### Exit Codes

RamSleuth uses specific exit codes for different scenarios:
- `0`: Success
- `1`: Insufficient privileges (not root)
- `4`: Database error (missing or invalid `die_database.json`)
- `5`: Dependency installation failure
- `6`: No SMBus detected in non-interactive mode
- `8`: Decoder execution failure or test data not found

---

## Feedback and Contributions

I welcome feedback, suggestions, and contributions from the community:

- **Questions or Issues**: Open an issue on the repository
- **Feature Requests**: Share your ideas for improvements
- **Database Contributions**: Help expand the die detection database
- **Code Contributions**: Submit pull requests for bug fixes or enhancements
- **Documentation**: Help improve documentation and examples

If RamSleuth has been helpful for your memory analysis needs, please consider starring the repository to help others discover it.

### Contributing Guidelines

- Keep `ramsleuth.py` as the orchestrator (environment checks, I/O, CLI/TUI)
- Keep `RamSleuth_DB.py` pure (no printing, no exits, deterministic only)
- Include tests for parsing changes in `tests/test_parse_output.py`
- Include tests for heuristic changes in `tests/test_RamSleuth_DB_is_match.py`
- Follow existing patterns and maintain non-interactive compatibility
- Never implement automatic package installation without user approval

## Support the Project

- [GitHub Sponsors](https://github.com/sponsors/MadGoatHaz)
- [PayPal](https://www.paypal.com/paypalme/garretthazlett)

Donations support development. If you donate to the app, any feature requests you have will be pushed to the top of the request list based upon the donation amount.

---

## License

Refer to the LICENSE file in the repository for licensing information. External tools (`decode-dimms`, `i2c-tools`, `dmidecode`) are separate projects with their own licenses.