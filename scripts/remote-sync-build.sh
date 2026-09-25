#!/usr/bin/env bash
# Re-sync RamSleuth source from the SMB share and rebuild (release).
# Runs on the remote dev machine (elitegoat). Triggered over SSH.
set -euo pipefail

MNT="/mnt/ramsleuth-src"
PROJ="$HOME/ramsleuth-dev"

# Ensure the share is mounted
if ! mountpoint -q "$MNT"; then
    CRED="$HOME/.smb-ramsleuth.cred"
    mount -t cifs //192.168.51.145/scratch/RamSleuth "$MNT" \
        -o credentials="$CRED",uid=$(id -u),gid=$(id -g),iocharset=utf8
fi

# Sync source (preserve target/ for incremental builds)
rsync -a --exclude target --exclude .git "$MNT/" "$PROJ/"

cd "$PROJ"
cargo build --release 2>&1 | tail -5
echo "BUILD_OK $(git log -1 --format=%h 2>/dev/null || echo nongit) $(date -u +%FT%TZ)"
