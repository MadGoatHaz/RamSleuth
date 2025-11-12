# SettingsService Integration Guide

This guide provides step-by-step instructions for integrating the new `SettingsService` class into the RamSleuth project.

## Overview

The `SettingsService` class replaces the existing `load_config()` and `save_config()` functions with a more maintainable and testable solution. This guide covers:

1. Basic integration steps
2. Code examples for common use cases
3. Migration patterns for existing code
4. Testing recommendations
5. Advanced features and troubleshooting

## Quick Start

### 1. Import the SettingsService

```python
from settings_service import SettingsService

# Create a settings service instance
settings = SettingsService(debug=DEBUG)
```

### 2. Basic Usage Patterns

**Getting a setting:**
```python
# Old way
config = load_config()
theme = config.get("theme", "dark")

# New way
settings = SettingsService(debug=DEBUG)
theme = settings.get_setting("theme", "dark")
```

**Setting a value:**
```python
# Old way
config = load_config()
config["theme"] = "light"
save_config(config)

# New way
settings = SettingsService(debug=DEBUG)
settings.set_setting("theme", "light")  # Automatically saves
```

## Integration Examples

### Example 1: Theme Management in TUI

**Current code in [`ramsleuth.py`](ramsleuth.py:1937):**
```python
def action_toggle_dark(self) -> None:
    """
    Toggle dark mode and save the setting.
    
    This method is bound to Ctrl+T and provides the primary theme toggle
    functionality. It switches between dark and light themes and immediately
    persists the new setting to the config file.
    """
    try:
        # Determine the new theme
        current_theme = self.app.theme
        new_theme = "light" if current_theme == "dark" else "dark"
        
        _debug_print(f"action_toggle_dark: Current theme is '{current_theme}', toggling to '{new_theme}'")
        
        # Apply the new theme
        self.app.theme = new_theme
        self.dark = new_theme == "dark"
        self.refresh_css()
        
        _debug_print(f"action_toggle_dark: Theme changed to: {new_theme}")
        
        # Load the config, update it, and save it
        config = load_config()
        config["theme"] = new_theme
        save_config(config)
        
        _debug_print(f"action_toggle_dark: Theme saved to config successfully")
        
    except Exception as e:
        _debug_print(f"action_toggle_dark: Error toggling theme: {e}")
        import traceback
        _debug_print(f"action_toggle_dark: traceback: {traceback.format_exc()}")
        print(f"Error: Failed to toggle theme: {e}", file=sys.stderr)
```

**Enhanced version with SettingsService:**
```python
def action_toggle_dark(self) -> None:
    """Set dark theme from command palette."""
    self.dark = True
    self.refresh_css()
    _debug_print("Theme changed to: dark")
    
    # Save the new state using settings service
    self.settings.set_setting("theme", "dark")
```

**Note:** You'll need to pass the settings service instance to the app:
```python
class RamSleuthApp(App):
    def __init__(self, dimms_data, raw_data, settings, initial_theme="dark"):
        super().__init__()
        self.dimms_data = dimms_data
        self.raw_data = raw_data
        self.settings = settings  # Store settings service instance
        self.active_tab = "summary"
        self.initial_theme = initial_theme
```

### Example 2: Tab State Persistence

**Current code in [`ramsleuth.py`](ramsleuth.py:2158):**
```python
def on_tabs_tab_activated(self, event: Tabs.TabActivated) -> None:
    """Handle tab activation events"""
    self._update_pane_visibility()
    
    # Save the active tab to config
    config = load_config()
    config["active_tab"] = event.tab.id
    save_config(config)
    _debug_print(f"on_tabs_tab_activated: saved active_tab={event.tab.id}")
```

**New code:**
```python
def on_tabs_tab_activated(self, event: Tabs.TabActivated) -> None:
    """Handle tab activation events"""
    self._update_pane_visibility()
    
    # Save the active tab to config
    self.settings.set_setting("active_tab", event.tab.id)
    _debug_print(f"on_tabs_tab_activated: saved active_tab={event.tab.id}")
```

### Example 3: Initial Configuration Load

**Current code in [`ramsleuth.py`](ramsleuth.py:1710):**
```python
# Load configuration and determine initial theme
config = load_config()
initial_theme = config.get("theme", "dark")  # Default to 'dark'
_debug_print(f"launch_tui: initial_theme={initial_theme}")
```

**New code:**
```python
# Load configuration and determine initial theme
settings = SettingsService(debug=DEBUG)
initial_theme = settings.get_setting("theme", "dark")  # Default to 'dark'
_debug_print(f"launch_tui: initial_theme={initial_theme}")
```

### Example 4: Automatic Theme Persistence with Watchers

**Current implementation in [`ramsleuth.py`](ramsleuth.py:1982):**
```python
def watch_dark(self, dark: bool) -> None:
    """
    Watches for changes to the dark mode setting and persists them.
    
    This method is automatically called by Textual whenever the dark property
    changes, ensuring that ALL theme changes (from any source) are captured
    and persisted to the config file.
    """
    new_theme = "dark" if dark else "light"
    _debug_print(f"watch_dark: Theme changed to: {new_theme}, saving to config.")
    config = load_config()
    config["theme"] = new_theme
    save_config(config)
```

**Enhanced version with SettingsService:**
```python
def watch_dark(self, dark: bool) -> None:
    """Watch for dark mode changes and persist automatically."""
    new_theme = "dark" if dark else "light"
    _debug_print(f"watch_dark: Theme changed to: {new_theme}")
    self.settings.set_setting("theme", new_theme)
```

### Example 5: Current Memory Settings Integration

**Current implementation in [`ramsleuth.py`](ramsleuth.py:1888):**
```python
# Get current settings with SPD output and DIMM data for XMP extraction
current_settings = get_current_memory_settings(spd_output=spd_output, dimms_data=self.dimms_data)
```

This function integrates with the SettingsService for configuration management while providing enhanced system information.

## Integration Steps

### Step 1: Add SettingsService to Main Function

In the main function, create a single `SettingsService` instance and pass it to components that need it:

```python
def main() -> None:
    # ... existing setup code ...
    
    # Initialize settings service once
    settings = SettingsService(debug=DEBUG)
    
    # Pass settings to TUI launch
    if args.tui or (not non_interactive and dimms):
        launch_tui(dimms, raw_individual, settings)
    
    # ... rest of main function ...
```

### Step 2: Update launch_tui Function

Modify the `launch_tui` function to accept and use the settings service:

```python
def launch_tui(dimms: List[Dict[str, Any]], raw_individual: Dict[str, str], 
               settings: SettingsService) -> None:
    """
    Launch a Textual-based TUI for interactive DIMM exploration.
    
    Args:
        dimms: List of DIMM data dictionaries
        raw_individual: Raw decode-dimms output blocks
        settings: SettingsService instance for configuration management
    """
    # ... existing code ...
    
    # Use settings service instead of load_config
    initial_theme = settings.get_setting("theme", "dark")
    _debug_print(f"launch_tui: initial_theme={initial_theme}")
    
    # ... rest of function ...
```

### Step 3: Update RamSleuthApp Class

Modify the `RamSleuthApp` class to use the settings service:

```python
class RamSleuthApp(App):
    def __init__(
        self,
        dimms_data: List[Dict[str, Any]],
        raw_data: Dict[str, str],
        settings: SettingsService,
        initial_theme: str = "dark",
    ) -> None:
        super().__init__()
        self.dimms_data = dimms_data
        self.raw_data = raw_data
        self.settings = settings  # Store settings service
        self.active_tab = "summary"
        self.initial_theme = initial_theme

    def on_mount(self) -> None:
        # Apply the initial theme
        self.dark = self.initial_theme == "dark"
        self.refresh_css()
        _debug_print(f"RamSleuthApp.on_mount: applied initial_theme={self.initial_theme}, dark={self.dark}")
        
        # Load configuration and restore active tab using settings service
        saved_active_tab = self.settings.get_setting("active_tab", "summary_tab")
        _debug_print(f"on_mount: loaded saved active_tab={saved_active_tab}")
        
        # ... rest of method ...
```

### Step 4: Update Theme Action Methods

Replace all theme-related config operations with settings service calls:

```python
def action_set_theme_dark(self) -> None:
    """Set dark theme from command palette."""
    self.dark = True
    self.refresh_css()
    _debug_print("Theme changed to: dark")
    
    # Save the new state using settings service
    self.settings.set_setting("theme", "dark")

def action_set_theme_light(self) -> None:
    """Set light theme from command palette."""
    self.dark = False
    self.refresh_css()
    _debug_print("Theme changed to: light")
    
    # Save the new state using settings service
    self.settings.set_setting("theme", "light")

def action_toggle_dark(self) -> None:
    """Toggle dark mode and save the setting."""
    super().action_toggle_dark()  # Let Textual do the work
    
    # Now, save the new state using settings service
    new_theme = "dark" if self.dark else "light"
    _debug_print(f"Theme changed to: {new_theme}")
    self.settings.set_setting("theme", new_theme)
```

### Step 5: Integrate with Dependency Engine

The SettingsService works seamlessly with the dependency engine for configuration management:

```python
from dependency_engine import check_and_install_dependencies
from settings_service import SettingsService

def main() -> None:
    # Initialize settings service
    settings = SettingsService(debug=DEBUG)
    
    # Check dependencies with settings-aware features
    requested_features = {
        "core": True,
        "tui": True,  # Will need textual and related packages
    }
    
    check_and_install_dependencies(
        requested_features=requested_features,
        interactive=True,
    )
    
    # ... rest of main function uses settings service
```

### Step 6: Add Current Memory Settings Integration

Integrate the enhanced settings system with current memory monitoring:

```python
def on_mount(self) -> None:
    # ... existing theme setup ...
    
    # Populate current memory settings pane
    spd_output = self.raw_data.get("dimm_0", "") if self.raw_data else ""
    current_settings = get_current_memory_settings(
        spd_output=spd_output, 
        dimms_data=self.dimms_data
    )
    
    # Display settings with proper formatting
    settings_pane = self.query_one("#current_settings_pane", Static)
    settings_text = self._format_current_settings(current_settings)
    settings_pane.update(settings_text)
```

## Backward Compatibility

The `SettingsService` includes backward compatibility functions that maintain the existing `load_config()` and `save_config()` API:

```python
from settings_service import load_config, save_config

# These still work exactly as before
config = load_config(debug=DEBUG)
config["theme"] = "light"
save_config(config, debug=DEBUG)
```

This means you can migrate incrementally - some parts of the code can use the new `SettingsService` class while others continue using the old functions.

## Advanced Usage

### Batch Settings Updates

If you need to update multiple settings at once:

```python
# Update multiple settings efficiently
settings.set_setting("theme", "dark")
settings.set_setting("active_tab", "full_tab")
settings.set_setting("default_view_tab", "full")
```

### Settings Validation

The service automatically validates settings:

```python
# This will be rejected (invalid theme value)
settings.set_setting("theme", "blue")  # Debug log: validation failed

# This will be accepted
settings.set_setting("theme", "dark")  # Debug log: successfully set
```

### Getting All Settings

```python
# Get a copy of all current settings
all_settings = settings.get_all_settings()
print(f"Current settings: {all_settings}")
```

### Resetting to Defaults

```python
# Reset all settings to defaults (use with caution!)
settings.reset_to_defaults()
```

### Custom Theme Support

The enhanced system supports custom Textual themes beyond basic dark/light:

```python
def action_set_theme(self, theme: str) -> None:
    """
    Set theme from command palette or other sources.
    Supports both basic themes ("dark", "light") and custom themes.
    """
    # Validate theme against available themes
    available_themes = self.get_available_themes()
    
    if theme not in available_themes and theme not in ["dark", "light"]:
        print(f"Error: Theme '{theme}' is not available.", file=sys.stderr)
        theme = "dark"  # Fallback
    
    # Apply the theme
    self.app.theme = theme
    self.dark = "dark" in theme.lower()
    self.refresh_css()
    
    # Save using settings service
    self.settings.set_setting("theme", theme)
```

## Testing Integration

### Unit Test Example

```python
import tempfile
import os
from settings_service import SettingsService

def test_settings_service():
    # Create a temporary directory for testing
    with tempfile.TemporaryDirectory() as temp_dir:
        # Mock XDG config home
        os.environ['XDG_CONFIG_HOME'] = temp_dir
        
        # Create settings service
        settings = SettingsService(debug=True)
        
        # Test default values
        assert settings.get_setting("theme") == "dark"
        
        # Test setting and persistence
        settings.set_setting("theme", "light")
        assert settings.get_setting("theme") == "light"
        
        # Create new instance to test persistence
        settings2 = SettingsService(debug=True)
        assert settings2.get_setting("theme") == "light"
```

### Integration Test Example

```python
def test_integration_with_ramsleuth():
    from ramsleuth import launch_tui
    from settings_service import SettingsService
    
    # Create settings service
    settings = SettingsService(debug=True)
    
    # Mock DIMM data
    dimms = [...]  # Your test data
    raw_individual = {...}  # Your test data
    
    # Launch TUI with settings service
    launch_tui(dimms, raw_individual, settings)
```

### Dependency Engine Integration Test

```python
def test_dependency_settings_integration():
    from settings_service import SettingsService
    from dependency_engine import check_and_install_dependencies
    
    # Test with settings service
    settings = SettingsService(debug=True)
    
    # Verify settings work after dependency check
    check_and_install_dependencies(
        requested_features={"core": True, "tui": True},
        interactive=False
    )
    
    # Settings should still function correctly
    settings.set_setting("theme", "dark")
    assert settings.get_setting("theme") == "dark"
```

## Troubleshooting

### Common Issues

**Issue: Settings not persisting**
- Check that `save_settings()` is being called (automatic with `set_setting()`)
- Verify file permissions in config directory
- Enable debug mode to see detailed logging
- Check sudo context and file ownership

**Issue: Invalid setting values**
- Check validation rules in `VALIDATION_RULES`
- Use `validate_setting()` method to test values
- Enable debug mode to see validation failures
- Ensure you're using valid enum values (e.g., "dark"/"light" for theme)

**Issue: Sudo context problems**
- Verify `SUDO_USER` environment variable is set
- Check that chown operations are succeeding (check debug logs)
- Ensure original user has write permissions to config directory
- SettingsService automatically handles sudo context - verify it's being used

**Issue: Theme changes not applying**
- Check that `watch_dark` and `watch_theme` methods are properly defined
- Verify Textual theme system is working correctly
- Ensure theme names are valid (check `get_available_themes()`)
- Debug theme change flow with print statements

**Issue: Integration with dependency engine fails**
- Ensure SettingsService is initialized before dependency checks
- Verify no circular imports between modules
- Check that config directory is accessible after dependency installation
- Test with both interactive and non-interactive modes

**Issue: Current memory settings not displaying**
- Verify `dmidecode` is available and working
- Check that SPD output is being passed correctly
- Ensure `get_current_memory_settings` is called with proper parameters
- Debug the settings parsing logic

### Debug Mode

Enable debug mode to see detailed logging:

```python
settings = SettingsService(debug=True)
settings.set_setting("theme", "light")
# Will print: [SettingsService:DEBUG] set_setting: changed 'theme' from 'dark' to 'light'
# Will print: [SettingsService:DEBUG] save_settings: successfully saved config to /path/to/config
```

### Configuration File Location

The SettingsService uses XDG Base Directory specification:
- Normal execution: `~/.config/ramsleuth/ramsleuth_config.json`
- Sudo execution: Uses original user's home directory, not root's
- Custom XDG_CONFIG_HOME: Respects the environment variable

### Validation Rules

Current validation rules include:
- `theme`: Must be "dark" or "light" (or valid custom theme name)
- `active_tab`: Must be "summary_tab" or "full_tab"
- `default_view_tab`: Must be "summary" or "full"

Add custom validation:
```python
# Add custom validation rule
SettingsService.VALIDATION_RULES["custom_setting"] = lambda x: x in ["option1", "option2"]
```

## Migration Checklist

- [ ] Import `SettingsService` in [`ramsleuth.py`](ramsleuth.py)
- [ ] Create settings service instance in `main()` function
- [ ] Update `launch_tui()` to accept settings service parameter
- [ ] Update `RamSleuthApp.__init__()` to store settings service
- [ ] Replace all `load_config()` calls with `settings.get_setting()`
- [ ] Replace all `save_config()` patterns with `settings.set_setting()`
- [ ] Update theme action methods to use settings service
- [ ] Update tab persistence logic to use settings service
- [ ] Add `watch_dark` and `watch_theme` methods for automatic persistence
- [ ] Integrate with `get_current_memory_settings()` for enhanced system info
- [ ] Test theme changes persist across runs
- [ ] Test tab state persists across runs
- [ ] Test sudo context handling
- [ ] Verify backward compatibility functions still work
- [ ] Test dependency engine integration
- [ ] Validate custom theme support
- [ ] Test current memory settings display
- [ ] Verify pane focus management works with settings

## Summary

The `SettingsService` provides a clean, maintainable API for configuration management while maintaining full backward compatibility. Key benefits include:

- **Centralized configuration management** with XDG compliance
- **Automatic persistence** on setting changes
- **Sudo-aware path resolution** and file ownership handling
- **Built-in validation** with clear error handling
- **Enhanced theme support** including custom themes
- **Integration with dependency engine** for seamless operation
- **Current memory settings integration** for enhanced system monitoring
- **Comprehensive debug logging** for troubleshooting

The integration is straightforward and can be done incrementally, allowing you to migrate one component at a time while ensuring the application continues to work correctly. The enhanced features like automatic theme persistence, custom theme support, and current memory settings integration provide significant improvements to the user experience.