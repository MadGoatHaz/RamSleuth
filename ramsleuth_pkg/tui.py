import sys
import re
import json
from typing import Any, Dict, List
from settings_service import SettingsService
from .utils import _debug_print
from .scanner import perform_system_scan, get_current_memory_settings


def output_summary(dimms: List[Dict[str, Any]]) -> None:
    """
    Print a concise, one-line summary per DIMM.

    Fields:
    - Slot/index
    - Generation
    - Capacity (module_gb)
    - Manufacturer
    - Part Number
    - Die Type
    - Notes (if present)
    """
    if not dimms:
        print("No DIMMs detected.")
        return

    for idx, dimm in enumerate(dimms):
        slot = dimm.get("slot", f"DIMM_{idx}")
        generation = dimm.get("generation", "?")
        module_gb = dimm.get("module_gb", "?")
        manufacturer = dimm.get("manufacturer", "?")
        part = dimm.get("module_part_number", dimm.get("Part Number", "?"))
        die_type = dimm.get("die_type", "Unknown")
        timings = dimm.get("timings", "")
        notes = dimm.get("notes")

        line = (
            f"{slot}: {generation} {module_gb}GB {manufacturer} {part} -> {die_type}"
        )
        if timings:
            line += f" [{timings}]"
        if notes:
            line += f" [{notes}]"
        print(line)


def output_full(dimms: List[Dict[str, Any]], raw_individual: Dict[str, str]) -> None:
    """
    Print detailed information for each DIMM, including raw decoder block.

    Behavior:
    - For each DIMM with index i:
        - Print header with slot/index and die_type.
        - Print key attributes.
        - If `dimm_i` exists in raw_individual, print its raw block.
    """
    if not dimms:
        print("No DIMMs detected.")
        return

    for idx, dimm in enumerate(dimms):
        slot = dimm.get("slot", f"DIMM_{idx}")
        die_type = dimm.get("die_type", "Unknown")

        print("=" * 60)
        print(f"{slot} :: Die Type: {die_type}")
        notes = dimm.get("notes")
        if notes:
            print(f"Notes: {notes}")
        print("-" * 60)

        # Key attributes
        keys_of_interest = [
            "generation",
            "module_gb",
            "manufacturer",
            "module_part_number",
            "dram_mfg",
            "module_ranks",
            "chip_org",
            "timings_xmp",
            "timings_jdec",
            "timings",
            "voltage_xmp",
            "configured_speed",
            "configured_voltage",
            "min_voltage",
            "max_voltage",
            "corsair_version",
            "gskill_sticker_code",
            "crucial_sticker_suffix",
            "hynix_ic_part_number",
        ]
        for k in keys_of_interest:
            if k in dimm:
                print(f"{k}: {dimm[k]}")

        # Raw decode-dimms block if available
        raw_key = f"dimm_{idx}"
        if raw_key in raw_individual:
            print("-" * 60)
            print(raw_individual[raw_key].rstrip())
        print()


def output_json(dimms: List[Dict[str, Any]]) -> None:
    """
    Emit DIMM data as JSON to stdout.

    Requirements:
    - No extra logging or messages to stdout.
    """
    print(json.dumps(dimms, indent=2))


def launch_tui(dimms: List[Dict[str, Any]], raw_individual: Dict[str, str]) -> None:
    """
    Launch a Textual-based TUI for interactive DIMM exploration.

    Functional layout (spec-aligned, not pixel-perfect):
    - Header:
        "RamSleuth - RAM SPD Inspector"
    - Body:
        - Left panel:
            * List of DIMMs:
                slot/index, manufacturer, module_part_number, die_type.
        - Right panel:
            * Summary view for selected DIMM:
                Generation, Module Manufacturer, Part Number, DRAM Manufacturer,
                Capacity (module_gb), Ranks, Chip org, JEDEC voltage,
                Die Type, Notes, DDR5 extras (PMIC, Hynix IC PN).
            * Full dump view:
                Summary data + raw decode-dimms block.
    - Footer:
        Keybind hints:
            Up/Down or j/k to change selection.
            s/f to toggle Summary/Full.
            q to quit.

    If Textual is missing:
    - Print clear message to stderr.
    - Fallback to output_summary(dimms) instead of crashing.
    """
    try:
        from textual.app import App, ComposeResult
        from textual.containers import Horizontal, VerticalScroll, ScrollableContainer, Container
        from textual.widgets import (
            Header,
            Footer,
            DataTable,
            Static,
            Tabs,
            Tab,
            Button,
            Label,
        )
        from textual.message import Message
        from textual.command import CommandPalette, Provider
        from textual import work
    except ImportError:
        print(
            "Warning: Textual is not installed; falling back to summary output.",
            file=sys.stderr,
        )
        output_summary(dimms)
        return

    # Load configuration and determine initial theme
    # Initialize SettingsService
    settings_service = SettingsService()
    initial_theme = settings_service.get_setting("theme", "dark")  # Default to 'dark'
    _debug_print(f"launch_tui: initial_theme={initial_theme}")

    class DIMMCard(Static):
        """A clickable card widget representing a single DIMM slot."""
        
        class Selected(Message):
            """Message sent when the card is clicked."""
            def __init__(self, card: "DIMMCard") -> None:
                self.card = card
                super().__init__()

        def __init__(self, dimm_data: Dict[str, Any], index: int) -> None:
            self.dimm_data = dimm_data
            self.index = index
            self.selected = False
            super().__init__()
            
        def compose(self) -> ComposeResult:
            slot = self.dimm_data.get("slot", f"DIMM_{self.index}")
            mfg = self.dimm_data.get("manufacturer", "?")
            part = self.dimm_data.get("module_part_number", self.dimm_data.get("Part Number", "?"))
            die = self.dimm_data.get("die_type", "Unknown")
            
            yield Label(f"{slot}", classes="dimm-slot")
            yield Label(f"{mfg}", classes="dimm-mfg")
            yield Label(f"{part}", classes="dimm-part")
            if die != "Unknown":
                yield Label(f" Die: {die}", classes="dimm-die")
                
        def on_click(self) -> None:
            self.post_message(self.Selected(self))
            
        def set_selected(self, selected: bool) -> None:
            self.selected = selected
            if selected:
                self.add_class("selected")
            else:
                self.remove_class("selected")

    class RamSleuthApp(App):
        CSS = """
        Screen {
            layout: vertical;
        }
        DIMMCard {
            layout: vertical;
            background: $panel;
            border: solid $primary;
            margin: 1;
            padding: 1;
            height: auto;
            min-height: 8;
        }
        DIMMCard.selected {
            background: $accent;
            border: double $secondary;
        }
        .dimm-slot {
            text-style: bold;
            color: $text;
        }
        .dimm-mfg {
            color: $text-muted;
        }
        
        #toolbar {
            height: 3;
            dock: top;
            padding: 0 1;
            align-vertical: middle;
        }
        #toolbar Button {
            margin-right: 1;
        }
        #body {
            height: 1fr;
            layout: grid;
            grid-size: 2;
            grid-columns: 1fr 3fr;
        }
        #dimm_selector_container {
            border: solid gray;
        }
        #right_scroll {
            border: solid gray;
        }
        #detail_tabs {
            dock: top;
        }
        #summary_pane, #full_pane {
            padding: 1 1;
            height: 1fr;
        }
        #current_settings_pane {
            height: 8;
            overflow-y: auto;
            border: solid gray;
            padding: 0 1;
        }
        """

        BINDINGS = [
            ("q", "quit", "Quit"),
            ("s,ctrl+s", "show_summary", "Summary"),
            ("f,ctrl+f", "show_full", "Full"),
            ("ctrl+t", "toggle_dark", "Toggle Theme"),
            ("ctrl+p", "command_palette", "Command Palette"),
            ("tab", "toggle_pane_focus", "Toggle Pane Focus"),
            ("up", "cursor_up", "Up"),
            ("down", "cursor_down", "Down"),
            ("j", "cursor_down", "Down"),
            ("k", "cursor_up", "Up"),
        ]

        def __init__(
            self,
            dimms_data: List[Dict[str, Any]],
            raw_data: Dict[str, str],
            initial_theme: str = "dark",
            settings: SettingsService = None
        ) -> None:
            super().__init__()
            self.dimms_data = dimms_data
            self.raw_data = raw_data
            self.active_tab = "summary"
            self.initial_theme = initial_theme
            self.settings = settings or SettingsService()

        def compose(self) -> ComposeResult:  # type: ignore[override]
            yield Header(show_clock=False)
            with Horizontal(id="toolbar"):
                yield Button("Rescan", id="rescan_btn", variant="primary")
                yield Button("Export JSON", id="export_btn", variant="default")
            with Container(id="body"):
                with VerticalScroll(id="dimm_selector_container"):
                    # This will be populated with DIMMCards
                    yield Static("Select a DIMM:", classes="section-title")
                    yield Static(id="current_settings_pane") # Moved settings pane for now
                    # DIMMCards will be added here dynamically
                with ScrollableContainer(id="right_scroll"):
                    yield Tabs(
                        Tab("Summary", id="summary_tab"),
                        Tab("Full", id="full_tab"),
                        id="detail_tabs"
                    )
                    yield Static("", id="summary_pane")
                    yield Static("", id="full_pane")
            yield Footer()

        def on_mount(self) -> None:
            # DEBUG: Check initial theme state
            _debug_print(f"on_mount: Initial self.theme = {getattr(self, 'theme', 'NOT_SET')}")
            _debug_print(f"on_mount: Initial self.app.theme = {getattr(self.app, 'theme', 'NOT_SET')}")
            _debug_print(f"on_mount: Initial dark = {getattr(self, 'dark', 'NOT_SET')}")
            
            # Apply the initial theme - support both old "dark/light" values and new theme names
            self.settings.load_settings()  # Refresh settings from disk
            saved_theme = self.settings.get_setting("theme", "dark")
            _debug_print(f"on_mount: Loading theme from config: '{saved_theme}'")
            
            # Validate theme against available themes
            available_themes = self.get_available_themes()
            _debug_print(f"on_mount: Available Textual themes: {list(available_themes)}")
            
            # Handle both old boolean-style themes and new theme names
            if saved_theme in ["dark", "light"]:
                self.dark = saved_theme == "dark"
                _debug_print(f"on_mount: Applied basic theme '{saved_theme}', dark={self.dark}")
            elif saved_theme in available_themes:
                # New theme name (like "tokyo-night") - let Textual handle it
                self.app.theme = saved_theme
                # Set dark based on theme name (heuristic)
                self.dark = "dark" in saved_theme.lower()
                _debug_print(f"on_mount: Applied custom theme '{saved_theme}', dark={self.dark}")
            else:
                # Theme not found in available themes - use fallback
                _debug_print(f"on_mount: Warning - theme '{saved_theme}' not found in available themes, falling back to 'dark'")
                print(f"Warning: Theme '{saved_theme}' not recognized, using default 'dark' theme", file=sys.stderr)
                self.dark = True
                # Update config to use valid theme
                self.settings.set_setting("theme", "dark")
            
            self.refresh_css()
            _debug_print(f"on_mount: Final theme state - theme={self.app.theme}, dark={self.dark}")
            
            # Load configuration and restore active tab if available
            saved_active_tab = self.settings.get_setting("active_tab", "summary_tab")
            _debug_print(f"on_mount: loaded saved active_tab={saved_active_tab}")
            
            # Populate the DIMM selector list
            self.refresh_dimm_list()
            
            # Initialize the tabs and restore saved active tab
            tabs = self.query_one("#detail_tabs", Tabs)
            tabs.active = saved_active_tab
            self._update_pane_visibility()
            self.update_views(0)
            
            # Populate current memory settings pane
            _debug_print("on_mount: about to call get_current_memory_settings()")
            
            # Get SPD output for JEDEC speed parsing
            spd_output = ""
            if self.raw_data and "dimm_0" in self.raw_data:
                spd_output = self.raw_data["dimm_0"]
                _debug_print(f"on_mount: Found SPD output for dimm_0, length={len(spd_output)}")
            else:
                _debug_print("on_mount: No SPD output found in raw_data")
            
            _debug_print(f"on_mount: dimms_data has {len(self.dimms_data)} entries")
            if self.dimms_data:
                _debug_print(f"on_mount: First dimm data keys: {list(self.dimms_data[0].keys())}")
            
            # Get current settings with SPD output and DIMM data for XMP extraction
            current_settings = get_current_memory_settings(spd_output=spd_output, dimms_data=self.dimms_data)
            _debug_print(f"on_mount: get_current_memory_settings returned {current_settings}")
            settings_pane = self.query_one("#current_settings_pane", Static)
            
            # Build settings text with all available fields
            settings_lines = ["[b]Current Settings (from Memory Controller)[/b]"]
            
            # Always show Size if available
            if "Size" in current_settings and current_settings["Size"] != "N/A":
                settings_lines.append(f"Size:               {current_settings['Size']}")
            
            # Show JEDEC Speed (from SPD/dmidecode)
            if "JEDEC Speed" in current_settings and current_settings["JEDEC Speed"] != "N/A":
                settings_lines.append(f"JEDEC Speed:        {current_settings['JEDEC Speed']}")
            
            # Show XMP Profile Speed (Active)
            if "XMP Profile" in current_settings and current_settings["XMP Profile"] != "N/A":
                settings_lines.append(f"XMP Profile:        {current_settings['XMP Profile']}")
            
            # Show Rated XMP (capabilities)
            if "Rated XMP" in current_settings and current_settings["Rated XMP"] != "N/A":
                settings_lines.append(f"Rated XMP:          {current_settings['Rated XMP']}")
            
            # Show Configured Speed (current actual speed)
            if "Configured Speed" in current_settings and current_settings["Configured Speed"] != "N/A":
                settings_lines.append(f"Configured Speed:   {current_settings['Configured Speed']}")
            
            # Show Manufacturer
            if "Manufacturer" in current_settings and current_settings["Manufacturer"] != "N/A":
                settings_lines.append(f"Manufacturer:       {current_settings['Manufacturer']}")
            
            # Show Part Number
            if "Part Number" in current_settings and current_settings["Part Number"] != "N/A":
                settings_lines.append(f"Part Number:        {current_settings['Part Number']}")
            
            # Show XMP Timings
            if "XMP Timings" in current_settings and current_settings["XMP Timings"] != "N/A":
                settings_lines.append(f"XMP Timings:        {current_settings['XMP Timings']}")
            
            # Show Configured Voltage
            if "Configured Voltage" in current_settings and current_settings["Configured Voltage"] != "N/A":
                settings_lines.append(f"Configured Voltage: {current_settings['Configured Voltage']}")
            
            # Join all lines
            settings_text = "\n".join(settings_lines)
            
            _debug_print(f"on_mount: updating settings pane with text: {settings_text}")
            settings_pane.update(settings_text)
            _debug_print("on_mount: settings pane updated successfully")

        def action_quit(self) -> None:
            self.exit()

        def action_toggle_dark(self) -> None:
            """
            Toggle dark mode and save the setting.
            
            This method is bound to Ctrl+T and provides the primary theme toggle
            functionality. It switches between dark and light themes and immediately
            persists the new setting to the config file.
            
            The toggle works by:
            1. Determining the new theme state (dark or light)
            2. Setting self.app.theme to the new value
            3. Loading current config, updating the theme key, and saving
            
            Persistence:
                - Saves to ~/.config/ramsleuth/ramsleuth_config.json
                - Works correctly with sudo (saves to original user's home)
                - Changes are immediate, not deferred until exit
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
                
                # Save via SettingsService
                self.settings.set_setting("theme", new_theme)
                
                _debug_print(f"action_toggle_dark: Theme saved to config successfully")
                
            except Exception as e:
                _debug_print(f"action_toggle_dark: Error toggling theme: {e}")
                import traceback
                _debug_print(f"action_toggle_dark: traceback: {traceback.format_exc()}")
                print(f"Error: Failed to toggle theme: {e}", file=sys.stderr)

        def watch_dark(self, dark: bool) -> None:
            """
            Watches for changes to the dark mode setting and persists them.
            
            This method is automatically called by Textual whenever the dark property
            changes, ensuring that ALL theme changes (from any source) are captured
            and persisted to the config file.
            
            This includes:
            - Ctrl+T toggles
            - Command palette theme selections
            - Any other theme changes made through Textual's built-in mechanisms
            
            Args:
                dark: The new dark mode state (True for dark, False for light)
            """
            new_theme = "dark" if dark else "light"
            _debug_print(f"watch_dark: Theme changed to: {new_theme}, saving to config.")
            self.settings.set_setting("theme", new_theme)

        def watch_theme(self, theme: str) -> None:
            """
            Watch for theme changes and persist them.
            
            This method is automatically called by Textual whenever the theme
            property changes, capturing theme changes from all sources including
            the command palette and Ctrl+T toggles.
            
            Args:
                theme: The new theme name (e.g., "dark", "light", "tokyo-night")
            """
            _debug_print(f"watch_theme: TRIGGERED! theme = {theme}")
            try:
                _debug_print(f"watch_theme: Theme changed to {theme}, saving to config")
                # Reload settings to ensure we have latest state before saving
                self.settings.load_settings()
                self.settings.set_setting("theme", theme)
                _debug_print(f"watch_theme: Successfully saved theme to config")
            except Exception as e:
                _debug_print(f"watch_theme: Error saving theme: {e}")
                import traceback
                _debug_print(f"watch_theme: traceback: {traceback.format_exc()}")
                print(f"Error: Failed to save theme setting: {e}", file=sys.stderr)

        def get_available_themes(self) -> set:
            """
            Get the set of available themes from Textual.
            
            Returns:
                A set of available theme names
            """
            try:
                # Textual provides available themes through the app
                if hasattr(self.app, 'available_themes'):
                    return set(self.app.available_themes)
                else:
                    # Fallback to basic themes if available_themes not accessible
                    return {"dark", "light"}
            except Exception as e:
                _debug_print(f"get_available_themes: Error getting available themes: {e}")
                # Return basic themes as fallback
                return {"dark", "light"}

        def action_set_theme(self, theme: str) -> None:
            """
            Set theme from command palette or other sources.
            
            This method handles theme changes from Textual's built-in command palette
            and other sources. It supports both old "dark/light" values and new
            theme names like "tokyo-night", "solarized-dark", etc.
            
            Args:
                theme: The theme name to set (e.g., "dark", "light", "tokyo-night")
            """
            _debug_print(f"action_set_theme: Called with theme = {theme}")
            _debug_print(f"action_set_theme: Before change - self.theme = {getattr(self, 'theme', 'NOT_SET')}")
            _debug_print(f"action_set_theme: Before change - self.app.theme = {getattr(self.app, 'theme', 'NOT_SET')}")
            
            try:
                _debug_print(f"action_set_theme: Attempting to set theme to '{theme}'")
                
                # Validate theme against available themes
                available_themes = self.get_available_themes()
                _debug_print(f"action_set_theme: Available themes: {available_themes}")
                
                if theme not in available_themes and theme not in ["dark", "light"]:
                    _debug_print(f"action_set_theme: Invalid theme '{theme}' not in available themes")
                    print(f"Error: Theme '{theme}' is not available. Using fallback 'dark'.", file=sys.stderr)
                    theme = "dark"
                
                # Apply the theme
                self.app.theme = theme
                self.refresh_css()
                _debug_print(f"action_set_theme: After change - self.theme = {getattr(self, 'theme', 'NOT_SET')}")
                _debug_print(f"action_set_theme: After change - self.app.theme = {getattr(self.app, 'theme', 'NOT_SET')}")
                _debug_print(f"action_set_theme: Theme successfully changed to: {theme}")
                
                # Save the new state
                self.settings.set_setting("theme", theme)
                _debug_print(f"action_set_theme: Theme saved to config")
                
            except Exception as e:
                _debug_print(f"action_set_theme: Error setting theme '{theme}': {e}")
                import traceback
                _debug_print(f"action_set_theme: traceback: {traceback.format_exc()}")
                print(f"Error: Failed to set theme '{theme}': {e}", file=sys.stderr)
                
                # Fallback to default theme on error
                try:
                    _debug_print("action_set_theme: Attempting fallback to 'dark' theme")
                    self.app.theme = "dark"
                    self.dark = True
                    self.refresh_css()
                    self.settings.set_setting("theme", "dark")
                    print("Warning: Using fallback 'dark' theme due to error", file=sys.stderr)
                except Exception as fallback_error:
                    _debug_print(f"action_set_theme: Fallback also failed: {fallback_error}")

        def action_show_summary(self) -> None:
            tabs = self.query_one("#detail_tabs", Tabs)
            tabs.active = "summary_tab"
            self._update_pane_visibility()

        def action_show_full(self) -> None:
            tabs = self.query_one("#detail_tabs", Tabs)
            tabs.active = "full_tab"
            self._update_pane_visibility()
            
        def on_button_pressed(self, event: Button.Pressed) -> None:
            if event.button.id == "rescan_btn":
                self.action_rescan()
            elif event.button.id == "export_btn":
                self.action_export()
                
        def finish_rescan(self, new_dimms, new_raw, new_settings=None, error=None) -> None:
            """Callback to update UI after threaded scan completes."""
            if error:
                 self.notify(f"Rescan failed: {error}", severity="error", title="Error")
                 return

            self.dimms_data = new_dimms
            self.raw_data = new_raw
            
            # Refresh UI
            self.refresh_dimm_list()
            self.update_views(0)
            self.refresh_settings_pane(settings=new_settings)
            
            self.notify(f"Rescan complete. Found {len(new_dimms)} DIMMs.", title="Success")

        @work(thread=True)
        def action_rescan(self) -> None:
            """Perform a system rescan in the background."""
            self.app.call_from_thread(self.notify, "Scanning system...", title="Rescan")
            try:
                # We force test_data_mode=False for a real rescan
                new_dimms, new_raw = perform_system_scan(test_data_mode=False, fail_on_no_smbus=False)
                
                # Fetch settings in background to avoid blocking main thread
                spd_output = ""
                if new_raw and "dimm_0" in new_raw:
                    spd_output = new_raw["dimm_0"]
                new_settings = get_current_memory_settings(spd_output=spd_output, dimms_data=new_dimms)

                self.app.call_from_thread(self.finish_rescan, new_dimms, new_raw, new_settings)
            except Exception as e:
                _debug_print(f"action_rescan: error: {e}")
                self.app.call_from_thread(self.finish_rescan, None, None, None, error=e)

        @work(thread=True)
        def action_export(self) -> None:
            """Export current DIMM data to JSON."""
            try:
                filename = "ramsleuth_export.json"
                with open(filename, "w", encoding="utf-8") as f:
                    json.dump(self.dimms_data, f, indent=2)
                self.app.call_from_thread(self.notify, f"Exported to {filename}", title="Export Successful")
            except Exception as e:
                self.app.call_from_thread(self.notify, f"Export failed: {e}", severity="error", title="Error")
                
        def refresh_dimm_list(self) -> None:
            """Re-populate the DIMM selector list with cards."""
            container = self.query_one("#dimm_selector_container", VerticalScroll)
            
            # Clear existing cards (keeping the static title/settings)
            # A safer way is to remove all DIMMCards specifically
            for child in container.query("DIMMCard"):
                child.remove()
                
            for idx, dimm in enumerate(self.dimms_data):
                card = DIMMCard(dimm, idx)
                container.mount(card)
                if idx == 0:
                    card.set_selected(True)

        def refresh_settings_pane(self, settings: Dict[str, str] = None) -> None:
            """Update the settings pane with fresh data."""
            if settings:
                current_settings = settings
            else:
                spd_output = ""
                if self.raw_data and "dimm_0" in self.raw_data:
                    spd_output = self.raw_data["dimm_0"]
                current_settings = get_current_memory_settings(spd_output=spd_output, dimms_data=self.dimms_data)

            settings_pane = self.query_one("#current_settings_pane", Static)
            
            # (Reuse the logic from on_mount - ideally refactored, but duplicating for safety/speed)
            settings_lines = ["[b]Current Settings (from Memory Controller)[/b]"]
            
            if "Size" in current_settings and current_settings["Size"] != "N/A":
                settings_lines.append(f"Size:               {current_settings['Size']}")
            if "JEDEC Speed" in current_settings and current_settings["JEDEC Speed"] != "N/A":
                settings_lines.append(f"JEDEC Speed:        {current_settings['JEDEC Speed']}")
            if "XMP Profile" in current_settings and current_settings["XMP Profile"] != "N/A":
                settings_lines.append(f"XMP Profile:        {current_settings['XMP Profile']}")
            
            # Show Rated XMP (capabilities)
            if "Rated XMP" in current_settings and current_settings["Rated XMP"] != "N/A":
                settings_lines.append(f"Rated XMP:          {current_settings['Rated XMP']}")

            if "Configured Speed" in current_settings and current_settings["Configured Speed"] != "N/A":
                settings_lines.append(f"Configured Speed:   {current_settings['Configured Speed']}")
            if "Manufacturer" in current_settings and current_settings["Manufacturer"] != "N/A":
                settings_lines.append(f"Manufacturer:       {current_settings['Manufacturer']}")
            if "Part Number" in current_settings and current_settings["Part Number"] != "N/A":
                settings_lines.append(f"Part Number:        {current_settings['Part Number']}")
            if "XMP Timings" in current_settings and current_settings["XMP Timings"] != "N/A":
                settings_lines.append(f"XMP Timings:        {current_settings['XMP Timings']}")
            if "Configured Voltage" in current_settings and current_settings["Configured Voltage"] != "N/A":
                settings_lines.append(f"Configured Voltage: {current_settings['Configured Voltage']}")
            
            # Add Active Profile info (Real Timings)
            if "Active Profile" in current_settings and current_settings["Active Profile"] != "Unknown":
                settings_lines.append(f"\n[b]Active Profile[/b]: {current_settings['Active Profile']}")
            if "Active Timings" in current_settings and current_settings["Active Timings"] != "N/A":
                 settings_lines.append(f"Timings: {current_settings['Active Timings']}")

            settings_text = "\n".join(settings_lines)
            settings_pane.update(settings_text)


        def action_toggle_pane_focus(self) -> None:
            """Toggle focus between the DIMM selector list and the right scrollable pane"""
            try:
                dimm_container = self.query_one("#dimm_selector_container", VerticalScroll)
                right_scroll = self.query_one("#right_scroll", ScrollableContainer)
                
                if dimm_container.has_focus:
                    right_scroll.focus()
                else:
                    dimm_container.focus()
            except Exception as e:
                _debug_print(f"action_toggle_pane_focus: exception occurred: {e}")

        def _update_pane_visibility(self) -> None:
            """Update pane visibility based on active tab"""
            try:
                tabs = self.query_one("#detail_tabs", Tabs)
                summary_pane = self.query_one("#summary_pane", Static)
                full_pane = self.query_one("#full_pane", Static)
                
                active_tab = tabs.active
                summary_pane.display = active_tab == "summary_tab"
                full_pane.display = active_tab == "full_tab"
            except Exception:
                # Widgets might not be mounted yet
                pass

        def on_tabs_tab_activated(self, event: Tabs.TabActivated) -> None:
            """Handle tab activation events"""
            self._update_pane_visibility()
            
            # Save the active tab to config
            self.settings.set_setting("active_tab", event.tab.id)
            _debug_print(f"on_tabs_tab_activated: saved active_tab={event.tab.id}")

        def action_cursor_up(self) -> None:
            self.move_selection(-1)

        def action_cursor_down(self) -> None:
            self.move_selection(1)
            
        def move_selection(self, delta: int) -> None:
            cards = self.query("DIMMCard")
            if not cards:
                return
                
            current_index = -1
            for i, card in enumerate(cards):
                if card.selected:
                    current_index = i
                    break
            
            new_index = max(0, min(len(cards) - 1, current_index + delta))
            
            if new_index != current_index:
                if current_index >= 0:
                    cards[current_index].set_selected(False)
                cards[new_index].set_selected(True)
                self.update_views(cards[new_index].index)
                cards[new_index].scroll_visible()

        def on_dimm_card_selected(self, message: DIMMCard.Selected) -> None:
            """Handle card click event."""
            self.update_views(message.card.index)
            
            # Update visual selection state
            for card in self.query("DIMMCard"):
                card.set_selected(card == message.card)

        def update_views(self, index: int) -> None:
            if not self.dimms_data:
                return

            if index < 0 or index >= len(self.dimms_data):
                index = 0

            dimm = self.dimms_data[index]
            slot = dimm.get("slot", f"DIMM_{index}")
            die_type = dimm.get("die_type", "Unknown")
            notes = dimm.get("notes") or ""

            # Summary content - structured sections
            summary_sections = []
            
            # Identity section
            identity_lines = [
                f"[b]Identity[/b]",
                f"Slot: {slot}",
                f"Manufacturer: {dimm.get('manufacturer', '?')}",
                f"Part Number: {dimm.get('module_part_number', dimm.get('Part Number', '?'))}",
            ]
            summary_sections.extend(identity_lines)
            
            # Die Info section
            die_lines = [
                f"\n[b]Die Info[/b]",
                f"Die Type: {die_type}",
            ]
            if notes:
                die_lines.append(f"Notes: {notes}")
            if dimm.get('dram_mfg'):
                die_lines.append(f"DRAM Manufacturer: {dimm.get('dram_mfg')}")
            summary_sections.extend(die_lines)
            
            # Config section
            config_lines = [
                f"\n[b]Config[/b]",
                f"Generation: {dimm.get('generation', '?')}",
                f"Capacity: {dimm.get('module_gb', '?')} GB",
                f"Ranks: {dimm.get('module_ranks', '?')}",
                f"Chip Organization: {dimm.get('chip_org', dimm.get('SDRAM Device Width', '?'))}",
            ]
            summary_sections.extend(config_lines)
            
            # Timings section
            timings_lines = [f"\n[b]Timings[/b]"]
            if dimm.get('timings'):
                timings_lines.append(f"Inferred: {dimm.get('timings')}")
            if dimm.get('timings_jdec'):
                timings_lines.append(f"JEDEC: {dimm.get('timings_jdec')}")
            if dimm.get('timings_xmp'):
                timings_lines.append(f"XMP/EXPO: {dimm.get('timings_xmp')}")
            if len(timings_lines) > 1:  # Only add if we have actual timings
                summary_sections.extend(timings_lines)

            # Voltage section
            voltage_lines = []
            if dimm.get('configured_voltage') or dimm.get('min_voltage') or dimm.get('max_voltage'):
                voltage_lines.append(f"\n[b]Voltage[/b]")
                if dimm.get('configured_voltage'):
                     voltage_lines.append(f"Configured: {dimm.get('configured_voltage')}")
                if dimm.get('min_voltage'):
                     voltage_lines.append(f"Min: {dimm.get('min_voltage')}")
                if dimm.get('max_voltage'):
                     voltage_lines.append(f"Max: {dimm.get('max_voltage')}")
                summary_sections.extend(voltage_lines)
            
            # DDR5 Extras section
            ddr5_extras = []
            if dimm.get('PMIC Manufacturer') and dimm.get('PMIC Manufacturer') != 'N/A':
                ddr5_extras.append(f"PMIC Manufacturer: {dimm.get('PMIC Manufacturer')}")
            if dimm.get('hynix_ic_part_number') and dimm.get('hynix_ic_part_number') != 'N/A':
                ddr5_extras.append(f"Hynix IC Part Number: {dimm.get('hynix_ic_part_number')}")
            if dimm.get('corsair_version') and dimm.get('corsair_version') != 'N/A':
                ddr5_extras.append(f"Corsair Version: {dimm.get('corsair_version')}")
            if dimm.get('gskill_sticker_code') and dimm.get('gskill_sticker_code') != 'N/A':
                ddr5_extras.append(f"G.Skill Sticker Code: {dimm.get('gskill_sticker_code')}")
            if dimm.get('crucial_sticker_suffix') and dimm.get('crucial_sticker_suffix') != 'N/A':
                ddr5_extras.append(f"Crucial Sticker Suffix: {dimm.get('crucial_sticker_suffix')}")
            
            if ddr5_extras:
                summary_sections.append(f"\n[b]DDR5 Extras[/b]")
                summary_sections.extend(ddr5_extras)

            summary_text = "\n".join(summary_sections)

            # Full dump content (summary + raw block)
            raw_key = f"dimm_{index}"
            full_lines = []
            
            # Add the complete raw data for the full view
            if raw_key in self.raw_data:
                full_lines.append(self.raw_data[raw_key].rstrip())
            else:
                full_lines.append("No raw data available")
            
            full_text = "\n".join(full_lines)

            # Update both panes
            summary_pane = self.query_one("#summary_pane", Static)
            full_pane = self.query_one("#full_pane", Static)
            
            summary_pane.update(summary_text)
            full_pane.update(full_text)

    app = RamSleuthApp(dimms, raw_individual, initial_theme=initial_theme, settings=settings_service)
    app.run()