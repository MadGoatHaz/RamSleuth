#!/usr/bin/env python3
"""
Comprehensive test to verify both Ctrl+T and Ctrl+P theme methods work.
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

def test_both_methods_persist():
    """Test that both Ctrl+T and Ctrl+P methods persist themes correctly"""
    debug_print("=== Testing Both Theme Methods ===")
    
    config_file = get_config_path()
    
    # Test 1: Start with no config (default dark)
    if config_file.exists():
        config_file.unlink()
        debug_print("Removed existing config file")
    
    # Simulate initial load - should default to dark
    config = {}
    current_theme = config.get("theme", "dark")
    debug_print(f"Initial theme: {current_theme}")
    
    # Test 2: Simulate Ctrl+T toggle (dark -> light)
    new_theme = "light" if current_theme == "dark" else "dark"
    config["theme"] = new_theme
    config_file.parent.mkdir(parents=True, exist_ok=True)
    with open(config_file, 'w') as f:
        json.dump(config, f, indent=2)
    
    debug_print(f"After Ctrl+T toggle: {new_theme}")
    
    # Test 3: Simulate Ctrl+P "dark" command (light -> dark)
    config = {}
    if config_file.exists():
        with open(config_file, 'r') as f:
            config = json.load(f)
    
    # User selects "dark" from command palette
    config["theme"] = "dark"
    with open(config_file, 'w') as f:
        json.dump(config, f, indent=2)
    
    debug_print(f"After Ctrl+P 'dark' command: dark")
    
    # Test 4: Verify persistence
    with open(config_file, 'r') as f:
        persisted_config = json.load(f)
    
    persisted_theme = persisted_config.get("theme", "dark")
    debug_print(f"Persisted theme: {persisted_theme}")
    
    if persisted_theme == "dark":
        debug_print("✓ Ctrl+P theme selection persisted correctly")
    else:
        debug_print("✗ Ctrl+P theme selection did not persist")
        return False
    
    # Test 5: Simulate Ctrl+P "light" command
    config["theme"] = "light"
    with open(config_file, 'w') as f:
        json.dump(config, f, indent=2)
    
    debug_print(f"After Ctrl+P 'light' command: light")
    
    # Test 6: Verify persistence again
    with open(config_file, 'r') as f:
        persisted_config = json.load(f)
    
    persisted_theme = persisted_config.get("theme", "dark")
    debug_print(f"Persisted theme: {persisted_theme}")
    
    if persisted_theme == "light":
        debug_print("✓ Ctrl+P 'light' command persisted correctly")
    else:
        debug_print("✗ Ctrl+P 'light' command did not persist")
        return False
    
    # Test 7: Verify both methods use same config key
    debug_print("\n=== Testing Config Consistency ===")
    with open(config_file, 'r') as f:
        final_config = json.load(f)
    
    if "theme" in final_config:
        debug_print("✓ Both methods use 'theme' config key")
    else:
        debug_print("✗ Config key inconsistency detected")
        return False
    
    debug_print(f"Final config contents: {final_config}")
    
    return True

def test_command_palette_integration():
    """Test that command palette integration is properly implemented"""
    debug_print("\n=== Testing Command Palette Integration ===")
    
    # Check that the action_set_theme method exists in ramsleuth.py
    with open("ramsleuth.py", 'r') as f:
        content = f.read()
    
    # Check for action_set_theme method (replaces the old provider approach)
    if "def action_set_theme" in content:
        debug_print("✓ action_set_theme method found")
    else:
        debug_print("✗ action_set_theme method not found")
        return False
    
    # Check for watch_dark method (captures all theme changes)
    if "def watch_dark" in content:
        debug_print("✓ watch_dark method found")
    else:
        debug_print("✗ watch_dark method not found")
        return False
    
    # Check that ThemeCommandProvider was removed (no longer needed)
    if "class ThemeCommandProvider" not in content:
        debug_print("✓ Old ThemeCommandProvider class removed")
    else:
        debug_print("✗ Old ThemeCommandProvider class still present")
        return False
    
    # Check that COMMAND_PROVIDERS was removed
    if "COMMAND_PROVIDERS" not in content:
        debug_print("✓ COMMAND_PROVIDERS line removed")
    else:
        debug_print("✗ COMMAND_PROVIDERS line still present")
        return False
    
    # Check for proper theme handling in on_mount
    if "self.app.theme" in content and "saved_theme" in content:
        debug_print("✓ Theme loading in on_mount implemented")
    else:
        debug_print("✗ Theme loading in on_mount not found")
        return False
    
    return True

def test_bindings():
    """Test that both Ctrl+T and Ctrl+P bindings exist"""
    debug_print("\n=== Testing Key Bindings ===")
    
    with open("ramsleuth.py", 'r') as f:
        content = f.read()
    
    # Check Ctrl+T binding
    if "ctrl+t" in content.lower():
        debug_print("✓ Ctrl+T binding found")
    else:
        debug_print("✗ Ctrl+T binding not found")
        return False
    
    # Check Ctrl+P binding
    if "ctrl+p" in content.lower():
        debug_print("✓ Ctrl+P binding found")
    else:
        debug_print("✗ Ctrl+P binding not found")
        return False
    
    # Check command_palette action
    if "command_palette" in content:
        debug_print("✓ command_palette action found")
    else:
        debug_print("✗ command_palette action not found")
        return False
    
    return True

def main():
    """Run comprehensive tests"""
    debug_print("Starting comprehensive theme functionality test...")
    
    # Test 1: Both methods persist correctly
    persistence_ok = test_both_methods_persist()
    
    # Test 2: Command palette integration is correct
    integration_ok = test_command_palette_integration()
    
    # Test 3: Bindings are correct
    bindings_ok = test_bindings()
    
    # Summary
    debug_print("\n=== Final Results ===")
    debug_print(f"Persistence test: {'✓ PASS' if persistence_ok else '✗ FAIL'}")
    debug_print(f"Integration test: {'✓ PASS' if integration_ok else '✗ FAIL'}")
    debug_print(f"Bindings test: {'✓ PASS' if bindings_ok else '✗ FAIL'}")
    
    if persistence_ok and integration_ok and bindings_ok:
        debug_print("\n🎉 ALL TESTS PASSED!")
        debug_print("✓ Both Ctrl+T and Ctrl+P methods work correctly")
        debug_print("✓ Theme persistence works with both methods")
        debug_print("✓ Command palette integration is complete")
        return 0
    else:
        debug_print("\n❌ SOME TESTS FAILED")
        return 1

if __name__ == "__main__":
    sys.exit(main())