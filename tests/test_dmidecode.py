#!/usr/bin/env python3
import sys
import os
sys.path.insert(0, '/home/madgoat/Documents/RamSleuth')

# Enable debug mode by importing and setting the global DEBUG flag
import ramsleuth
ramsleuth.DEBUG = True

from ramsleuth_pkg.scanner import get_current_memory_settings

print("Testing get_current_memory_settings()...")
settings = get_current_memory_settings()
print(f"Result: {settings}")