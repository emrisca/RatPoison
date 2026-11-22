# RatPoison Swarm Contract & File Ownership

Repository: `/Users/abelmora/Documents/Avalon/RatPoison` (`https://github.com/emrisca/RatPoison`)
Stack: **Rust (Cargo workspace) + Tauri v2 + TypeScript/React + Vite**

## Strict File Ownership (Do Not Edit Other Agents' Files)

1. **UI/UX & Frontend Agent**
   - Code: `frontend/` (full Vite + React + TypeScript + CSS project), `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`
   - Doc: `docs/swarm/01-ui-ux-frontend.md`
2. **Core Engine & Algorithm Agent**
   - Code: `crates/ratpoison-core/`
   - Doc: `docs/swarm/02-core-engine-algorithms.md`
3. **Input/HID & Security Agent**
   - Code: `crates/ratpoison-hid/`
   - Doc: `docs/swarm/03-input-hid-security.md`
4. **Cross-Platform & Accessibility Agent**
   - Code: `crates/ratpoison-a11y/`, `packaging/flatpak/`
   - Doc: `docs/swarm/04-cross-platform-a11y.md`
5. **Licensing, Update & DevOps Agent**
   - Code: `crates/ratpoison-license/`, `.github/workflows/release.yml`
   - Doc: `docs/swarm/05-licensing-update-devops.md`
6. **Testing & QA Agent**
   - Code: `crates/ratpoison-tests/`
   - Doc: `docs/swarm/06-testing-qa.md`

## Shared Serialization & IPC Conventions
- Screen coordinates use `f64` logical points (`LogicalPoint { x, y }`, `LogicalRect { x, y, width, height }`) paired with `scale_factor: f64` (`MonitorInfo`) to convert cleanly to `PhysicalPoint { x: i32, y: i32 }`.
- Tauri overlay events emitted from Rust to Frontend:
  - `ratpoison://overlay-state` (mode, active monitor, grid cells, subgrid rect, label badges, cursor preview, active modifiers)
  - `ratpoison://config-updated` (full user settings JSON)
- Tauri commands invoked from Frontend to Rust:
  - `get_config`, `update_config`, `reset_setting_key`, `get_monitors`, `get_license_status`, `activate_license`, `deactivate_machine`, `generate_offline_challenge`, `apply_offline_response`, `check_for_updates`
