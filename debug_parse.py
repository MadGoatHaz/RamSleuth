#!/usr/bin/env python3
"""
Debug script to trace parse_output behavior
"""

import sys
from pathlib import Path

# Add current directory to path
sys.path.insert(0, str(Path(__file__).parent))

from ramsleuth import parse_output

# Load test data
test_data_path = Path(__file__).parent / "test_data.txt"
with open(test_data_path, "r", encoding="utf-8") as f:
    raw_output = f.read()

print("=== RAW TEST DATA ===")
print(raw_output)
print("\n=== END RAW TEST DATA ===\n")

# Parse the output
parsed_dimms = parse_output(raw_output)

print(f"Total DIMMs parsed: {len(parsed_dimms)}")
print("\n=== PARSED DIMMS ===")
for i, dimm in enumerate(parsed_dimms):
    print(f"\nDIMM {i}:")
    print(f"  slot: {dimm.get('slot', 'N/A')}")
    print(f"  Guessing DIMM is in: {dimm.get('Guessing DIMM is in', 'N/A')}")
    print(f"  generation: {dimm.get('generation', 'N/A')}")
    print(f"  manufacturer: {dimm.get('manufacturer', 'N/A')}")
    print(f"  module_part_number: {dimm.get('module_part_number', 'N/A')}")
    print(f"  module_gb: {dimm.get('module_gb', 'N/A')}")
    print(f"  JEDEC_voltage: {dimm.get('JEDEC_voltage', 'N/A')}")
    print(f"  JEDEC Timings: {dimm.get('JEDEC Timings', 'N/A')}")
    
    # Show all keys for first few DIMMs
    if i < 2:
        print(f"  ALL KEYS: {list(dimm.keys())}")