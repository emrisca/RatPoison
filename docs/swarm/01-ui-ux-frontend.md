# 01 — UI/UX & Frontend Architecture (`RatPoison`)

> **Owner**: Sub-Agent 1 (UI/UX & Frontend Engineer)  
> **Scope**: `frontend/`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `docs/swarm/01-ui-ux-frontend.md`  
> **Stack**: Tauri v2 + Vite 6 + React 18 + TypeScript 5.6 (strict) + Zustand 5 + HTML5 Canvas 2D / SVG

---

## 1. Tech Stack & Click-Through Overlay Architecture

RatPoison ships a single lightweight Vite/React bundle rendered into **two isolated Tauri v2 WebView windows** routed via query string (`?view=overlay` vs `?view=settings`) with fallback to `getCurrentWebviewWindow().label`:

| Window Label | URL | Purpose | Key Window Flags |
| --- | --- | --- | --- |
| `overlay` (and `overlay-*`) | `index.html?view=overlay` | Transparent always-on-top HUD for Grid, Subgrid Zoom, Label Mode, and Free Mode | `transparent: true`, `decorations: false`, `alwaysOnTop: true`, `visibleOnAllWorkspaces: true`, `skipTaskbar: true`, `focus: false`, `shadow: false`, `visible: false`, `acceptFirstMouse: false` |
| `settings` | `index.html?view=settings` | Resizable, searchable 8-tab desktop preferences window opened from tray / hotkey | `resizable: true`, `decorations: true`, `center: true`, `visible: false`, `width: 1000`, `height: 680`, `minWidth: 780`, `minHeight: 540` |

### 1.1 Tauri v2 Configuration & Capabilities
- **`src-tauri/tauri.conf.json`**:
  - Sets `app.macOSPrivateApi: true` (required by Tauri v2 / wry to enable true alpha transparency on `WKWebView` / `NSWindow`).
  - Declares both `overlay` and `settings` windows hidden at boot (`visible: false`) so startup leaves zero visual flicker while keeping the overlay WebView **pre-warmed** in memory for instant activation.
  - Enforces a strict Content Security Policy (`default-src 'self'; img-src 'self' asset: data:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; script-src 'self'`).
- **`src-tauri/capabilities/default.json`**:
  - Grants scoped permissions to `["overlay", "overlay-*", "settings"]`, explicitly allowing `core:window:allow-set-ignore-cursor-events`, `core:window:allow-show/hide/set-position/set-size/set-always-on-top/set-visible-on-all-workspaces`, and `core:event:allow-listen/unlisten/emit`.

### 1.2 Platform WebView Click-Through & Always-on-Top Mechanics

| Platform | WebView Engine | Native Click-Through Mechanism | Always-on-Top & Full-Screen Space Caveats |
| --- | --- | --- | --- |
| **macOS** | `WKWebView` (AppKit `NSWindow` / `NSPanel`) | `window.set_ignore_cursor_events(true)` sets `NSWindow.ignoresMouseEvents = YES`. Frontend also enforces `pointer-events: none` on `html[data-view="overlay"]`. | **Critical macOS Full-Screen Space Caveat**: Standard `NSWindow` instances with `alwaysOnTop: true` (`NSFloatingWindowLevel`) will **not** appear over native macOS full-screen Spaces and may force a Space switch when shown. In `src-tauri/src/lib.rs`, the Rust shell MUST apply `NSWindowCollectionBehaviorCanJoinAllSpaces \| NSWindowCollectionBehaviorFullScreenAuxiliary \| NSWindowCollectionBehaviorStationary`, promote the window to `NSPanel` with `NSWindowStyleMaskNonactivatingPanel`, and elevate level to `kCGStatusWindowLevel` / `kCGScreenSaverWindowLevel` (e.g. `tauri-nspanel` or `objc2-app-kit`) so the overlay draws cleanly over full-screen apps without stealing keyboard focus or triggering Mission Control transitions. |
| **Windows** | `WebView2` (Chromium / DirectComposition) | `set_ignore_cursor_events(true)` applies extended Win32 styles `WS_EX_TRANSPARENT \| WS_EX_LAYERED` plus `WS_EX_NOACTIVATE \| WS_EX_TOOLWINDOW` (from `focus: false` + `skipTaskbar: true`). | Requires `WS_EX_TOPMOST` via `SetWindowPos(..., HWND_TOPMOST, SWP_NOACTIVATE)`. Exclusive-fullscreen DirectX/Vulkan games bypass DWM compositing; borderless-windowed apps work cleanly. Per-monitor DPI awareness V2 (`DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`) must be enabled so logical coordinates match `MonitorInfo.scaleFactor`. |
| **Linux (X11 & Wayland)** | `WebKitGTK` (`gtk3` / `webkit2gtk-4.1`) | **X11**: `set_ignore_cursor_events(true)` sets an empty XShape input region (`XShapeCombineRectangles` with `ShapeInput`). Requires a compositor (Picom, Mutter, KWin) for RGBA alpha transparency. **Wayland**: Standard `xdg_toplevel` cannot self-position or stay above all windows globally. | **Critical Wayland Caveat**: On wlroots/KDE compositors supporting `zwlr_layer_shell_v1`, the overlay should bind to the `OVERLAY` layer with `keyboard_interactivity = NONE` and an empty `wl_region` input mask. On GNOME/Mutter (which does not expose `wlr-layer-shell`), fallback requires either XWayland or a GNOME Shell extension bridge. WebKitGTK hardware acceleration (`WEBKIT_DISABLE_DMABUF_RENDERER` edge cases on NVIDIA) should be flagged in runtime diagnostics. |

### 1.3 Multi-Monitor Overlay Strategy
RatPoison exposes `general.multiMonitorStrategy` with three selectable modes:
1. **`active-follow` (Default, Recommended)**: Maintains a single pre-warmed `overlay` window. When the user triggers Grid/Label/Free mode (or presses `Tab` to cycle displays), Rust repositions and resizes `overlay` to the active `MonitorInfo.bounds` (`LogicalRect`) before emitting `ratpoison://overlay-state`. Lowest RAM footprint (~35–55 MB for one WebView) and avoids heterogeneous-DPI seam distortion.
2. **`per-monitor`**: Spawns one pre-warmed window per display (`overlay-<monitor_id>`, pre-authorized by `"overlay-*"` in `capabilities/default.json`). Each WebView renders at its native `window.devicePixelRatio` with zero resize latency when jumping across displays.
3. **`virtual-span`**: Spans one window across the bounding box of all monitors. Works on homogeneous-DPI X11/macOS setups, but **not recommended** on mixed-DPI Windows or macOS configurations with "Displays have separate Spaces" enabled (macOS clips windows at Space boundaries).

### 1.4 Sub-Frame Rendering Latency Budget (<16ms / 60–120Hz)
To stay well under a 16.6ms (60Hz) or 8.3ms (120Hz ProMotion) frame budget:
- **Zero cold-start cost**: Overlay window is pre-created at launch and toggled via `show()`/`hide()` (and `visible: true/false` payload flag).
- **Zero React reconciliation in Grid/Subgrid hot path**: `GridCanvas` subscribes directly to `useOverlayStore.subscribe()` (Zustand transient subscription) and schedules a single `requestAnimationFrame` callback. Typing a prefix character never re-renders React components for the 400–900 grid cells.
- **Batched Canvas 2D geometry**: All vertical and horizontal grid lines are accumulated into a single `ctx.beginPath()` / `ctx.stroke()` call, pixel-snapped (`Math.floor(coord) + 0.5`) for razor-sharp 1px hairlines on 1x and 2x displays, using `canvas.getContext('2d', { alpha: true, desynchronized: true })`.
- **GPU-composited DOM/SVG layers**: Label Mode pills and the Free Mode HUD animate exclusively via `transform: translate3d(x, y, 0)` and `will-change: transform, opacity` so the compositor never triggers layout reflows.
- **Measured paint target**: ~0.6–1.8ms per frame on an 18×26 (468-cell) or 26×36 (936-cell) Retina grid.

---

## 2. Dynamic Overlay Rendering Logic (`frontend/src/components/overlay/`)

### 2.1 Grid Mode (`GridCanvas.tsx`)
- Renders an arbitrary `gridRows × gridCols` matrix supplied by `OverlayStatePayload.gridCells` (logical coordinates mapped 1:1 onto the `devicePixelRatio`-scaled Canvas).
- **2-Character Case-Sensitive Labels**: Dynamically adapts font size to cell dimensions while honoring `appearance.gridFontFamily`, `gridFontSize`, and `gridFontWeight`, with a subtle dark drop shadow halo so labels stay readable over both pure white and dark wallpapers.
- **Live Prefix Filtering**:
  - When `typedPrefix.length > 0`, cells where `matchedPrefix === true` receive a subtle accent wash and render split-colored typography: typed prefix character(s) in `gridAccentColor`, remaining character(s) in `gridTextColor`.
  - Non-matching cells smoothly dim to `grid.dimOpacity` (or stay un-dimmed if `dimNonMatchingCells === false`).
  - Exact matches (`exactMatch === true`) draw a crisp 2px accent selection border.

### 2.2 Subgrid Zoom & Nudge Crosshair (`GridCanvas.tsx`)
- Triggered when `overlay.mode === 'subgrid'` and `overlay.subgrid !== null`.
- Softly dims the outer viewport, draws a dashed outline over the true physical footprint (`subgrid.parentRect`), and renders a magnified inset card scaled by `grid.subgridZoomScale` (clamped cleanly within monitor margins so edge cells never clip off-screen).
- Draws nested `subgrid.rows × subgrid.cols` subdivisions and labels, a header tag (`<parentCode> · nudge <nudgeStepPx>px`), and — when `grid.showSubgridCrosshair` is enabled — a full-inset precision **Nudge crosshair** with center target reticle and live logical `(x, y)` coordinate pill mapped proportionally from `subgrid.crosshair`.

### 2.3 Label Mode (`LabelLayer.tsx` + `colorUtils.ts`)
- Consumes accessibility bounding boxes (`overlay.labelBadges`) capped at `labelMode.maxBadges`.
- **Collision-Aware Placement (`resolveLabelCollisions`)**: Deterministic O(N×K) spiral search around each element's preferred anchor (`smart`, `top-left`, `center`, `top-right`), honoring precomputed Rust `collisionOffset` if present and clamping inside the monitor viewport.
- When a pill is nudged away from its anchor to prevent overlap, the SVG underlay draws a dashed leader line + anchor dot back to the exact interactive element, alongside subtle bounding-box outlines.
- Typing a prefix immediately filters non-matching badges out and splits matching badge codes into `labelMatchPrefixColor` (typed characters) + `labelTextColor` (remaining characters), with optional compact role tags (`BTN`, `LNK`, `TXT`, `CHK`).

### 2.4 Free Mode HUD (`FreeModeHud.tsx`)
- Renders a minimal, frosted-glass floating pill when `mode === 'free'` and `freeMode.showHudBadge === true`.
- Displays active mode (`FREE` / `SCROLL` / `DRAG`), speed profile (`SLOW` / `NORMAL` / `FAST`), live highlighted `I/J/K/L` directional keycaps, and `A/S/D/F` modifier indicators.
- Anchors to any screen corner (`top-left`, `top-right`, `bottom-left`, `bottom-right`) or follows the pointer (`cursor-follow`) via `translate3d` without ever intercepting mouse events.

---

## 3. Searchable Settings Window (`frontend/src/components/settings/`)

### 3.1 8-Tab Organization & Live Fuzzy/Substring Search
All settings are registered in a single canonical metadata catalog (`SETTING_REGISTRY` in `frontend/src/config/defaults.ts`) across 8 tabs:
1. **General** — Startup, tray icon, multi-monitor strategy, cursor hiding, click auto-dismiss, Esc cancellation.
2. **Grid & Subgrid** — NxM dimensions, case-sensitive charset, dimming opacity, subgrid rows/cols, zoom scale, coarse/fine nudge step, crosshair toggle.
3. **Free Mode** — Base velocity, fast/slow multipliers, acceleration curve, scroll speed, natural scroll, HUD placement, customizable `I/J/K/L` + `A/S/D/F` bindings.
4. **Label Mode** — A11y scanning toggle, hint charset, minimum element size, max badge cap, collision avoidance, anchor placement, menu bar / Dock scope, role micro-tags.
5. **Keybindings** — Interactive hotkey recorder (`HotkeyRecorderControl`) for mode activators, mouse clicks, hold/drag, monitor cycle, and Settings shortcut, complete with real-time duplicate chord conflict detection.
6. **Appearance** — Theme switcher (`Dark` / `Light` / `System`), live hex/native color pickers, opacity sliders, font-family preset + custom stack inputs, font-size/weight sliders, and an embedded interactive **`LiveOverlayPreview`** (switching between Grid + Subgrid, Label Badges, and Free HUD with simulated prefix chips).
7. **Licensing** — 14-day trial / Licensed status banner, 5-machine seat meter, online license key activation (`activate_license`), machine release (`deactivate_machine`), and collapsible **Offline Air-Gapped Challenge-Response** flow (`generate_offline_challenge` + `apply_offline_response`).
8. **Updates** — Release channel (`Stable` / `Beta`), auto-check & background download toggles, signed version banner, `check_for_updates` trigger, and formatted release notes.

**Live Search Engine (`scoreSettingMatch`)**:
- Focused anytime via `⌘K` / `Ctrl+K` or `/`; cleared via `Esc`.
- Scores exact label matches (1000), prefix matches (850), label substrings (700), keyword matches (620), dotted key paths (560), section headers (500), descriptions (420), multi-token AND queries (380), and ordered fuzzy subsequences (180+ with consecutive-character bonus).
- Displays live match counts on sidebar tabs and renders **directly editable** controls via the shared `SettingControlByPath` registry with highlighted query text and tab badges.

### 3.2 Right-Click "Restore Default" & Modified-State Tracking
- Every setting control is wrapped in `<SettingRow path="..." />`.
- **Modified Dot Indicator**: Compares `getByPath(config, path)` against `getByPath(DEFAULT_CONFIG, path)`. Modified settings display a glowing amber indicator dot (clicking it or right-clicking the row restores default), and each sidebar tab displays a count badge of modified settings.
- **Right-Click Context Menu (`RestoreContextMenu.tsx`)**: Right-clicking anywhere on a setting row opens a viewport-clamped floating menu showing:
  - Setting title and dotted config key (`e.g. appearance.gridAccentColor`)
  - Side-by-side **Current** vs **Default** value preview
  - **"Restore Default"** action invoking Tauri command `reset_setting_key(key)` (with optimistic rollback safety and toast confirmation)
  - **"Copy Setting Key"** utility action

---

## 4. Full Component Tree

```text
frontend/src/main.tsx  (resolves ?view=overlay | ?view=settings | window.label)
├── OverlayView                         [data-view="overlay"]
│   ├── GridCanvas                      (HiDPI Canvas 2D: NxM Grid + Subgrid Zoom + Nudge Crosshair)
│   ├── LabelLayer                      (SVG bounding boxes + collision leader lines + composited pills)
│   ├── FreeModeHud                     (Minimal corner/cursor-follow speed & modifier HUD)
│   └── OverlayDevBar                   (Browser-only preview toolbar; unmounted inside Tauri runtime)
│
└── SettingsView                        [data-view="settings"]
    ├── Sidebar
    │   ├── BrandHeader
    │   ├── GlobalSearchInput           (⌘K / '/' shortcut, live fuzzy/substring filter)
    │   ├── TabNavList                  (8 tabs with search match counts & modified setting counts)
    │   └── SidebarFooter               (Detected MonitorInfo count + guarded Reset All Defaults)
    ├── MainContentPane
    │   ├── ContentHeader               (Title, subtitle, per-tab Reset button)
    │   ├── ErrorBanner                 (Dismissible IPC error surface)
    │   ├── ScrollBody
    │   │   ├── SearchResultsCard       (When searchQuery non-empty -> SettingControlByPath list)
    │   │   ├── LicensingPanel          (Trial/Licensed status, 5-seat meter, online + offline activation)
    │   │   ├── UpdatesPanel            (Version status, Check for Updates, release notes, update prefs)
    │   │   └── TabSectionsStack
    │   │       ├── LiveOverlayPreview  (Mounted on Appearance tab: Grid+Subgrid / Label / HUD tabs)
    │   │       └── SectionCard[]
    │   │           └── SettingControlByPath[]
    │   │               └── SettingRow  (Modified dot + right-click context menu trigger)
    │   │                   └── Toggle | Slider | Select | Text | ColorPicker | FontFamily | HotkeyRecorder
    │   └── ToastNotification
    └── RestoreContextMenu              (Viewport-clamped right-click "Restore Default" menu)
```

---

## 5. CSS Token & Styling System (`frontend/src/styles/`)

- **`tokens.css`**: Defines semantic design tokens scoped to `:root[data-theme='dark']` (default) and `:root[data-theme='light']`:
  - Surfaces: `--rp-bg-app`, `--rp-bg-sidebar`, `--rp-bg-surface`, `--rp-bg-elevated`, `--rp-bg-input`, `--rp-bg-hover`
  - Borders: `--rp-border-subtle`, `--rp-border-strong`
  - Typography: `--rp-font-sans`, `--rp-font-mono`, `--rp-text-primary`, `--rp-text-secondary`, `--rp-text-muted`
  - Accents & States: `--rp-accent`, `--rp-accent-soft`, `--rp-modified-dot`, `--rp-modified-soft`, `--rp-success`, `--rp-danger`, `--rp-danger-soft`
  - Radii, shadows, and snappy motion tokens (`--rp-ease-snappy`, `--rp-duration-fast`)
- **`global.css`**: Enforces hard view separation:
  - `html[data-view='overlay']` locks `background: transparent !important; overflow: hidden; user-select: none; pointer-events: none;`.
  - `html[data-view='settings']` applies native-feeling desktop app background and theme tokens.
- **`overlay.css`**: Zero-reflow GPU-composited rules for `.rp-overlay-canvas`, `.rp-label-pill`, and `.rp-free-hud`.
- **`settings.css`**: Modular grid layout, search highlights, tactile toggles/sliders, color swatches, hotkey conflict chips, live preview mock window, and context menu styling.

---

## 6. State Management & Tauri IPC Bridge

### 6.1 Zustand Store Split
1. **`useOverlayStore` (`frontend/src/store/useOverlayStore.ts`)**:
   - Holds `overlay: OverlayStatePayload | null`, `config: RatPoisonConfig`, `frameRevision: number`, and `lastRenderMs: number`.
   - `GridCanvas` uses `useOverlayStore.subscribe()` transiently to draw on `requestAnimationFrame` without React re-renders.
   - `LabelLayer` and `FreeModeHud` subscribe via fine-grained selectors so they remain completely idle while in Grid/Subgrid mode.
2. **`useSettingsStore` (`frontend/src/store/useSettingsStore.ts`)**:
   - Holds `config`, `monitors`, `licenseStatus`, `offlineChallenge`, `updateResult`, `activeTab`, `searchQuery`, `contextMenu`, and transient toast/error feedback.
   - Performs optimistic updates on `updateSetting` / `resetSettingKey` with rollback on IPC rejection, and reconciles external changes emitted via `ratpoison://config-updated`.

### 6.2 IPC Contract (`frontend/src/ipc/tauriBridge.ts` & `frontend/src/types/ipc.ts`)
Aligned strictly with `docs/SWARM_CONTRACT.md`:
- **Events listened (Rust → Frontend)**:
  - `ratpoison://overlay-state` → `OverlayStatePayload` (logical `f64` rects/points + `MonitorInfo.scaleFactor`)
  - `ratpoison://config-updated` → `RatPoisonConfig`
- **Commands invoked (Frontend → Rust)**:
  - `get_config()` → `RatPoisonConfig`
  - `update_config({ config })` → `RatPoisonConfig`
  - `reset_setting_key({ key })` → `RatPoisonConfig`
  - `get_monitors()` → `MonitorInfo[]`
  - `get_license_status()` → `LicenseStatus`
  - `activate_license({ licenseKey })` → `LicenseStatus`
  - `deactivate_machine()` → `LicenseStatus`
  - `generate_offline_challenge()` → `OfflineChallenge`
  - `apply_offline_response({ responseBlob })` → `LicenseStatus`
  - `check_for_updates({ channel })` → `UpdateCheckResult`
- **Browser Dev Fallback**: When `isTauriRuntime()` is `false` (running `npm run dev` directly in a browser), `tauriBridge.ts` serves deterministic mock IPC handlers and a synthetic demo overlay state builder so UI work can be iterated independently of the Rust daemon.

---

## 7. Platform WebView Limitations & Orchestrator Synthesis Notes

1. **Rust Window Setup (`src-tauri/src/lib.rs`)**:
   - On startup, immediately call `overlay_win.set_ignore_cursor_events(true)` in Rust (do not wait solely for the JS defensive call).
   - On macOS, configure the overlay window with `NSPanel` non-activating behavior and `NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary` so it renders above native macOS full-screen apps without switching Spaces.
   - On Windows, verify `WS_EX_NOACTIVATE` is preserved across `show()` calls so invoking Grid Mode never blurs the user's active application window (otherwise Label Mode accessibility queries and clicks will target the wrong foreground window).
2. **Coordinate System Alignment (`ratpoison-core` & `ratpoison-a11y`)**:
   - All `LogicalRect` and `LogicalPoint` values emitted in `ratpoison://overlay-state` MUST be **relative to the active overlay window's origin** (`monitor.bounds.x`, `monitor.bounds.y` subtracted) in logical points (`physical / scaleFactor`), because the Canvas/SVG viewport spans `(0, 0) .. (monitor.bounds.width, monitor.bounds.height)`.
3. **JSON Serde Naming Convention**:
   - TypeScript IPC payloads currently use **camelCase** field names (e.g. `scaleFactor`, `gridCells`, `typedPrefix`, `licenseKeyMasked`). Rust structs serialized to the frontend should annotate `#[serde(rename_all = "camelCase")]` (and command arguments match Tauri v2's automatic `snake_case` ↔ `camelCase` JS mapping, e.g., Rust `license_key: String` ↔ JS `{ licenseKey }`).
4. **Bundle Icons**:
   - `src-tauri/tauri.conf.json` references standard icon paths under `src-tauri/icons/` (`32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`, `icon.ico`). Ensure placeholder or brand icons are generated before running `cargo tauri build`.
