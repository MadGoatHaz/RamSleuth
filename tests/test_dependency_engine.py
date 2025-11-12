#!/usr/bin/env python3
"""
Simple test script to verify dependency_engine.py functionality.
"""

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from dependency_engine import (
    detect_distribution,
    check_tool_available,
    check_python_package,
    get_missing_dependencies,
    construct_install_command,
    handle_unknown_distro
)

def test_detect_distribution():
    """Test distribution detection."""
    print("Testing distribution detection...")
    try:
        distro_info = detect_distribution()
        print(f"✓ Distribution detected: {distro_info}")
        return True
    except Exception as e:
        print(f"✗ Distribution detection failed: {e}")
        return False

def test_check_tool():
    """Test tool availability checking."""
    print("\nTesting tool availability...")
    try:
        # Test with a tool that should exist
        result = check_tool_available("python3")
        print(f"✓ python3 availability: {result}")
        
        # Test with a tool that likely doesn't exist
        result = check_tool_available("nonexistent_tool_xyz")
        print(f"✓ nonexistent_tool_xyz availability: {result}")
        return True
    except Exception as e:
        print(f"✗ Tool checking failed: {e}")
        return False

def test_check_python_package():
    """Test Python package checking."""
    print("\nTesting Python package checking...")
    try:
        # Test with a package that should exist
        result = check_python_package("sys")
        print(f"✓ sys package availability: {result}")
        
        # Test with a package that likely doesn't exist
        result = check_python_package("nonexistent_package_xyz")
        print(f"✓ nonexistent_package_xyz availability: {result}")
        return True
    except Exception as e:
        print(f"✗ Package checking failed: {e}")
        return False

def test_get_missing_dependencies():
    """Test missing dependency detection."""
    print("\nTesting missing dependency detection...")
    try:
        requested_features = {"core": True, "tui": True}
        missing = get_missing_dependencies(requested_features)
        print(f"✓ Missing dependencies: {missing}")
        return True
    except Exception as e:
        print(f"✗ Missing dependency detection failed: {e}")
        return False

def test_construct_install_command():
    """Test install command construction."""
    print("\nTesting install command construction...")
    try:
        # Test with some sample packages
        packages = ["i2c-tools", "dmidecode"]
        distro_info = detect_distribution()
        cmd = construct_install_command(packages, distro_info)
        print(f"✓ Install command: {cmd}")
        return True
    except Exception as e:
        print(f"✗ Install command construction failed: {e}")
        return False

def test_handle_unknown_distro():
    """Test unknown distribution handling."""
    print("\nTesting unknown distribution handling...")
    try:
        handle_unknown_distro("unknown_distro")
        print("✗ Should have exited with error")
        return False
    except SystemExit as e:
        print(f"✓ Correctly exited with code {e.code}")
        return True
    except Exception as e:
        print(f"✗ Unexpected error: {e}")
        return False

def main():
    """Run all tests."""
    print("Testing dependency_engine.py functionality...\n")
    
    tests = [
        test_detect_distribution,
        test_check_tool,
        test_check_python_package,
        test_get_missing_dependencies,
        test_construct_install_command,
        test_handle_unknown_distro
    ]
    
    passed = 0
    total = len(tests)
    
    for test in tests:
        try:
            if test():
                passed += 1
        except Exception as e:
            print(f"✗ Test failed with exception: {e}")
    
    print(f"\n{passed}/{total} tests passed")
    
    if passed == total:
        print("✓ All tests passed!")
        sys.exit(0)
    else:
        print("✗ Some tests failed")
        sys.exit(1)

if __name__ == "__main__":
    main()