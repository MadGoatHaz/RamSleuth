import pytest

from RamSleuth_DB import is_match, normalize_dimm_data, load_database, find_die_type


def base_dimm(**overrides):
    """
    Helper to build a DIMM dict with raw + normalized-style defaults.

    Tests may:
    - Use this as already-normalized input to is_match().
    - Or construct a raw dict and pass it through normalize_dimm_data() explicitly
      when they want to validate end-to-end behavior.
    """
    dimm = {
        # Canonical normalized-style defaults (P1000-ish high-end DDR4 kit)
        "generation": "DDR4",
        "manufacturer": "Corsair",
        "dram_mfg": "SK Hynix",
        "module_gb": 16,
        "module_ranks": "2R",
        "chip_org": "x8",
        "module_part_number": "CMK16GX4M2B3200C16",
        "timings_xmp": "3200-16",
        "timings_jdec": "2133-15-15-15",
        "voltage_xmp": "1.35",
        "corsair_version": "",
        "gskill_sticker_code": "",
        "crucial_sticker_suffix": "",
        "hynix_ic_part_number": "",
    }
    dimm.update(overrides)
    return dimm


# ---------------------------------------------------------------------------
# Basic positive/negative behavior for primitive constraints
# ---------------------------------------------------------------------------


def test_match_generation_exact():
    dimm = base_dimm(generation="DDR4")
    entry = {"generation": "DDR4"}
    assert is_match(dimm, entry) is True


def test_match_generation_mismatch():
    dimm = base_dimm(generation="DDR5")
    entry = {"generation": "DDR4"}
    assert is_match(dimm, entry) is False


def test_match_manufacturer_substring():
    # is_match: manufacturer is substring/contains check (case-insensitive)
    dimm = base_dimm(manufacturer="Corsair Memory Inc.")
    entry = {"manufacturer": "corsair"}
    assert is_match(dimm, entry) is True


def test_match_manufacturer_negative():
    dimm = base_dimm(manufacturer="G.Skill")
    entry = {"manufacturer": "corsair"}
    assert is_match(dimm, entry) is False


def test_match_dram_mfg_exact_case_insensitive():
    dimm = base_dimm(dram_mfg="SK Hynix")
    entry = {"dram_mfg": "sk hynix"}
    assert is_match(dimm, entry) is True

    dimm = base_dimm(dram_mfg="Samsung")
    assert is_match(dimm, entry) is False


def test_match_module_gb_numeric():
    dimm = base_dimm(module_gb=16)
    entry = {"module_gb": 16}
    assert is_match(dimm, entry) is True

    entry = {"module_gb": 32}
    assert is_match(dimm, entry) is False


def test_match_module_ranks_exact():
    dimm = base_dimm(module_ranks="2R")
    entry = {"module_ranks": "2R"}
    assert is_match(dimm, entry) is True

    entry = {"module_ranks": "1R"}
    assert is_match(dimm, entry) is False


def test_match_chip_org_exact():
    dimm = base_dimm(chip_org="x8")
    entry = {"chip_org": "x8"}
    assert is_match(dimm, entry) is True

    entry = {"chip_org": "x16"}
    assert is_match(dimm, entry) is False


def test_match_part_number_contains():
    dimm = base_dimm(module_part_number="CMK16GX4M2B3200C16")
    entry = {"part_number_contains": "3200C16"}
    assert is_match(dimm, entry) is True

    entry = {"part_number_contains": "9999"}
    assert is_match(dimm, entry) is False


def test_match_part_number_exact():
    dimm = base_dimm(module_part_number="F4-3600C16D-16GTZ")
    entry = {"part_number_exact": "f4-3600c16d-16gtz"}  # case-insensitive
    assert is_match(dimm, entry) is True

    entry = {"part_number_exact": "F4-3600C16D-16GTZ-R"}
    assert is_match(dimm, entry) is False


def test_match_timings_xmp_exact_or_substring():
    dimm = base_dimm(timings_xmp="3200-16-18-18")
    # Exact
    entry = {"timings_xmp": "3200-16-18-18"}
    assert is_match(dimm, entry) is True
    # Substring allowed
    entry = {"timings_xmp": "3200-16"}
    assert is_match(dimm, entry) is True
    # Negative
    entry = {"timings_xmp": "3600-16"}
    assert is_match(dimm, entry) is False


def test_match_timings_jdec_exact():
    dimm = base_dimm(timings_jdec="2133-15-15-15")
    entry = {"timings_jdec": "2133-15-15-15"}
    assert is_match(dimm, entry) is True

    entry = {"timings_jdec": "2400-16-16-16"}
    assert is_match(dimm, entry) is False


def test_match_voltage_xmp_string_compare():
    dimm = base_dimm(voltage_xmp="1.35")
    entry = {"voltage_xmp": "1.35"}
    assert is_match(dimm, entry) is True

    entry = {"voltage_xmp": "1.40"}
    assert is_match(dimm, entry) is False


# ---------------------------------------------------------------------------
# Lootbox / sticker style constraints
# ---------------------------------------------------------------------------


def test_corsair_version_prefix_semantics():
    # DB: "3." means any corsair_version starting with "3."
    dimm = base_dimm(corsair_version="3.31")
    entry = {"corsair_version": "3."}
    assert is_match(dimm, entry) is True

    dimm = base_dimm(corsair_version="4.20")
    assert is_match(dimm, entry) is False

    # DB: exact when no trailing dot
    dimm = base_dimm(corsair_version="4.32")
    entry = {"corsair_version": "4.32"}
    assert is_match(dimm, entry) is True

    dimm = base_dimm(corsair_version="4.321")
    assert is_match(dimm, entry) is False


def test_gskill_sticker_code_substring():
    dimm = base_dimm(gskill_sticker_code="ABC21A")
    entry = {"gskill_sticker_code": "21A"}
    assert is_match(dimm, entry) is True

    entry = {"gskill_sticker_code": "ZZZ"}
    assert is_match(dimm, entry) is False


def test_crucial_sticker_suffix_case_insensitive_exact():
    dimm = base_dimm(crucial_sticker_suffix=".M8FE1")
    entry = {"crucial_sticker_suffix": ".m8fe1"}
    assert is_match(dimm, entry) is True

    entry = {"crucial_sticker_suffix": ".M8FE2"}
    assert is_match(dimm, entry) is False


def test_hynix_ic_parse_8th_character():
    # Indexing: 0-based; 7th index is 8th character
    dimm = base_dimm(hynix_ic_part_number="H5CG48AGBDX018")
    entry = {"hynix_ic_parse_8th": "G"}  # 8th char of "H5CG48AGBDX018" is "G"
    assert is_match(dimm, entry) is True

    entry = {"hynix_ic_parse_8th": "B"}
    assert is_match(dimm, entry) is False

    # Too short or missing -> must not match
    dimm = base_dimm(hynix_ic_part_number="H5CG48A")
    entry = {"hynix_ic_parse_8th": "A"}
    assert is_match(dimm, entry) is False

    dimm = base_dimm(hynix_ic_part_number=None)
    assert is_match(dimm, entry) is False


# ---------------------------------------------------------------------------
# Edge conditions and determinism
# ---------------------------------------------------------------------------


def test_unknown_keys_in_entry_are_ignored():
    dimm = base_dimm()
    entry = {"generation": "DDR4", "unknown_constraint": "foo"}
    # unknown_constraint should not affect result
    assert is_match(dimm, entry) is True


def test_all_constraints_and_tight_match():
    """
    Synthetic "P1000-style" rule:
    - Tight constraints on high-end DDR4 Corsair/Hynix kit
    - Includes lootbox-style hynix_ic_parse_8th
    """
    raw = {
        "generation": "DDR4 SDRAM",
        "Module Manufacturer": "Corsair",
        "DRAM Manufacturer": "SK Hynix",
        "Module Capacity": "16 GB",
        "Ranks": "2R",
        "SDRAM Device Width": "8 bits",
        "Part Number": "CMK16GX4M2B3200C16",
        "timings_xmp": "3200-16-18-18",
        "JEDEC Timings": "2133-15-15-15",
        "XMP Voltage": "1.35 V",
        "hynix_ic_part_number": "H5CG48AGBDX018",
        "corsair_version": "4.32",
    }
    dimm = normalize_dimm_data(raw)
    entry = {
        "generation": "DDR4",
        "manufacturer": "Corsair",
        "dram_mfg": "SK Hynix",
        "module_gb": 16,
        "module_ranks": "2R",
        "chip_org": "x8",
        "part_number_exact": "CMK16GX4M2B3200C16",
        "timings_xmp": "3200-16",
        "timings_jdec": "2133-15-15-15",
        "voltage_xmp": "1.35",
        "corsair_version": "4.32",
        "hynix_ic_parse_8th": "G",  # 8th char of "H5CG48AGBDX018" is "G"
    }
    assert is_match(dimm, entry) is True


def test_f4_3600c18_32gvk_priority_500_rule_matches_correct_die_type():
    """
    Validate the production heuristic for:
      G.Skill F4-3600C18-32GVK, SK Hynix DRAM, 16GB 1Rx8 DDR4

    Expectations (per die_database.json):
    - generation: DDR4
    - module_part_number contains "F4-3600C18-32GVK"
    - dram_mfg: SK Hynix
    - module_gb: 16
    - timings_xmp: "3600-18-22-22"
    - Rule priority: 500
    - die_type: "SK Hynix 16Gbit MJR (M-Die)"
    """
    # Load the real heuristic database using the production helper
    db_entries = load_database("die_database.json")

    dimm = {
        "generation": "DDR4",
        "manufacturer": "G.Skill",
        "dram_mfg": "SK Hynix",
        "module_gb": 16,
        "module_ranks": "1R",
        "chip_org": "x8",
        "module_part_number": "F4-3600C18-32GVK",
        # Optional descriptive fields; should not conflict
        "timings_xmp": "3600-18-22-22",
        "timings_jdec": "2133-15-15-15",
        "voltage_xmp": "1.35",
    }

    die_type, meta = find_die_type(dimm, db_entries)

    assert die_type == "SK Hynix 16Gbit MJR (M-Die)"
    # Verify we matched the P500 rule with correct characteristics

def test_ddr4_samsung_b_die_detection():
    """
    Test Case 1: DDR4 Samsung B-Die
    Should match: priority: 400, timings_xmp: "3200-14-14-14", dram_mfg: "Samsung"
    """
    db_entries = load_database("die_database.json")
    
    # Create test DIMM object with Samsung B-Die characteristics
    dimm = {
        "generation": "DDR4",
        "dram_mfg": "Samsung",
        "timings_xmp": "3200-14-14-14",
        "module_ranks": "1R",
        "module_gb": 8,
    }
    
    die_type, notes = find_die_type(dimm, db_entries)
    
    # Should not return "Unknown"
    assert die_type != "Unknown", f"Expected Samsung B-Die detection, got Unknown. Notes: {notes}"
    
    # Should match Samsung 8Gbit B-Die rule
    assert "Samsung" in die_type
    assert "B-Die" in die_type
    print(f"✓ DDR4 Samsung B-Die detected: {die_type}")


def test_ddr5_hynix_a_die_detection():
    """
    Test Case 2: DDR5 Hynix A-Die
    Needs hynix_ic_part_number (e.g., "H5CG48AGBD")
    Should match: priority: 1000, hynix_ic_parse_8th: "A"
    """
    db_entries = load_database("die_database.json")
    
    # Create test DIMM object with Hynix A-Die characteristics
    dimm = {
        "generation": "DDR5",
        "dram_mfg": "SK Hynix",
        "hynix_ic_part_number": "H5CG48AAGBDX018",  # 8th char is 'A' (index 7)
    }
    
    die_type, notes = find_die_type(dimm, db_entries)
    
    # Should not return "Unknown"
    assert die_type != "Unknown", f"Expected Hynix A-Die detection, got Unknown. Notes: {notes}"
    
    # Should match Hynix A-Die rule
    assert "SK Hynix" in die_type
    assert "A-Die" in die_type
    print(f"✓ DDR5 Hynix A-Die detected: {die_type}")


def test_corsair_ddr4_module_detection():
    """
    Test Case 3: Corsair DDR4 module
    Needs corsair_version (e.g., "4.31")
    Should match: priority: 1000, corsair_version: "4.31"
    """
    db_entries = load_database("die_database.json")
    
    # Create test DIMM object with Corsair characteristics
    dimm = {
        "generation": "DDR4",
        "manufacturer": "Corsair",
        "corsair_version": "4.31",  # Should match Samsung B-Die
    }
    
    die_type, notes = find_die_type(dimm, db_entries)
    
    # Should not return "Unknown"
    assert die_type != "Unknown", f"Expected Corsair detection, got Unknown. Notes: {notes}"
    
    # Should match Corsair rule (Samsung B-Die for v4.31)
    assert "Samsung" in die_type
    assert "B-Die" in die_type
    print(f"✓ Corsair DDR4 module detected: {die_type} (version 4.31)")
    # Verify we correctly identified Samsung B-Die
    assert "Samsung" in die_type and "B-Die" in die_type