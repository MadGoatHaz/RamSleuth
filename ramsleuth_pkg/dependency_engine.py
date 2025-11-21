#!/usr/bin/env python3
"""
System-native dependency engine for RamSleuth.

This module provides autonomous dependency management using system-native
package managers only. It completely eliminates pip-based installation and
handles 15+ Linux distributions with appropriate package managers.
"""

import importlib.util
import os
import platform
import shutil
import subprocess
import sys
import time
from typing import Any, Dict, List, Optional, Tuple


# ============================================================================
# SYSTEM PACKAGE MAPPINGS
# ============================================================================

SYSTEM_PACKAGE_MAP: Dict[str, Dict[str, List[str]]] = {
    # Core tools required for RamSleuth functionality
    "i2cdetect": {
        "arch": ["i2c-tools"],
        "debian": ["i2c-tools"],
        "ubuntu": ["i2c-tools"],
        "fedora": ["i2c-tools"],
        "rhel": ["i2c-tools"],
        "centos": ["i2c-tools"],
        "rocky": ["i2c-tools"],
        "almalinux": ["i2c-tools"],
        "opensuse": ["i2c-tools"],
        "gentoo": ["sys-apps/i2c-tools"],
    },
    "i2c-tools": {
        "arch": ["i2c-tools"],
        "debian": ["i2c-tools"],
        "ubuntu": ["i2c-tools"],
        "fedora": ["i2c-tools"],
        "rhel": ["i2c-tools"],
        "centos": ["i2c-tools"],
        "rocky": ["i2c-tools"],
        "almalinux": ["i2c-tools"],
        "opensuse": ["i2c-tools"],
        "gentoo": ["sys-apps/i2c-tools"],
    },
    "decode-dimms": {
        "arch": ["i2c-tools"],
        "debian": ["i2c-tools"],
        "ubuntu": ["i2c-tools"],
        "fedora": ["i2c-tools"],
        "rhel": ["i2c-tools"],
        "centos": ["i2c-tools"],
        "rocky": ["i2c-tools"],
        "almalinux": ["i2c-tools"],
        "opensuse": ["i2c-tools"],
        "gentoo": ["sys-apps/i2c-tools"],
    },
    "dmidecode": {
        "arch": ["dmidecode"],
        "debian": ["dmidecode"],
        "ubuntu": ["dmidecode"],
        "fedora": ["dmidecode"],
        "rhel": ["dmidecode"],
        "centos": ["dmidecode"],
        "rocky": ["dmidecode"],
        "almalinux": ["dmidecode"],
        "opensuse": ["dmidecode"],
        "gentoo": ["sys-apps/dmidecode"],
    },
    # Textual TUI framework (system packages only)
    "textual": {
        "arch": ["python-textual"],
        "debian": ["python3-textual"],
        "ubuntu": ["python3-textual"],
        "fedora": ["python3-textual"],
        "rhel": ["python3-textual"],
        "centos": ["python3-textual"],
        "rocky": ["python3-textual"],
        "almalinux": ["python3-textual"],
        "opensuse": ["python3-textual"],
        "gentoo": ["dev-python/textual"],
    },
    # linkify-it-py for Textual's Markdown widget
    "linkify_it": {
        "arch": ["python-linkify-it-py"],
        "debian": ["python3-linkify-it-py"],
        "ubuntu": ["python3-linkify-it-py"],
        "fedora": ["python3-linkify-it-py"],
        "rhel": ["python3-linkify-it-py"],
        "centos": ["python3-linkify-it-py"],
        "rocky": ["python3-linkify-it-py"],
        "almalinux": ["python3-linkify-it-py"],
        "opensuse": ["python3-linkify-it-py"],
        "gentoo": ["dev-python/linkify-it-py"],
    },
}


# ============================================================================
# DISTRIBUTION DETECTION
# ============================================================================

def detect_distribution() -> Dict[str, Any]:
    """
    Enhanced OS detection that handles 15+ distributions.
    
    Returns structured distribution information with package manager details.
    
    Returns:
        Dict containing:
        - id: Distribution ID (arch, debian, ubuntu, fedora, etc.)
        - name: Full distribution name
        - version: Distribution version
        - package_manager: Package manager command (pacman, apt, dnf, etc.)
        - install_cmd: Full install command prefix
        - check_cmd: Package check command
    """
    distro_info = {
        "id": "unknown",
        "name": "Unknown",
        "version": "",
        "package_manager": "",
        "install_cmd": "",
        "check_cmd": "",
    }
    
    # Try /etc/os-release first (most reliable)
    os_release = "/etc/os-release"
    if os.path.exists(os_release):
        try:
            with open(os_release, "r", encoding="utf-8") as f:
                os_release_data = {}
                for line in f:
                    line = line.strip()
                    if "=" in line:
                        key, value = line.split("=", 1)
                        # Remove quotes
                        if value.startswith('"') and value.endswith('"'):
                            value = value[1:-1]
                        elif value.startswith("'") and value.endswith("'"):
                            value = value[1:-1]
                        os_release_data[key] = value
            
            distro_id = os_release_data.get("ID", "unknown").lower()
            distro_info["name"] = os_release_data.get("NAME", "Unknown")
            distro_info["version"] = os_release_data.get("VERSION_ID", "")
            
            # Map distribution IDs to our internal categories
            if distro_id in {"arch", "artix", "archlinux", "archarm"}:
                distro_info["id"] = "arch"
                distro_info["package_manager"] = "pacman"
                distro_info["install_cmd"] = "pacman -S --noconfirm --needed"
                distro_info["check_cmd"] = "pacman -Qi"
            elif distro_id == "debian":
                distro_info["id"] = "debian"
                distro_info["package_manager"] = "apt"
                distro_info["install_cmd"] = "apt install -y"
                distro_info["check_cmd"] = "dpkg -s"
            elif distro_id in {"ubuntu", "linuxmint", "pop", "elementary", "zorin"}:
                distro_info["id"] = "ubuntu"
                distro_info["package_manager"] = "apt"
                distro_info["install_cmd"] = "apt install -y"
                distro_info["check_cmd"] = "dpkg -s"
            elif distro_id in {"fedora"}:
                distro_info["id"] = "fedora"
                distro_info["package_manager"] = "dnf"
                distro_info["install_cmd"] = "dnf install -y"
                distro_info["check_cmd"] = "rpm -q"
            elif distro_id in {"rhel", "centos", "rocky", "almalinux", "oracle"}:
                distro_info["id"] = distro_id
                distro_info["package_manager"] = "dnf"
                distro_info["install_cmd"] = "dnf install -y"
                distro_info["check_cmd"] = "rpm -q"
            elif distro_id in {"opensuse", "opensuse-leap", "opensuse-tumbleweed", "suse", "sles"}:
                distro_info["id"] = "opensuse"
                distro_info["package_manager"] = "zypper"
                distro_info["install_cmd"] = "zypper install -y"
                distro_info["check_cmd"] = "rpm -q"
            elif distro_id in {"gentoo"}:
                distro_info["id"] = "gentoo"
                distro_info["package_manager"] = "emerge"
                distro_info["install_cmd"] = "emerge --ask n"
                distro_info["check_cmd"] = "qlist -I"
            else:
                distro_info["id"] = "unknown"
                
        except Exception:
            distro_info["id"] = "unknown"
    
    # Fallback to platform detection
    if distro_info["id"] == "unknown":
        system = platform.system().lower()
        if system == "linux":
            # Try lsb_release as fallback
            try:
                result = subprocess.run(
                    ["lsb_release", "-si"],
                    capture_output=True,
                    text=True,
                    check=False,
                    timeout=30
                )
                if result.returncode == 0:
                    distro_id = result.stdout.strip().lower()
                    if distro_id in {"arch", "artix", "endeavouros", "manjaro", "garuda"}:
                        distro_info["id"] = "arch"
                        distro_info["package_manager"] = "pacman"
                        distro_info["install_cmd"] = "pacman -S --noconfirm --needed"
                        distro_info["check_cmd"] = "pacman -Qi"
                    elif distro_id == "debian":
                        distro_info["id"] = "debian"
                        distro_info["package_manager"] = "apt"
                        distro_info["install_cmd"] = "apt install -y"
                        distro_info["check_cmd"] = "dpkg -s"
                    elif distro_id in {"ubuntu", "linuxmint"}:
                        distro_info["id"] = "ubuntu"
                        distro_info["package_manager"] = "apt"
                        distro_info["install_cmd"] = "apt install -y"
                        distro_info["check_cmd"] = "dpkg -s"
            except Exception:
                pass
    
    return distro_info


# ============================================================================
# DEPENDENCY CHECKING
# ============================================================================

def check_tool_available(tool_name: str) -> bool:
    """
    Robust tool checking using multiple methods.
    
    Args:
        tool_name: Name of the tool to check
        
    Returns:
        True if tool is available, False otherwise
    """
    # Method 1: shutil.which() - most reliable for executables
    if shutil.which(tool_name) is not None:
        return True
    
    # Method 2: Try to run the tool with --version or --help
    try:
        result = subprocess.run(
            [tool_name, "--version"],
            capture_output=True,
            check=False,
            timeout=30
        )
        if result.returncode == 0:
            return True
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass
    
    # Method 3: Try --help as fallback
    try:
        result = subprocess.run(
            [tool_name, "--help"],
            capture_output=True,
            check=False,
            timeout=30
        )
        if result.returncode in {0, 1}:  # Many tools return 1 for --help
            return True
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass
    
    return False


def check_python_package(package_name: str) -> bool:
    """
    Check if a Python package is available using importlib.
    
    Args:
        package_name: Name of the Python package
        
    Returns:
        True if package is available, False otherwise
    """
    try:
        spec = importlib.util.find_spec(package_name)
        return spec is not None
    except Exception:
        return False


def check_dependency(package_name: str) -> bool:
    """
    Check if a dependency is satisfied.
    
    Args:
        package_name: Name of the package or tool to check.
        
    Returns:
        True if available, False otherwise.
    """
    # If it's a known tool key with associated binaries, check for the binary
    if package_name == "i2c-tools":
        return check_tool_available("decode-dimms")
    
    # Default to checking if it's a tool in the path
    return check_tool_available(package_name)


def install_dependency(package_name: str, interactive: bool = True) -> bool:
    """
    Install a dependency using the system package manager.
    
    Args:
        package_name: Name of the package to install (must be in SYSTEM_PACKAGE_MAP)
        interactive: If True, may prompt user (though auto_install is mostly non-interactive logic)
        
    Returns:
        True if successful, False otherwise.
    """
    distro_info = detect_distribution()
    if distro_info["id"] == "unknown":
        return False
        
    # We use auto_install_dependencies which handles the map lookup
    return auto_install_dependencies([package_name], distro_info)


def get_missing_dependencies(requested_features: Dict[str, bool]) -> Dict[str, List[str]]:
    """
    Check for missing dependencies based on requested features.
    
    Args:
        requested_features: Dictionary with feature flags:
            - "core": Core tools (i2cdetect, decode-dimms, dmidecode)
            - "tui": Textual TUI framework
            
    Returns:
        Dictionary with missing packages categorized by type:
        - "system": List of missing system packages
        - "python": List of missing Python packages
    """
    missing = {"system": [], "python": []}
    
    # Check core tools
    if requested_features.get("core", False):
        core_tools = ["i2cdetect", "decode-dimms", "dmidecode"]
        for tool in core_tools:
            if not check_tool_available(tool):
                missing["system"].append(tool)
    
    # Check TUI dependencies
    if requested_features.get("tui", False):
        if not check_python_package("textual"):
            missing["python"].append("textual")
        if not check_python_package("linkify_it"):
            missing["python"].append("linkify_it")
    
    return missing


# ============================================================================
# INSTALLATION COMMAND CONSTRUCTION
# ============================================================================

def construct_install_command(packages: List[str], distro_info: Dict[str, Any]) -> str:
    """
    Build appropriate installation commands for each package manager.
    
    Args:
        packages: List of package names to install
        distro_info: Distribution information from detect_distribution()
        
    Returns:
        Complete installation command string
    """
    if not packages:
        return ""
    
    distro_id = distro_info.get("id", "unknown")
    install_cmd = distro_info.get("install_cmd", "")
    
    if not install_cmd:
        return ""
    
    # Get system package names for the detected distribution
    system_packages = []
    for package in packages:
        if package in SYSTEM_PACKAGE_MAP:
            distro_packages = SYSTEM_PACKAGE_MAP[package].get(distro_id, [])
            system_packages.extend(distro_packages)
    
    if not system_packages:
        return ""
    
    # Construct the full command
    if distro_id == "gentoo":
        # Gentoo uses emerge with specific syntax
        packages_str = " ".join(system_packages)
        return f"sudo {install_cmd} {packages_str}"
    else:
        # Most other distributions
        packages_str = " ".join(system_packages)
        return f"sudo {install_cmd} {packages_str}"


# ============================================================================
# AUTONOMOUS INSTALLATION
# ============================================================================

def auto_install_dependencies(packages: List[str], distro_info: Dict[str, Any]) -> bool:
    """
    Execute autonomous installation with timeout protection.
    
    Args:
        packages: List of packages to install
        distro_info: Distribution information
        
    Returns:
        True if installation succeeded, False otherwise
    """
    if not packages or distro_info.get("id") == "unknown":
        return False
    
    install_cmd = construct_install_command(packages, distro_info)
    if not install_cmd:
        return False
    
    print(f"Executing installation command: {install_cmd}")
    
    try:
        # Set a reasonable timeout (10 minutes for package installation)
        result = subprocess.run(
            install_cmd,
            shell=True,
            check=False,
            capture_output=True,
            text=True,
            timeout=30
        )
        
        if result.returncode == 0:
            print("✓ Installation completed successfully")
            return True
        else:
            print(f"✗ Installation failed with return code {result.returncode}")
            if result.stderr:
                print(f"Error output: {result.stderr.strip()}")
            return False
            
    except subprocess.TimeoutExpired:
        print("✗ Installation timed out after 10 minutes")
        return False
    except Exception as e:
        print(f"✗ Installation failed with exception: {e}")
        return False


# ============================================================================
# ERROR HANDLING
# ============================================================================

def handle_unknown_distro(distro_id: str) -> None:
    """
    Fatal error handling with manual installation instructions.
    
    Args:
        distro_id: The unknown distribution identifier
    """
    print(f"Error: Unsupported Linux distribution: {distro_id}", file=sys.stderr)
    print("", file=sys.stderr)
    print("RamSleuth requires system-native package installation for:", file=sys.stderr)
    print("  - i2c-tools (provides i2cdetect and decode-dimms)", file=sys.stderr)
    print("  - dmidecode", file=sys.stderr)
    print("  - python-textual (optional, for TUI mode)", file=sys.stderr)
    print("  - python-linkify-it-py (optional, for TUI help)", file=sys.stderr)
    print("", file=sys.stderr)
    print("Please install these packages manually using your distribution's package manager.", file=sys.stderr)
    print("", file=sys.stderr)
    print("Common package names by distribution:", file=sys.stderr)
    print("  - Arch Linux: i2c-tools dmidecode python-textual", file=sys.stderr)
    print("  - Debian/Ubuntu: i2c-tools dmidecode python3-textual", file=sys.stderr)
    print("  - Fedora/RHEL/CentOS: i2c-tools dmidecode python3-textual", file=sys.stderr)
    print("  - openSUSE: i2c-tools dmidecode python3-textual", file=sys.stderr)
    print("  - Gentoo: sys-apps/i2c-tools sys-apps/dmidecode dev-python/textual", file=sys.stderr)
    print("", file=sys.stderr)
    sys.exit(5)


def handle_installation_failure(missing_dependencies: Dict[str, List[str]]) -> None:
    """
    Handle installation failure with clear error messages.
    
    Args:
        missing_dependencies: Dictionary of missing system and python packages
    """
    system_missing = missing_dependencies.get("system", [])
    python_missing = missing_dependencies.get("python", [])
    
    print("Error: Failed to install required dependencies.", file=sys.stderr)
    
    if system_missing:
        print(f"Missing system tools: {', '.join(system_missing)}", file=sys.stderr)
    
    if python_missing:
        print(f"Missing Python packages: {', '.join(python_missing)}", file=sys.stderr)
    
    print("", file=sys.stderr)
    print("Please install the required packages manually and re-run RamSleuth.", file=sys.stderr)
    sys.exit(5)


# ============================================================================
# MAIN INTEGRATION FUNCTION
# ============================================================================

def check_and_install_dependencies(interactive: bool, requested_features: Dict[str, bool]) -> None:
    """
    Main integration function that orchestrates the entire dependency flow.
    
    Args:
        interactive: Whether running in interactive mode
        requested_features: Dictionary with feature flags:
            - "core": Core tools (i2cdetect, decode-dimms, dmidecode)
            - "tui": Textual TUI framework
            
    Behavior:
        - Detects distribution and package manager
        - Checks for missing dependencies
        - For interactive mode: attempts autonomous installation
        - For non-interactive mode: fails fast with clear guidance
        - Handles unknown distributions with fatal errors
    """
    # Detect distribution
    distro_info = detect_distribution()
    
    if distro_info["id"] == "unknown":
        handle_unknown_distro("unknown")
    
    # Check for missing dependencies
    missing_dependencies = get_missing_dependencies(requested_features)
    
    # If no missing dependencies, we're done
    if not missing_dependencies["system"] and not missing_dependencies["python"]:
        return
    
    # Non-interactive mode: fail fast with guidance
    if not interactive:
        print("Error: Missing required dependencies.", file=sys.stderr)
        
        if missing_dependencies["system"]:
            system_packages = []
            for tool in missing_dependencies["system"]:
                if tool in SYSTEM_PACKAGE_MAP:
                    packages = SYSTEM_PACKAGE_MAP[tool].get(distro_info["id"], [])
                    system_packages.extend(packages)
            
            if system_packages:
                install_cmd = construct_install_command(missing_dependencies["system"], distro_info)
                print(f"Please install: {', '.join(system_packages)}", file=sys.stderr)
                print(f"Command: {install_cmd}", file=sys.stderr)
        
        if missing_dependencies["python"]:
            python_packages = missing_dependencies["python"]
            print(f"Please install Python packages: {', '.join(python_packages)}", file=sys.stderr)
            if "textual" in python_packages:
                if distro_info["id"] == "arch":
                    print(f"Command: sudo {distro_info['install_cmd']} python-textual", file=sys.stderr)
                else:
                    print(f"Command: sudo {distro_info['install_cmd']} python3-textual", file=sys.stderr)
        
        print("", file=sys.stderr)
        print("Alternatively, re-run RamSleuth in interactive mode to attempt automatic installation.", file=sys.stderr)
        sys.exit(5)
    
    # Interactive mode: attempt autonomous installation
    print("RamSleuth has detected missing dependencies:", file=sys.stderr)
    
    if missing_dependencies["system"]:
        print(f"  - System Tools: {', '.join(missing_dependencies['system'])}", file=sys.stderr)
    
    if missing_dependencies["python"]:
        print(f"  - Python Libs: {', '.join(missing_dependencies['python'])}", file=sys.stderr)
    
    print("", file=sys.stderr)
    
    try:
        response = input("May I attempt to install them using your system's package manager (sudo)? [y/N] ")
        if response.lower() != 'y':
            print("Installation cancelled. Exiting.", file=sys.stderr)
            sys.exit(6)
    except (EOFError, KeyboardInterrupt):
        print("\nInstallation cancelled. Exiting.", file=sys.stderr)
        sys.exit(6)
    
    print("", file=sys.stderr)
    print("Attempting automatic installation...")
    print(f"Distribution: {distro_info['name']}")
    print(f"Package manager: {distro_info['package_manager']}")
    print("")
    
    # Install system packages
    if missing_dependencies["system"]:
        print("Installing system packages...")
        if not auto_install_dependencies(missing_dependencies["system"], distro_info):
            handle_installation_failure(missing_dependencies)
        
        # Verify installation
        still_missing = get_missing_dependencies(requested_features)
        if still_missing["system"]:
            print("✗ System packages still missing after installation attempt", file=sys.stderr)
            handle_installation_failure(still_missing)
    
    # Install Python packages
    if missing_dependencies["python"]:
        print("Installing Python packages...")
        if not auto_install_dependencies(missing_dependencies["python"], distro_info):
            handle_installation_failure(missing_dependencies)
        
        # Verify installation
        still_missing = get_missing_dependencies(requested_features)
        if still_missing["python"]:
            print("✗ Python packages still missing after installation attempt", file=sys.stderr)
            handle_installation_failure(still_missing)
    
    print("✓ All dependencies installed successfully")