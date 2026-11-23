# RatPoison

**RatPoison** is a 100% free and open-source (MIT), lightning-fast, memory-safe, keyboard-driven mouse control utility built with **Rust** and **Tauri v2**, inspired by [mouseless.click](https://mouseless.click).

## Core Modes & Capabilities

- **Grid / Overlay Mode**: Two-character case-sensitive NxM grid (supporting `>676` cells via mixed-case alphabets) → nested 3×3 Subgrid zoom → sub-pixel nudging → click/drag/scroll, with exact multi-monitor mixed-DPI affine coordinate math.
- **Free Mode**: Overlay-free relative cursor control (`I`, `J`, `K`, `L`), scrolling, unit-normalized diagonals, and stacked speed modifiers (`A`, `S`, `D`, `F`), plus Quick Select → Free Mode preset handoff.
- **Label Mode**: Budgeted (`<120ms` SLA) OS accessibility tree scanning (`AXUIElement` on macOS, `UIAutomation` on Windows, `AT-SPI2` on Linux) with IoU deduplication and prefix-free home-row badges.
- **Automatic OS-Native Keybindings**: Detects the host OS (`macOS`, `Windows`, `Linux`) on startup and configures conflict-free native modifier chords automatically (`Option+Space` / `Cmd+,` on macOS; `Ctrl+Alt+Space` / `Ctrl+,` on Windows; `Super+Alt+Space` / `Ctrl+,` on Linux), with one-click OS preset switching in Settings.
- **Strict Memory Hygiene**: Compile-time bounded `SecureKeyBuffer<const N: usize>` (`3 <= N <= 5`), volatile `0x00` zeroization on eviction/Escape/idle-timeout/`Drop`, `mlock` / `MADV_DONTDUMP` / `VirtualLock` page pinning, `RLIMIT_CORE=0`, redacted `Debug`, and compile-time prohibition on `Serialize` and `Display`.
- **Searchable Settings UI**: 7-tab React + TypeScript interface with `⌘K`/`Ctrl+K`/`/` live fuzzy search, live overlay preview, right-click **"Restore Default"** per setting key, and forward-compatible config preservation (`#[serde(flatten)]`).
- **100% Free & Open Source**: No trials, no license keys, no telemetry, no activation servers — MIT licensed forever.

## Repository Layout

```text
RatPoison/
├── Cargo.toml                         # Rust workspace root (4 crates)
├── crates/
│   ├── ratpoison-core/                # #![forbid(unsafe_code)] DPI math, Grid/Subgrid/Nudge, CoreEngine FSM, Config
│   ├── ratpoison-hid/                 # OS keyboard hooks, mouse simulation, SecureKeyBuffer, Hold-For/Cycle interceptor
│   ├── ratpoison-a11y/                # AXUIElement / UIAutomation / AT-SPI2 adapters & prefix-free Label Mode badges
│   └── ratpoison-tests/               # Multi-monitor DPI matrix, Memory Hygiene security audit & E2E pipeline tests
├── frontend/                          # Vite 6 + React 19 + TypeScript Overlay & 7-tab Searchable Settings UI
├── src-tauri/                         # Tauri v2 desktop binary, OS-aware IPC commands, tray menu & app icons (io.github.emrisca.RatPoison)
├── packaging/flatpak/                 # Flathub Flatpak manifest, AppStream metainfo & udev uinput rules
├── .github/workflows/release.yml      # Cross-platform macOS / Windows / Linux Tauri v2 build & GitHub Release pipeline
└── docs/
    ├── CLONE_ARCHITECTURE.md          # Unified Clone Architecture Document & Integration Conflict Matrix
    ├── SWARM_CONTRACT.md              # Cross-crate domain & IPC contract
    └── swarm/                         # Detailed subsystem specifications
```

## Quick Verification

```bash
# Type-check and build the Tauri overlay & settings frontend
npm --prefix frontend install
npm --prefix frontend run build

# Run all Rust workspace & Tauri shell tests
cargo test --workspace
cargo test --manifest-path src-tauri/Cargo.toml --features custom-protocol

# Build the standalone desktop binary with embedded UI assets
cargo build --release --manifest-path src-tauri/Cargo.toml --features custom-protocol

# Enforce zero Clippy warnings across all workspace targets
cargo clippy --workspace --all-targets -- -D warnings
```

## License

MIT — 100% Free and Open Source Software.
