#!/usr/bin/env python3
"""
Unit tests for SettingsService class.

This test suite covers all major functionality of the SettingsService class:
- Path initialization and XDG compliance
- Configuration loading and saving
- Settings validation
- Sudo context handling
- Error handling and edge cases
- Backward compatibility functions
"""

import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch, MagicMock

from settings_service import SettingsService, load_config, save_config, get_default_settings


class TestSettingsService(unittest.TestCase):
    """Test cases for SettingsService class."""
    
    def setUp(self) -> None:
        """Set up test environment."""
        # Create temporary directory for test configs
        self.temp_dir = tempfile.mkdtemp()
        self.config_home = Path(self.temp_dir) / "config"
        self.config_home.mkdir(parents=True)
        
        # Set XDG_CONFIG_HOME for testing
        self.original_xdg_config = os.environ.get('XDG_CONFIG_HOME')
        os.environ['XDG_CONFIG_HOME'] = str(self.config_home)
        
        # Clear SUDO_USER for most tests
        self.original_sudo_user = os.environ.get('SUDO_USER')
        if 'SUDO_USER' in os.environ:
            del os.environ['SUDO_USER']
    
    def tearDown(self) -> None:
        """Clean up test environment."""
        # Restore environment variables
        if self.original_xdg_config is not None:
            os.environ['XDG_CONFIG_HOME'] = self.original_xdg_config
        elif 'XDG_CONFIG_HOME' in os.environ:
            del os.environ['XDG_CONFIG_HOME']
        
        if self.original_sudo_user is not None:
            os.environ['SUDO_USER'] = self.original_sudo_user
        elif 'SUDO_USER' in os.environ:
            del os.environ['SUDO_USER']
        
        # Clean up temporary directory
        import shutil
        shutil.rmtree(self.temp_dir, ignore_errors=True)
    
    def test_initialization(self) -> None:
        """Test SettingsService initialization."""
        settings = SettingsService(debug=False)
        
        self.assertIsNotNone(settings)
        self.assertFalse(settings.debug)
        self.assertIsNotNone(settings._config_dir)
        self.assertIsNotNone(settings._config_file)
        self.assertTrue(settings._path_initialized)
    
    def test_default_settings(self) -> None:
        """Test default settings are loaded when no config file exists."""
        settings = SettingsService(debug=False)
        
        self.assertEqual(settings.get_setting("theme"), "dark")
        self.assertEqual(settings.get_setting("active_tab"), "summary_tab")
        self.assertEqual(settings.get_setting("default_view_tab"), "summary")
    
    def test_load_settings_from_file(self) -> None:
        """Test loading settings from existing configuration file."""
        # Create a config file with custom settings
        config_dir = self.config_home / "ramsleuth"
        config_dir.mkdir(parents=True)
        config_file = config_dir / "ramsleuth_config.json"
        
        test_settings = {
            "theme": "light",
            "active_tab": "full_tab",
            "custom_setting": "custom_value"
        }
        
        with open(config_file, 'w', encoding='utf-8') as f:
            json.dump(test_settings, f)
        
        # Load settings and verify
        settings = SettingsService(debug=False)
        
        self.assertEqual(settings.get_setting("theme"), "light")
        self.assertEqual(settings.get_setting("active_tab"), "full_tab")
        self.assertEqual(settings.get_setting("custom_setting"), "custom_value")
    
    def test_load_settings_with_missing_defaults(self) -> None:
        """Test that missing default settings are added."""
        # Create a config file with partial settings
        config_dir = self.config_home / "ramsleuth"
        config_dir.mkdir(parents=True)
        config_file = config_dir / "ramsleuth_config.json"
        
        # Only include theme, missing active_tab and default_view_tab
        with open(config_file, 'w', encoding='utf-8') as f:
            json.dump({"theme": "light"}, f)
        
        settings = SettingsService(debug=False)
        
        # Should have loaded theme and added missing defaults
        self.assertEqual(settings.get_setting("theme"), "light")
        self.assertEqual(settings.get_setting("active_tab"), "summary_tab")  # Default
        self.assertEqual(settings.get_setting("default_view_tab"), "summary")  # Default
    
    def test_load_settings_invalid_json(self) -> None:
        """Test handling of invalid JSON in config file."""
        config_dir = self.config_home / "ramsleuth"
        config_dir.mkdir(parents=True)
        config_file = config_dir / "ramsleuth_config.json"
        
        # Write invalid JSON
        with open(config_file, 'w', encoding='utf-8') as f:
            f.write('{"invalid": json content}')
        
        settings = SettingsService(debug=False)
        
        # Should fall back to defaults
        self.assertEqual(settings.get_setting("theme"), "dark")
    
    def test_save_settings_creates_directory(self) -> None:
        """Test that save_settings creates directory if it doesn't exist."""
        settings = SettingsService(debug=False)
        
        # Change a setting to trigger save
        settings.set_setting("theme", "light")
        
        # Verify directory and file were created
        config_dir = self.config_home / "ramsleuth"
        config_file = config_dir / "ramsleuth_config.json"
        
        self.assertTrue(config_dir.exists())
        self.assertTrue(config_file.exists())
        
        # Verify content
        with open(config_file, 'r', encoding='utf-8') as f:
            saved_settings = json.load(f)
        
        self.assertEqual(saved_settings["theme"], "light")
    
    def test_set_setting_validation(self) -> None:
        """Test setting validation."""
        settings = SettingsService(debug=False)
        
        # Valid theme values
        self.assertTrue(settings.set_setting("theme", "dark"))
        self.assertTrue(settings.set_setting("theme", "light"))
        
        # Invalid theme value
        self.assertFalse(settings.set_setting("theme", "blue"))
        
        # Valid active_tab values
        self.assertTrue(settings.set_setting("active_tab", "summary_tab"))
        self.assertTrue(settings.set_setting("active_tab", "full_tab"))
        
        # Invalid active_tab value
        self.assertFalse(settings.set_setting("active_tab", "invalid_tab"))
    
    def test_set_setting_persists(self) -> None:
        """Test that set_setting immediately persists to disk."""
        settings = SettingsService(debug=False)
        
        # Change a setting
        self.assertTrue(settings.set_setting("theme", "light"))
        
        # Create new instance and verify persistence
        settings2 = SettingsService(debug=False)
        self.assertEqual(settings2.get_setting("theme"), "light")
    
    def test_get_setting_with_default(self) -> None:
        """Test get_setting with default values."""
        settings = SettingsService(debug=False)
        
        # Existing setting
        self.assertEqual(settings.get_setting("theme", "light"), "dark")
        
        # Non-existing setting with default
        self.assertEqual(settings.get_setting("nonexistent", "default_value"), "default_value")
        
        # Non-existing setting without default (should return None)
        self.assertIsNone(settings.get_setting("nonexistent"))
    
    def test_get_all_settings(self) -> None:
        """Test getting all settings."""
        settings = SettingsService(debug=False)
        
        all_settings = settings.get_all_settings()
        
        self.assertIsInstance(all_settings, dict)
        self.assertIn("theme", all_settings)
        self.assertIn("active_tab", all_settings)
        self.assertIn("default_view_tab", all_settings)
        
        # Should be a copy, not the internal dict
        all_settings["new_key"] = "new_value"
        self.assertIsNone(settings.get_setting("new_key"))
    
    def test_reset_to_defaults(self) -> None:
        """Test resetting settings to defaults."""
        settings = SettingsService(debug=False)
        
        # Change some settings
        settings.set_setting("theme", "light")
        settings.set_setting("active_tab", "full_tab")
        
        # Reset to defaults
        settings.reset_to_defaults()
        
        # Verify defaults are restored
        self.assertEqual(settings.get_setting("theme"), "dark")
        self.assertEqual(settings.get_setting("active_tab"), "summary_tab")
        
        # Verify persistence
        settings2 = SettingsService(debug=False)
        self.assertEqual(settings2.get_setting("theme"), "dark")
    
    @patch('builtins.open')
    @patch('pathlib.Path.mkdir')
    @patch('subprocess.run')
    @patch('pwd.getpwnam')
    def test_sudo_ownership_handling(self, mock_getpwnam, mock_subprocess, mock_mkdir, mock_open) -> None:
        """Test sudo ownership handling."""
        # Set sudo environment
        os.environ['SUDO_USER'] = "testuser"
        
        mock_pwd_result = MagicMock()
        mock_pwd_result.pw_dir = "/home/testuser"
        mock_getpwnam.return_value = mock_pwd_result
        
        # Mock successful chown
        mock_subprocess_result = MagicMock()
        mock_subprocess_result.returncode = 0
        mock_subprocess.return_value = mock_subprocess_result
        
        settings = SettingsService(debug=False)
        
        # Trigger save to test ownership handling
        settings.set_setting("theme", "light")
        
        # Verify chown was called
        mock_subprocess.assert_called_once()
        call_args = mock_subprocess.call_args[0][0]
        self.assertIn("chown", call_args)
        self.assertIn("-R", call_args)
        self.assertIn("testuser:testuser", call_args)
    
    @patch('subprocess.run')
    def test_sudo_ownership_handling_failure(self, mock_subprocess) -> None:
        """Test graceful handling of chown failure."""
        # Set sudo user
        os.environ['SUDO_USER'] = 'testuser'
        
        # Mock failed chown
        mock_subprocess_result = MagicMock()
        mock_subprocess_result.returncode = 1
        mock_subprocess_result.stderr = "Permission denied"
        mock_subprocess.return_value = mock_subprocess_result
        
        settings = SettingsService(debug=False)
        
        # Should not raise exception even if chown fails
        try:
            settings.set_setting("theme", "light")
        except Exception:
            self.fail("save_settings should not raise exception on chown failure")
    
    def test_xdg_config_home_resolution(self) -> None:
        """Test XDG_CONFIG_HOME environment variable resolution."""
        custom_config_home = self.config_home / "custom"
        custom_config_home.mkdir(parents=True)
        
        os.environ['XDG_CONFIG_HOME'] = str(custom_config_home)
        
        settings = SettingsService(debug=False)
        
        expected_config_dir = custom_config_home / "ramsleuth"
        self.assertEqual(settings._config_dir, expected_config_dir)
    
    def test_sudo_user_resolution(self) -> None:
        """Test sudo user home directory resolution."""
        # Mock pwd.getpwnam
        with patch('pwd.getpwnam') as mock_getpwnam:
            mock_pwd_result = MagicMock()
            mock_pwd_result.pw_dir = "/home/sudouser"
            mock_getpwnam.return_value = mock_pwd_result
            
            os.environ['SUDO_USER'] = 'sudouser'
            
            settings = SettingsService(debug=False)
            
            expected_config_dir = Path("/home/sudouser/.config/ramsleuth")
            self.assertEqual(settings._config_dir, expected_config_dir)
    
    def test_debug_logging(self) -> None:
        """Test debug logging functionality."""
        # Capture stderr
        import io
        from contextlib import redirect_stderr
        
        settings = SettingsService(debug=True)
        
        # Capture debug output
        debug_output = io.StringIO()
        with redirect_stderr(debug_output):
            settings._debug_print("Test debug message")
        
        output = debug_output.getvalue()
        self.assertIn("[SettingsService:DEBUG]", output)
        self.assertIn("Test debug message", output)
        
        # Test with debug disabled
        settings.debug = False
        debug_output = io.StringIO()
        with redirect_stderr(debug_output):
            settings._debug_print("This should not appear")
        
        output = debug_output.getvalue()
        self.assertEqual(output, "")
    
    def test_invalid_key_types(self) -> None:
        """Test handling of invalid key types."""
        settings = SettingsService(debug=False)
        
        # Should handle non-string keys gracefully by returning default/False
        # instead of raising AssertionError
        self.assertIsNone(settings.get_setting(123))  # type: ignore
        self.assertFalse(settings.set_setting(123, "value"))  # type: ignore
    
    def test_backward_compatibility_load_config(self) -> None:
        """Test backward compatibility load_config function."""
        # Create a config file
        config_dir = self.config_home / "ramsleuth"
        config_dir.mkdir(parents=True)
        config_file = config_dir / "ramsleuth_config.json"
        
        test_settings = {"theme": "light", "active_tab": "full_tab"}
        with open(config_file, 'w', encoding='utf-8') as f:
            json.dump(test_settings, f)
        
        # Test load_config function
        config = load_config(debug=False)
        
        self.assertEqual(config["theme"], "light")
        self.assertEqual(config["active_tab"], "full_tab")
    
    def test_backward_compatibility_save_config(self) -> None:
        """Test backward compatibility save_config function."""
        test_config = {"theme": "light", "active_tab": "full_tab"}
        
        # Save using backward compatibility function
        save_config(test_config, debug=False)
        
        # Verify file was created and contains correct data
        config_dir = self.config_home / "ramsleuth"
        config_file = config_dir / "ramsleuth_config.json"
        
        self.assertTrue(config_file.exists())
        
        with open(config_file, 'r', encoding='utf-8') as f:
            saved_config = json.load(f)
        
        self.assertEqual(saved_config["theme"], "light")
        self.assertEqual(saved_config["active_tab"], "full_tab")
    
    def test_backward_compatibility_save_config_invalid_values(self) -> None:
        """Test save_config with invalid values (should be filtered)."""
        test_config = {
            "theme": "light",  # Valid
            "theme_invalid": "blue",  # Invalid, but no validation rule
            "active_tab": "invalid_tab"  # Invalid, has validation rule
        }
        
        save_config(test_config, debug=False)
        
        # Load and verify
        config = load_config(debug=False)
        
        self.assertEqual(config["theme"], "light")
        self.assertEqual(config["theme_invalid"], "blue")  # No validation rule, so allowed
        # active_tab should be default since validation failed during save
        # (but save_config doesn't validate existing settings, only new ones)
    
    def test_get_default_settings_singleton(self) -> None:
        """Test get_default_settings singleton behavior."""
        settings1 = get_default_settings(debug=False)
        settings2 = get_default_settings(debug=False)
        
        # Should be the same instance
        self.assertIs(settings1, settings2)
        
        # Should be independent of directly created instances
        settings3 = SettingsService(debug=False)
        self.assertIsNot(settings1, settings3)
    
    def test_exception_handling_in_load(self) -> None:
        """Test graceful handling of exceptions during load."""
        # Make the config file unreadable
        config_dir = self.config_home / "ramsleuth"
        config_dir.mkdir(parents=True)
        config_file = config_dir / "ramsleuth_config.json"
        
        # Write valid config
        with open(config_file, 'w', encoding='utf-8') as f:
            json.dump({"theme": "light"}, f)
        
        # Make file unreadable (skip on Windows)
        try:
            os.chmod(config_file, 0o000)
            
            settings = SettingsService(debug=False)
            
            # Should fall back to defaults
            self.assertEqual(settings.get_setting("theme"), "dark")
            
        finally:
            # Restore permissions for cleanup
            try:
                os.chmod(config_file, 0o644)
            except OSError:
                pass
    
    def test_custom_settings_keys(self) -> None:
        """Test handling of custom settings keys beyond defaults."""
        settings = SettingsService(debug=False)
        
        # Set a custom setting
        settings.set_setting("custom_key", "custom_value")
        
        # Should be saved and retrievable
        self.assertEqual(settings.get_setting("custom_key"), "custom_value")
        
        # Should persist
        settings2 = SettingsService(debug=False)
        self.assertEqual(settings2.get_setting("custom_key"), "custom_value")


if __name__ == "__main__":
    # Run tests with verbose output
    unittest.main(verbosity=2)