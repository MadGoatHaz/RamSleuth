#!/usr/bin/env bash
# build-deb.sh — reproducible .deb build for RamSleuth
#
# Takes the release tarball (or a local repo checkout) and produces a
# Debian binary package with the same file set as the AUR PKGBUILDs.
#
# Usage:
#   packaging/build-deb.sh --version <ver> [--tarball <path>]
#
#   --version   Required. The package version (e.g. 2.4.30).
#   --tarball   Optional. Path to the release tarball. Defaults to
#               ramsleuth-<ver>-x86_64.tar.zst in the current directory.
#               If the tarball does not exist, the script builds from
#               the current repo checkout (must be run from the repo root).
#
# Output: ramsleuth_<ver>_amd64.deb in the current directory.
#
# The script is idempotent: it cleans its build directory on every run.
# It uses dpkg-deb --build when available (Debian/Ubuntu hosts), and
# falls back to the manual ar/tar format on other hosts (e.g. Arch).

set -euo pipefail

VERSION=""
TARBALL=""

usage() {
    echo "Usage: $0 --version <ver> [--tarball <path>]" >&2
    echo "" >&2
    echo "  --version   Package version (required), e.g. 2.4.30" >&2
    echo "  --tarball   Release tarball path (optional)." >&2
    echo "              Defaults to ramsleuth-<ver>-x86_64.tar.zst." >&2
    echo "              If absent, builds from the current repo checkout." >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || { echo "Error: --version requires a value" >&2; exit 1; }
            VERSION="$2"; shift 2 ;;
        --tarball)
            [ $# -ge 2 ] || { echo "Error: --tarball requires a value" >&2; exit 1; }
            TARBALL="$2"; shift 2 ;;
        -h|--help)
            usage ;;
        *)
            echo "Error: unknown argument '$1'" >&2
            usage ;;
    esac
done

# Validate version (or read from Cargo.toml if not given).
# The script lives in packaging/; resolve the workspace Cargo.toml by
# walking up from the current directory (repo root, packaging/, or a
# nested subdir). Prefer `cargo metadata` (authoritative workspace
# version) and fall back to a targeted grep on the [workspace.package]
# version key.
if [ -z "$VERSION" ]; then
    CARGO_TOML=""
    for rel in "" ".." "../.."; do
        candidate="${rel:+$rel/}Cargo.toml"
        if [ -f "$candidate" ]; then
            CARGO_TOML="$candidate"
            break
        fi
    done
    if [ -n "$CARGO_TOML" ]; then
        # Authoritative: ask cargo for the workspace version (fast with
        # --no-deps; first package shares [workspace.package] version).
        VERSION=$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
                  | grep -o '"version":"[^"]*"' | head -1 \
                  | sed 's/"version":"//;s/"//')
        # Fallback: grep the [workspace.package] version key directly.
        if [ -z "$VERSION" ]; then
            VERSION=$(grep -m1 '^version = ' "$CARGO_TOML" | sed 's/version = "\(.*\)"/\1/')
        fi
    fi
fi

if [ -z "$VERSION" ]; then
    echo "Error: no version specified and could not read from Cargo.toml" >&2
    exit 1
fi

case "$VERSION" in
    [0-9]*.[0-9]*.[0-9]*[-+a-zA-Z0-9.]*) ;;
    *)
        echo "Error: invalid version '$VERSION' (expected MAJOR.MINOR.PATCH, optional -pre/+build)" >&2
        exit 1 ;;
esac

# Determine source: tarball or local repo
if [ -z "$TARBALL" ]; then
    TARBALL="ramsleuth-$VERSION-x86_64.tar.zst"
fi

# Save original working directory for the output
ORIG_CWD=$(pwd)

# Clean build directory
BUILD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/ramsleuth-deb.XXXXXX")
trap 'rm -rf "$BUILD_DIR"' EXIT

PKG_DIR="$BUILD_DIR/pkg"
DEBIAN_DIR="$PKG_DIR/DEBIAN"

OUTPUT="$ORIG_CWD/ramsleuth_${VERSION}_amd64.deb"

echo "Building ramsleuth_${VERSION}_amd64.deb"
echo "Source: $TARBALL"
echo "Build dir: $BUILD_DIR"

# Prepare the dpkg data tree
mkdir -p "$PKG_DIR" "$DEBIAN_DIR"

# Resolve the source root
SRC_ROOT=""
if [ -f "$TARBALL" ]; then
    echo "Extracting tarball..."
    EXTRACT_DIR="$BUILD_DIR/extract"
    mkdir -p "$EXTRACT_DIR"
    tar --extract --file "$TARBALL" --directory "$EXTRACT_DIR"
    # The tarball's top-level dir is ramsleuth-<ver>/
    SRC_ROOT=$(find "$EXTRACT_DIR" -maxdepth 1 -type d -name "ramsleuth-$VERSION" | head -1)
    if [ -z "$SRC_ROOT" ]; then
        # Fallback: any top-level dir (tarball layout changed)
        SRC_ROOT=$(find "$EXTRACT_DIR" -maxdepth 1 -type d | head -1)
    fi
    if [ -z "$SRC_ROOT" ] || [ ! -d "$SRC_ROOT" ]; then
        echo "Error: could not find extracted tarball contents" >&2
        exit 1
    fi
    # Validate the source root actually looks like a RamSleuth checkout:
    # it must contain at least one of the known top-level dirs, otherwise
    # an unexpected layout would silently produce a broken package.
    if [ ! -d "$SRC_ROOT/bin" ] && [ ! -d "$SRC_ROOT/systemd" ] \
       && [ ! -d "$SRC_ROOT/crates" ] && [ ! -f "$SRC_ROOT/Cargo.toml" ]; then
        echo "Error: extracted dir '$SRC_ROOT' does not look like a RamSleuth source tree (no bin/, systemd/, crates/, or Cargo.toml)" >&2
        exit 1
    fi
    echo "Source root: $SRC_ROOT"
else
    # No tarball — build from local repo
    # Determine if we're in the repo root or in packaging/
    if [ -f "Cargo.toml" ] || [ -f "../Cargo.toml" ] || [ -f "../../Cargo.toml" ]; then
        if [ -f "Cargo.toml" ]; then
            SRC_ROOT="."
        elif [ -f "../Cargo.toml" ]; then
            SRC_ROOT=".."
        else
            SRC_ROOT="../.."
        fi
        echo "No tarball found ($TARBALL); building from local repo at $SRC_ROOT"
    else
        echo "Error: tarball '$TARBALL' not found and not in a repo checkout" >&2
        exit 1
    fi
fi

# --- Copy files into the dpkg data tree ---

# Helper: copy a file if it exists, else skip (no-panic contract)
copy_file() {
    local src="$1" dest="$2" mode="${3:-644}"
    if [ -f "$src" ]; then
        install -Dm"$mode" "$src" "$dest"
    fi
}

# (1) 6 binaries
for b in ramsleuth-daemon ramsleuth-client ramsleuth-tui ramsleuth ramsleuth-bench ramsleuth-telemetry; do
    if [ -f "$SRC_ROOT/bin/$b" ]; then
        install -Dm755 "$SRC_ROOT/bin/$b" "$PKG_DIR/usr/bin/$b"
    elif [ -f "$SRC_ROOT/target/release/$b" ]; then
        install -Dm755 "$SRC_ROOT/target/release/$b" "$PKG_DIR/usr/bin/$b"
    else
        echo "Error: binary $b not found" >&2
        echo "Hint: run 'cargo build --release --workspace' first, or pass --tarball pointing to a release tarball." >&2
        exit 1
    fi
done
echo "  (1) 6 binaries"

# (2) systemd unit
copy_file "$SRC_ROOT/systemd/ramsleuth.service" "$PKG_DIR/usr/lib/systemd/system/ramsleuth.service" 644
echo "  (2) systemd unit"

# (3) systemd preset
if [ -f "$SRC_ROOT/systemd/ramsleuth.preset" ]; then
    install -Dm644 "$SRC_ROOT/systemd/ramsleuth.preset" "$PKG_DIR/usr/lib/systemd/system-preset/ramsleuth.preset"
elif [ -f "$SRC_ROOT/packaging/shared/ramsleuth.preset" ]; then
    install -Dm644 "$SRC_ROOT/packaging/shared/ramsleuth.preset" "$PKG_DIR/usr/lib/systemd/system-preset/ramsleuth.preset"
fi
echo "  (3) systemd preset"

# (4) AMD DKMS helper
if [ -f "$SRC_ROOT/scripts/install-ryzen-smu-dkms.sh" ]; then
    install -Dm755 "$SRC_ROOT/scripts/install-ryzen-smu-dkms.sh" "$PKG_DIR/usr/bin/ramsleuth-install-ryzen-smu-dkms"
fi
echo "  (4) AMD DKMS helper"

# (5) Intel DKMS helper (guarded)
if [ -f "$SRC_ROOT/scripts/install-intel-dkms.sh" ]; then
    install -Dm755 "$SRC_ROOT/scripts/install-intel-dkms.sh" "$PKG_DIR/usr/bin/ramsleuth-install-intel-dkms"
fi
echo "  (5) Intel DKMS helper"

# (6) install.sh
if [ -f "$SRC_ROOT/install.sh" ]; then
    install -Dm755 "$SRC_ROOT/install.sh" "$PKG_DIR/usr/share/ramsleuth/install.sh"
fi
echo "  (6) install.sh"

# (7) LICENSE
if [ -f "$SRC_ROOT/LICENSE" ]; then
    install -Dm644 "$SRC_ROOT/LICENSE" "$PKG_DIR/usr/share/licenses/ramsleuth/LICENSE"
fi
echo "  (7) LICENSE"

# (8) Desktop file
if [ -f "$SRC_ROOT/RamSleuth.desktop" ]; then
    install -Dm644 "$SRC_ROOT/RamSleuth.desktop" "$PKG_DIR/usr/share/applications/RamSleuth.desktop"
elif [ -f "$SRC_ROOT/packaging/shared/RamSleuth.desktop" ]; then
    install -Dm644 "$SRC_ROOT/packaging/shared/RamSleuth.desktop" "$PKG_DIR/usr/share/applications/RamSleuth.desktop"
fi
echo "  (8) Desktop file"

# (9) One-click setup helper
if [ -f "$SRC_ROOT/ramsleuth-setup.sh" ]; then
    install -Dm755 "$SRC_ROOT/ramsleuth-setup.sh" "$PKG_DIR/usr/bin/ramsleuth-setup"
elif [ -f "$SRC_ROOT/scripts/ramsleuth-setup.sh" ]; then
    install -Dm755 "$SRC_ROOT/scripts/ramsleuth-setup.sh" "$PKG_DIR/usr/bin/ramsleuth-setup"
fi
echo "  (9) One-click setup helper"

# (10) Polkit policy (guarded)
if [ -f "$SRC_ROOT/90-ramsleuth-setup.policy" ]; then
    install -Dm644 "$SRC_ROOT/90-ramsleuth-setup.policy" "$PKG_DIR/usr/share/polkit-1/actions/90-ramsleuth-setup.policy"
elif [ -f "$SRC_ROOT/packaging/polkit/90-ramsleuth-setup.policy" ]; then
    install -Dm644 "$SRC_ROOT/packaging/polkit/90-ramsleuth-setup.policy" "$PKG_DIR/usr/share/polkit-1/actions/90-ramsleuth-setup.policy"
fi
echo "  (10) Polkit policy"

# (11) Icons (guarded, hicolor tree)
# Icons can be at $SRC_ROOT/icons/ (tarball layout) or $SRC_ROOT/assets/icons/ (repo layout)
ICON_ROOT=""
if [ -d "$SRC_ROOT/icons/hicolor" ]; then
    ICON_ROOT="$SRC_ROOT/icons/hicolor"
elif [ -d "$SRC_ROOT/assets/icons/hicolor" ]; then
    ICON_ROOT="$SRC_ROOT/assets/icons/hicolor"
fi
if [ -n "$ICON_ROOT" ]; then
    for size in 16 24 32 48 64 128 256 512; do
        icon_src="$ICON_ROOT/$size/apps/ramsleuth.png"
        if [ -f "$icon_src" ]; then
            install -Dm644 "$icon_src" "$PKG_DIR/usr/share/icons/hicolor/$size/apps/ramsleuth.png"
            install -Dm644 "$icon_src" "$PKG_DIR/usr/share/icons/hicolor/${size}x${size}/apps/ramsleuth.png"
            install -Dm644 "$icon_src" "$PKG_DIR/usr/share/icons/hicolor/$size/apps/RamSleuth.png"
            install -Dm644 "$icon_src" "$PKG_DIR/usr/share/icons/hicolor/${size}x${size}/apps/RamSleuth.png"
        fi
    done
    icon_48="$ICON_ROOT/48/apps/ramsleuth.png"
    if [ -f "$icon_48" ]; then
        install -Dm644 "$icon_48" "$PKG_DIR/usr/share/pixmaps/ramsleuth.png"
        install -Dm644 "$icon_48" "$PKG_DIR/usr/share/pixmaps/RamSleuth.png"
    fi
fi
echo "  (11) Icons"

# (12) Intel DKMS source tree (guarded)
if [ -d "$SRC_ROOT/kernel/ramsleuth-intel" ]; then
    for f in dkms.conf Makefile ramsleuth_intel.c README.md; do
        if [ -f "$SRC_ROOT/kernel/ramsleuth-intel/$f" ]; then
            install -Dm644 "$SRC_ROOT/kernel/ramsleuth-intel/$f" "$PKG_DIR/usr/share/ramsleuth-intel-dkms/src/$f"
        fi
    done
fi
echo "  (12) Intel DKMS source tree"

# (13) AMD ryzen_smu vendor tree (guarded)
if [ -d "$SRC_ROOT/packaging/ryzen-smu-dkms/vendor/ryzen-smu" ]; then
    for f in LICENSE Makefile dkms.conf drv.c smu.c smu.h apob.c apob.h; do
        if [ -f "$SRC_ROOT/packaging/ryzen-smu-dkms/vendor/ryzen-smu/$f" ]; then
            install -Dm644 "$SRC_ROOT/packaging/ryzen-smu-dkms/vendor/ryzen-smu/$f" "$PKG_DIR/usr/share/ryzen-smu-dkms/vendor/ryzen-smu/$f"
        fi
    done
    if [ -f "$SRC_ROOT/packaging/ryzen-smu-dkms/vendor/SUMS.sha256" ]; then
        install -Dm644 "$SRC_ROOT/packaging/ryzen-smu-dkms/vendor/SUMS.sha256" "$PKG_DIR/usr/share/ryzen-smu-dkms/vendor/SUMS.sha256"
    fi
    if [ -f "$SRC_ROOT/packaging/ryzen-smu-dkms/vendor/NOTICE.md" ]; then
        install -Dm644 "$SRC_ROOT/packaging/ryzen-smu-dkms/vendor/NOTICE.md" "$PKG_DIR/usr/share/ryzen-smu-dkms/vendor/NOTICE.md"
    fi
fi
echo "  (13) AMD ryzen_smu vendor tree"

# (14) AMD ryzen_smu dkms.conf (guarded)
if [ -f "$SRC_ROOT/packaging/ryzen-smu-dkms/dkms.conf" ]; then
    install -Dm644 "$SRC_ROOT/packaging/ryzen-smu-dkms/dkms.conf" "$PKG_DIR/usr/share/ryzen-smu-dkms/dkms.conf"
fi
echo "  (14) AMD ryzen_smu dkms.conf"

# --- Generate DEBIAN/control ---
cat > "$DEBIAN_DIR/control" <<EOF
Package: ramsleuth
Version: $VERSION
Architecture: amd64
Maintainer: MadGoatHaz <madgoathaz@users.noreply.github.com>
Depends: systemd, acl, libxkbcommon0, libx11-6, libxrandr2, libxi6, libxcursor1, libxinerama1, libwayland-client0, libgl1
Recommends: dkms, linux-headers
Description: Pure-Rust RAM latency/bandwidth telemetry daemon + clients
 RamSleuth is a pure-Rust system for live RAM telemetry and memory/
 latency benchmarking on AMD and Intel desktops. It ships a privileged
 root daemon (CAP_SYS_RAWIO only, Unix-socket IPC) feeding dual
 frontends (ratatui TUI + egui GUI), plus CLI and standalone tools.
 .
 Binaries: ramsleuth-daemon, ramsleuth-client, ramsleuth-tui, ramsleuth
 (GUI), ramsleuth-bench, ramsleuth-telemetry, plus DKMS helpers and
 systemd integration.
EOF
echo "  DEBIAN/control"

# --- Generate DEBIAN/preinst ---
cat > "$DEBIAN_DIR/preinst" <<'EOF'
#!/bin/sh
set -e
# The daemon unit uses Group=ramsleuth; the group must exist on the
# target system or the service fails to start. Create it idempotently.
getent group ramsleuth >/dev/null 2>&1 || groupadd -r ramsleuth
exit 0
EOF
chmod 0755 "$DEBIAN_DIR/preinst"
echo "  DEBIAN/preinst"

# --- Generate DEBIAN/postinst ---
cat > "$DEBIAN_DIR/postinst" <<'EOF'
#!/bin/sh
set -e
systemctl daemon-reload 2>/dev/null || true
# polkit: (re)install the one-click policy; restart polkitd so the
# branded org.freedesktop.ramsleuth.setup action is live.
systemctl restart polkit 2>/dev/null || true
if ! systemctl enable --now ramsleuth.service 2>/dev/null; then
    echo "ramsleuth: ramsleuth.service not started (non-fatal);" \
         "run: systemctl enable --now ramsleuth" >&2
fi
# Zero-touch: when the install ran under sudo, grant the invoking user
# group membership + current-session socket access.
if [ -n "${SUDO_USER:-}" ]; then
    case "$SUDO_USER" in
        [A-Za-z_][A-Za-z0-9_-]*) : ;;
        *) SUDO_USER="" ;;
    esac
fi
if [ -n "$SUDO_USER" ] && [ "$SUDO_USER" != "root" ] \
    && id -u "$SUDO_USER" >/dev/null 2>&1; then
    if ! id -nG "$SUDO_USER" 2>/dev/null | tr ' ' '\n' | grep -qx -- ramsleuth; then
        usermod -aG ramsleuth "$SUDO_USER" 2>/dev/null || true
        echo "ramsleuth: ${SUDO_USER} added to group 'ramsleuth' (persistent; effective at next login)"
    fi
    install -d -m 0755 /etc/ramsleuth 2>/dev/null || true
    if ! grep -qx -F -- "$SUDO_USER" /etc/ramsleuth/authorized-users 2>/dev/null; then
        printf '%s\n' "$SUDO_USER" >> /etc/ramsleuth/authorized-users 2>/dev/null || true
        echo "ramsleuth: ${SUDO_USER} seeded in /etc/ramsleuth/authorized-users (current-session access, no re-login)"
    fi
    chmod 0644 /etc/ramsleuth/authorized-users 2>/dev/null || true
    systemctl restart ramsleuth.service 2>/dev/null || true
fi
echo "ramsleuth: installed — start the GUI with \`ramsleuth\` (the TUI with \`ramsleuth-tui\`)"
echo "ramsleuth: one-click setup — 'Set up RamSleuth' in the GUI, or: sudo ramsleuth-setup"
exit 0
EOF
chmod 0755 "$DEBIAN_DIR/postinst"
echo "  DEBIAN/postinst"

# --- Generate DEBIAN/postrm ---
cat > "$DEBIAN_DIR/postrm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = "remove" ]; then
    systemctl stop ramsleuth.service 2>/dev/null || true
fi
if [ "$1" = "purge" ]; then
    systemctl disable ramsleuth.service 2>/dev/null || true
fi
systemctl daemon-reload 2>/dev/null || true
exit 0
EOF
chmod 0755 "$DEBIAN_DIR/postrm"
echo "  DEBIAN/postrm"

# --- Normalize ownership ---
if command -v chown >/dev/null 2>&1; then
    chown -R 0:0 "$PKG_DIR" || true
fi

# --- Build the .deb ---
if command -v dpkg-deb >/dev/null 2>&1; then
    echo "Using dpkg-deb to build the package..."
    dpkg-deb --build --root-owner-group "$PKG_DIR" "$OUTPUT"
else
    echo "dpkg-deb not found; using manual ar/tar format..."
    # debian-control.tar.gz
    (cd "$DEBIAN_DIR/.." && \
     tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='@0' \
         -cf - DEBIAN | gzip -9n > "$BUILD_DIR/control.tar.gz")
    # debian-data.tar.gz (everything except DEBIAN/)
    (cd "$PKG_DIR" && \
     tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='@0' \
         -cf - --exclude=./DEBIAN . | gzip -9n > "$BUILD_DIR/data.tar.gz")
    # debian-binary
    printf '2.0\n' > "$BUILD_DIR/debian-binary"
    # Assemble with ar (cd back to original dir so the output lands there)
    (cd "$ORIG_CWD" && \
     ar rc "$OUTPUT" "$BUILD_DIR/debian-binary" "$BUILD_DIR/control.tar.gz" "$BUILD_DIR/data.tar.gz")
fi

# --- Verify ---
if command -v dpkg-deb >/dev/null 2>&1; then
    echo ""
    echo "=== dpkg-deb --info ==="
    dpkg-deb --info "$OUTPUT"
    echo ""
    echo "=== dpkg-deb -c (file list) ==="
    dpkg-deb -c "$OUTPUT"
else
    echo ""
    echo "=== Package structure (no dpkg-deb) ==="
    file "$OUTPUT"
    ar t "$OUTPUT"
fi

echo ""
echo "Built: $OUTPUT ($(du -h "$OUTPUT" | cut -f1))"

# --- Create sha256 sidecar ---
sha256sum "$OUTPUT" > "${OUTPUT}.sha256"
echo "Checksum: ${OUTPUT}.sha256"
