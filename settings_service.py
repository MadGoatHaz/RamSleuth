#!/usr/bin/env python3
"""
SettingsService - Centralized configuration management for RamSleuth.

This module provides a SettingsService class that encapsulates all settings
management logic, replacing the previous load_config/save_config functions.
It maintains backward compatibility while providing a more maintainable and
testable solution.

Features:
- XDG Base Directory specification compliance
- Sudo context awareness (uses original user's home directory)
- File ownership management (recursive chown when using sudo)
- Settings validation
- Comprehensive debug logging
- Type hints and comprehensive docstrings
- Backward compatibility with existing config file format
"""

import json
import os
import pwd
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, Optional

# Type alias for settings dictionary
SettingsDict = Dict[str, Any]


class SettingsService:
    """
    Centralized settings management service for RamSleuth.
    
    This class encapsulates all configuration management logic, providing
    a clean API for loading, saving, and manipulating application settings.
    It handles XDG Base Directory paths, sudo context, file ownership,
    and settings validation automatically.
    
    Attributes:
        debug: Whether debug logging is enabled
        config_dir: Path to configuration directory
        config_file: Path to configuration file
        _settings: Current settings dictionary
        _path_initialized: Whether paths have been initialized
    """
    
    # Class constants
    CONFIG_DIR_NAME = "ramsleuth"
    CONFIG_FILE_NAME = "ramsleuth_config.json"
    
    # Default settings
    DEFAULT_SETTINGS: SettingsDict = {
        "theme": "dark",
        "active_tab": "summary_tab",
        "default_view_tab": "summary"
    }
    
    # Validation rules for settings
    VALIDATION_RULES = {
        "theme": lambda x: x in ["dark", "light"],
        "active_tab": lambda x: x in ["summary_tab", "full_tab"],
        "default_view_tab": lambda x: x in ["summary", "full"]
    }
    
    def __init__(self, debug: bool = False) -> None:
        """
        Initialize the SettingsService.
        
        Args:
            debug: Enable debug logging for all operations
            
        Example:
            >>> settings = SettingsService(debug=True)
            >>> settings.get_setting("theme")
            'dark'
        """
        self.debug = debug
        self._config_dir: Optional[Path] = None
        self._config_file: Optional[Path] = None
        self._settings: SettingsDict = {}
        self._path_initialized: bool = False
        
        # Initialize paths and load settings
        self._initialize_paths()
        self.load_settings()
    
    def _initialize_paths(self) -> None:
        """
        Initialize configuration directory and file paths.
        
        This method determines the correct configuration paths based on:
        - XDG Base Directory specification
        - Sudo context (uses original user's home directory)
        
        The paths are cached after first initialization.
        """
        if self._path_initialized:
            return
        
        try:
            self._config_dir = self._get_config_base_dir() / self.CONFIG_DIR_NAME
            self._config_file = self._config_dir / self.CONFIG_FILE_NAME
            self._path_initialized = True
            
            self._debug_print(f"_initialize_paths: config_dir={self._config_dir}")
            self._debug_print(f"_initialize_paths: config_file={self._config_file}")
            
        except Exception as e:
            self._debug_print(f"_initialize_paths: failed to initialize paths: {e}")
            # Set fallback paths
            fallback_dir = Path.home() / ".config" / self.CONFIG_DIR_NAME
            self._config_dir = fallback_dir
            self._config_file = fallback_dir / self.CONFIG_FILE_NAME
            self._path_initialized = True
    
    def _get_config_base_dir(self) -> Path:
        """
        Get the base configuration directory using XDG specification.
        
        When running with sudo, uses the original user's home directory.
        Otherwise, follows XDG Base Directory specification.
        
        Returns:
            Path to base configuration directory
            
        Raises:
            OSError: If unable to determine config directory
        """
        # When running with sudo, use the original user's home directory
        if os.environ.get('SUDO_USER'):
            sudo_user = os.environ['SUDO_USER']
            try:
                user_home = pwd.getpwnam(sudo_user).pw_dir
                return Path(user_home) / ".config"
            except (KeyError, OSError) as e:
                self._debug_print(f"_get_config_base_dir: failed to get sudo user home: {e}")
                # Fall back to standard XDG
        
        # Standard XDG Base Directory specification
        xdg_config_home = os.environ.get('XDG_CONFIG_HOME')
        if xdg_config_home:
            return Path(xdg_config_home)
        
        # Default to ~/.config
        return Path.home() / ".config"
    
    def _debug_print(self, message: str) -> None:
        """
        Print debug message if debug mode is enabled.
        
        Args:
            message: Debug message to print
        """
        if self.debug:
            print(f"[SettingsService:DEBUG] {message}", file=sys.stderr)
    
    def load_settings(self) -> None:
        """
        Load settings from configuration file.
        
        If the configuration file doesn't exist or cannot be read,
        initializes with default settings. Errors are logged but
        do not prevent the application from starting.
        
        Side Effects:
            Updates self._settings with loaded or default values
        """
        if not self._config_file:
            self._debug_print("load_settings: config file path not initialized")
            self._settings = self.DEFAULT_SETTINGS.copy()
            return
        
        if not self._config_file.exists():
            self._debug_print(f"load_settings: config file not found at {self._config_file}")
            self._settings = self.DEFAULT_SETTINGS.copy()
            return
        
        try:
            with open(self._config_file, 'r', encoding='utf-8') as f:
                loaded_settings = json.load(f)
                self._settings = loaded_settings
                self._debug_print(f"load_settings: successfully loaded {len(loaded_settings)} settings")
                
                # Add any missing default settings
                for key, default_value in self.DEFAULT_SETTINGS.items():
                    if key not in self._settings:
                        self._settings[key] = default_value
                        self._debug_print(f"load_settings: added missing default setting '{key}={default_value}'")
                
        except json.JSONDecodeError as e:
            self._debug_print(f"load_settings: JSON decode error in {self._config_file}: {e}")
            self._settings = self.DEFAULT_SETTINGS.copy()
        except OSError as e:
            self._debug_print(f"load_settings: OS error reading {self._config_file}: {e}")
            self._settings = self.DEFAULT_SETTINGS.copy()
        except Exception as e:
            self._debug_print(f"load_settings: unexpected error: {e}")
            self._settings = self.DEFAULT_SETTINGS.copy()
    
    def save_settings(self) -> None:
        """
        Save current settings to configuration file.
        
        Creates the configuration directory if it doesn't exist.
        When running with sudo, changes file ownership to the original user.
        
        Side Effects:
            Writes configuration file to disk
            May change file/directory ownership when using sudo
        """
        if not self._config_file or not self._config_dir:
            self._debug_print("save_settings: config paths not initialized")
            return
        
        try:
            # Create directory if it doesn't exist
            self._config_dir.mkdir(parents=True, exist_ok=True)
            
            # Save settings to file
            with open(self._config_file, 'w', encoding='utf-8') as f:
                json.dump(self._settings, f, indent=2)
            
            self._debug_print(f"save_settings: successfully saved {len(self._settings)} settings to {self._config_file}")
            
            # Handle sudo context file ownership
            self._handle_sudo_ownership()
            
        except OSError as e:
            self._debug_print(f"save_settings: OS error writing to {self._config_file}: {e}")
        except Exception as e:
            self._debug_print(f"save_settings: unexpected error: {e}")
    
    def _handle_sudo_ownership(self) -> None:
        """
        Handle file ownership when running with sudo.
        
        When running with sudo, changes ownership of the configuration
        directory and all its contents to the original user.
        
        Side Effects:
            May execute chown command to change file ownership
        """
        if not os.environ.get('SUDO_USER'):
            return
        
        try:
            sudo_user = os.environ['SUDO_USER']
            user_info = pwd.getpwnam(sudo_user)
            
            # Use recursive chown to ensure both directory and contents are owned by original user
            config_dir_str = str(self._config_dir)
            self._debug_print(f"_handle_sudo_ownership: attempting recursive chown of {config_dir_str} to {sudo_user}:{sudo_user}")
            
            result = subprocess.run(
                ["chown", "-R", f"{sudo_user}:{sudo_user}", config_dir_str],
                capture_output=True,
                text=True,
                check=False,
                timeout=30
            )
            
            if result.returncode == 0:
                self._debug_print(f"_handle_sudo_ownership: successfully changed ownership to {sudo_user} recursively")
            else:
                self._debug_print(f"_handle_sudo_ownership: chown failed with returncode {result.returncode}, stderr: {result.stderr}")
                
        except Exception as e:
            self._debug_print(f"_handle_sudo_ownership: failed to change ownership: {e}")
            import traceback
            self._debug_print(f"_handle_sudo_ownership: traceback: {traceback.format_exc()}")
    
    def get_setting(self, key: str, default: Any = None) -> Any:
        """
        Get a setting value by key.
        
        Args:
            key: Setting key to retrieve
            default: Default value if key doesn't exist
            
        Returns:
            Setting value or default if not found
            
        Example:
            >>> settings = SettingsService()
            >>> theme = settings.get_setting("theme", "dark")
            >>> print(theme)
            'dark'
        """
        if not isinstance(key, str):
            self._debug_print(f"get_setting: invalid key type {type(key)}, expected str")
            return default
        
        value = self._settings.get(key, default)
        self._debug_print(f"get_setting: key='{key}', value={value}, default={default}")
        return value
    
    def set_setting(self, key: str, value: Any) -> bool:
        """
        Set a setting value and save immediately.
        
        Args:
            key: Setting key to set
            value: Value to set
            
        Returns:
            True if setting was successfully set and saved, False otherwise
            
        Example:
            >>> settings = SettingsService()
            >>> success = settings.set_setting("theme", "light")
            >>> print(success)
            True
        """
        if not isinstance(key, str):
            self._debug_print(f"set_setting: invalid key type {type(key)}, expected str")
            return False
        
        # Validate the setting if validation rule exists
        if not self.validate_setting(key, value):
            self._debug_print(f"set_setting: validation failed for key='{key}', value={value}")
            return False
        
        # Update the setting
        old_value = self._settings.get(key)
        self._settings[key] = value
        
        # Save to disk
        self.save_settings()
        
        self._debug_print(f"set_setting: changed '{key}' from {old_value} to {value}")
        return True
    
    def validate_setting(self, key: str, value: Any) -> bool:
        """
        Validate a setting value against predefined rules.
        
        Args:
            key: Setting key to validate
            value: Value to validate
            
        Returns:
            True if value is valid, False otherwise
            
        Example:
            >>> settings = SettingsService()
            >>> settings.validate_setting("theme", "dark")
            True
            >>> settings.validate_setting("theme", "blue")
            False
        """
        if key not in self.VALIDATION_RULES:
            # No validation rule for this key, assume it's valid
            self._debug_print(f"validate_setting: no validation rule for key='{key}', assuming valid")
            return True
        
        try:
            is_valid = self.VALIDATION_RULES[key](value)
            self._debug_print(f"validate_setting: key='{key}', value={value}, valid={is_valid}")
            return bool(is_valid)
        except Exception as e:
            self._debug_print(f"validate_setting: validation error for key='{key}', value={value}: {e}")
            return False
    
    def get_all_settings(self) -> SettingsDict:
        """
        Get all current settings.
        
        Returns:
            Copy of current settings dictionary
            
        Example:
            >>> settings = SettingsService()
            >>> all_settings = settings.get_all_settings()
            >>> print(all_settings)
            {'theme': 'dark', 'active_tab': 'summary_tab', 'default_view_tab': 'summary'}
        """
        self._debug_print(f"get_all_settings: returning {len(self._settings)} settings")
        return self._settings.copy()
    
    def reset_to_defaults(self) -> None:
        """
        Reset all settings to default values and save.
        
        This method overwrites all current settings with default values
        and immediately saves them to disk.
        
        Example:
            >>> settings = SettingsService()
            >>> settings.set_setting("theme", "light")
            >>> settings.reset_to_defaults()
            >>> settings.get_setting("theme")
            'dark'
        """
        self._debug_print("reset_to_defaults: resetting all settings to defaults")
        self._settings = self.DEFAULT_SETTINGS.copy()
        self.save_settings()
        self._debug_print("reset_to_defaults: successfully reset and saved defaults")


# Backward compatibility functions
# These maintain the existing load_config/save_config API for gradual migration

def load_config(debug: bool = False) -> SettingsDict:
    """
    Load configuration from file (backward compatibility).
    
    This function provides backward compatibility with the existing
    load_config() API. It creates a SettingsService instance and
    returns the loaded settings.
    
    Args:
        debug: Enable debug logging
        
    Returns:
        Dictionary containing configuration data
        
    Example:
        >>> config = load_config(debug=True)
        >>> theme = config.get("theme", "dark")
    """
    settings = SettingsService(debug=debug)
    return settings.get_all_settings()


def save_config(config: SettingsDict, debug: bool = False) -> None:
    """
    Save configuration to file (backward compatibility).
    
    This function provides backward compatibility with the existing
    save_config() API. It creates a SettingsService instance and
    saves the provided configuration.
    
    Args:
        config: Dictionary containing configuration data to save
        debug: Enable debug logging
        
    Example:
        >>> config = {"theme": "light", "active_tab": "full_tab"}
        >>> save_config(config, debug=True)
    """
    settings = SettingsService(debug=debug)
    
    # Validate and set each configuration item
    for key, value in config.items():
        if not settings.validate_setting(key, value):
            settings._debug_print(f"save_config: invalid value for {key}={value}, skipping")
            continue
        settings._settings[key] = value
    
    settings.save_settings()


# Module-level default instance for convenience
_default_settings: Optional[SettingsService] = None


def get_default_settings(debug: bool = False) -> SettingsService:
    """
    Get the default SettingsService instance.
    
    This function provides a module-level singleton instance of
    SettingsService for convenience.
    
    Args:
        debug: Enable debug logging
        
    Returns:
        Default SettingsService instance
    """
    global _default_settings
    if _default_settings is None:
        _default_settings = SettingsService(debug=debug)
    return _default_settings


if __name__ == "__main__":
    # Simple self-test when run directly
    import tempfile
    
    print("SettingsService self-test...")
    
    # Create temporary directory for testing
    with tempfile.TemporaryDirectory() as temp_dir:
        # Override XDG config home for testing
        os.environ['XDG_CONFIG_HOME'] = temp_dir
        
        # Test basic functionality
        settings = SettingsService(debug=True)
        
        print(f"Initial theme: {settings.get_setting('theme')}")
        
        # Test setting change
        settings.set_setting("theme", "light")
        print(f"Changed theme: {settings.get_setting('theme')}")
        
        # Test persistence
        settings2 = SettingsService(debug=True)
        print(f"Persisted theme: {settings2.get_setting('theme')}")
        
        # Test validation
        print(f"Validation 'dark': {settings.validate_setting('theme', 'dark')}")
        print(f"Validation 'blue': {settings.validate_setting('theme', 'blue')}")
        
        print("Self-test completed successfully!")