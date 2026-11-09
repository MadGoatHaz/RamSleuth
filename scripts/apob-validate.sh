#!/bin/bash
# APOB Timing Validation Script for RamSleuth
# Run this on your TRX40 system after installing the dev build
# Save output and send to MadGoat for analysis

set -e

echo "=== RamSleuth APOB Validation ==="
echo "Date: $(date)"
echo "System: $(uname -a)"
echo ""

echo "--- Step 1: Check kernel module ---"
if lsmod | grep -q ryzen_smu; then
    echo "ryzen_smu module: LOADED"
    modinfo ryzen_smu 2>/dev/null | head -5
else
    echo "ryzen_smu module: NOT LOADED"
    echo "Please load it: sudo modprobe ryzen_smu"
    echo "If not installed, see build instructions."
fi
echo ""

echo "--- Step 2: Check APOB availability ---"
if [ -f /sys/kernel/ryzen_smu_drv/apob_available ]; then
    echo "APOB available: $(cat /sys/kernel/ryzen_smu_drv/apob_available)"
    if [ -f /sys/kernel/ryzen_smu_drv/apob_size ]; then
        echo "APOB size: $(cat /sys/kernel/ryzen_smu_drv/apob_size) bytes"
    fi
    if [ -f /sys/kernel/ryzen_smu_drv/apob_version ]; then
        echo "APOB version: $(cat /sys/kernel/ryzen_smu_drv/apob_version)"
    fi
else
    echo "APOB sysfs interface: NOT FOUND"
    echo "Kernel module may need updating."
fi
echo ""

echo "--- Step 3: Run ramsleuth-telemetry ---"
if command -v ramsleuth-telemetry &>/dev/null; then
    echo "Version: $(ramsleuth-telemetry --version)"
    echo ""
    echo "Full telemetry output:"
    ramsleuth-telemetry 2>&1 || echo "(ramsleuth-telemetry failed or daemon not running)"
    echo ""
    echo "JSON output:"
    ramsleuth-telemetry --json 2>&1 || echo "(JSON output failed)"
else
    echo "ramsleuth-telemetry: NOT FOUND"
fi
echo ""

echo "--- Step 4: Run probe report ---"
if command -v ramsleuth-client &>/dev/null; then
    echo "Probe report (save this!):"
    ramsleuth-client probe --stdout 2>&1 || echo "(probe failed)"
else
    echo "ramsleuth-client: NOT FOUND"
fi
echo ""

echo "--- Step 5: Raw APOB data (hexdump of first 256 bytes) ---"
if [ -f /sys/kernel/ryzen_smu_drv/apob ] && [ "$(cat /sys/kernel/ryzen_smu_drv/apob_available 2>/dev/null)" = "1" ]; then
    head -c 256 /sys/kernel/ryzen_smu_drv/apob | xxd
else
    echo "APOB data not available"
fi
echo ""

echo "=== Validation complete ==="
echo "Please send this entire output (and the probe report) to MadGoat."
