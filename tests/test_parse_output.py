import json
from pathlib import Path

import pytest

from ramsleuth import parse_output


FIXTURE_PATH = Path(__file__).resolve().parent.parent / "test_data.txt"


def load_fixture() -> str:
    # Read the side-by-side style test data as raw text
    return FIXTURE_PATH.read_text(encoding="utf-8")


@pytest.fixture(scope="module")
def parsed_dimms():
    raw = load_fixture()
    dimms = parse_output(raw)
    return dimms


def test_dimm_count(parsed_dimms):
    # Expect 4 DIMM dicts for the structured 4-column input
    # plus 2 DIMMs fanned out from the aggregate bank 3/bank 4 block.
    assert isinstance(parsed_dimms, list)
    assert len(parsed_dimms) == 6


def test_basic_fields(parsed_dimms):
    d0, d1, d2, d3, a_bank3, a_bank4 = parsed_dimms

    # DIMM 0: Corsair DDR3 kit
    assert d0.get("generation") == "DDR3 SDRAM"
    assert "Corsair" in d0.get("manufacturer", "")
    assert d0.get("module_part_number") == "CMZ8GX3M2A1600C9"
    assert d0.get("dram_mfg") == "SK Hynix"

    # DIMM 1: G.Skill DDR4 kit
    assert d1.get("generation") == "DDR4 SDRAM"
    assert "G.Skill" in d1.get("manufacturer", "")
    assert d1.get("module_part_number") == "F4-3600C16D-16GTZ"
    assert d1.get("dram_mfg") == "Samsung"

    # DIMM 2: Crucial DDR4 Lootbox candidate (BL2K16G36C16U4B)
    assert d2.get("generation") == "DDR5 SDRAM" or d2.get("generation") == "DDR4 SDRAM"
    assert "Crucial" in d2.get("manufacturer", "")
    assert d2.get("module_part_number") == "BL2K16G36C16U4B"
    assert d2.get("dram_mfg") == "Micron"

    # DIMM 3: DDR5 SK Hynix-based kit with PMIC + Hynix IC PN
    assert d3.get("generation") == "DDR4 SDRAM" or d3.get("generation") == "DDR5 SDRAM"
    assert d3.get("module_part_number") == "DDR5-6000-SKH-PMIC"
    assert d3.get("dram_mfg") == "SK Hynix"

    # Aggregate-slot fan-out DIMMs from the plain-text decode-dimms block:
    # Both should share attributes from the block and represent individual banks.
    for dimm in (a_bank3, a_bank4):
        assert dimm.get("manufacturer") == "G.Skill"
        assert dimm.get("module_part_number") == "F4-3600C18-32GVK"
        assert dimm.get("dram_mfg") == "SK Hynix"
        # Raw size/ranks consistent with a 16GB 1R module as provided in the snippet
        assert dimm.get("Module Capacity") == "16 GB"
        assert dimm.get("Ranks") == "1R"


def test_capacity_ranks_width(parsed_dimms):
    d0, d1, d2, d3, a_bank3, a_bank4 = parsed_dimms

    # Raw values should be preserved as parsed
    assert d0.get("module_gb") == "8192 MB"
    assert d1.get("module_gb") == "8192 MB"
    assert d2.get("module_gb") == "16384 MB"
    assert d3.get("module_gb") == "16384 MB"

    # Aggregate fan-out DIMMs may be normalized differently; ensure SDRAM width/ranks preserved
    for dimm in (a_bank3, a_bank4):
        assert dimm.get("SDRAM Device Width") == "8 bits"
        assert dimm.get("Ranks") == "1R"

    assert d0.get("module_ranks") == "1R"
    assert d1.get("module_ranks") == "1R"
    assert d2.get("module_ranks") == "2R"
    assert d3.get("module_ranks") == "1R"

    # SDRAM Device Width should be preserved for normalize_dimm_data() -> chip_org
    assert d0.get("SDRAM Device Width") == "8 bits"
    assert d1.get("SDRAM Device Width") == "8 bits"
    assert d2.get("SDRAM Device Width") == "8 bits"
    assert d3.get("SDRAM Device Width") == "16 bits"


def test_jedec_voltage_mapping(parsed_dimms):
    d0, d1, d2, d3, a_bank3, a_bank4 = parsed_dimms

    # Module Nominal Voltage should be mapped into JEDEC_voltage field
    assert d0.get("JEDEC_voltage") == "1.50 V"
    assert d1.get("JEDEC_voltage") == "1.20 V"
    assert d2.get("JEDEC_voltage") == "1.35 V"
    assert d3.get("JEDEC_voltage") == "1.25 V"

    # Aggregate fan-out DIMMs should preserve JEDEC timings from the block
    for dimm in (a_bank3, a_bank4):
        assert dimm.get("JEDEC Timings") == "DDR4-2133 15-15-15"


def test_slot_derivation_from_guessing_dimm_is_in(parsed_dimms):
    d0, d1, d2, d3, a_bank3, a_bank4 = parsed_dimms

    # Treat "Guessing DIMM is in" as required behavior:
    # must be preserved AND normalized to "slot"
    assert d0.get("Guessing DIMM is in") == "DIMM_A1"
    assert d1.get("Guessing DIMM is in") == "DIMM_B1"
    assert d2.get("Guessing DIMM is in") == "DIMM_A2"
    assert d3.get("Guessing DIMM is in") == "DIMM_B2"

    assert d0.get("slot") == "DIMM_A1"
    assert d1.get("slot") == "DIMM_B1"
    assert d2.get("slot") == "DIMM_A2"
    assert d3.get("slot") == "DIMM_B2"

    # Aggregate-slot plain-text decode-dimms block:
    # Must fan out "Guessing DIMM is in  bank 3 bank 4" into two distinct DIMMs.
    slots = {str(d.get("slot", "")).lower() for d in (a_bank3, a_bank4)}

    # No DIMM may keep the combined aggregate slot string.
    for dimm in (a_bank3, a_bank4):
        slot_val = str(dimm.get("slot", "")).lower()
        assert "bank 3 bank 4" not in slot_val

    # We expect exactly one DIMM resolved to bank 3 and one to bank 4.
    assert any(s == "bank 3" or s.replace(" ", "") == "bank3" for s in slots)
    assert any(s == "bank 4" or s.replace(" ", "") == "bank4" for s in slots)
    assert len(slots) == 2


def test_preserve_jedec_and_ddr5_specific_fields(parsed_dimms):
    d0, d1, d2, d3, a_bank3, a_bank4 = parsed_dimms

    # JEDEC timing row should be preserved literally
    assert d0.get("JEDEC Timings") == "DDR3-1333 9-9-9"
    assert d1.get("JEDEC Timings") == "DDR4-2133 15-15-15"
    assert d2.get("JEDEC Timings") == "DDR4-2133 15-15-15"
    assert d3.get("JEDEC Timings") == "DDR5-4800 40-40-40"

    # Additional malformed timings row should not break parsing; ensure it is preserved
    for dimm in parsed_dimms:
        assert "Additional JEDEC Timings malformed" in dimm

    # DDR5-related PMIC / Hynix IC PN should be captured on DIMM 3
    assert parsed_dimms[3].get("PMIC Manufacturer") == "Renesas"
    assert parsed_dimms[3].get("Hynix IC Part Number") == "H5CG48AGBDX018"


def test_malformed_and_extra_lines_are_tolerated(parsed_dimms):
    # Ensure parser ignored non-conforming / garbage lines without raising,
    # and did not create extra DIMM entries beyond the expected:
    # 4 side-by-side DIMMs + 2 fan-out DIMMs from the aggregate block.
    assert len(parsed_dimms) == 6

    # No DIMM should have taken pure garbage lines as keys
    for dimm in parsed_dimms:
        for bad in (
            "Random garbage line not using pipes at all",
            "Noise",
        ):
            assert bad not in dimm

    # Line with too many columns should be safely truncated to existing DIMM count
    for dimm in parsed_dimms:
        assert "Malformed Line With Too Many Columns" in dimm


def test_deterministic_results(parsed_dimms):
    # Repeat parse and compare to ensure deterministic behavior
    raw = load_fixture()
    dimms2 = parse_output(raw)

    # Compare as JSON-serializable structure; order of keys in dict is irrelevant
    assert json.dumps(parsed_dimms, sort_keys=True) == json.dumps(dimms2, sort_keys=True)


def test_no_multi_slot_aggregate_when_per_slot_exists():
    """
    Ensure that when decode-dimms-style content contains both:
    - a combined "bank 3           bank 4" style label, and
    - explicit per-slot DIMM entries for bank 3 and bank 4

    the parser does NOT emit a merged aggregate entry whose slot contains both.
    """
    synthetic = """
Decoding EEPROM 5-0052 5-0053
Guessing DIMM is in  bank 3           bank 4
Module Manufacturer  ExampleCorp
Part Number          EX1234
Fundamental memory type  DDR4 SDRAM

Decoding EEPROM 5-0052
Guessing DIMM is in  bank 3
Module Manufacturer  ExampleCorp
Part Number          EX1234
Fundamental memory type  DDR4 SDRAM

Decoding EEPROM 5-0053
Guessing DIMM is in  bank 4
Module Manufacturer  ExampleCorp
Part Number          EX1234
Fundamental memory type  DDR4 SDRAM
"""
    dimms = parse_output(synthetic)
    slots = {str(d.get("slot", "")).lower() for d in dimms}

    # No slot string may mention both bank 3 and bank 4 simultaneously.
    for s in slots:
        assert not ("bank 3" in s and "bank 4" in s)

    # We must have distinct entries for bank 3 and bank 4 (allowing minor variants).
    assert any("bank3" == s.replace(" ", "") or s == "bank 3" for s in slots)
    assert any("bank4" == s.replace(" ", "") or s == "bank 4" for s in slots)