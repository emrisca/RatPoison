# 04 — Cross-Platform Accessibility Engine, Flatpak Packaging & Wayland Portal Strategy

**Owner**: Sub-Agent 4 (Cross-Platform & Accessibility Agent)  
**Scope**: `crates/ratpoison-a11y/`, `packaging/flatpak/`, `docs/swarm/04-cross-platform-a11y.md`

---

## 1. Cross-Platform Accessibility Tree Engine (`crates/ratpoison-a11y`)

### 1.1 Unified Domain Model & `<120ms` Traversal Budget
Label Mode queries the operating system's Accessibility Tree to place instant keyboard hint badges over every actionable control on screen. Because modern desktop apps frequently embed massive Chromium/Electron or WebView DOM trees (often exceeding 15,000 nodes), unconstrained recursive tree walks can stall the UI thread for hundreds of milliseconds.

To guarantee Label Mode responds in **`<120ms`**, `ratpoison-a11y` enforces a strict `AccessibilityBudget`:
- `max_depth: 18` (default)
- `max_nodes: 1500` (default)
- `timeout: Duration::from_millis(110)` (default hard wall-clock cutoff)

Every platform adapter implements the unified `AccessibilityProvider` trait and emits `AccessibleElement` records normalized to logical screen points (`LogicalRect` / `LogicalPoint`):

```rust
pub struct AccessibleElement {
    pub id: String,
    pub role: ElementRole,
    pub title: Option<String>,
    pub bounds: Rect,          // LogicalRect { x, y, width, height } in f64 logical points
    pub center: LogicalPoint,  // LogicalPoint { x, y }
    pub is_clickable: bool,
    pub is_visible: bool,
    pub z_order: u32,          // 0 = frontmost window layer; higher = background windows
    pub app_pid: u32,
}
```

`ElementRole` normalizes toolkit-specific role taxonomies into:
`Button`, `Link`, `TextField`, `MenuItem`, `Tab`, `Checkbox`, `Radio`, `Slider`, `ListItem`, `WindowControl`, `Toolbar`, `Window`, `Container`, `StaticText`, `Image`, and `Unknown`.

---

### 1.2 Deterministic Label Mode Badge Generator (`crates/ratpoison-a11y/src/labels.rs`)

`generate_label_badges(elements, &LabelConfig)` transforms raw `AccessibleElement` snapshots into ergonomic `LabelBadge` items through a 4-stage pipeline:

1. **Visibility & Viewport Clipping**:
   - Prunes elements where `!is_visible` or neither `is_clickable` nor `role.is_default_interactive()` holds.
   - Computes the intersection of `element.bounds` with `config.viewport` (the active monitor's `LogicalRect`).
   - Discards off-screen elements and clipped boxes smaller than `min_dimension` (default `6.0` logical points).
2. **Z-Order Occlusion & Bounding-Box IoU / Centroid Deduplication**:
   - Candidates are ordered by `z_order` ascending (`0` = frontmost window), then by ascending bounding-box area (so specific inner icons/buttons win over redundant outer wrapper divs), then by deterministic ID.
   - **Near-Duplicate Pruning**: Any candidate whose bounding-box Intersection-over-Union (`IoU`) with an already-accepted element is `>= config.iou_threshold` (`0.65`) or whose center distance is `<= config.centroid_distance_threshold` (`8.0` pt) is discarded.
   - **Window Occlusion Pruning**: Any element at a deeper window layer (`cand.z_order > kept.z_order`) whose center lies inside a frontmost element and whose area is `>= 90%` covered by that frontmost element is discarded.
3. **Spatial Reading Order & Cursor Proximity Sorting**:
   - By default, surviving elements are quantized into 12-point horizontal reading bands (`(rect.y / 12.0).floor()`) and sorted top-to-bottom, left-to-right.
   - When `sort_by_cursor_proximity = true` and `cursor_position` is supplied, elements closest to the cursor are prioritized so the single-key home-row hints (`a`, `s`, `d`, `f`, ...) land nearest the pointer.
4. **Prefix-Free Home-Row-First Hint Assignment (`generate_prefix_free_hints`)**:
   - Uses the ergonomic home-row-first alphabet `"asdfghjklqwertyuiopzxcvbnm"` ($K = 26$).
   - If element count $N \le K$, emits single-character hints (`a`, `s`, `d`, ...).
   - If $N > K$, solves for the maximum number of single-character hints $S = \lfloor (K^2 - N) / (K - 1) \rfloor$ and reserves the remaining $K - S$ characters as two-character prefixes. This guarantees **strict prefix-freedom**: no single-character hint is ever the first character of any two-character hint, allowing immediate trigger execution without timeout delay.

---

## 2. Platform Accessibility Adapters & OS Quirks

### 2.1 macOS: `AXUIElement` (`crates/ratpoison-a11y/src/macos.rs`)
- **Framework**: `ApplicationServices.framework` (`AXUIElement*`) and `CoreFoundation.framework`.
- **Permissions**:
  - Checked via `AXIsProcessTrusted()`.
  - Prompted via `AXIsProcessTrustedWithOptions({ kAXTrustedCheckOptionPrompt: kCFBooleanTrue })`, which opens **System Settings → Privacy & Security → Accessibility**.
- **Traversal Strategy**:
  1. Calls `AXUIElementCreateSystemWide()` and reads `AXFocusedApplication` (or targets an explicit PID via `AXUIElementCreateApplication(pid)`).
  2. **Chromium / Electron / WebKit Activation**: Sets `AXEnhancedUserInterface = kCFBooleanTrue` and `AXManualAccessibility = kCFBooleanTrue` via `AXUIElementSetAttributeValue` on the application element. Without these attributes, Chrome, Brave, Edge, Arc, VS Code, Slack, and Discord lazily omit their renderer DOM accessibility trees.
  3. Queries `AXWindows` (falling back to `AXFocusedWindow`) and performs an iterative bounded DFS using `AXVisibleChildren` first (falling back to `AXChildren`) so off-screen table rows in huge scroll views are skipped at the OS level.
  4. Extracts `AXRole`, `AXSubrole`, `AXTitle` / `AXDescription`, `AXPosition` (`kAXValueCGPointType`), `AXSize` (`kAXValueCGSizeType`), and `AXUIElementCopyActionNames` (`AXPress`, `AXConfirm`, `AXShowMenu`, `AXPick`).

### 2.2 Windows: `UIAutomation` (`crates/ratpoison-a11y/src/windows.rs`)
- **API**: Windows `UIAutomation` COM API (`IUIAutomation`, `CLSID_CUIAutomation`).
- **Traversal & Batch RPC Caching**:
  - Cross-process COM calls per property are slow (~0.3–1.0 ms per call). To remain under `<120ms`, the Windows adapter must use `IUIAutomationCacheRequest` with `ControlViewCondition` and `TreeWalker::GetFirstChildElementBuildCache` / `GetNextSiblingElementBuildCache`.
  - Cached property IDs:
    - `UIA_ControlTypePropertyId` (`30003`)
    - `UIA_NamePropertyId` (`30005`)
    - `UIA_BoundingRectanglePropertyId` (`30001`)
    - `UIA_IsOffscreenPropertyId` (`30022`)
    - `UIA_IsEnabledPropertyId` (`30010`)
    - `UIA_ProcessIdPropertyId` (`30002`)
    - `UIA_IsInvokePatternAvailablePropertyId` (`30031`)
    - `UIA_IsTogglePatternAvailablePropertyId` (`30041`)
    - `UIA_IsSelectionItemPatternAvailablePropertyId` (`30036`)
    - `UIA_IsExpandCollapsePatternAvailablePropertyId` (`30028`)
- **DPI Scaling**:
  - `UIA_BoundingRectanglePropertyId` returns physical screen coordinates. `UiaElementRecord::into_accessible_element(scale_factor)` divides by the monitor's DPI scale factor (`GetDpiForMonitor` / `scale_factor: f64`) to produce normalized `LogicalRect` values matching Tauri's overlay coordinate space.
- **Elevated Windows Caveat (`UIAccess`)**:
  - To inspect or inject clicks into processes running as Administrator (elevated integrity level), the binary must be signed and include `<requestedExecutionLevel level="asInvoker" uiAccess="true" />` in its Windows application manifest, and be installed under `%ProgramFiles%`.

### 2.3 Linux: `AT-SPI2` (`crates/ratpoison-a11y/src/linux.rs`)
- **API**: D-Bus Accessibility Bus (`org.a11y.Bus` → `org.a11y.Bus.GetAddress`, then `org.a11y.atspi.Accessible`, `org.a11y.atspi.Component`, and `org.a11y.atspi.Action`).
- **Traversal**:
  - Queries `/org/a11y/atspi/accessible/root` on `org.a11y.atspi.Registry`.
  - Calls `Component.GetExtents(ATSPI_COORD_TYPE_SCREEN = 0)` for desktop-global coordinates and `Action.GetNActions` to detect clickable widgets.
- **Toolkit Environment Variables**:
  Many Linux desktops disable AT-SPI bridges unless assistive technology flags are exported in the session environment:
  - `ACCESSIBILITY_ENABLED=1` (Chromium, Electron, Firefox)
  - `GNOME_ACCESSIBILITY=1` (GTK2/GTK3 legacy applications)
  - `GTK_MODULES=gail:atk-bridge` (GTK3/GTK4)
  - `QT_ACCESSIBILITY=1` and `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` (Qt5/Qt6/KDE apps)

---

## 3. Linux Flatpak Packaging (`packaging/flatpak/`)

### 3.1 Files Created
- [`click.ratpoison.RatPoison.yml`](file:///Users/abelmora/Documents/Avalon/RatPoison/packaging/flatpak/click.ratpoison.RatPoison.yml) — Complete Flatpak Builder manifest using `org.gnome.Platform` / `org.gnome.Sdk` `47` (which bundles `webkit2gtk-4.1` required by Tauri v2) plus `rust-stable` and `node20` SDK extensions.
- [`click.ratpoison.RatPoison.metainfo.xml`](file:///Users/abelmora/Documents/Avalon/RatPoison/packaging/flatpak/click.ratpoison.RatPoison.metainfo.xml) — AppStream metadata compliant with Flathub validation (`appstreamcli validate`).
- [`99-ratpoison-uinput.rules`](file:///Users/abelmora/Documents/Avalon/RatPoison/packaging/flatpak/99-ratpoison-uinput.rules) — Host udev rule granting non-root `/dev/uinput` write access to the `input` group and logged-in seat user (`TAG+="uaccess"`).

### 3.2 Flatpak Sandbox Permissions (`finish-args`)

| Permission Flag | Purpose in RatPoison |
| :--- | :--- |
| `--socket=wayland` | Native Wayland display connection for Tauri WebKitGTK overlay |
| `--socket=fallback-x11` | X11 connection on Xorg or XWayland fallback |
| `--share=ipc` | X11 MIT-SHM shared memory segments |
| `--device=all` & `--device=input` | Direct access to `/dev/uinput` and `/dev/input/event*` for low-latency virtual pointer/keyboard injection when portals are unavailable |
| `--talk-name=org.a11y.Bus` | Connects to the session AT-SPI2 Accessibility Bus (`org.a11y.Bus`) to read widget trees |
| `--talk-name=org.freedesktop.portal.Desktop` | Invokes `org.freedesktop.portal.GlobalShortcuts` and `org.freedesktop.portal.RemoteDesktop` |
| `--talk-name=org.kde.StatusNotifierWatcher` | System tray icon integration |
| `--filesystem=xdg-config/ratpoison:create` | Persists `~/.var/app/click.ratpoison.RatPoison/config/ratpoison/config.json` and license state |

---

## 4. Wayland Integration & Portal Strategy

Wayland deliberately isolates clients from reading global keystrokes, warping the global cursor, or positioning arbitrary always-on-top windows. RatPoison overcomes these constraints using a tiered architecture across GNOME (Mutter), KDE Plasma (KWin), and wlroots compositors (Sway, Hyprland):

### 4.1 Tier 1: XDG Desktop Portals (`GlobalShortcuts` + `RemoteDesktop` + `libei`/EIS)
1. **Global Hotkey Capture (`org.freedesktop.portal.GlobalShortcuts`)**:
   - Supported on KDE Plasma 5.27+, GNOME 47+, and `xdg-desktop-portal-hyprland`.
   - RatPoison creates a portal session (`CreateSession`), calls `BindShortcuts` for the Grid Mode (`Alt+Space`) and Label Mode (`Alt+Shift+Space`) triggers, and listens to the `Activated` and `Deactivated` D-Bus signals.
2. **Compositor-Safe Input Injection (`org.freedesktop.portal.RemoteDesktop`)**:
   - Requests `DEVICE_POINTER | DEVICE_KEYBOARD` with `persist_mode = 2` (persist permission across restarts via portal restore token stored in `config.json`).
   - Calls `NotifyPointerMotionAbsolute(session_handle, options, stream, x, y)` and `NotifyPointerButton(session_handle, options, button, state)` (or connects to the Emulated Input Server `ConnectToEIS` fd via `libei`).
   - **Advantage**: Zero root privileges or udev rules required; works inside strict Flatpak sandboxes on GNOME and KDE.

### 4.2 Tier 2: `/dev/uinput` & `evdev` Fallback (wlroots / Sway / Hyprland)
- Once Grid Mode or Label Mode is activated, if the active compositor does not support `RemoteDesktop.NotifyPointerMotionAbsolute` or if the user prefers zero-portal latency:
  - RatPoison opens `/dev/uinput` (authorized via [`99-ratpoison-uinput.rules`](file:///Users/abelmora/Documents/Avalon/RatPoison/packaging/flatpak/99-ratpoison-uinput.rules) with `GROUP="input", MODE="0660", TAG+="uaccess"`).
  - Registers a virtual absolute pointer device (`UI_SET_EVBIT(EV_ABS)`, `ABS_X`, `ABS_Y` mapped to the desktop bounding box) and virtual mouse buttons (`BTN_LEFT`, `BTN_RIGHT`, `BTN_MIDDLE`).

### 4.3 Wayland Overlay Positioning (`zwlr_layer_shell_v1` vs WebKitGTK Windowing)
- **The Challenge**: Standard Wayland `xdg_toplevel` windows cannot position themselves at global `(x, y)` coordinates or float above fullscreen windows on demand.
- **Workarounds by Compositor**:
  1. **wlroots / Sway / Hyprland / KDE Plasma (`zwlr_layer_shell_v1`)**:
     - Attach the Tauri GTK window surface to the `ZWLR_LAYER_SHELL_V1_LAYER_OVERLAY` layer via `gtk-layer-shell` (`gtk_layer_init_for_window`), anchored to all four edges (`TOP | BOTTOM | LEFT | RIGHT`) of the target `gdk::Monitor` with exclusive zone `-1` and keyboard interactivity set to `ON_DEMAND` (or `EXCLUSIVE` while Grid/Label Mode is active).
  2. **GNOME Wayland (Mutter)**:
     - Mutter does not implement `zwlr_layer_shell_v1`. On GNOME Wayland, RatPoison sets the overlay window to fullscreen transparent mode on the target monitor (`window.set_fullscreen(true)` + `window.set_always_on_top(true)`) or runs the overlay window under XWayland (`GDK_BACKEND=x11` fallback) where `_NET_WM_STATE_ABOVE` and global coordinate placement remain supported.
  3. **AT-SPI2 Wayland Coordinate Caveat**:
     - Under pure Wayland clients (especially GTK4 on Wayland), `org.a11y.atspi.Component.GetExtents(ATSPI_COORD_TYPE_SCREEN)` may return window-relative coordinates `(0, 0)` because the client does not know its global screen offset.
     - **Mitigation**: Query window geometry from the compositor (via KWin scripting D-Bus API on KDE, `swaymsg -t get_tree` on Sway, `hyprctl clients -j` on Hyprland, or GNOME Shell Introspection D-Bus `org.gnome.Shell.Introspect.GetWindows`) and add the active window's top-left `(win_x, win_y)` offset whenever an application's root frame reports `(0, 0)` screen extents.
