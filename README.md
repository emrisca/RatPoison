# RatPoison

**RatPoison** is a lightning-fast, memory-safe, keyboard-driven mouse control utility built with **Rust** and **Tauri v2**, inspired by [mouseless.click](https://mouseless.click).

## Core Modes & Capabilities

- **Grid / Overlay Mode**: Two-character case-sensitive NxM grid (supporting `>676` cells via mixed-case alphabets) → nested 3×3 Subgrid zoom → sub-pixel nudging → click/drag/scroll, with exact multi-monitor mixed-DPI affine coordinate math.
- **Free Mode**: Overlay-free relative cursor control (`I`, `J`, `K`, `L`), scrolling, unit-normalized diagonals, and stacked speed modifiers (`A`, `S`, `D`, `F`), plus Quick Select → Free Mode preset handoff.
- **Label Mode**: Budgeted (`<120ms` SLA) OS accessibility tree scanning (`AXUIElement` on macOS, `UIAutomation` on Windows, `AT-SPI2` on Linux) with IoU deduplication and prefix-free home-row badges.
- **Strict Memory Hygiene**: Compile-time bounded `SecureKeyBuffer<const N: usize>` (`3 <= N <= 5`), volatile `0x00` zeroization on eviction/Escape/idle-timeout/`Drop`, `mlock` / `MADV_DONTDUMP` / `VirtualLock` page pinning, `RLIMIT_CORE=0`, redacted `Debug`, and compile-time prohibition on `Serialize` and `Display`.
- **Searchable Settings UI**: 8-tab React + TypeScript interface with `⌘K`/`/` live fuzzy search, live overlay preview, right-click **"Restore Default"** per setting key, and forward-compatible config preservation (`#[serde(flatten)]`).
- **5-Machine Licensing & Air-Gapped Offline Activation**: 14-day trial with monotonic clock-rollback guard, salted SHA-256 hardware fingerprinting, 5-seat deactivation swap, Ed25519 offline challenge-response tokens, and multi-OS update verification (Tauri Updater, Sparkle, WinSparkle, Flatpak OSTree).

## Repository Layout

```text
RatPoison/
├── Cargo.toml                         # Rust workspace root (5 crates)
├── crates/
│   ├── ratpoison-core/                # #![forbid(unsafe_code)] DPI math, Grid/Subgrid/Nudge, CoreEngine FSM, Config
│   ├── ratpoison-hid/                 # OS keyboard hooks, mouse simulation, SecureKeyBuffer, Hold-For/Cycle interceptor
│   ├── ratpoison-a11y/                # AXUIElement / UIAutomation / AT-SPI2 adapters & prefix-free Label Mode badges
│   ├── ratpoison-license/             # 14-day trial, 5-seat licensing, offline Ed25519 protocol, multi-OS updater
│   └── ratpoison-tests/               # Multi-monitor DPI matrix, Memory Hygiene security audit & E2E pipeline tests
├── frontend/                          # Vite 6 + React 19 + TypeScript Overlay & 8-tab Searchable Settings UI
├── src-tauri/                         # Tauri v2 window config & scoped capabilities (io.github.emrisca.RatPoison)
├── packaging/flatpak/                 # Flathub Flatpak manifest, AppStream metainfo & udev uinput rules
├── .github/workflows/release.yml      # Cross-platform macOS / Windows / Linux build, signing & release pipeline
└── docs/
    ├── CLONE_ARCHITECTURE.md          # Unified Clone Architecture Document & Integration Conflict Matrix
    ├── SWARM_CONTRACT.md              # Cross-crate domain & IPC contract
    └── swarm/                         # Detailed subsystem specifications (01 through 06)
```

## Quick Verification

```bash
# Run all 94 Rust unit, integration, security audit, and compile-fail tests
cargo test --workspace

# Enforce zero Clippy warnings across all workspace targets
cargo clippy --workspace --all-targets -- -D warnings

# Type-check and build the Tauri overlay & settings frontend
npm --prefix frontend install
npm --prefix frontend run build
```

## Architecture Documentation

- Start with **[docs/CLONE_ARCHITECTURE.md](docs/CLONE_ARCHITECTURE.md)** for the full synthesized system architecture, integration conflict resolutions, and verification summary.
- Subsystem specs:
  1. [01 — UI/UX & Frontend](docs/swarm/01-ui-ux-frontend.md)
  2. [02 — Core Engine & Algorithms](docs/swarm/02-core-engine-algorithms.md)
  3. [03 — Input/HID & Security](docs/swarm/03-input-hid-security.md)
  4. [04 — Cross-Platform & Accessibility](docs/swarm/04-cross-platform-a11y.md)
  5. [05 — Licensing, Updates & DevOps](docs/swarm/05-licensing-update-devops.md)
  6. [06 — Testing, QA & Security Audits](docs/swarm/06-testing-qa.md)

## License

MIT
