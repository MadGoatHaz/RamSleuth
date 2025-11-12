"""
Heuristic die identification engine for RamSleuth.

This module encapsulates all logic for:
- Loading the heuristic database from die_database.json
- Normalizing DIMM/module metadata
- Evaluating deterministic, side-effect-free heuristic matches
- Selecting an appropriate die_type based on prioritized rules

Design constraints:
- This is the ONLY component that directly reads die_database.json.
- All filesystem access is read-only.
- All paths are resolved relative to this file to preserve portability.
- All functions are pure with respect to their inputs (aside from load_database
  performing a deterministic JSON file read) and do not print or exit.
"""

import json
import os
import re
from typing import Any, Dict, List, Tuple, Optional


def load_database(filepath: str = "die_database.json") -> List[Dict[str, Any]]:
    """
    Load and validate the die heuristic database.

    Behavior:
    - Resolve the JSON path relative to this file.
    - Load JSON content.
    - Validate structure:
      * Top-level must be a list.
      * Each item must be a dict containing:
        - "priority": int
        - "die_type": non-empty str
      * Other keys are accepted as-is.
    - Sort entries by:
      * Descending "priority"
      * Stable within equal-priority entries.

    Error handling:
    - Propagates:
      * FileNotFoundError
      * json.JSONDecodeError
      * ValueError on structural issues

    Returns:
        Sorted list of validated database entries.
    """
    base_dir = os.path.dirname(os.path.abspath(__file__))
    db_path = os.path.join(base_dir, filepath)

    with open(db_path, "r", encoding="utf-8") as f:
        data = json.load(f)

    if not isinstance(data, list):
        raise ValueError("die_database.json must contain a top-level JSON array")

    validated: List[Dict[str, Any]] = []
    for idx, raw in enumerate(data):
        if not isinstance(raw, dict):
            raise ValueError(f"Entry {idx} must be an object/dict")

        if "priority" not in raw:
            raise ValueError(f"Entry {idx} missing required key 'priority'")
        if not isinstance(raw["priority"], int):
            raise ValueError(f"Entry {idx} 'priority' must be int")

        if "die_type" not in raw:
            raise ValueError(f"Entry {idx} missing required key 'die_type'")
        if not isinstance(raw["die_type"], str) or not raw["die_type"].strip():
            raise ValueError(f"Entry {idx} 'die_type' must be a non-empty string")

        validated.append(raw)

    # Sort by descending priority, stable within equal priorities
    validated.sort(key=lambda e: e["priority"], reverse=True)
    return validated


def parse_xmp_from_part_number(part_number: Optional[str]) -> Optional[str]:
    """
    Extract a normalized XMP-style timing summary from a DIMM part number.

    Patterns:
    - (\\d{4})C(\\d{2})  => "freqCL"     -> "freq-cl"
      Example: "F4-3600C16D-16GTZ" -> "3600-16"
    - (\\d{4})-(\\d{2})-(\\d{2})-(\\d{2})
      Example: "3200-14-14-14" -> "3200-14-14-14"

    Rules:
    - If part_number is falsy, return None.
    - Prefer the more specific full-timing pattern when both patterns could apply.
    - Return normalized string or None if no pattern matches.
    """
    if not part_number:
        return None

    text = str(part_number)

    # More specific: full timing pattern
    full_pattern = re.search(r"(\d{4})-(\d{2})-(\d{2})-(\d{2})", text)
    if full_pattern:
        freq, t1, t2, t3 = full_pattern.groups()
        return f"{freq}-{t1}-{t2}-{t3}"

    # Less specific: frequency + CL pattern
    simple = re.search(r"(\d{4})C(\d{2})", text)
    if simple:
        freq, cl = simple.groups()
        return f"{freq}-{cl}"

    return None


def _normalize_generation(value: Any) -> Optional[str]:
    if not value:
        return None
    s = str(value).strip().upper()
    # Keep only DDRx prefix
    if s.startswith("DDR"):
        # e.g. "DDR4 SDRAM" -> "DDR4"
        parts = s.split()
        core = parts[0]
        # Normalize variants like DDR-4
        core = core.replace("-", "")
        if core in {"DDR", "DDR1"}:
            return "DDR1"
        if core == "DDR2":
            return "DDR2"
        if core == "DDR3":
            return "DDR3"
        if core == "DDR4":
            return "DDR4"
        if core == "DDR5":
            return "DDR5"
    return None


def _normalize_manufacturer(value: Any) -> Optional[str]:
    if not value:
        return None
    s = str(value).strip()
    if not s:
        return None
    # Conservative: return stripped as-is
    return s


def _normalize_dram_mfg(value: Any) -> Optional[str]:
    if not value:
        return None
    s = str(value).strip()
    if not s:
        return None

    lower = s.lower()
    # Minimal normalization for common variants
    if "sk" in lower and "hynix" in lower:
        return "SK Hynix"
    if "hynix" in lower:
        return "SK Hynix"
    if "samsung" in lower:
        return "Samsung"
    if "micron" in lower:
        return "Micron"
    if "nanya" in lower:
        return "Nanya"
    return s


def _parse_module_gb(value: Any) -> Optional[float]:
    if value is None:
        return None

    # If already numeric, trust it
    if isinstance(value, (int, float)):
        return float(value)

    s = str(value).strip().upper()
    if not s:
        return None

    # Patterns:
    # - "16384 MB", "32768MB"
    # - "16 GB", "16GB"
    mb_match = re.match(r"^\s*(\d+(?:\.\d+)?)\s*MB\s*$", s)
    gb_match = re.match(r"^\s*(\d+(?:\.\d+)?)\s*GB\s*$", s)

    if mb_match:
        mb = float(mb_match.group(1))
        # convert MB to GB
        return mb / 1024.0

    if gb_match:
        gb = float(gb_match.group(1))
        return gb

    # If plain number, assume GB
    if re.fullmatch(r"\d+(?:\.\d+)?", s):
        return float(s)

    return None


def _normalize_module_ranks(value: Any) -> Optional[str]:
    if not value:
        return None
    s = str(value).strip().upper()
    if not s:
        return None

    # Accept already normalized forms like "1R", "2R"
    if re.fullmatch(r"\dR", s):
        return s

    # Handle words
    if s in {"SINGLE", "SINGLE RANK"}:
        return "1R"
    if s in {"DUAL", "DUAL RANK"}:
        return "2R"
    if s in {"QUAD", "QUAD RANK"}:
        return "4R"

    # Bare digit -> add "R"
    if s.isdigit():
        return f"{s}R"

    return None


def _normalize_chip_org(value: Any) -> Optional[str]:
    if not value:
        return None
    s = str(value).strip().lower()
    if not s:
        return None

    # Examples:
    # - "8 bits"  -> "x8"
    # - "16 bits" -> "x16"
    bits_match = re.search(r"(\d+)\s*bits?", s)
    if bits_match:
        return f"x{bits_match.group(1)}"

    # If already like x8/x16 (case-insensitive)
    x_match = re.fullmatch(r"x\d+", s)
    if x_match:
        return s.lower()

    return None


def _extract_first_str(d: Dict[str, Any], keys: List[str]) -> Optional[str]:
    for key in keys:
        if key in d and d[key]:
            val = str(d[key]).strip()
            if val:
                return val
    return None


def normalize_dimm_data(dimm: Dict[str, Any]) -> Dict[str, Any]:
    """
    Produce a normalized view of DIMM/module metadata for heuristic matching.

    Notes:
    - Does NOT mutate the input dict.
    - Intended to be merged back by caller via: dimm.update(normalize_dimm_data(dimm)).
    - Uses conservative parsing: only emits keys where a safe interpretation exists.
    """
    norm: Dict[str, Any] = {}

    # generation
    gen = (
        _normalize_generation(dimm.get("generation"))
        or _normalize_generation(dimm.get("Memory Type"))
        or _normalize_generation(dimm.get("DRAM Generation"))
    )
    if gen:
        norm["generation"] = gen

    # manufacturer (module/brand)
    manufacturer = _extract_first_str(
        dimm,
        [
            "Module Manufacturer",
            "Manufacturer",
            "manufacturer",
            "Brand",
            "Module Vendor",
        ],
    )
    manufacturer = _normalize_manufacturer(manufacturer)
    if manufacturer:
        norm["manufacturer"] = manufacturer

    # dram manufacturer (IC vendor)
    dram_mfg = _extract_first_str(
        dimm,
        [
            "DRAM Manufacturer",
            "DRAM MFG",
            "IC Manufacturer",
            "dram_mfg",
        ],
    )
    dram_mfg = _normalize_dram_mfg(dram_mfg)
    if dram_mfg:
        norm["dram_mfg"] = dram_mfg

    # module_gb
    module_gb_source = (
        dimm.get("module_gb")
        or dimm.get("Module Capacity")
        or dimm.get("Module Capacity (MB)")
        or dimm.get("Size")
        or dimm.get("Module Size")
    )
    module_gb = _parse_module_gb(module_gb_source)
    if module_gb is not None:
        # Prefer int if integral, else float
        norm["module_gb"] = int(module_gb) if module_gb.is_integer() else module_gb

    # module_ranks
    ranks_source = (
        dimm.get("module_ranks")
        or dimm.get("Ranks")
        or dimm.get("Rank")
        or dimm.get("rank")
        or dimm.get("Module Ranks")
    )
    module_ranks = _normalize_module_ranks(ranks_source)
    if module_ranks:
        norm["module_ranks"] = module_ranks

    # chip_org
    chip_org_source = (
        dimm.get("chip_org")
        or dimm.get("SDRAM Device Width")
        or dimm.get("Chip Organization")
        or dimm.get("Organization")
    )
    chip_org = _normalize_chip_org(chip_org_source)
    if chip_org:
        norm["chip_org"] = chip_org

    # module_part_number
    part_number = _extract_first_str(
        dimm,
        [
            "Part Number",
            "Module Part Number",
            "module_part_number",
            "PartNumber",
            "P/N",
        ],
    )
    if part_number:
        norm["module_part_number"] = part_number

    # sticker / version / IC-specific fields
    for key in [
        "gskill_sticker_code",
        "corsair_version",
        "crucial_sticker_suffix",
        "hynix_ic_part_number",
    ]:
        if key in dimm and dimm[key]:
            val = str(dimm[key]).strip()
            if val:
                norm[key] = val

    # timings_xmp
    timings_xmp = dimm.get("timings_xmp")
    if timings_xmp:
        timings_xmp_str = str(timings_xmp).strip()
    else:
        timings_xmp_str = parse_xmp_from_part_number(part_number) if part_number else None
    if timings_xmp_str:
        norm["timings_xmp"] = timings_xmp_str

    # timings_jdec / JEDEC timings (minimal normalization)
    timings_jdec = (
        dimm.get("timings_jdec")
        or dimm.get("timings_jedec")
        or dimm.get("JEDEC Timings")
    )
    if timings_jdec:
        timings_jdec_str = str(timings_jdec).strip()
        if timings_jdec_str:
            norm["timings_jdec"] = timings_jdec_str

    # voltage_xmp: retain as string for matching consistency
    voltage_src = (
        dimm.get("voltage_xmp")
        or dimm.get("XMP Voltage")
        or dimm.get("Voltage XMP")
    )
    if voltage_src is not None:
        if isinstance(voltage_src, (int, float)):
            voltage_norm = str(voltage_src)
        else:
            s = str(voltage_src).strip()
            m = re.match(r"^(\d+(?:\.\d+)?)", s)
            voltage_norm = m.group(1) if m else s if s else None
        if voltage_norm:
            norm["voltage_xmp"] = voltage_norm

    # voltage_jdec: normalized numeric-like value derived from JEDEC / nominal voltage.
    # This is additive and does not affect existing matching semantics.
    jedec_voltage_src = (
        dimm.get("JEDEC_voltage")
        or dimm.get("Module Nominal Voltage")
        or dimm.get("Nominal Voltage")
    )
    if jedec_voltage_src is not None:
        if isinstance(jedec_voltage_src, (int, float)):
            v_norm = float(jedec_voltage_src)
        else:
            s = str(jedec_voltage_src).strip()
            m = re.match(r"^(\d+(?:\.\d+)?)", s)
            v_norm = float(m.group(1)) if m else None
        if v_norm is not None:
            norm["voltage_jdec"] = v_norm

    return norm


def is_match(dimm: Dict[str, Any], entry: Dict[str, Any]) -> bool:
    """
    Determine whether a normalized DIMM record satisfies all constraints in a DB entry.

    Rules:
    - Only recognized constraint keys participate in matching.
    - All specified constraints are combined with logical AND.
    - Unknown keys in entry are ignored for matching.
    - Comparison is deterministic and side-effect-free.
    """
    # generation: exact match
    if "generation" in entry:
        if dimm.get("generation") != entry["generation"]:
            return False

    # manufacturer: substring or exact match (case-insensitive)
    if "manufacturer" in entry:
        expected = str(entry["manufacturer"]).lower()
        actual = str(dimm.get("manufacturer", "")).lower()
        if expected not in actual:
            return False

    # dram_mfg: exact, case-insensitive
    if "dram_mfg" in entry:
        expected = str(entry["dram_mfg"]).lower()
        actual = dimm.get("dram_mfg")
        if not isinstance(actual, str) or actual.lower() != expected:
            return False

    # module_gb: numeric equality (tolerate int/float)
    if "module_gb" in entry:
        expected = entry["module_gb"]
        actual = dimm.get("module_gb")
        if actual is None:
            return False
        try:
            if float(actual) != float(expected):
                return False
        except (TypeError, ValueError):
            return False

    # module_ranks: exact
    if "module_ranks" in entry:
        if dimm.get("module_ranks") != entry["module_ranks"]:
            return False

    # chip_org: exact
    if "chip_org" in entry:
        if dimm.get("chip_org") != entry["chip_org"]:
            return False

    # part_number_contains: case-insensitive substring in module_part_number
    if "part_number_contains" in entry:
        expected = str(entry["part_number_contains"]).lower()
        actual = str(dimm.get("module_part_number", "")).lower()
        if expected not in actual:
            return False

    # part_number_exact: case-insensitive equality
    if "part_number_exact" in entry:
        expected = str(entry["part_number_exact"]).lower()
        actual = dimm.get("module_part_number")
        if not isinstance(actual, str) or actual.lower() != expected:
            return False

    # timings_xmp: exact or substring match vs dimm["timings_xmp"]
    if "timings_xmp" in entry:
        expected = str(entry["timings_xmp"])
        actual = str(dimm.get("timings_xmp", ""))
        if not (actual == expected or expected in actual):
            return False

    # timings_jdec: exact match vs dimm["timings_jdec"]
    if "timings_jdec" in entry:
        expected = str(entry["timings_jdec"])
        actual = dimm.get("timings_jdec")
        if not isinstance(actual, str) or actual != expected:
            return False

    # voltage_xmp: compare as strings
    if "voltage_xmp" in entry:
        expected = str(entry["voltage_xmp"])
        actual = dimm.get("voltage_xmp")
        if actual is None:
            return False
        if str(actual) != expected:
            return False

    # corsair_version:
    if "corsair_version" in entry:
        expected = str(entry["corsair_version"])
        actual = str(dimm.get("corsair_version", ""))
        if expected.endswith("."):
            # prefix semantics: DB value like "3." means versions starting with "3."
            if not actual.startswith(expected):
                return False
        else:
            if actual != expected:
                return False

    # gskill_sticker_code: entry substring must appear (case-insensitive)
    if "gskill_sticker_code" in entry:
        expected = str(entry["gskill_sticker_code"]).lower()
        actual = str(dimm.get("gskill_sticker_code", "")).lower()
        if expected not in actual:
            return False

    # crucial_sticker_suffix: exact, case-insensitive
    if "crucial_sticker_suffix" in entry:
        expected = str(entry["crucial_sticker_suffix"]).lower()
        actual = dimm.get("crucial_sticker_suffix")
        if not isinstance(actual, str) or actual.lower() != expected:
            return False

    # hynix_ic_parse_8th: check 8th char of hynix_ic_part_number (case-insensitive)
    if "hynix_ic_parse_8th" in entry:
        expected = str(entry["hynix_ic_parse_8th"]).upper()
        # Support both normalized key and raw SPD-like key.
        ic = (
            dimm.get("hynix_ic_part_number")
            or dimm.get("Hynix IC Part Number")
            or dimm.get("hynix_ic_pn")
        )
        # If we don't have a usable IC string, this constraint fails.
        if not isinstance(ic, str) or len(ic) < 8:
            return False
        # Compare the 8th character (index 7) of the IC string) in a
        # case-insensitive manner to tolerate database vs SPD casing.
        if ic[7].upper() != expected:
            return False

    return True


def find_die_type(dimm: Dict[str, Any], db: List[Dict[str, Any]]) -> Tuple[str, Optional[str]]:
    """
    Given a normalized DIMM record and a loaded database, determine the die_type.

    Selection algorithm:
    - Iterate entries in priority-sorted order.
    - Collect matches at the highest encountered priority:
      * If an entry does not match, skip it.
      * If first match, set best_priority and start best_matches list.
      * If another match with same priority, append.
      * If entry has lower priority than current best_priority, it cannot
        supersede current matches; continue scanning without changing best.
    - Resolution:
      * If no matches:
          ("Unknown", "No heuristic match found in database.")
      * If exactly one best match:
          (die_type, notes)
      * If multiple matches at same best_priority:
          - If all share same die_type:
                (die_type, combined_notes)
          - Else:
                ("Ambiguous",
                 "Multiple matching heuristics at same priority: <sorted die_types>")
    """
    best_matches: List[Dict[str, Any]] = []
    best_priority: Optional[int] = None

    for entry in db:
        if not is_match(dimm, entry):
            continue

        priority = entry["priority"]

        if best_priority is None or priority > best_priority:
            best_priority = priority
            best_matches = [entry]
        elif priority == best_priority:
            best_matches.append(entry)
        # If priority < best_priority: ignore for selection, but continue loop to
        # preserve determinism / not early-return.

    if not best_matches:
        return "Unknown", "No heuristic match found in database."

    if len(best_matches) == 1:
        entry = best_matches[0]
        return entry["die_type"], entry.get("notes")

    # Multiple matches with same best priority.
    die_types = sorted({e["die_type"] for e in best_matches})
    if len(die_types) == 1:
        # All share the same die_type; combine notes.
        notes_list: List[str] = []
        for e in best_matches:
            note = e.get("notes")
            if isinstance(note, str) and note.strip():
                notes_list.append(note.strip())
        combined_notes = " | ".join(notes_list) if notes_list else None
        return die_types[0], combined_notes

    # Ambiguous across multiple die_types
    msg = "Multiple matching heuristics at same priority: " + ", ".join(die_types)
    return "Ambiguous", msg