import sys
import os
import json
import re
from typing import Any, Dict, List
import RamSleuth_DB

DEBUG = False

def set_debug(value: bool) -> None:
    global DEBUG
    DEBUG = value

def _debug_print(msg: str) -> None:
    """
    Lightweight debug logger controlled by the global DEBUG flag.

    This avoids importing logging and keeps behavior deterministic/minimal.
    """
    if DEBUG:
        print(f"[RamSleuth:DEBUG] {msg}", file=sys.stderr)

def check_root() -> None:
    """
    Ensure RamSleuth is executed with sufficient privileges.

    Requirements:
    - For accessing SMBus/SPD data reliably, root or equivalent capabilities
      are required.

    Behavior:
    - If not root (os.geteuid() != 0):
        - Print error to stderr.
        - Exit code 1.
    """
    try:
        geteuid = os.geteuid  # type: ignore[attr-defined]
    except AttributeError:
        # Non-POSIX (very rare for this tool). Assume OK.
        return

    if geteuid() != 0:
        print(
            "Error: RamSleuth must be run as root (or with appropriate privileges) to access SMBus/SPD data.",
            file=sys.stderr,
        )
        sys.exit(1)

def load_die_database() -> List[Dict[str, Any]]:
    """
    Wrapper around RamSleuth_DB.load_database() with CLI-safe error handling.

    Behavior:
    - On success: return loaded database list.
    - On FileNotFoundError:
        - Print fatal error and exit(4).
    - On JSON/Value errors:
        - Print fatal error with message and exit(4).
    """
    try:
        # Assuming die_database.json is in the project root, where the script is run
        return RamSleuth_DB.load_database("die_database.json")
    except FileNotFoundError:
        print(
            "FATAL: die_database.json not found! Please ensure it's in the same directory as the script.",
            file=sys.stderr,
        )
        sys.exit(4)
    except (json.JSONDecodeError, ValueError) as e:
        print(
            f"FATAL: Failed to parse die_database.json: {e}",
            file=sys.stderr,
        )
        sys.exit(4)

def prompt_for_sticker_code(dimm_index: int, brand: str) -> str:
    """
    Prompt user for brand-specific sticker/IC codes (interactive only).

    Brand hints:
    - "Corsair": Ask for "Ver X.XX" or similar from module label.
    - "G.Skill": Ask for the small alphanumeric sticker on heatspreader edge.
    - "Crucial_Lootbox": Ask for suffix like ".M8FE1" printed near barcode.
    - "Hynix_IC": Ask for full H5... IC part number from DRAM package.

    Returns:
        User-provided code (stripped). May be empty.
    """
    if brand == "Corsair":
        print(
            f"[DIMM {dimm_index}] Corsair module detected. "
            "Enter 'Ver X.XX' code from the small sticker (or leave blank):"
        )
    elif brand == "G.Skill":
        print(
            f"[DIMM {dimm_index}] G.Skill module detected. "
            "Enter the small 2-3 char sticker code (e.g. '21A') if present:"
        )
    elif brand == "Crucial_Lootbox":
        print(
            f"[DIMM {dimm_index}] Crucial 'Lootbox' kit detected. "
            "Enter suffix like '.M8FE1' from the white sticker:"
        )
    elif brand == "Hynix_IC":
        print(
            f"[DIMM {dimm_index}] SK Hynix DDR5 detected. "
            "Enter DRAM IC P/N (e.g. 'H5CG48AGBD') if readable:"
        )
    else:
        print(
            f"[DIMM {dimm_index}] Enter additional sticker/IC code for {brand}:"
        )

    try:
        value = input("> ")
        # Input validation for user-provided data
        if not isinstance(value, str):
            _debug_print(f"prompt_for_sticker_code: Invalid input type received: {type(value)}")
            return ""
        # Sanitize input - remove potentially dangerous characters
        sanitized_value = re.sub(r'[<>&|;"`\x00]', '', value)
        if sanitized_value != value:
            _debug_print(f"prompt_for_sticker_code: Input sanitized from '{value}' to '{sanitized_value}'")
        return sanitized_value.strip()
    except EOFError:
        _debug_print("prompt_for_sticker_code: EOFError during input")
        return ""
    except Exception as e:
        _debug_print(f"prompt_for_sticker_code: Unexpected error during input: {e}")
        return ""

def apply_lootbox_prompts(dimms: List[Dict[str, Any]], interactive: bool) -> None:
    """
    Optionally enrich DIMM metadata with user-supplied sticker/IC codes.

    Constraints:
    - Only run in interactive contexts (interactive=True).
    - Must never be invoked from non-interactive / batch code paths
      (JSON/summary/full/CI).

    Triggers (spec-aligned):
    - Corsair:
        * If dimm["manufacturer"] contains "Corsair" (case-insensitive),
          prompt_for_sticker_code(..., "Corsair") -> corsair_version.
    - G.Skill:
        * If dimm["manufacturer"] contains "G.Skill" or "GSkill"
          (case-insensitive), prompt_for_sticker_code(..., "G.Skill")
          -> gskill_sticker_code.
    - Crucial Lootbox (strict):
        * If dimm["manufacturer"] contains "Crucial" (ci)
          AND dimm["module_part_number"] equals "BL2K16G36C16U4B"
          (case-insensitive), prompt_for_sticker_code(..., "Crucial_Lootbox")
          -> crucial_sticker_suffix.
    - Hynix DDR5:
        * If dimm["dram_mfg"] contains "Hynix" (ci)
          AND normalized generation indicates DDR5, then
          prompt_for_sticker_code(..., "Hynix_IC") -> hynix_ic_part_number.
    """
    if not interactive:
        return

    for idx, dimm in enumerate(dimms):
        manufacturer = str(dimm.get("manufacturer", "")).lower()
        part_number = str(dimm.get("module_part_number", "")).lower()
        dram_mfg = str(dimm.get("dram_mfg", "")).lower()
        generation = str(dimm.get("generation", "")).upper()
        is_ddr5 = "DDR5" in generation

        # Corsair
        if "corsair" in manufacturer:
            code = prompt_for_sticker_code(idx, "Corsair")
            if code:
                dimm["corsair_version"] = code

        # G.Skill
        if "g.skill" in manufacturer or "gskill" in manufacturer:
            code = prompt_for_sticker_code(idx, "G.Skill")
            if code:
                dimm["gskill_sticker_code"] = code

        # Crucial Lootbox (BL2K16G36C16U4B only, strict)
        if "crucial" in manufacturer and part_number == "bl2k16g36c16u4b":
            code = prompt_for_sticker_code(idx, "Crucial_Lootbox")
            if code:
                dimm["crucial_sticker_suffix"] = code

        # Hynix DDR5 IC P/N
        if "hynix" in dram_mfg and is_ddr5:
            code = prompt_for_sticker_code(idx, "Hynix_IC")
            if code:
                dimm["hynix_ic_part_number"] = code