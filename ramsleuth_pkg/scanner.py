import shutil
import subprocess
import os
import sys
import re
from typing import Any, Dict, List, Tuple
import RamSleuth_DB
from .utils import _debug_print, load_die_database, DEBUG
from .parser import parse_output, extract_profiles_from_spd
from .dependency_engine import check_dependency, install_dependency

class SmbusNotFoundError(Exception):
    """Raised when no SMBus/I2C busses are detected."""
    pass

def find_smbus() -> List[int]:
    """
    Discover SMBus/I2C adapters via `i2cdetect -l`.

    Heuristics:
    - Lines containing any of:
        "SMBus", "smbus", "PIIX4", "AMD", "Intel"
      are considered candidates.
    - Lines containing:
        "NVIDIA", "nvidia", "GPU", "Graphics"
      are excluded as GPU-related.
    - Extract adapter ID from first token "i2c-N" -> N.

    Returns:
        Sorted list of unique bus IDs. On failure, returns [].
    """
    which = shutil.which("i2cdetect")
    if which is None:
        return []

    try:
        proc = subprocess.run(
            [which, "-l"],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=30
        )
    except FileNotFoundError:
        _debug_print(f"find_smbus: i2cdetect not found at {which}")
        return []
    except Exception as e:
        _debug_print(f"find_smbus: unexpected error running i2cdetect -l: {e}")
        return []

    if proc.returncode != 0:
        stderr_text = proc.stderr or ""
        _debug_print(
            f"find_smbus: i2cdetect -l failed with returncode {proc.returncode}"
            f"{': ' + stderr_text.strip() if stderr_text else ''}"
        )
        return []

    candidates: List[int] = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        lower = line.lower()
        if any(tag.lower() in lower for tag in ["nvidia", "gpu", "graphics"]):
            continue
        if not any(
            tag.lower() in lower
            for tag in ["smbus", "piix4", "amd", "intel"]
        ):
            continue
        # First token typically "i2c-N"
        first = line.split()[0]
        if first.startswith("i2c-"):
            num_str = first[4:]
            if num_str.isdigit():
                bus_id = int(num_str)
                if bus_id not in candidates:
                    candidates.append(bus_id)

    return sorted(candidates)

def scan_bus(bus_id: int) -> List[int]:
    """
    Scan a given I2C bus using `i2cdetect -y` and detect SPD addresses.

    We look for addresses in the 0x50-0x57 range.

    Returns:
        Sorted unique list of integer addresses.
        If the command fails or no addresses found, returns [].
    """
    which = shutil.which("i2cdetect")
    if which is None:
        return []

    try:
        proc = subprocess.run(
            [which, "-y", str(bus_id)],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=30
        )
    except FileNotFoundError:
        _debug_print(f"scan_bus: i2cdetect not found at {which}")
        return []
    except Exception as e:
        _debug_print(f"scan_bus: unexpected error running i2cdetect -y {bus_id}: {e}")
        return []

    if proc.returncode != 0:
        stderr_text = proc.stderr or ""
        _debug_print(
            f"scan_bus: i2cdetect -y {bus_id} failed with returncode {proc.returncode}"
            f"{': ' + stderr_text.strip() if stderr_text else ''}"
        )
        return []

    addrs: List[int] = []
    for line in proc.stdout.splitlines():
        # Typical i2cdetect table rows; tokens may be:
        # "50", "51", or "--" etc.
        parts = line.strip().split()
        for token in parts:
            # Skip header labels like "0:", "10:", etc.
            if token.endswith(":"):
                continue
            # Only consider hex-like tokens
            if all(c in "0123456789abcdefABCDEF" for c in token) and len(
                token
            ) in (2, 3):
                try:
                    val = int(token, 16)
                except ValueError:
                    continue
                if 0x50 <= val <= 0x57:
                    if val not in addrs:
                        addrs.append(val)
    return sorted(addrs)

def get_current_memory_settings(spd_output: str = "", dimms_data: List[Dict[str, Any]] = None) -> Dict[str, str]:
    """
    Parses `dmidecode -t memory` to find current operating speed and voltage.
    Matches this against SPD data to infer the active profile.
    """
    settings = {
        "Configured Speed": "N/A",
        "Configured Voltage": "N/A",
        "Active Profile": "Unknown",
        "Active Timings": "N/A"
    }
    
    # Check if dmidecode is available
    if not shutil.which("dmidecode"):
        return settings
    
    cmd = ["dmidecode", "-t", "memory"]
    
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=False, timeout=10)
        if result.returncode != 0:
            return settings
            
        # Parse dmidecode output for the first populated slot
        current_speed = None
        current_voltage = None
        part_number = "Unknown"
        
        current_device = {}
        in_device = False
        
        for line in result.stdout.splitlines():
            line = line.strip()
            if line.startswith("Memory Device"):
                in_device = True
                current_device = {}
                continue
            if not line and in_device:
                # End of device
                size = current_device.get("Size", "")
                if size and "No Module" not in size and "Not Installed" not in size:
                    # Found populated device
                    current_speed = current_device.get("Configured Memory Speed", current_device.get("Speed"))
                    current_voltage = current_device.get("Configured Voltage")
                    part_number = current_device.get("Part Number")
                    settings["Part Number"] = part_number
                    settings["Manufacturer"] = current_device.get("Manufacturer", "Unknown")
                    break # Use first found
                in_device = False
                continue
                
            if in_device and ":" in line:
                k, v = line.split(":", 1)
                current_device[k.strip()] = v.strip()
        
        # Handle last device if loop finished inside a device block
        if in_device:
            size = current_device.get("Size", "")
            if size and "No Module" not in size and "Not Installed" not in size:
                 # Found populated device (last one)
                 # Only update if we haven't found one already (though the break above handles early exit)
                 if not current_speed:
                     current_speed = current_device.get("Configured Memory Speed", current_device.get("Speed"))
                     current_voltage = current_device.get("Configured Voltage")
                     part_number = current_device.get("Part Number")
                     settings["Part Number"] = part_number
                     settings["Manufacturer"] = current_device.get("Manufacturer", "Unknown")

        if current_speed:
            settings["Configured Speed"] = current_speed
        if current_voltage:
            settings["Configured Voltage"] = current_voltage
            
        # Inference Logic
        if current_speed:
            # Clean current speed (remove "MT/s" etc)
            speed_val_match = re.search(r'(\d+)', current_speed)
            if speed_val_match:
                speed_val = int(speed_val_match.group(1))
                
                # Default Assumption
                # If speed > 2666 (typical DDR4 JEDEC max) or > 4800 (DDR5 JEDEC base), likely Manual/OC if no XMP match
                settings["Active Profile"] = "JEDEC / Manual"
                
                # We need extracted profiles.
                profiles = {}
                if spd_output:
                     profiles = extract_profiles_from_spd(spd_output)
                
                # Find the "Max" XMP/EXPO profile regardless of current speed
                max_xmp_speed = 0
                max_xmp_profile = None
                
                for speed_key, profile_data in profiles.items():
                    # speed_key is like "3600 MT/s"
                    p_speed_match = re.search(r'(\d+)', speed_key)
                    if p_speed_match:
                        p_speed_val = int(p_speed_match.group(1))
                        if p_speed_val > max_xmp_speed:
                            max_xmp_speed = p_speed_val
                            max_xmp_profile = profile_data

                # Fallback: Check dimms_data if spd_output yielded nothing
                if not max_xmp_profile and dimms_data and len(dimms_data) > 0:
                    first_dimm = dimms_data[0]
                    xmp_str = first_dimm.get("timings_xmp", "")
                    if xmp_str:
                        # Format like "3600-18-22-22"
                        xmp_speed_match = re.search(r'^(\d+)', xmp_str)
                        if xmp_speed_match:
                            max_xmp_speed = int(xmp_speed_match.group(1))
                            
                            # Parse timings from the string "3600-18-22-22" -> "18-22-22"
                            timings_part = "Unknown"
                            if "-" in xmp_str:
                                parts = xmp_str.split("-", 1)
                                if len(parts) > 1:
                                    timings_part = parts[1]

                            max_xmp_profile = {
                                "type": "XMP/EXPO",
                                "timings": timings_part,
                                "description": f"{max_xmp_speed} MT/s (Inferred from Summary)"
                            }

                # Fallback: Infer from Part Number if still no XMP profile found
                if not max_xmp_profile and part_number and part_number != "Unknown":
                     # Heuristics for common vendors (G.Skill, Corsair, Kingston, etc.)
                     # G.Skill: F4-3600C18... -> 3600, CL18
                     # Corsair: ...3600C18...
                     p_upper = part_number.upper().strip()
                     inferred_speed = 0
                     inferred_cl = ""
                     
                     # Regex for G.Skill (F4-3600C18...), Corsair (CM...3600C18), etc.
                     # Looks for 4 digits (speed) followed immediately by C and digits (latency)
                     # Matches: 3600C18, 3200C16, 6000J30 (G.Skill DDR5 uses J sometimes)
                     match = re.search(r'(\d{4})[CJ](\d+)', p_upper)
                     if match:
                         inferred_speed = int(match.group(1))
                         inferred_cl = match.group(2)
                     else:
                         # Kingston/HyperX Shortened: KF432C16 (32=3200)
                         # Look for specific prefixes to be safe
                         k_match = re.search(r'(?:KF|HX)4(\d{2})C(\d+)', p_upper)
                         if k_match:
                             inferred_speed = int(k_match.group(1)) * 100
                             inferred_cl = k_match.group(2)

                     if inferred_speed > 0 and inferred_cl:
                         # Sanity check speed range (DDR3/4/5)
                         if 1000 <= inferred_speed <= 10000:
                             max_xmp_speed = inferred_speed
                             max_xmp_profile = {
                                 "type": "Inferred from Part #",
                                 "timings": f"CL{inferred_cl}",
                                 "description": f"{max_xmp_speed} MT/s (Inferred from Part #)"
                             }

                # Store Max Capabilities
                if max_xmp_profile:
                    timings = max_xmp_profile.get("timings", "Unknown")
                    if timings != "Unknown":
                        # If timings start with "CL", don't prepend "CL" again
                        msg_timings = timings
                        if not msg_timings.startswith("CL"):
                             msg_timings = f"CL{msg_timings}"
                             
                        settings["Rated XMP"] = f"{max_xmp_speed} MT/s ({msg_timings})"
                    else:
                        settings["Rated XMP"] = f"{max_xmp_speed} MT/s"
                    settings["Rated XMP Timings"] = timings

                # Perform Matching
                matched_profile = None
                if max_xmp_speed > 0:
                    # Check exact match with Max first
                    if abs(speed_val - max_xmp_speed) < 100:
                        matched_profile = max_xmp_profile
                    else:
                        # Check other profiles
                        for speed_key, profile_data in profiles.items():
                             p_speed_match = re.search(r'(\d+)', speed_key)
                             if p_speed_match:
                                 p_speed_val = int(p_speed_match.group(1))
                                 if abs(speed_val - p_speed_val) < 100:
                                     matched_profile = profile_data
                                     break
                
                if matched_profile:
                    settings["Active Profile"] = "XMP/EXPO (Active)"
                    settings["Active Timings"] = matched_profile.get("timings", "Unknown")
                    settings["XMP Profile"] = matched_profile.get("description", "Unknown")
                else:
                    # No match found.
                    # If current speed is significantly higher than standard JEDEC (e.g. > 2666 for DDR4 context),
                    # label it as Manual/OC.
                    # This is a heuristic.
                    if speed_val > 2666:
                         settings["Active Profile"] = "Manual / OC"
                    else:
                         settings["Active Profile"] = "JEDEC / Standard"
                    
    except Exception as e:
        _debug_print(f"Error in get_current_memory_settings: {e}")
        
    return settings

def load_modules() -> None:
    """
    Best-effort, idempotent kernel module loading helper.

    Responsibilities:
    - Try to modprobe the following modules (if available on the system):
        * i2c-dev
        * ee1004
        * at24
    - Detect CPU vendor and try to modprobe:
        * i2c-amd-mp2-pci (for AMD)
        * i2c-i801 (for Intel)

    Behavioral rules:
    - No interactivity; no prompts.
    - Never hard-fail solely because a module failed to load.
      Failures are only surfaced via debug logging when DEBUG is True.
    - Safe to call multiple times:
        * modprobe is idempotent for loaded modules.
    """
    def _try_modprobe(module: str) -> None:
        try:
            # modprobe is generally idempotent; ignore non-zero exits here.
            subprocess.run(
                ["modprobe", module],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=30
            )
            _debug_print(f"Attempted to load module '{module}'.")
        except FileNotFoundError:
            _debug_print(f"modprobe command not found; cannot load module '{module}'")
        except Exception as exc:  # pragma: no cover - defensive
            _debug_print(f"modprobe '{module}' failed: {exc}")

    # Core modules useful for SPD/EEPROM access
    # 'eeprom' is for legacy/DDR3, 'ee1004' for DDR4/5, 'at24' generic
    for base_mod in ("i2c-dev", "eeprom", "ee1004", "at24"):
        _try_modprobe(base_mod)

    # Try to determine CPU vendor to load the appropriate SMBus controller driver.
    cpu_vendor = None

    # Preferred: /proc/cpuinfo
    try:
        if os.path.exists("/proc/cpuinfo"):
            with open("/proc/cpuinfo", "r", encoding="utf-8") as f:
                for line in f:
                    lower = line.lower()
                    if "vendor_id" in lower or "model name" in lower:
                        if "amd" in lower:
                            cpu_vendor = "amd"
                            break
                        if "intel" in lower:
                            cpu_vendor = "intel"
                            break
    except Exception:  # pragma: no cover - defensive
        cpu_vendor = None

    # Fallback: lscpu
    if cpu_vendor is None:
        try:
            proc = subprocess.run(
                ["lscpu"],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=30
            )
            if proc.returncode == 0 and proc.stdout:
                lower = proc.stdout.lower()
                if "amd" in lower:
                    cpu_vendor = "amd"
                elif "intel" in lower:
                    cpu_vendor = "intel"
            else:
                stderr_text = proc.stderr or ""
                _debug_print(
                    f"lscpu failed with returncode {proc.returncode}"
                    f"{': ' + stderr_text.strip() if stderr_text else ''}"
                )
        except FileNotFoundError:
            _debug_print("lscpu command not found; cannot detect CPU vendor")
        except Exception as e:  # pragma: no cover - defensive
            _debug_print(f"lscpu failed with unexpected error: {e}")

    if cpu_vendor == "amd":
        # Try AMD SMBus drivers in order of preference
        amd_modules = ["i2c-amd-mp2-pci", "i2c-amd-mp2"]
        for module in amd_modules:
            _try_modprobe(module)
    elif cpu_vendor == "intel":
        # Try Intel SMBus drivers in order of preference
        intel_modules = ["i2c-i801", "i2c-piix4"]
        for module in intel_modules:
            _try_modprobe(module)
    else:
        _debug_print("CPU vendor unknown; skipping vendor-specific i2c module probes.")

def register_devices(bus_id: int, addresses: List[int]) -> None:
    """
    Best-effort sysfs registration of detected SPD EEPROM devices.

    Behavior:
    - Unconditionally attempts to register each provided SPD address via:
        /sys/bus/i2c/devices/i2c-<bus_id>/new_device
    - Uses "ee1004" as primary driver for DDR4/DDR5 compatibility.
    - Writes are best-effort:
        * If the new_device path is missing, returns silently after debug log.
        * If a write fails (e.g., "File exists", permission issues), the error
          is NOT propagated; failures are only surfaced via _debug_print() when
          DEBUG is enabled.
    - Safe to call once per run; duplicate registrations are tolerated.
    """
    new_device_path = f"/sys/bus/i2c/devices/i2c-{bus_id}/new_device"

    if not os.path.exists(new_device_path):
        _debug_print(
            f"register_devices: new_device path '{new_device_path}' does not exist; "
            f"skipping sysfs registration."
        )
        return

    for addr in addresses:
        # Primary driver hint; ee1004 is appropriate for DDR4/DDR5 SPD EEPROMs
        # and is generally safe for modern platforms. We intentionally keep this
        # simple and non-interactive.
        driver = "ee1004"
        
        # Prepare the command string to write to sysfs
        echo_cmd = f"{driver} 0x{addr:02x}"
        
        try:
            # Use direct file I/O instead of subprocess since we are root
            with open(new_device_path, 'w', encoding='utf-8') as f:
                f.write(echo_cmd)
                
            _debug_print(f"register_devices: successfully wrote '{echo_cmd}' to {new_device_path}")
            
        except OSError as e:
            # EEXIST (File exists) is common and harmless (device already registered)
            # EBUSY (Device or resource busy) is also possible
            if e.errno == 17:  # EEXIST
                _debug_print(f"register_devices: device 0x{addr:02x} already registered on bus {bus_id} (File exists)")
            elif e.errno == 16:  # EBUSY
                _debug_print(f"register_devices: device 0x{addr:02x} or bus {bus_id} is busy (EBUSY)")
            elif e.errno == 22:  # EINVAL
                _debug_print(f"register_devices: invalid argument for 0x{addr:02x} on bus {bus_id} (EINVAL)")
            else:
                _debug_print(f"register_devices: OS error registering 0x{addr:02x} on bus {bus_id}: {e}")
        except Exception as exc:  # pragma: no cover - defensive
            _debug_print(
                f"register_devices: unexpected exception while registering 0x{addr:02x} "
                f"on bus {bus_id}: {exc}"
            )

def _split_decode_dimms_blocks_wrapper(output: str) -> Dict[str, str]:
    """
    Wrapper to call _split_decode_dimms_blocks from parser module, exposed here for run_decoder.
    This is slightly redundant but keeps run_decoder logic clean if it was relying on a local function.
    However, run_decoder calls it directly.
    We import it from .parser.
    """
    from .parser import _split_decode_dimms_blocks
    return _split_decode_dimms_blocks(output)

def run_decoder() -> Tuple[str, Dict[str, str]]:
    """
    Invoke `decode-dimms` to obtain SPD/EEPROM information.

    Behavior:
    - Ensure `decode-dimms` exists; if not, raise RuntimeError.
    - Run plain `decode-dimms` as the primary, parseable source:
      * Modern 4.x prints a structured, per-DIMM text report including
        "Decoding EEPROM" headers and key/value lines per DIMM.
      * This is what we feed directly into parse_output().
    - Only run `decode-dimms --side-by-side` as fallback if plain fails:
      * For environments where plain fails but side-by-side works.
      * Side-by-side failures must NOT mask a working plain decode.
    - Robustness:
      * Raise only if BOTH invocations fail (no usable output at all).
      * Otherwise:
        - combined_output -> parse_output()
        - raw_individual_blocks -> _split_decode_dimms_blocks() from plain when available,
          empty dict when using side-by-side fallback.

    Returns:
        (combined_output_for_parser, raw_individual_blocks)
    """
    decoder = shutil.which("decode-dimms")
    if decoder is None:
        raise RuntimeError("Required tool 'decode-dimms' not found")

    # Try plain decode-dimms first (primary source - works on 95%+ of systems)
    try:
        try:
            plain_proc = subprocess.run(
                [decoder],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=30
            )
        except subprocess.TimeoutExpired:
            _debug_print(f"run_decoder: decode-dimms timed out after 30 seconds")
            raise RuntimeError("decode-dimms execution timed out")
        except Exception as exc:
            _debug_print(f"run_decoder: decode-dimms execution failed: {exc}")
            raise RuntimeError(f"decode-dimms execution failed: {exc}") from exc
    except FileNotFoundError:
        raise RuntimeError("Required tool 'decode-dimms' not found")
    except Exception as exc:  # pragma: no cover - defensive
        raise RuntimeError(f"decode-dimms execution failed: {exc}") from exc

    plain_ok = plain_proc.returncode == 0 and (plain_proc.stdout or "").strip() != ""
    
    if plain_ok:
        # Primary case: plain decode-dimms succeeded
        # Use plain output for both combined and raw_individual blocks
        combined = plain_proc.stdout
        from .parser import _split_decode_dimms_blocks
        raw_individual = _split_decode_dimms_blocks(plain_proc.stdout)
        
        if DEBUG:
            _debug_print(
                f"run_decoder: plain decode-dimms succeeded, "
                f"combined_len={len(combined)}, blocks={len(raw_individual)}"
            )
    else:
        # Fallback: try side-by-side if plain failed
        try:
            try:
                side_proc = subprocess.run(
                    [decoder, "--side-by-side"],
                    check=False,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    timeout=30
                )
            except subprocess.TimeoutExpired:
                _debug_print(f"run_decoder: decode-dimms --side-by-side timed out after 30 seconds")
                side_proc = None
            except Exception as exc:
                _debug_print(f"run_decoder: decode-dimms --side-by-side failed: {exc}")
                side_proc = None
        except Exception as exc:  # pragma: no cover - defensive
            side_proc = None  # Mark as failed
        
        side_ok = side_proc and side_proc.returncode == 0 and (side_proc.stdout or "").strip() != ""
        
        if not side_ok:
            # Both plain and side-by-side failed
            plain_stderr = plain_proc.stderr or ""
            side_stderr = side_proc.stderr if side_proc else ""
            
            if DEBUG:
                _debug_print(
                    f"run_decoder: both decode-dimms invocations failed - "
                    f"plain returncode={plain_proc.returncode}"
                    f"{', stderr: ' + plain_stderr.strip() if plain_stderr else ''}"
                    f"{', side-by-side returncode=' + str(side_proc.returncode) if side_proc else ''}"
                    f"{', stderr: ' + side_stderr.strip() if side_stderr else ''}"
                )
            
            raise RuntimeError("decode-dimms execution failed (no usable output)")
        
        # Use side-by-side output (no individual blocks available)
        combined = side_proc.stdout
        raw_individual = {}  # No individual blocks from side-by-side
        
        if DEBUG:
            _debug_print(
                f"run_decoder: plain failed, side-by-side succeeded as fallback, "
                f"combined_len={len(combined)}"
            )

    return combined, raw_individual

def perform_system_scan(test_data_mode: bool = False, fail_on_no_smbus: bool = False) -> Tuple[List[Dict[str, Any]], Dict[str, str]]:
    """
    Execute the full system scan, parsing, and identification workflow.

    Args:
        test_data_mode: If True, load from test_data.txt instead of hardware scan.
        fail_on_no_smbus: If True, raise SmbusNotFoundError if no busses found.

    Returns:
        Tuple of (dimms_list, raw_individual_blocks_dict)
    
    Raises:
        SmbusNotFoundError: If no SMBus/I2C busses are found and fail_on_no_smbus is True.
    """
    # Dependency check for i2c-tools (decode-dimms)
    if not test_data_mode:
        if not check_dependency("i2c-tools"):
            print("Dependencies missing: i2c-tools is required for reading SPD data.", file=sys.stderr)
            try:
                # Basic interactive prompt if we are in a terminal
                if sys.stdin.isatty():
                    resp = input("Install i2c-tools now? [y/N] ")
                    if resp.lower() == 'y':
                        if install_dependency("i2c-tools"):
                             print("Dependency installed successfully.")
                        else:
                             print("Failed to install dependency. Scan may be incomplete.", file=sys.stderr)
                    else:
                        print("Skipping installation. Scan may be incomplete.", file=sys.stderr)
                else:
                     _debug_print("Non-interactive mode, skipping dependency installation.")
            except Exception as e:
                _debug_print(f"Dependency check failed: {e}")

    # Best-effort module loading
    load_modules()

    # SMBus discovery
    buses = find_smbus()
    if not buses and not test_data_mode:
        if fail_on_no_smbus:
            raise SmbusNotFoundError("No SMBus/I2C busses detected.")
        else:
            print(
                "Warning: No SMBus/I2C busses detected. "
                "Continuing; decode-dimms may still provide data.",
                file=sys.stderr,
            )
    elif buses and not test_data_mode:
        # Collect and register
        bus_address_pairs = []
        for bus in buses:
            addresses = scan_bus(bus)
            if addresses:
                bus_address_pairs.append((bus, addresses))
        
        for bus_id, addresses in bus_address_pairs:
            register_devices(bus_id, addresses)

    # Run decoder or use test data
    combined_output = ""
    raw_individual = {}
    
    if test_data_mode:
        # We need to find test_data.txt relative to the package or main script
        # Assuming run from project root for now
        test_data_path = os.path.abspath("test_data.txt")
        if os.path.exists(test_data_path):
            _debug_print(f"perform_system_scan: using test data from {test_data_path}")
            with open(test_data_path, "r", encoding="utf-8") as f:
                combined_output = f.read()
            raw_individual = {}
        else:
            # Try next to the file
            test_data_path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "test_data.txt")
            if os.path.exists(test_data_path):
                 _debug_print(f"perform_system_scan: using test data from {test_data_path}")
                 with open(test_data_path, "r", encoding="utf-8") as f:
                    combined_output = f.read()
                 raw_individual = {}
            else:
                print("Error: test_data.txt not found for --test-data mode", file=sys.stderr)
                raise RuntimeError("test_data.txt not found")
    else:
        # Normal hardware detection
        try:
            _debug_print("perform_system_scan: invoking run_decoder()")
            combined_output, raw_individual = run_decoder()
        except RuntimeError as e:
            if fail_on_no_smbus: # Proxy for non-interactive
                # main exited with 8 here
                raise e
            else:
                print(
                    f"Warning: {e}. Proceeding without decode-dimms output.",
                    file=sys.stderr,
                )
                combined_output = ""
                raw_individual = {}

    # Parse
    dimms: List[Dict[str, Any]] = []
    if combined_output:
        parsed = parse_output(combined_output)
        
        # Deduplicate
        seen_keys = set()
        deduped: List[Dict[str, Any]] = []
        for d in parsed:
            slot = str(d.get("slot", "")).strip().lower()
            part = str(d.get("module_part_number", d.get("Part Number", ""))).strip().lower()
            mfg = str(d.get("manufacturer", "")).strip().lower()
            gen = str(d.get("generation", "")).strip().lower()
            key = (slot, part, mfg, gen)
            if key in seen_keys and any(key):
                continue
            seen_keys.add(key)
            deduped.append(d)
        dimms = deduped

    # Load DB
    db = load_die_database()

    # Normalize and Resolve
    for idx, dimm in enumerate(dimms):
        if "slot" not in dimm:
            dimm["slot"] = f"DIMM_{idx}"
        norm = RamSleuth_DB.normalize_dimm_data(dimm)
        dimm.update(norm)
        
    for dimm in dimms:
        die_type, notes = RamSleuth_DB.find_die_type(dimm, db)
        dimm["die_type"] = die_type
        if notes:
            dimm["notes"] = notes
            
    # Second pass normalization/resolution
    for idx, dimm in enumerate(dimms):
        dimm.update(RamSleuth_DB.normalize_dimm_data(dimm))
        die_type, notes = RamSleuth_DB.find_die_type(dimm, db)
        dimm["die_type"] = die_type
        if notes:
            dimm["notes"] = notes

    return dimms, raw_individual