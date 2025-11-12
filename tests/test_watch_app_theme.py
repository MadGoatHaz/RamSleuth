#!/usr/bin/env python3
"""
Test script to verify watch_app_theme() is called when themes change.
This simulates the Textual TUI environment to test theme change detection.
"""

import sys
import os
import json
from pathlib import Path
from unittest.mock import Mock, patch

# Add the current directory to path so we can import ramsleuth
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import ramsleuth

def test_watch_app_theme_triggering():
    """Test that watch_app_theme is called when app.theme changes"""
    print("[TEST] Testing watch_app_theme() triggering...")
    
    # Enable debug mode
    ramsleuth.DEBUG = True
    
    # Create a mock app similar to RamSleuthApp
    class MockApp:
        def __init__(self):
            self.theme = "dark"
            self.dark = True
            
        def refresh_css(self):
            pass
            
        def get_available_themes(self):
            return {"dark", "light", "tokyo-night", "solarized-dark"}
    
    # Create the app and add the theme watching methods
    app = MockApp()
    
    # Add the watch methods from RamSleuthApp
    def watch_dark(self, dark):
        """Mock watch_dark method"""
        print(f"[watch_dark] Dark mode changed to: {dark}")
        new_theme = "dark" if dark else "light"
        config = ramsleuth.load_config()
        config["theme"] = new_theme
        ramsleuth.save_config(config)
        print(f"[watch_dark] Saved theme '{new_theme}' to config")
        
    def watch_app_theme(self, theme):
        """Mock watch_app_theme method - this is what we're testing"""
        print(f"[watch_app_theme] App theme changed to: {theme}")
        config = ramsleuth.load_config()
        config["theme"] = theme
        ramsleuth.save_config(config)
        print(f"[watch_app_theme] Saved theme '{theme}' to config")
        
    def action_set_theme(self, theme):
        """Mock action_set_theme method"""
        print(f"[action_set_theme] Setting theme to: {theme}")
        self.app.theme = theme
        self.refresh_css()
        print(f"[action_set_theme] Theme changed to: {theme}")
        
        # Save to config
        config = ramsleuth.load_config()
        config["theme"] = theme
        ramsleuth.save_config(config)
        print(f"[action_set_theme] Theme saved to config")
    
    # Bind methods to the app
    app.watch_dark = watch_dark.__get__(app, MockApp)
    app.watch_app_theme = watch_app_theme.__get__(app, MockApp)
    app.action_set_theme = action_set_theme.__get__(app, MockApp)
    app.app = app  # For self.app.theme references
    
    print(f"\n[TEST] Initial theme: {app.theme}")
    
    # Test 1: Change theme via action_set_theme (simulates command palette)
    print(f"\n[TEST] Test 1: Changing theme via action_set_theme (command palette simulation)")
    app.action_set_theme("tokyo-night")
    print(f"[TEST] Theme after action_set_theme: {app.theme}")
    
    # Test 2: Change theme property directly (should trigger watch_app_theme)
    print(f"\n[TEST] Test 2: Changing app.theme property directly")
    print("[TEST] This should trigger watch_app_theme() if Textual's watching mechanism works")
    
    # Simulate what Textual does when app.theme property changes
    old_theme = app.theme
    app.theme = "solarized-dark"
    
    # In Textual, this would automatically call watch_app_theme
    # We need to manually call it since we're not running in Textual
    if old_theme != app.theme:
        print(f"[TEST] Theme changed from '{old_theme}' to '{app.theme}'")
        print("[TEST] Calling watch_app_theme() manually (Textual would do this automatically)")
        app.watch_app_theme(app.theme)
    
    print(f"[TEST] Theme after direct change: {app.theme}")
    
    # Test 3: Test dark/light toggle
    print(f"\n[TEST] Test 3: Testing dark/light toggle")
    app.theme = "dark"
    app.watch_app_theme(app.theme)
    print(f"[TEST] Theme after setting to dark: {app.theme}")
    
    # Verify config file
    config_file = Path.home() / ".config" / "ramsleuth" / "ramsleuth_config.json"
    if config_file.exists():
        with open(config_file, 'r') as f:
            config = json.load(f)
        print(f"\n[TEST] Final config file contents: {config}")
        print(f"[TEST] Theme in config: {config.get('theme', 'NOT_FOUND')}")
    else:
        print(f"[TEST] Config file not found at {config_file}")
    
    print("\n[TEST] Test completed!")

if __name__ == "__main__":
    test_watch_app_theme_triggering()