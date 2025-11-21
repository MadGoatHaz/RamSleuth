#!/usr/bin/env python3
"""
RamSleuth CLI/TUI orchestrator.

This script:
- Performs environment checks and dependency discovery.
- Discovers SMBus/I2C busses and SPD addresses.
- Invokes `decode-dimms` (if available).
- Parses side-by-side output into canonical DIMM dictionaries.
- Uses RamSleuth_DB to resolve die types from die_database.json.
- Exposes clean CLI modes:
  * --summary
  * --full
  * --json
  * --tui
- Provides a Textual-based TUI when requested or by default (if available).
- Respects non-interactive vs interactive constraints.

Sysfs and dependency behavior:
- May perform best-effort SPD EEPROM registration via
  `/sys/bus/i2c/devices/i2c-*/new_device` on detected busses/addresses when run
  as root. These writes:
  * Are part of normal Phase 3 operation.
  * Are best-effort and non-fatal; failures are only surfaced via debug logs
    when DEBUG is enabled.
- Never performs automatic package installation; only suggests commands for the user.
"""

import argparse
import sys
from typing import Dict, Any

import RamSleuth_DB
from ramsleuth_pkg.dependency_engine import check_and_install_dependencies
from ramsleuth_pkg.utils import check_root, load_die_database, apply_lootbox_prompts, set_debug
from ramsleuth_pkg.scanner import perform_system_scan, SmbusNotFoundError
from ramsleuth_pkg.tui import output_summary, output_full, output_json, launch_tui

__version__ = "1.2.0"

def parse_arguments() -> argparse.Namespace:
    """
    Parse CLI arguments and return the populated namespace.

    Modes (mutually exclusive):
    - --summary
    - --full
    - --json
    - --tui

    Other flags:
    - --no-interactive / --ci (alias)
    - --debug (optional; reserved; may enable verbose diagnostics)
    - --test-data (hidden, for validation only)

    Defaults:
    - When no explicit output flag is given:
        - Prefer TUI (if available) in interactive contexts.
        - Fallback to summary text.
    """
    parser = argparse.ArgumentParser(
        prog="RamSleuth",
        description="DIMM die identification and SPD analysis tool.",
    )

    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--summary",
        action="store_true",
        help="Print concise DIMM summary to stdout.",
    )
    group.add_argument(
        "--full",
        action="store_true",
        help="Print detailed DIMM information and raw SPD dumps.",
    )
    group.add_argument(
        "--json",
        action="store_true",
        help="Output DIMM data as JSON (no interactive prompts).",
    )
    group.add_argument(
        "--tui",
        action="store_true",
        help="Launch Textual-based TUI interface.",
    )

    parser.add_argument(
        "--no-interactive",
        dest="no_interactive",
        action="store_true",
        help="Disable all interactive prompts (for scripting/CI).",
    )
    parser.add_argument(
        "--ci",
        dest="ci",
        action="store_true",
        help="Alias for --no-interactive (CI-friendly).",
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="Enable debug mode (reserved for verbose diagnostics).",
    )
    parser.add_argument(
        "--test-data",
        action="store_true",
        help=argparse.SUPPRESS,  # Hidden flag for validation
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"%(prog)s {__version__}",
        help="Show program's version number and exit.",
    )

    return parser.parse_args()


def main() -> None:
    """
    Main orchestrator for RamSleuth.

    Execution flow:
    1. Parse arguments.
    2. Determine non_interactive.
    3. Check root privileges.
    4. Check dependencies and prompt as needed.
    5. Load kernel modules (best-effort, non-fatal).
    6. Discover SMBus/I2C busses and SPD addresses (non-fatal if none).
    7. Run decode-dimms and capture outputs.
    8. Parse side-by-side output into DIMM dictionaries.
    9. Load die heuristic database.
    10. Normalize DIMM data and apply heuristics.
    11. Optionally apply interactive 'lootbox' prompts and re-normalize.
    12. Resolve die_type for each DIMM.
    13. Dispatch output according to CLI mode.

    Output mode precedence (if multiple flags are provided):
        --json > --full > --summary > --tui
    """
    args = parse_arguments()

    # Initialize debug flag (used by _debug_print and other helpers).
    set_debug(bool(getattr(args, "debug", False)))

    # Determine non-interactive behavior:
    # - Explicit flags (--no-interactive/--ci)
    # - Any concrete non-TUI mode (--json/--summary/--full)
    non_interactive = (
        args.no_interactive
        or args.ci
        or args.json
        or args.summary
        or args.full
    )

    # Root check early to fail fast when meaningful
    # Skip root check when using test data
    if not getattr(args, "test_data", False):
        check_root()

    # Determine requested features for dependency handling.
    requested_features: Dict[str, bool] = {
        "core": True,
        "tui": bool(
            args.tui
            or (
                not non_interactive
                and not (args.summary or args.full or args.json)
            )
        ),
    }

    # Unified dependency handling with autonomous system-native installation.
    check_and_install_dependencies(
        requested_features=requested_features,
        interactive=not non_interactive,
    )

    # Execute the system scan
    try:
        dimms, raw_individual = perform_system_scan(
            test_data_mode=getattr(args, "test_data", False),
            fail_on_no_smbus=non_interactive
        )
    except SmbusNotFoundError as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(6)
    except RuntimeError as e:
        if non_interactive:
            print(f"Error: {e}", file=sys.stderr)
            sys.exit(8)
        else:
            # Should not be reached given perform_system_scan logic for RuntimeError
            # but safety fallback
            print(f"Warning: {e}", file=sys.stderr)
            dimms = []
            raw_individual = {}

    # If we are in an interactive/TUI-capable flow (no explicit non-interactive
    # output mode), run lootbox prompts and re-resolve using enriched data.
    interactive_lootbox = (not non_interactive) and not (
        args.json or args.summary or args.full
    )
    if interactive_lootbox:
        # Load DB for re-resolution
        db = load_die_database()
        apply_lootbox_prompts(dimms, interactive=True)
        for dimm in dimms:
            dimm.update(RamSleuth_DB.normalize_dimm_data(dimm))
            die_type, notes = RamSleuth_DB.find_die_type(dimm, db)
            dimm["die_type"] = die_type
            if notes:
                dimm["notes"] = notes

    # Dispatch output according to requested mode.
    # Enforce precedence if multiple flags were somehow provided:
    if args.json:
        # JSON mode must not emit garbage on stdout on error; all errors above
        # exit with non-zero prior to reaching here.
        output_json(dimms)
        return

    if args.full:
        output_full(dimms, raw_individual)
        return

    if args.summary:
        output_summary(dimms)
        return

    # TUI or default behavior:
    # If --tui explicitly, attempt TUI (only if Textual is available);
    # on failure, launch_tui() will fall back to summary.
    if args.tui:
        launch_tui(dimms, raw_individual)
        return

    # Default when no explicit mode:
    # - If interactive: try TUI when DIMMs are present (with fallback inside launch_tui()).
    # - If interactive but no DIMMs: print summary instead of launching an empty TUI.
    # - Else: summary.
    if not non_interactive:
        if dimms:
            launch_tui(dimms, raw_individual)
        else:
            output_summary(dimms)
        return

    # Fallback summary for any non-interactive default.
    output_summary(dimms)


if __name__ == "__main__":
    main()