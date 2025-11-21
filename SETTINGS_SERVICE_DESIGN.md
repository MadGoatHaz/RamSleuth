# SettingsService Design Document

## Overview

This document outlines the design and implementation of a dedicated `SettingsService` class for the RamSleuth project. The `SettingsService` provides centralized, maintainable, and testable configuration management, replacing the previous `load_config()` and `save_config()` functions with a robust, production-ready solution.

## Current Implementation Analysis

### Existing Functions in Monolithic [`ramsleuth.py`](ramsleuth.py) (Pre-refactoring)

**Previous Configuration Management:**
- `load_config()` - Loads configuration from JSON file (originally in [`ramsleuth.py`](ramsleuth.py))
- `save_config(config)` - Saves configuration to JSON file (originally in [`ramsleuth.py`](ramsleuth.py))
- Configuration stored in `~/.config/ramsleuth/ramsleuth_config.json`
- Used XDG Base Directory specification

**Previous Settings Usage:**
- `theme`: "dark" or "light" (default: "dark")
- `active_tab`: "summary_tab" or "full_tab" (default: "summary_tab")
- Settings loaded/saved in multiple places throughout codebase

**Key Features of Previous Implementation:**
- XDG Base Directory compliance
- Sudo context awareness (uses original user's home directory)
- File ownership management (recursive chown when using sudo)
- Debug logging for all operations
- JSON format with indentation

## Design Goals

1. **Encapsulation**: Centralize all settings management logic in a single, cohesive class
2. **Maintainability**: Provide clear interfaces and validation for easy extension
3. **Testability**: Enable comprehensive unit and integration testing
4. **Backward Compatibility**: Maintain existing config file format and API compatibility
5. **Type Safety**: Add comprehensive type hints and runtime validation
6. **Error Handling**: Implement graceful failure modes with detailed logging
7. **Extensibility**: Support custom settings beyond predefined defaults
8. **Performance**: Optimize path resolution and minimize I/O operations

## SettingsService Class Design

### Class Structure

```python
class SettingsService:
    """Centralized settings management service for RamSleuth."""
    
    # Class constants
    CONFIG_DIR_NAME = "ramsleuth"
    CONFIG_FILE_NAME = "ramsleuth_config.json"
    DEFAULT_SETTINGS = {
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
        """Initialize the SettingsService with optional debug mode."""
        self.debug = debug
        self._config_dir: Optional[Path] = None
        self._config_file: Optional[Path] = None
        self._settings: Dict[str, Any] = {}
        self._path_initialized: bool = False
        
        # Initialize paths and load settings
        self._initialize_paths()
        self.load_settings()
    
    def _initialize_paths(self) -> None:
        """Initialize configuration directory and file paths with fallback handling."""
        # Implementation includes XDG compliance and sudo context detection
    
    def _get_config_base_dir(self) -> Path:
        """Get base configuration directory using XDG specification with sudo support."""
        # Handles SUDO_USER environment variable for correct path resolution
    
    def _debug_print(self, message: str) -> None:
        """Print debug message to stderr if debug mode is enabled."""
        # Centralized debug logging with consistent formatting
    
    def load_settings(self) -> None:
        """Load settings from configuration file with graceful error handling."""
        # Implements fallback to defaults on any load failure
    
    def save_settings(self) -> None:
        """Save current settings to configuration file with directory creation."""
        # Includes sudo ownership handling and atomic write patterns
    
    def _handle_sudo_ownership(self) -> None:
        """Handle file ownership changes when running with sudo."""
        # Recursive chown implementation with error isolation
    
    def get_setting(self, key: str, default: Any = None) -> Any:
        """Retrieve a setting value by key with type validation."""
        # Type-safe key access with debug logging
    
    def set_setting(self, key: str, value: Any) -> bool:
        """Set a setting value, validate it, and save immediately."""
        # Validation-first approach with immediate persistence
    
    def validate_setting(self, key: str, value: Any) -> bool:
        """Validate a setting value against predefined rules."""
        # Extensible validation system with custom rule support
    
    def get_all_settings(self) -> Dict[str, Any]:
        """Return a copy of all current settings."""
        # Defensive copy to prevent external mutation
    
    def reset_to_defaults(self) -> None:
        """Reset all settings to default values and save."""
        # Complete reset with immediate persistence
```

### Key Features

#### 1. Path Management
- **XDG Base Directory Compliance**: Follows XDG specification with `XDG_CONFIG_HOME` support
- **Sudo Context Awareness**: Automatically detects and handles `SUDO_USER` environment variable
- **Fallback Paths**: Graceful degradation to `~/.config/` if XDG resolution fails
- **Path Caching**: Resolves paths once and caches for performance
- **Cross-Platform Support**: Uses `pathlib.Path` for platform-independent path handling

#### 2. Settings Validation
```python
# Validation rules for different setting types
VALIDATION_RULES = {
    "theme": lambda x: x in ["dark", "light"],
    "active_tab": lambda x: x in ["summary_tab", "full_tab"],
    "default_view_tab": lambda x: x in ["summary", "full"]
}

# Custom settings without validation rules are automatically accepted
# Validation errors are logged but don't crash the application
```

#### 3. Error Handling Strategy
- **Graceful Degradation**: All operations fallback to safe defaults
- **Comprehensive Logging**: Debug mode provides detailed operation tracing
- **Error Isolation**: File operation failures don't affect application stability
- **Type Safety**: Runtime type checking prevents common programming errors
- **Exception Isolation**: Each operation catches and logs its own exceptions

#### 4. Debug Logging System
```python
# Debug output format for consistent troubleshooting
[SettingsService:DEBUG] _initialize_paths: config_dir=/home/user/.config/ramsleuth
[SettingsService:DEBUG] load_settings: successfully loaded 3 settings
[SettingsService:DEBUG] set_setting: changed 'theme' from dark to light
```

#### 5. Backward Compatibility Layer
```python
# Existing API maintained for gradual migration
def load_config(debug: bool = False) -> SettingsDict:
    """Backward compatibility function returning settings dictionary."""
    settings = SettingsService(debug=debug)
    return settings.get_all_settings()

def save_config(config: SettingsDict, debug: bool = False) -> None:
    """Backward compatibility function saving configuration dictionary."""
    settings = SettingsService(debug=debug)
    # Validation and save logic

def get_default_settings(debug: bool = False) -> SettingsService:
    """Module-level singleton instance for convenience."""
    # Lazy initialization with global instance caching
```

## Implementation Details

### Configuration File Structure
```json
{
  "theme": "dark",
  "active_tab": "summary_tab", 
  "default_view_tab": "summary",
  "custom_user_setting": "user_value"
}
```

### Path Resolution Algorithm
1. Check for `SUDO_USER` environment variable
2. If sudo detected, resolve original user's home directory via `pwd.getpwnam()`
3. Fall back to `XDG_CONFIG_HOME` environment variable
4. Default to `~/.config/` if XDG not set
5. Append `ramsleuth/` directory and `ramsleuth_config.json` file

### Sudo Context Handling
- **Detection**: `SUDO_USER` environment variable presence
- **Path Resolution**: Use original user's home directory, not root's
- **Ownership Management**: Recursive `chown -R user:user` after file operations
- **Error Isolation**: Ownership failures logged but don't block operations
- **Security**: Never writes to root-owned locations, maintains user permissions

### Settings Loading Process
1. Initialize paths if not already done
2. Check if configuration file exists
3. If missing, initialize with `DEFAULT_SETTINGS`
4. If present, attempt JSON parsing
5. On parse error, log error and use defaults
6. Merge missing default settings into loaded configuration
7. Cache settings in memory for fast access

### Settings Saving Process
1. Validate paths are initialized
2. Create configuration directory with `parents=True`
3. Write JSON with 2-space indentation for readability
4. Handle sudo ownership if `SUDO_USER` is present
5. Log success or failure with detailed error information

## API Documentation

### SettingsService Methods

#### `__init__(debug: bool = False) -> None`
Initialize the settings service with optional debug mode.

**Parameters:**
- `debug`: Enable debug logging for all operations (default: False)

**Behavior:**
- Initializes path resolution
- Loads existing settings or defaults
- Sets up debug logging infrastructure

**Example:**
```python
settings = SettingsService(debug=True)
# [SettingsService:DEBUG] _initialize_paths: config_dir=/home/user/.config/ramsleuth
# [SettingsService:DEBUG] _initialize_paths: config_file=/home/user/.config/ramsleuth/ramsleuth_config.json
```

#### `load_settings() -> None`
Load settings from the configuration file with comprehensive error handling.

**Error Handling:**
- Missing file → Initialize defaults
- Invalid JSON → Log error, use defaults
- Permission errors → Log error, use defaults
- OS errors → Log error, use defaults

**Side Effects:**
- Updates `self._settings` with loaded or default values
- Merges missing default settings into existing configuration

**Example:**
```python
settings = SettingsService()
settings.load_settings()  # Called automatically in __init__
```

#### `save_settings() -> None`
Save current settings to configuration file with atomic write patterns.

**Features:**
- Automatic directory creation
- Sudo ownership handling
- JSON formatting with indentation
- Comprehensive error logging

**Side Effects:**
- Writes configuration file to disk
- May change file/directory ownership when using sudo
- Creates parent directories if needed

**Example:**
```python
settings = SettingsService()
settings.set_setting("theme", "light")  # Triggers save_settings()
```

#### `get_setting(key: str, default: Any = None) -> Any`
Retrieve a setting value by key with type validation and debug logging.

**Parameters:**
- `key`: Setting key to retrieve (must be string)
- `default`: Default value if key doesn't exist

**Returns:**
- Setting value if found
- Default value if key missing
- `None` if key missing and no default provided

**Validation:**
- Key must be string type
- Non-string keys log error and return default

**Example:**
```python
settings = SettingsService()
theme = settings.get_setting("theme", "dark")
# Returns "dark" if theme not set, actual value otherwise

custom = settings.get_setting("custom_key")
# Returns None if custom_key doesn't exist
```

#### `set_setting(key: str, value: Any) -> bool`
Set a setting value, validate it, and save immediately.

**Parameters:**
- `key`: Setting key to set (must be string)
- `value`: Value to set (validated if rule exists)

**Returns:**
- `True` if setting was successfully set and saved
- `False` if validation failed or save error occurred

**Validation:**
- Key must be string type
- Value validated against `VALIDATION_RULES` if rule exists
- Custom settings without rules are automatically accepted

**Persistence:**
- Immediately saves to disk after successful validation
- Debug logs the change with old and new values

**Example:**
```python
settings = SettingsService()

# Valid setting
success = settings.set_setting("theme", "light")
print(success)  # True
print(settings.get_setting("theme"))  # "light"

# Invalid setting
success = settings.set_setting("theme", "blue")
print(success)  # False
print(settings.get_setting("theme"))  # Still "light"
```

#### `validate_setting(key: str, value: Any) -> bool`
Validate a setting value against predefined rules.

**Parameters:**
- `key`: Setting key to validate
- `value`: Value to validate

**Returns:**
- `True` if value is valid or no validation rule exists
- `False` if validation rule exists and value fails

**Validation Rules:**
- Rules defined in `VALIDATION_RULES` dictionary
- Lambda functions for simple validation
- Exceptions during validation return `False`

**Example:**
```python
settings = SettingsService()

# Valid values
print(settings.validate_setting("theme", "dark"))   # True
print(settings.validate_setting("theme", "light"))  # True

# Invalid value
print(settings.validate_setting("theme", "blue"))   # False

# No validation rule (automatically valid)
print(settings.validate_setting("custom_key", "any_value"))  # True
```

#### `get_all_settings() -> Dict[str, Any]`
Return a copy of all current settings.

**Returns:**
- Deep copy of current settings dictionary
- Includes defaults, loaded settings, and custom settings

**Protection:**
- Returns copy to prevent external mutation
- Internal settings remain encapsulated

**Example:**
```python
settings = SettingsService()
all_settings = settings.get_all_settings()
print(all_settings)
# {'theme': 'dark', 'active_tab': 'summary_tab', 'default_view_tab': 'summary'}

# Modifying returned dict doesn't affect internal state
all_settings["new_key"] = "new_value"
print(settings.get_setting("new_key"))  # None
```

#### `reset_to_defaults() -> None`
Reset all settings to default values and save immediately.

**Behavior:**
- Overwrites all current settings with `DEFAULT_SETTINGS`
- Immediately persists to disk
- Debug logs the reset operation

**Use Cases:**
- Factory reset functionality
- Testing and development
- Recovery from corrupted settings

**Example:**
```python
settings = SettingsService()
settings.set_setting("theme", "light")
print(settings.get_setting("theme"))  # "light"

settings.reset_to_defaults()
print(settings.get_setting("theme"))  # "dark"
```

### Backward Compatibility Functions

#### `load_config(debug: bool = False) -> SettingsDict`
Backward compatibility function for existing code.

**Parameters:**
- `debug`: Enable debug logging

**Returns:**
- Dictionary containing all configuration data

**Implementation:**
- Creates `SettingsService` instance internally
- Returns `get_all_settings()` result
- Maintains exact API compatibility

**Migration Path:**
```python
# Old code
from ramsleuth import load_config
config = load_config()
theme = config.get("theme", "dark")

# New code
from settings_service import SettingsService
settings = SettingsService()
theme = settings.get_setting("theme", "dark")
```

#### `save_config(config: SettingsDict, debug: bool = False) -> None`
Backward compatibility function for existing code.

**Parameters:**
- `config`: Dictionary containing configuration to save
- `debug`: Enable debug logging

**Behavior:**
- Creates `SettingsService` instance internally
- Validates each configuration item
- Saves valid settings, skips invalid ones
- Maintains API compatibility with original function

**Migration Path:**
```python
# Old code
from ramsleuth import save_config
config = {"theme": "light"}
save_config(config)

# New code
from settings_service import SettingsService
settings = SettingsService()
settings.set_setting("theme", "light")
```

#### `get_default_settings(debug: bool = False) -> SettingsService`
Module-level singleton instance for convenience.

**Parameters:**
- `debug`: Enable debug logging

**Returns:**
- Singleton `SettingsService` instance

**Behavior:**
- Lazy initialization on first call
- Global instance caching
- Independent of directly created instances

**Example:**
```python
# Get singleton instance
settings1 = get_default_settings()
settings2 = get_default_settings()

# Same instance
print(settings1 is settings2)  # True

# Independent instance
settings3 = SettingsService()
print(settings1 is settings3)  # False
```

## Testing Strategy

### Unit Test Coverage

#### Path Resolution Tests
- **XDG Compliance**: Verify `XDG_CONFIG_HOME` environment variable usage
- **Sudo Context**: Test `SUDO_USER` detection and original user path resolution
- **Fallback Paths**: Verify graceful degradation when XDG resolution fails
- **Path Caching**: Ensure paths are resolved only once per instance

#### Settings Load/Save Tests
- **Default Initialization**: Verify defaults loaded when config missing
- **File Reading**: Test valid JSON loading and settings merging
- **Error Recovery**: Verify graceful handling of invalid JSON, permissions errors
- **Directory Creation**: Test automatic parent directory creation
- **Atomic Writes**: Verify file operations are atomic and consistent

#### Validation Tests
- **Valid Values**: Confirm all valid enum values pass validation
- **Invalid Values**: Verify invalid values are rejected with appropriate logging
- **Custom Settings**: Test settings without validation rules are accepted
- **Type Safety**: Verify type checking prevents common errors

#### Sudo Handling Tests
- **Ownership Changes**: Test recursive `chown` execution
- **Error Isolation**: Verify `chown` failures don't block operations
- **Security**: Confirm no root-owned file creation
- **Cross-Platform**: Handle platforms without `chown` gracefully

#### Integration Tests
- **Theme Persistence**: Verify theme changes persist across application restarts
- **Tab State**: Test active tab persistence between sessions
- **Sudo Context**: Test correct behavior when running with elevated privileges
- **Backward Compatibility**: Verify old config files work with new service

### Test Utilities
- **Temporary Directories**: Isolated test environments
- **Environment Mocking**: Controlled `XDG_CONFIG_HOME` and `SUDO_USER` values
- **Debug Output Capture**: Verification of logging behavior
- **Permission Testing**: Simulated permission denied scenarios

## Integration Patterns

### Direct SettingsService Usage
```python
from settings_service import SettingsService

# Initialize with debug logging
settings = SettingsService(debug=True)

# Get settings with defaults
theme = settings.get_setting("theme", "dark")
active_tab = settings.get_setting("active_tab", "summary_tab")

# Set and persist settings
if settings.set_setting("theme", "light"):
    print("Theme updated successfully")
else:
    print("Failed to update theme")
```

### Backward Compatibility Layer
```python
from settings_service import load_config, save_config

# Existing code continues to work
config = load_config(debug=True)
theme = config.get("theme", "dark")
config["theme"] = "light"
save_config(config, debug=True)
```

### Singleton Pattern
```python
from settings_service import get_default_settings

# Module-level singleton
settings = get_default_settings(debug=False)

# Use throughout application
theme = settings.get_setting("theme")
settings.set_setting("active_tab", "full_tab")
```

### Error Handling Pattern
```python
from settings_service import SettingsService

settings = SettingsService(debug=True)

# All operations are safe and won't crash
try:
    # Validation prevents invalid values
    if not settings.set_setting("theme", "invalid"):
        print("Invalid theme value")
    
    # Graceful fallback on missing settings
    value = settings.get_setting("nonexistent", "default")
    
except Exception as e:
    # Even unexpected errors are caught and logged internally
    print(f"Settings operation failed: {e}")
```

## Migration Guide

### Phase 1: SettingsService Implementation (Complete)
- ✅ Created [`settings_service.py`](settings_service.py) with full functionality
- ✅ Implemented comprehensive validation and error handling
- ✅ Added complete debug logging system
- ✅ Created extensive unit test suite

### Phase 2: Integration (In Progress)
- ✅ Backward compatibility functions implemented
- ⏳ Update [`ramsleuth.py`](ramsleuth.py) to use `SettingsService` internally
- ⏳ Replace direct config dictionary manipulation with service calls
- ⏳ Maintain existing debug logging patterns

### Phase 3: Testing and Validation (In Progress)
- ✅ Unit tests for all `SettingsService` functionality
- ✅ Integration tests for backward compatibility
- ⏳ End-to-end testing with full application
- ⏳ Sudo context testing in real environments

### Migration Example

**Before Migration:**
```python
# ramsleuth.py current implementation (monolithic)
from ramsleuth import load_config, save_config

def some_function():
    config = load_config()
    theme = config.get("theme", "dark")
    config["theme"] = "light"
    save_config(config)
```

**After Migration:**
```python
# Implementation within the new package structure (e.g., ramsleuth_pkg/tui.py or ramsleuth.py)
from settings_service import SettingsService

def some_function():
    settings = SettingsService(debug=DEBUG)
    theme = settings.get_setting("theme", "dark")
    settings.set_setting("theme", "light")
```

## Advanced Usage Patterns

### Batch Settings Updates
```python
from settings_service import SettingsService

settings = SettingsService()

# Update multiple settings with single save
updates = {
    "theme": "dark",
    "active_tab": "full_tab", 
    "default_view_tab": "full"
}

all_success = True
for key, value in updates.items():
    if not settings.set_setting(key, value):
        all_success = False
        print(f"Failed to update {key}")

if all_success:
    print("All settings updated successfully")
```

### Settings Validation UI
```python
from settings_service import SettingsService

settings = SettingsService()

def validate_user_input(key: str, value: str) -> tuple[bool, str]:
    """Validate user input and return (is_valid, error_message)."""
    if not settings.validate_setting(key, value):
        valid_values = {
            "theme": ["dark", "light"],
            "active_tab": ["summary_tab", "full_tab"],
            "default_view_tab": ["summary", "full"]
        }
        allowed = valid_values.get(key, [])
        return False, f"Invalid value. Allowed values: {allowed}"
    
    return True, "Valid setting"

# Usage in UI
is_valid, message = validate_user_input("theme", user_input)
if not is_valid:
    display_error(message)
else:
    settings.set_setting("theme", user_input)
```

### Settings Backup and Restore
```python
from settings_service import SettingsService
import json
from pathlib import Path

def backup_settings(backup_path: Path) -> bool:
    """Backup current settings to specified file."""
    try:
        settings = SettingsService()
        all_settings = settings.get_all_settings()
        
        with open(backup_path, 'w', encoding='utf-8') as f:
            json.dump(all_settings, f, indent=2)
        
        return True
    except Exception as e:
        print(f"Backup failed: {e}")
        return False

def restore_settings(backup_path: Path) -> bool:
    """Restore settings from backup file."""
    try:
        with open(backup_path, 'r', encoding='utf-8') as f:
            backup_settings = json.load(f)
        
        settings = SettingsService()
        for key, value in backup_settings.items():
            if not settings.set_setting(key, value):
                print(f"Warning: Failed to restore {key}")
        
        return True
    except Exception as e:
        print(f"Restore failed: {e}")
        return False
```

## Performance Considerations

### Optimizations Implemented
1. **Path Caching**: Configuration paths resolved once per instance
2. **Lazy Loading**: Settings loaded on first access, not import
3. **In-Memory Cache**: Settings cached in `_settings` dictionary
4. **Defensive Copies**: `get_all_settings()` returns copy to prevent mutation
5. **Atomic Operations**: Each `set_setting()` triggers individual save

### Performance Characteristics
- **Path Resolution**: O(1) after first initialization
- **Setting Access**: O(1) dictionary lookup
- **Setting Update**: O(n) where n is file I/O cost (acceptable for config files)
- **Memory Usage**: O(m) where m is number of settings (typically < 100)

### Scalability Considerations
- Suitable for applications with hundreds of settings
- File I/O is acceptable for configuration (not high-frequency data)
- Singleton pattern reduces memory footprint
- Debug logging can be disabled in production

## Security Considerations

### File Permissions
- Configuration directory created with default umask permissions
- JSON files contain no sensitive data (themes, UI state)
- Sudo context ensures user-owned files, not root-owned

### Input Validation
- All setting values validated against allowed values
- Type checking prevents injection attacks
- Custom settings accepted but logged for audit

### Error Isolation
- Path information sanitized from error messages
- Debug logs go to stderr, not exposed to users
- Exception handlers prevent information leakage

### Sudo Safety
- Never writes to root-owned locations
- Maintains original user permissions
- Recursive ownership changes isolated from main logic

## Future Enhancements

### Planned Features
1. **Settings Categories**: Group settings by functionality (UI, hardware, debug)
2. **Configuration Schema**: JSON Schema validation for complex settings
3. **Settings UI**: Text-based UI for interactive settings management
4. **Configuration Profiles**: Support multiple named configuration sets
5. **Migration System**: Handle configuration version upgrades gracefully

### Extension Points
- **Custom Validation**: Add new validation rules to `VALIDATION_RULES`
- **Default Settings**: Extend `DEFAULT_SETTINGS` with new configuration options
- **Debug Logging**: Enhance `_debug_print()` with structured logging
- **Error Handling**: Add custom exception types for specific error conditions

## Conclusion

The `SettingsService` class provides a robust, maintainable, and testable solution for configuration management in RamSleuth. It successfully:

- ✅ Maintains full backward compatibility with existing code
- ✅ Provides comprehensive error handling and logging
- ✅ Implements type-safe, validated settings management
- ✅ Handles complex deployment scenarios (sudo, XDG paths)
- ✅ Enables extensive testing and quality assurance
- ✅ Provides foundation for future enhancements

The service represents a significant improvement in code organization, maintainability, and reliability while preserving the simplicity of the original API for existing users.