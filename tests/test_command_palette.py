#!/usr/bin/env python3
"""
Test script to verify command palette functionality exists and works.
This checks if Textual's CommandPalette is implemented for theme selection.
"""

import sys
import os
from pathlib import Path

def debug_print(msg):
    print(f"[TEST] {msg}")

def test_command_palette_exists():
    """Test if command palette functionality exists in ramsleuth.py"""
    debug_print("=== Testing Command Palette Existence ===")
    
    ramsleuth_path = Path("ramsleuth.py")
    if not ramsleuth_path.exists():
        debug_print("✗ ramsleuth.py not found")
        return False
    
    with open(ramsleuth_path, 'r') as f:
        content = f.read()
    
    # Check for CommandPalette imports
    command_palette_imports = [
        "from textual.command import CommandPalette",
        "from textual.screen import ModalScreen",
        "CommandPalette",
        "command_palette"
    ]
    
    found_imports = []
    for import_stmt in command_palette_imports:
        if import_stmt in content:
            found_imports.append(import_stmt)
    
    if found_imports:
        debug_print(f"✓ Found command palette imports: {found_imports}")
    else:
        debug_print("✗ No command palette imports found")
    
    # Check for command providers
    provider_patterns = [
        "class.*Provider",
        "def get_commands",
        "def discover_commands",
        "command_providers"
    ]
    
    found_providers = []
    for pattern in provider_patterns:
        if pattern in content:
            found_providers.append(pattern)
    
    if found_providers:
        debug_print(f"✓ Found command provider patterns: {found_providers}")
    else:
        debug_print("✗ No command provider patterns found")
    
    # Check for Ctrl+P binding
    if 'ctrl+p' in content.lower() or 'ctrl+p' in content:
        debug_print("✓ Found Ctrl+P binding")
    else:
        debug_print("✗ No Ctrl+P binding found")
    
    # Check for theme commands
    theme_commands = [
        "theme",
        "dark",
        "light",
        "toggle.*theme"
    ]
    
    found_theme_commands = []
    for cmd in theme_commands:
        if cmd in content.lower():
            found_theme_commands.append(cmd)
    
    if found_theme_commands:
        debug_print(f"✓ Found theme-related commands: {found_theme_commands}")
    else:
        debug_print("✗ No theme-related commands found")
    
    return len(found_imports) > 0

def test_current_theme_functionality():
    """Test what theme functionality currently exists"""
    debug_print("\n=== Testing Current Theme Functionality ===")
    
    ramsleuth_path = Path("ramsleuth.py")
    with open(ramsleuth_path, 'r') as f:
        content = f.read()
    
    # Check for Ctrl+T toggle
    if 'ctrl+t' in content.lower():
        debug_print("✓ Found Ctrl+T theme toggle binding")
    else:
        debug_print("✗ No Ctrl+T binding found")
    
    # Check for action_toggle_dark method
    if 'def action_toggle_dark' in content:
        debug_print("✓ Found action_toggle_dark method")
    else:
        debug_print("✗ No action_toggle_dark method found")
    
    # Check for theme persistence
    if 'save_config' in content and 'theme' in content:
        debug_print("✓ Found theme persistence logic")
    else:
        debug_print("✗ No theme persistence logic found")
    
    # Check for initial theme loading
    if 'load_config' in content and 'initial_theme' in content:
        debug_print("✓ Found initial theme loading")
    else:
        debug_print("✗ No initial theme loading found")

def test_textual_version():
    """Check if Textual version supports CommandPalette"""
    debug_print("\n=== Testing Textual Version ===")
    
    try:
        import textual
        version = textual.__version__
        debug_print(f"Textual version: {version}")
        
        # CommandPalette was introduced in Textual 0.20.0
        major, minor = map(int, version.split('.')[:2])
        if major > 0 or (major == 0 and minor >= 20):
            debug_print("✓ Textual version supports CommandPalette")
            return True
        else:
            debug_print("✗ Textual version does not support CommandPalette")
            return False
    except ImportError:
        debug_print("✗ Textual not installed")
        return False
    except Exception as e:
        debug_print(f"✗ Error checking Textual version: {e}")
        return False

def main():
    """Run all command palette tests"""
    debug_print("Starting command palette functionality test...")
    
    # Test 1: Check Textual version
    textual_ok = test_textual_version()
    
    # Test 2: Check if command palette exists in code
    palette_exists = test_command_palette_exists()
    
    # Test 3: Check current theme functionality
    test_current_theme_functionality()
    
    # Summary
    debug_print("\n=== Test Results ===")
    debug_print(f"Textual version OK: {textual_ok}")
    debug_print(f"Command palette exists: {palette_exists}")
    
    if not palette_exists:
        debug_print("\n=== ANALYSIS ===")
        debug_print("The current implementation does NOT have command palette support.")
        debug_print("Only Ctrl+T toggle is implemented for theme switching.")
        debug_print("To meet the original requirements, you need to:")
        debug_print("1. Add CommandPalette import from textual.command")
        debug_print("2. Create a ThemeCommandProvider class")
        debug_print("3. Register the provider with the app")
        debug_print("4. Add Ctrl+P binding to open command palette")
        debug_print("5. Implement 'dark' and 'light' commands")
    
    return 0 if palette_exists else 1

if __name__ == "__main__":
    sys.exit(main())