# Changelog

All notable per-release changes to RamSleuth. Newest first.

**Versioning policy.** The single source of truth for the version is `[workspace.package].version` in the root `Cargo.toml`; every member crate inherits it. A release = a version bump + the git tag `v<ver>` + the release workflow (`.github/workflows/release.yml`) publishing the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` + its `.sha256`. The AUR packages (`ramsleuth`, `ramsleuth-bin`, and the `ramsleuth-intel-dkms` extra) track this versioning and are maintained at the same pace as the project — with one deliberate exception: the `ryzen-smu-dkms` extra keeps its own `1.0` pkgver line (re-pinned per release wave via the commit pin + `pkgrel`, not version-bumped) and does not track the workspace version.

## [Unreleased]

## [2.4.16] - 2026-10-11

### Added
- **Single-source display: new shared `ramsleuth-display` crate** — the daemon is now the authoritative source for streamed values (burn-in elapsed is stamped from the daemon's own clock and forwarded in real time, with per-cell ticks re-stamped monotonically), and the TUI, GUI, and CLI all render through one shared implementation of the formatters, the N/A reason maps, and the CPU-temp scan instead of each frontend recomputing its own — removing a class of display divergence between frontends
- **Versioned wire protocol (`PROTOCOL_VERSION`)** — the 8-byte frame header now carries a protocol version alongside the length, with byte-pinned, length-prefixed payloads; a cross-generation frame is rejected with `VersionMismatch` (never a silent mis-parse), so the wire is forward-compatible across daemon/client versions
- **Latency-only benchmark run target** — a new `StreamTarget::Latency(Tier)` arm (append-only, byte-pinned) runs the DRAM pointer-chase pass only, with no bandwidth cells, exposed as a `--latency` flag on the standalone bench CLI

### Changed
- **Memory & Latency Benchmark per-tier fidelity (renamed from the retired "AIDA64-style" term)** — tier-aware write/copy kernels (the L1/L2/L3 cells are now cache-resident and no longer report DRAM; Memory uses non-temporal stores), a fixed 1M-hop latency chase (eliminating the ~30-min full-DRAM traversal, so a 5-minute burn-in can finish a pass), dead code + a duplicate latency kernel removed, and a release-path bounds check (a corrupted ring degrades to a non-counted 0.0 instead of UB)
- **AMD generality** — the AMD PM-table decode is now family/generation-keyed (Zen 4/5 and newer reach a live decode path instead of the whole section reading N/A, via a per-generation table-version plausibility filter), the AMD SMN layer is now unsafe-free, and the channel locator grammar is generalized (a board-keyed `Controller<digit>` profile + vendor-neutral rules)
- **Frontend/CLI hardening** — N/A reasons are user-facing (a human-readable phrase, not the Debug form) across the CLI, the standalone tools, and the dashboard; TUI/GUI layout rework (a per-surface erase idiom, a column-based GUI layout, a rendered parity note on the TUI/GUI graph surfaces); CLI dashboard/JSON consistency (the JSON carries the same fields the dashboard shows, including vCore); tier order aligned so every frontend renders the benchmark grid identically
- **Terminology** — "AIDA64-style" is purged repo-wide in favor of the canonical "Memory & Latency Benchmark"

### Fixed
- **RwLock poison recovery (TUI + GUI)** — a stray panic that poisons a shared-state lock now recovers the last-good value through poison-tolerant guard helpers, instead of re-panicking and cascading across both threads
- **Daemon telemetry-cache concurrency** — the cache is now a read/write-split `RwLock` (a fresh snapshot is a cheap shared read; a re-collect is an exclusive write), so concurrent TUI + GUI + CLI frontends no longer queue behind the full re-collect
- **Daemon burn-in duration re-clamp** — a client-supplied burn-in duration is re-clamped on the privileged daemon side to the sanctioned 0..=1440 range (`u32::MAX` can no longer become a ~138-year deadline)

## [2.4.15] - 2026-10-09

### Fixed
- **Intel single-channel misreport (SMBIOS Type 17 read)** — the `intel_smbios` cross-check read the DMI entry *directory* itself (`fs::read` → `EISDIR` → an empty topology, so the L3 firmware override never fired and a populated dual-channel kit was reported `Single-Channel`). It now reads the kernel's `17-<n>/raw` directory layout with the real SMBIOS 3.5 field offsets per DMTF DSP0134 Table 76 (the `Size` word at `0x0C` — bit 15 clear = MB, set = KB, the `0x7FFF`/`0xFFFF` sentinels promoting to the Extended Size dword at `0x1C`; Device Locator `0x10`, Bank Locator `0x11`, Memory Type `0x12`), the 1-based string references (real tables carry no leading empty segment), and the new `Controller<digit>` channel grammar (`0`→`A` … `3`→`D`, tried before the standalone `A`–`D` rules) — so the L3 override now fires **Dual** on real dual-channel hardware. The masked unit tests are unmasked, not weakened: `write_entry` mirrors the kernel dir+raw layout, `type17_file` drops the injected leading empty string, and two verbatim 183-byte 12600KF `17-0`/`17-1` fixtures (from the reference host's `/sys/firmware/dmi/entries`) parse to 2 × 16 GiB DDR4 Dual, with `resolve_channel_l3_override_on_real_topology` proving the override on that real topology
- **TUI graphs panel leaves stale zone values** — the panel's `inner` Rect is now blanked before the series rows paint (intra-frame layering — a `Block` is style-only and a `Paragraph` writes only its content width, so the zones painted earlier in the same fresh frame must be explicitly erased), and the ghosting guard is extended with `prev_graphs_shown` so the frame also clears on the `[g]` visible→hidden transition (`graph_panel_blanks_intra_frame_layering` proves none of the seeded glyphs survive)
- **TUI probe-report consent is a proper modal** — `render_probe_overlay` now blanks the whole frame (default surface) for both open phases before dispatching, and `render_consent` pads each line to the box inner width, so the consent/preview text is fully readable instead of floating over the live dashboard (`consent_blanks_the_frame_around_the_box` + `preview_still_covers_the_full_frame`)
- **Burn-in `m:ss` elapsed counter frozen between daemon ticks** — the counter now ticks live from a zone-local client clock (`BURN_IN_RUN_START`, the existing `NORMAL_RUN_START` pattern): latched on the burn-in start, rendered every 250 ms frame, backdated from the newest tick before the first frame, and cleared on terminal (`burn_in_row_uses_the_client_clock` + the fallback/clear tests; the status line and the burn-in row share the single source, so they can never disagree)
- **vCore `[est]` on the ASRock Z690M-ITX/AX** — a curated `ASROCK_Z690M_ITX_AX` VRM profile (key `Z690M-ITX/AX`, `INTEL_VCORE_RAILS`) + registry line (with a match/negative-control test) lands the reference board in the verified-seed tier, so its badge flips `[est]` → verified via the existing single-source `RailConfidence::badge()` (no display change)
- **Intel MCLK/MT/s doubling FIXED** — `decode_mclk` is now generation-keyed (11th-gen+ half-step ratio×refclk/2); DDR4-3600 now correctly reports 3600 MT/s across TUI/GUI/CLI; Skylake/Rocket decode anchors preserved

## [2.4.14] - 2026-10-09

### Added
- **Intel board-delivered vCore via the NCT6798 Super-I/O** — the Intel path now reports CPU vCore from the board's Super-I/O `in0` (the unattenuated direct VRM sense), using a hybrid strategy: a curated DMI-keyed seed profile (the high-volume `Z690` / `Z790` enthusiast / workstation boards) → `VerifiedProfile`; with no curated match, a consumer-vendor heuristic (Asus / ASRock / MSI / Micro-Star / Gigabyte) → `HeuristicDefault`; enterprise / OEM (Dell / HP / Lenovo / Supermicro) or unknown vendors abort to N/A (their BMCs / embedded controllers route primary power telemetry away from the Super-I/O); every reading is envelope-checked to **0.200–1.750 V** (out-of-bounds → N/A, never a garbage value). The `in0` delivered value is the primary; the processor VID is optional (`None` on Intel — there is no public Intel VID MSR). The new `RailConfidence` + `VoltageTelemetry` ride a trailing `SystemMemoryTelemetry.vcore` field that is now the **single vCore source of truth across the TUI, the GUI, and the CLI** (Intel = board `in0`; AMD = the SMU PM-table vCore, untouched)
- **AUR `ramsleuth` source package: native `target-cpus` codegen with a 3-tier safe fallback** — `build()` now probes `rustc --print cfg -C target-cpu=native` (Tier 1, full native tuning) → `-C target-cpu=x86-64-v2` (Tier 2, conservative baseline) → omit the flag (Tier 3, a non-x86 host or masked-VM CPUID), and **appends** the winning flag to any existing `makepkg.conf` `RUSTFLAGS` (never replaces it, no leading / trailing / double spaces, no manual feature pinning) so the build can never fail on an exotic host; `ramsleuth-bin` is unaffected (it ships the prebuilt release tarball, not a local build)

### Fixed
- **Intel CPUID disambiguation (full collision-set resolution)** — the pre-fix table keyed `IntelGen` on the CPUID model number alone, which mis-bucketed the shared-model collision set (`0x8E` / `0x9E` / `0xA5` / `0x97` / `0x9A` / `0xBA` — the model numbers that span two marketing generations; `0xB7` / `0xBF` Raptor Lake are *non-colliding* and stay model-authoritative); most consequentially `0x97` (Alder Lake-S, the i5-12600KF reference part) resolved to **Tiger Lake** and degraded the entire Intel readout to `Na(UnsupportedHardware)`. `detect()` now resolves every collision against the brand string's marketing generation (`intel_gen_from_brand`) — `0x97` + 13th-gen → Raptor Lake, `0x97` + 12th-gen / absent brand → Alder Lake (the fix), `0xA5` + 11th-gen → Rocket Lake, Core-Ultra-Series-1 tokens → Core Series 1 Mobile — with the model table as the conservative base and every non-colliding model staying model-authoritative. `IntelGen` is **append-only and wire-pinned to indices 0–15** (the new variants were appended at 11–15, never reordered), so the frozen wire encoding is preserved
- **Intel memory-channel detection (capacity-derived mode + 4-tier trust + SMBIOS cross-check)** — the user-visible channel mode was read straight off one IMC register, which misreported a populated dual-channel kit as single-channel. It is now **capacity-derived** and reconciled across two independent sources by a 4-tier trust procedure (`resolve_channel`): **L1 VerifiedHardware** (the MMIO decode, primary authority), **L2 VerifiedFirmware** (MMIO blocked → the SMBIOS population), **L3 Override** (the safety fix — the MMIO says `Single` but SMBIOS Type 17 confirms ≥ 2 DIMMs across channels → report **Dual**, flagged "untrusted-MMIO mismatch"), **L4 Heuristic** (MMIO blocked + ambiguous locators → dual, "unverified"), and a terminal N/A. The new `intel_smbios` module reads the kernel-exported SMBIOS Type 17 "Memory Device" structures **unprivileged** (`std::fs`-only, no EDAC) as the cross-check. The decode's global MAD register base migrates `0x5000` → `0xd800` for the 12th-gen+ (Alder Lake and later) family, selected **in-kernel** by the module from the CPU model it computes via `cpuid_eax(1)`; the DDR5 geometry is labelled `Dual-Channel (4x 32-bit Subchannels)` by the single-source `display_label` (consumed by both the TUI and the GUI). Two substantive sub-fixes ride in the same bullet: the per-channel `MAD_DIMM_CHx` slot words are now decoded with the **corrected 0.5 GiB 7-bit layout** (`DIMM_L_SIZE` at `[6:0]` + `DIMM_S_SIZE` at `[22:16]` — the pre-fix decode read a non-existent 1 GiB 6-bit layout, so a 16 GiB DIMM mis-decoded as 8 GiB), and the **`/dev/mem` fallback now reads the MAD registers at the generation-appropriate base** (`imc_base_for`: `0xd800` for 12th-gen+, `0x5000` otherwise) — previously it always passed `None` for those raws, so the channel mode was always `None` on that path

### Changed
- **Documentation + display alignment** — the vCore confidence badge is now a single unified **`[est]`** form across the CLI, TUI, and GUI (the per-frontend `[est]` / `[Unverified]` / `[UNVERIFIED]` spellings are gone), with `Docs/User_Guide.md` §8.3 and `Docs/Architecture.md` aligned to it; the Intel `gen_gate` unsupported-path message now names the full Tier 1 / 2 / 3 decode set (and its three doc sites match); the probe report renders the single-source `channel mode` label plus the 4-tier `channel annotation` reason; and the `ramsleuth` PKGBUILD header documents the 3-tier native `target-cpu` build posture.

## [2.4.13] - 2026-09-27

### Changed
- **Documentation pass** — the Intel driver README is now a full how-it-works reference (probe sequence, MCHBAR decode, all 25 register semantics, Tier 1/2/3 + the module coverage gap, DKMS build incl. Secure Boot, known hardware limitations); User_Guide / README / Architecture / packaging staleness fixed through 2.4.12; module MODULE_VERSION now tracks releases (2.4.13); dev logs consolidated to one canonical log.

## [2.4.12] - 2026-09-27

### Changed
- **The TUI info screen (`[d]`) is now a clean CONTROLS reference**: the redundant app description (what RamSleuth is, the daemon model, the capability summary) is removed — that is the docs' job (`Docs/User_Guide.md`) — and the screen now shows the full 17-key set as a responsive `key — action` grid grouped by function (DATA / BENCH / VIEWS), one to three columns by terminal width, so it uses the horizontal space well (the old fixed 10-row ABOUT block is replaced by a width-dependent height: 10 rows at ≥ 87 columns, 14 at 58–86, 22 below; the key set is pinned to the `events` contract by test so the screen and the handler table cannot drift)

## [2.4.11] - 2026-09-27

### Changed
- **The one-click setup is now genuinely one click**: a single `Set up RamSleuth` action runs `ramsleuth-setup --with-dkms` with the driver decided by the host's CPU vendor (AMD → `ryzen_smu`, Intel → `ramsleuth_intel`, other/unknown → daemon-only) — daemon enable+start, group join, the per-user socket ACL, and the offline DKMS driver build + `modprobe` all in ONE privileged pass on the first click (the requirement-list-based `--with-dkms` decision that left a second, driver-only stage behind on the daemon-down first launch is gone — the flag is now the host-vendor decision)
- **The GUI auto-reconnects to the daemon after setup (no manual restart)**: the one-click setup worker raises a post-setup reconnect edge that the background poller consumes with one immediate poll — independent of the refresh gate — so the app picks up the (re)started daemon on its own and the setup banner unmounts the moment the daemon serves (the C21-36 "Restart" modal is removed; the per-user ACL means the current session connects with no re-login and no app restart)
- **The setup script's polkit restart no longer kills a live session's auth agent mid-run**: `ramsleuth-setup` now restarts polkit ONLY when the branded `org.freedesktop.ramsleuth.setup` action is missing from the running polkitd's pool AND the policy file postdates the running polkitd (the true stale-pool case) — a mid-run polkitd restart drops the desktop session's registered polkit agent, which was the root cause of the prior two-stage setup loop (a polkitd stuck on a policy it never loads gets no mid-run restart; a reboot clears it)
- **The setup copy is now truthful**: the banner says "no reboot needed" and the contradictory re-login line is gone (the one-click setup applies a per-user ACL, so the current session connects immediately — the manual `usermod` command alone still activates at re-login)

## [2.4.10] - 2026-09-27

### Changed
- **One-click setup is now fully non-interactive on a fresh machine**: the DKMS helpers install build tooling with `pacman -S --needed --noconfirm`, so the GUI/pkexec path no longer stalls on a `Proceed? [Y/n]` prompt (the confirmed root cause of the fresh-install setup loop); the ryzen helper's git-clone fallback also sets `GIT_TERMINAL_PROMPT=0`, so a credential prompt can never hang the flow
- **The setup flow logs its full output + a state snapshot to `/var/lib/ramsleuth/setup.log` (world-readable)**: every run of `ramsleuth-setup` is self-documenting (a run header, the whole stdout + stderr transcript, and a unit/module/socket/group snapshot — appended before the done line and via an EXIT trap after every failure), the vendor helpers inherit the stream (a deduped tee guard), and the app shows that path on any setup failure so problems are diagnosable off-band
- **Daemon startup is journaled**: every (re)start writes a line to the journal (`journalctl -u ramsleuth`) with the version + the resolved args (socket path, max-age, spd-autobind), anchoring the startup output that already reaches the journal

## [2.4.9] - 2026-09-26

### Changed
- **Self-explanatory N/A (GUI + TUI)**: every `N/A` now shows the reason in plain language (e.g. "No data — this CPU doesn't expose the memory-controller registers RamSleuth reads. Expected on this part, not an error") with the daemon status kept **green** — "no data on this part" (unsupported hardware, unloaded driver, privilege, decode) is rendered as a healthy state, never mistaken for a failure
- **The setup prompt trigger is now a named, tested liveness predicate** (`first_run::requirements_strip_visible`): daemon down → the one-click setup prompt shows (fresh install, stopped daemon, and leftover-partial install all converge on it); daemon up → telemetry (the strip clears the moment the daemon serves, even when the served telemetry is all `N/A`)

## [2.4.8] - 2026-09-26

### Changed
- **Idempotent one-click DKMS install over a stale residual module** (both the AMD `ryzen_smu` and Intel `ramsleuth_intel` helpers): a re-run over an already-installed module (a residual `.ko` of the same module left by a prior build, or a wiped DKMS DB) previously aborted on DKMS's identical-module check ("already installed at version … override by specifying --force"); both helpers now pass `dkms install --force`, which overwrites the residual (a no-op on a clean first run) — so a re-click **always** succeeds with no manual cleanup
- **The broken `/usr/lib/depmod.d/<module>.conf` override is removed** from both helpers — the `override <mod> /extra/<mod>.ko` line is invalid syntax `depmod` rejects and is pointless on dkms 3.4.3 (`DEST_MODULE_LOCATION "/extra"` never materializes; the module deploys to `updates/dkms/` and resolves from there) — plus a **self-heal** that removes a stale entry an older helper left behind
- **The DKMS self-heal removal now uses the valid `dkms remove <module>/<version> --all --no-depmod` cleanup syntax** (verified against the installed dkms 3.4.3 CLI, which rejects `--all-kernels`), so the helper removes every registered version of the module before re-adding from the freshly staged source
- **The `ramsleuth_intel` module's `MODULE_VERSION` now tracks releases** (was the constant `1.0.0`; now `2.4.8`) with a track-releases note — the AMD vendored `ryzen_smu` module's `MODULE_VERSION` (`0.1.7`) is left untouched (a SUMS-verified frozen upstream copy; its DKMS version is the git-derived PKGVER, not that constant)
- **The makepkg 7.x source-integrity pin fix is carried forward**: all three git-source AUR packages (`ramsleuth`, `ramsleuth-intel-dkms`, `ryzen-smu-dkms`) keep the standard VCS `#commit=`-fragment source + content-addressed `sha256sums` (the sha256 of the pinned commit's `git archive` tarball) — the form makepkg 7.x generates (`makepkg -g`) and its integrity gate verifies (a bare `-` fails the gate on 7.x; `SKIP` passes only as a no-op)

## [2.4.7] - 2026-09-26

### Added
- **Secure-Boot-aware one-click DKMS** (both the AMD `ryzen_smu` and Intel `ramsleuth_intel` helpers): on a Secure Boot host the helper now detects Secure Boot **early** (`mokutil --sb-state`, with an EFI/lockdown fallback), **signs** the built module with a **persistent** RamSleuth key pair (generated once, idempotent, 10-year, at `/var/lib/ramsleuth/<module>-signing/`, wired through a per-module `/etc/dkms/framework.conf.d/` drop-in), **stages the cert for the one-time MOK enrollment** (`mokutil --import`), and exits **10** with **one-step guidance** (reboot → at the blue MOK screen choose `Enroll MOK key(s)` → `Continue` → `Yes` → re-click Setup). After the MOK step the re-click succeeds — the driver is already built + installed + signed, so nothing is rebuilt
- **Never-mysterious diagnostics**: the one-click flow surfaces the real failure — the GUI renders the first `ERROR:` line on stderr **plus the exit code** (`(exit N)`) verbatim, and maps exit **10** to an amber **"one step left"** state (the Secure Boot MOK enrollment is pending) **instead of a failure**; `ramsleuth-setup` passes the helper's exit codes through verbatim

### Changed
- **Idempotent, self-healing setup**: `ramsleuth-setup` now clears its own stale prior-run state at the start of **every** invocation — a `FAILED` unit (`reset-failed`), the `ramsleuth` group (ensured *before* the unit starts), the `authorized-users` state file (atomic self-healing overwrite: keep valid users, ensure ours, drop blanks/dups), a stale daemon socket (`restart`), and a stale polkit action pool (`restart`) — so a re-run **always** succeeds with no manual cleanup (the 2.4.5-failed → uninstall → 2.4.6-still-fails class is closed); the DKMS helpers likewise self-heal by removing **every registered version** of the module (`dkms remove … --all --no-depmod`) before re-adding from the freshly staged source
- **The repo's `ryzen-smu-dkms/dkms.conf` (`DEST=/extra`) now ships on every install path** — the release tarball (27 → **28** artifacts: 6 binaries + 22 auxiliary), both main AUR packages (new guarded `package()` step 14 → `/usr/share/ryzen-smu-dkms/dkms.conf`), and `install.sh` — so the DKMS helper resolves the repo's config (matching its `depmod` override) at its installed location; the vendored copy's `/kernel/drivers/ryzen_smu` is the last-resort fallback only

## [2.4.6] - 2026-09-26

### Added
- The AMD `ryzen_smu` driver source is now **bundled in both main packages** (`ramsleuth`, `ramsleuth-bin` → `/usr/share/ryzen-smu-dkms/vendor/`, SUMS-verified, guarded for pre-vendor tags) — the in-app one-click installs the AMD driver **offline** on a clean install (no git clone, no separate AUR extra), symmetric with the bundled Intel module source

### Changed
- The `ryzen-smu-dkms` AUR extra is now **mutually exclusive** with the main packages (like `ramsleuth-intel-dkms`): both bundle the same vendored source, so installing the extra removes a main package first — it is the **standalone provisioning path** only
- The daemon `CAP_SYS_RAWIO` privilege probe checks bit **17** (`CAP_SYS_RAWIO`) instead of bit 21 (`CAP_SYS_ADMIN`) — fixes the false `InsufficientPrivilege` warning on correctly-privileged daemons
- polkit branded-dialog fix: the inert `exec.arguments` annotation is removed (not a real polkit key), and the install paths (the AUR `.install` hooks + `install.sh`) restart polkit so the `org.freedesktop.ramsleuth.setup` action loads on install — the one-click prompt shows RamSleuth's branded message
- The release tarball grows to 27 artifacts (6 binaries + 21 auxiliary, including the 8 vendored `ryzen_smu` files); `ryzen-smu-dkms` keeps its own version line (pkgver 1.0, branch-pinned) and does not track the workspace version

## [2.4.5] - 2026-09-25

### Added
- ECC detection on **both** platforms: AMD via `UmcCapHi` (bit 30/31) and Intel via `CAPID0_A` (a new `capid0a` sysfs attribute — the 25th — decoded from bit 17)
- AMD channel-mode detection from the SMN CS-population readout (Single / Dual-Channel (Symmetric) / Dual-Channel (Flex))
- a 25-attribute `ramsleuth_intel` kernel module (adds `capid0a`)

### Changed
- SPD decoder re-baselined to **JESD79-4 Annex L**: correct maker / die-maker JEP106, density, rank, width, serial, XMP 2.0 profiles, and JEDEC base speed
- XMP 2.0 timing decode (tRCD / tRP / tRAS)
- Intel channel-mode DIMM-population cross-check: a firmware Dual-Symmetric demotes to Flex on asymmetric population
- SPD EEPROM auto-bind now falls back to the `ee1004` driver `bind` file (one attempt per process lifetime; the per-collect warning loop is gone)
- TUI/GUI polish: terminal ghosting fix, panel layout, header rework, probe modal, and the title-only GitHub issue URL

## [2.4.2] - 2026-09-25

### Added
- Intel generational expansion: Tier 1 (Skylake/Kaby), Tier 2 (Coffee Lake/Rocket Lake with Gear2 + brand-based gen disambiguation), Tier 3 (Alder/Raptor/Meteor/Arrow Lake with DDR5 4-subchannel, Gear4, 256 KiB window, MCL fallback, tile-routing probe)
- In-app "Submit Probe Report" feature: consent-gated markdown report (telemetry + raw IMC registers + system info) with GUI (GitHub issue / clipboard) and TUI (file / clipboard) flows
- `.github/ISSUE_TEMPLATE/probe-report.md` for pre-filled issue submission

### Changed
- Packaging: both main AUR packages now bundle `kernel/ramsleuth-intel/` source (guarded install); mutual exclusion with the standalone `ramsleuth-intel-dkms` extra
- Intel IMC decode now dispatches via `GenProfile` (window size, MCHBAR mask, channel count, gear cap, register map)

## v2.4.0 (2026-09-23)

**Intel hardening + ergonomics** — channel-mode decode, SPD auto-bind, and the MCLK/MCHBAR fixes that completed Intel parity:

- **Added:** five new Intel MAD channel/geometry registers (`mad_inter_channel`, `mad_intra_ch0`, `mad_intra_ch1`, `mad_dimm_ch0`, `mad_dimm_ch1`) exposed by the `ramsleuth_intel` module — sysfs attributes 19 → 24.
- **Added:** the hardware-derived Intel channel-mode label — the header channel label and the `Mode:` slot now come from the `MAD_INTER_CHANNEL` register (e.g. "Dual-Channel (Flex)" for asymmetric DIMMs) when the module is loaded, falling back to the installed-DIMM count otherwise.
- **Added:** SPD EEPROM auto-bind fallback — the daemon (as root) now attempts to bind SPD EEPROMs the kernel's `ee1004` driver missed (common on boards whose DSDT advertises a single DIMM slot). On by default; disable with `--no-spd-autobind`. Non-fatal and never unbinds.
- **Fixed:** the Intel MCLK decode — removed an erroneous ÷2 (MCLK = `CLK_RATIO` × refclk; MT/s = 2 × MCLK). A DDR4-2133 system now reports 1066.67 MHz instead of 533.33 MHz.
- **Fixed:** the Intel MCHBAR window mapped 1 MiB → 64 KiB (the Tier-1 datasheet window) in both the runtime and `/dev/mem` fallback maps, avoiding overlap with adjacent host-bridge BARs.
- **Fixed (DKMS):** the Intel DKMS install script now always rebuilds and reloads the module from the current source on re-run (previously an already-loaded module was left stale).
- **Changed:** docs — corrected the Intel MCLK formula and DDR4-2400 fixture (ratio 9 @ 133.3333 MHz refclk) in `Docs/Architecture.md` and the research doc; updated the `Docs/User_Guide.md` Intel sections (channel mode, SPD auto-bind, 24 attributes).

**Operator-gated release steps (out of scope for this commit):** the git tag `v2.4.0`, the release re-cut, the `ramsleuth-bin` tarball sha256 re-finalization (the in-tree pin is still the published v2.2.1 asset), and the AUR resubmits (`ramsleuth`, `ramsleuth-bin`, `ramsleuth-intel-dkms`).

## v2.3.0 (2026-09-23)

**Intel full parity** — live DRAM subtimings on Intel, matching the AMD experience:

- **Added:** live Intel DRAM subtimings (v1 / Tier-1: Skylake, Kaby Lake, Coffee Lake, Comet Lake — DDR4). The new, in-repo original `ramsleuth_intel` kernel module (written by the project, no upstream) exposes the raw IMC registers under `/sys/kernel/ramsleuth_intel/`, provisioned by the `ramsleuth-intel-dkms` AUR extra, the `install-intel-dkms.sh` operator helper, and the now-vendor-aware `ramsleuth-setup.sh`. Telemetry is sysfs-first with a `/dev/mem` fallback (used on `DriverMissing` only).
- **Fixed:** the Intel MCHBAR decode (bit 0 is `MCHBAR_EN`, not a PCI I/O-space flag — previously every enabled MCHBAR was rejected → all-N/A); the IMC register map (corrected to the verified Tier-1 layout: `MC_BIOS_REQ@0x5E00`, per-channel `TC_*` @ `0x4000`/`0x4400`).
- **CI:** an independent `kernel-module` build job (fails on any compiler warning).
- **Fixed (MSRV):** the pre-existing 1.75 test-profile E0382 in `facade.rs` — the MSRV test job is green again.

**Operator-gated release steps (out of scope for this commit):** the git tag `v2.3.0`, the release re-cut, the `ramsleuth-bin` tarball sha256 re-finalization, and the AUR resubmits (`ramsleuth`, `ramsleuth-bin`, `ramsleuth-intel-dkms`).

## v2.2.1 (2026-09-22)

**TUI parity** — the terminal TUI now matches the GUI:

- **16-key contract** (case-insensitive, modifiers ignored): bench `b` / memory-only `m` / burn-in `x` / cancel `c`; graphs `g`; settings `t`; requirements `d`; JSON export `e`; poll `p` / capacity units `u` / clock units `k` / refresh `a` / graphs window `w` — plus the original refresh `r` / snapshot `s` / quit `q`.
- **5-series graphs overlay** over a 1800-sample ring.
- **Requirements strip** — a mirror of the GUI `SETUP` (presence-driven prerequisites).
- **JSON export** to `$HOME` (`{ telemetry, bench }`) — F3 parity.

**CI portability:** vendor-conditional telemetry tests + graceful MCHBAR `BAR5=0` degradation (Intel + AMD + virtualized hosts).

**TUI display:** bare grey `N/A` (no parse-error detail); lighter-grey titles.

## v2.2.0

The one-click setup / first-run release: the in-app **Set up RamSleuth** (one polkit pass: daemon enable + start, `ramsleuth` group join, current-session socket ACL — no re-login), the offline-vendored `ryzen-smu` DKMS extra, and the establishment of the **15-artifact release contract** (the published tarball + `.sha256`).

## v2.1.1

Published milestone (release tag `v2.1.1`).

## v2.1.0

Published milestone (release tag `v2.1.0`).

## v2.0.0

Published milestone (release tag `v2.0.0`) — the RamSleuth v2 launch.
