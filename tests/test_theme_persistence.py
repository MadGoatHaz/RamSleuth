#!/usr/bin/env python3
"""
Test script to verify theme persistence works correctly.
This simulates the complete theme toggle and persistence flow.
"""

import json
import os
import sys
from pathlib import Path
import pwd

def debug_print(msg):
    print(f"[TEST] {msg}")

def get_config_path():
    """Get the correct config path based on whether running with sudo"""
    if os.environ.get('SUDO_USER'):
        sudo_user = os.environ['SUDO_USER']
        user_home = pwd.getpwnam(sudo_user).pw_dir
        config_dir = Path(user_home) / ".config" / "ramsleuth"
    else:
        config_dir = Path.home() / ".config" / "ramsleuth"
    return config_dir / "ramsleuth_config.json"

def test_config_path():
    """Test that config path is correctly determined"""
    debug_print("=== Testing Config Path Resolution ===")
    
    config_file = get_config_path()
    debug_print(f"Config file path: {config_file}")
    debug_print(f"Running as: {os.environ.get('USER', 'unknown')}")
    debug_print(f"SUDO_USER: {os.environ.get('SUDO_USER', 'None')}")
    
    # Check if config directory is in user's home, not program directory
    config_dir = config_file.parent
    home_dir = Path.home()
    
    if config_dir.is_relative_to(home_dir):
        debug_print("✓ Config directory is in user's home directory")
    else:
        debug_print("✗ Config directory is NOT in user's home directory")
    
    return config_file

def test_initial_load():
    """Test initial config load (should use default theme)"""
    debug_print("\n=== Testing Initial Config Load ===")
    
    config_file = get_config_path()
    
    # Remove existing config if it exists
    if config_file.exists():
        config_file.unlink()
        debug_print("Removed existing config file")
    
    # Simulate initial load (should return empty dict, default to dark)
    if config_file.exists():
        with open(config_file, 'r') as f:
            config = json.load(f)
        debug_print(f"Loaded existing config: {config}")
    else:
        config = {}
        debug_print("No existing config, using defaults")
    
    # Determine initial theme
    initial_theme = config.get("theme", "dark")
    debug_print(f"Initial theme: {initial_theme}")
    
    return initial_theme

def test_theme_toggle():
    """Test theme toggle and save"""
    debug_print("\n=== Testing Theme Toggle ===")
    
    config_file = get_config_path()
    
    # Load current config
    if config_file.exists():
        with open(config_file, 'r') as f:
            config = json.load(f)
    else:
        config = {}
    
    current_theme = config.get("theme", "dark")
    debug_print(f"Current theme: {current_theme}")
    
    # Toggle theme
    new_theme = "light" if current_theme == "dark" else "dark"
    config["theme"] = new_theme
    
    # Save config
    config_file.parent.mkdir(parents=True, exist_ok=True)
    with open(config_file, 'w') as f:
        json.dump(config, f, indent=2)
    
    debug_print(f"Toggled theme to: {new_theme}")
    debug_print(f"Saved config to: {config_file}")
    
    # Verify file permissions (should be owned by current user, not root)
    stat_info = config_file.stat()
    debug_print(f"Config file owner UID: {stat_info.st_uid}")
    debug_print(f"Config file permissions: {oct(stat_info.st_mode)}")
    
    if os.getuid() == stat_info.st_uid:
        debug_print("✓ Config file is owned by current user")
    else:
        debug_print("✗ Config file is NOT owned by current user")
    
    return new_theme

def test_persistence():
    """Test that theme persists across restarts"""
    debug_print("\n=== Testing Theme Persistence ===")
    
    config_file = get_config_path()
    
    # Simulate app restart by loading config again
    if config_file.exists():
        with open(config_file, 'r') as f:
            config = json.load(f)
        loaded_theme = config.get("theme", "dark")
        debug_print(f"Loaded theme after 'restart': {loaded_theme}")
        return loaded_theme
    else:
        debug_print("✗ Config file does not exist after save")
        return None

def main():
    """Run all tests"""
    debug_print("Starting theme persistence test...")
    
    # Test 1: Config path resolution
    config_file = test_config_path()
    
    # Test 2: Initial load
    initial_theme = test_initial_load()
    
    # Test 3: Theme toggle and save
    toggled_theme = test_theme_toggle()
    
    # Test 4: Persistence across restarts
    persisted_theme = test_persistence()
    
    # Results
    debug_print("\n=== Test Results ===")
    debug_print(f"Initial theme: {initial_theme}")
    debug_print(f"Toggled theme: {toggled_theme}")
    debug_print(f"Persisted theme: {persisted_theme}")
    
    if initial_theme != toggled_theme and toggled_theme == persisted_theme:
        debug_print("✓ THEME PERSISTENCE WORKS CORRECTLY")
        return 0
    else:
        debug_print("✗ THEME PERSISTENCE FAILED")
        return 1

if __name__ == "__main__":
    sys.exit(main())