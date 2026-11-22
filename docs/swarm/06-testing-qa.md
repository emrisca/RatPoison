# RatPoison Testing, QA & Security Audit Specification (`06-testing-qa.md`)

**Agent**: Sub-Agent 6 — Testing, QA & Security Audits  
**Owned Crate**: [`crates/ratpoison-tests`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-tests)  
**Target Workspace**: `/Users/abelmora/Documents/Avalon/RatPoison`

---

## 1. Executive Summary

`crates/ratpoison-tests` implements the cross-crate integration, multi-monitor mixed-DPI coordinate verification, and Memory Hygiene security audit test suite for **RatPoison** (a high-performance Rust + Tauri v2 clone of Mouseless). It directly wires and validates all four domain crates in the Cargo workspace:
- [`ratpoison-core`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-core): Multi-monitor mixed-DPI coordinate geometry, NxM grid & nested subgrid subdivision, >676 case-sensitive label alphabets, precision nudging, Free Mode velocity/acceleration physics, and deterministic state machine.
- [`ratpoison-hid`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-hid): Compile-time bounded (`3 <= N <= 5`) [`SecureKeyBuffer`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-hid/src/memory_hygiene.rs), OS page-locked [`LockedSecureKeyBuffer`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-hid/src/memory_hygiene.rs), volatile zeroization (`0x00`), anti-core-dump hardening (`RLIMIT_CORE = 0`, `PR_SET_DUMPABLE = 0`), zero-leak Hold-For / Cycle [`CommandInterceptor`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-hid/src/interceptor.rs), and [`MockHidBackend`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-hid/src/platform/mock.rs).
- [`ratpoison-a11y`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-a11y): Cross-platform OS Accessibility Tree traversal (`AXUIElement`, `UIAutomation`, `AT-SPI2`, [`MockAccessibilityProvider`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-a11y/src/mock.rs)) with `<120ms` wall-clock budget, IoU/occlusion pruning, and prefix-free home-row-first 1–2 character hint generation.
- [`ratpoison-license`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-license): 14-day evaluation trial, monotonic clock-rollback detection (`300s` NTP tolerance), 5-machine seat licensing with deactivation prompt & swap flow, offline/air-gapped Ed25519 challenge-response token verification, and update manifest verification.

---

## 2. Multi-Monitor Mixed-DPI QA Matrix

### 2.1 Virtual Desktop Topology & Scale Matrix

RatPoison operates on per-monitor local affine transforms:

$$\text{Physical}(p) = \text{PhysicalOrigin}_m + \operatorname{round}\!\big((\text{Logical}(p) - \text{LogicalOrigin}_m) \times s_m\big)$$

$$\text{Logical}(p_{\text{phys}}) = \text{LogicalOrigin}_m + \frac{p_{\text{phys}} - \text{PhysicalOrigin}_m}{s_m}$$

By anchoring every conversion to the owning monitor's origin (`logical_rect.x/y` and `physical_rect.x/y`), mixed-DPI neighbors never accumulate cross-monitor scaling drift across negative coordinates or fractional scale seams.

| Monitor ID | Display Role & Orientation | OS / Platform Profile | Scale Factor ($s_m$) | Logical Bounds `(x, y, w, h)` | Physical Bounds `(x, y, w, h)` | Key Edge Cases Covered |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| `RETINA-2X` | Primary Landscape | macOS Built-in Retina / HiDPI | `2.0x` (200%) | `(0.0, 0.0, 1512.0, 982.0)` | `(0, 0, 3024, 1964)` | Primary origin `(0,0)`, sub-pixel `1.5` logical px fine nudge = exact `3` physical px |
| `LEFT-1X` | Secondary Left Landscape | External 1080p HDMI / X11 | `1.0x` (100%) | `(-1920.0, -49.0, 1920.0, 1080.0)` | `(-1920, -49, 1920, 1080)` | Negative X and negative Y origin; `x = 0.0` seam adjacency with `RETINA-2X` |
| `TOP-150` | Secondary Top Landscape | Windows Per-Monitor V2 150% / Wayland `wp-fractional-scale-v1` | `1.5x` (150%) | `(-200.0, -1440.0, 2560.0, 1440.0)` | `(-300, -2160, 3840, 2160)` | Fractional `1.5x` scale with negative X & Y origin; `y = 0.0` seam above `RETINA-2X`; 1,200-cell (`40x30`) mixed-case grid |
| `RIGHT-PORTRAIT-125` | Secondary Right Portrait | Windows Per-Monitor V2 125% Vertical Display | `1.25x` (125%) | `(1512.0, -300.0, 1080.0, 1920.0)` | `(3024, -375, 1350, 2400)` | Portrait orientation (`height > width`), fractional `1.25x` scale, vertical offset seam at `x = 1512.0` |

### 2.2 Platform-Specific Multi-Monitor QA Matrix

1. **macOS (`CoreGraphics` / `NSScreen`)**:
   - **Retina `2.0x` + External `1.0x` 1080p**: Verify `CGEventCreateMouseEvent` receives logical global points while Tauri overlay windows position seamlessly across positive and negative `NSScreen` origins.
   - **Menu Bar / Dock Safe-Area & Full-Screen Spaces**: Verify overlay renders at `NSWindowLevel::PopUpMenu` (`kCGPopUpMenuWindowLevel`) above full-screen apps without stealing keyboard focus from the active application.
2. **Windows 10/11 (`DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`)**:
   - **Per-Monitor V2 `125%` / `150%` / `200%`**: Verify `WM_DPICHANGED` updates `MonitorLayout` dynamically via `EngineEvent::MonitorsUpdated` and re-tiles active `Grid`/`Subgrid` overlays without visual clipping.
   - **`SendInput` Virtual Desktop Normalization**: Verify [`ratpoison_hid::platform::normalize_virtual_desktop_coords`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-hid/src/platform/mod.rs#L96-L111) maps negative physical coordinates (`[-1920..3840]`) cleanly into `[0..65535]` for `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`.
3. **Linux (X11 `RandR` + Wayland `wlr-layer-shell` / `wp-fractional-scale-v1`)**:
   - **X11 Mixed RandR CTM**: Verify `XTestFakeMotionEvent` targets exact root-window physical pixels.
   - **Wayland Fractional Scaling (`1.25x`, `1.5x`, `1.75x`)**: Verify `/dev/uinput` relative/absolute coordinates and layer-shell overlay surfaces match logical-to-physical rounding invariants within $\le 1$ physical pixel.

---

## 3. Grid, Subgrid & Nudge Coordinate Math Invariants

All coordinate math invariants are executable and verified in [`crates/ratpoison-tests/tests/multi_monitor_dpi.rs`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-tests/tests/multi_monitor_dpi.rs):

1. **Exact Left-Inverse Round-Trip Invariant**:
   For every valid physical pixel $p \in [x_{\min}, x_{\max}] \times [y_{\min}, y_{\max}]$ on any monitor $m$ with scale factor $s_m \in \{1.0, 1.25, 1.5, 2.0\}$:
   $$\text{logical\_to\_physical}\big(\text{physical\_to\_logical}(p, m), m\big) = p$$
2. **Logical & Physical Rounding Error Bounds**:
   For any continuous logical point $q \in \text{logical\_rect}(m)$:
   - Physical projection rounding error:
     $$\big|\text{logical\_to\_physical}(q, m)_x - \big(\text{phys\_origin}_x + (q_x - \text{log\_origin}_x)\cdot s_m\big)\big| \le 0.5\text{ physical px}$$
   - Logical round-trip error (for all $s_m \ge 1.0$):
     $$\big|\text{physical\_to\_logical}\big(\text{logical\_to\_physical}(q, m), m\big) - q\big|_\infty \le \frac{0.5}{s_m} \le 0.5\text{ logical px}$$
3. **Zero-Gap Edge-Exact Grid & Subgrid Tiling**:
   - Cell $(r, c)$ in a $C \times R$ grid over `bounds` has edges:
     $$x_0 = \text{bounds}.x + \frac{c \cdot \text{bounds}.\text{width}}{C}, \quad x_1 = \text{bounds}.x + \frac{(c + 1) \cdot \text{bounds}.\text{width}}{C}$$
   - Adjacent cells share exact floating-point boundaries ($|a.\text{right}() - b.x| < 10^{-12}$), and the terminal cell $(R-1, C-1)$ aligns identically with `bounds.right()` and `bounds.bottom()`.
   - When `subgrid_cols == 3` and `subgrid_rows == 3`, the center subgrid cell (`"s"`, index 4) has center identical to the parent grid cell center ($|\text{sub}_4.\text{center} - \text{parent}.\text{center}| < 10^{-9}$).
4. **Half-Open Seam Containment & Spillover Prevention**:
   - `LogicalRect::contains_point` uses half-open intervals $[x, x+w) \times [y, y+h)$ so shared monitor borders (`x = 0.0` between `LEFT-1X` and `RETINA-2X`, `x = 1512.0` between `RETINA-2X` and `RIGHT-PORTRAIT-125`) are owned by **exactly one** display.
   - `clamp_logical_to_monitor` clamps the upper bound to `max_logical_x_inclusive()` / `max_logical_y_inclusive()`, which projects to `physical_rect.x + physical_rect.width - 1` (`max_x_inclusive()`), preventing off-by-one cursor spillover onto the right/bottom neighbor.
5. **Nudge Precision & L-Shaped Dead-Space Clamping**:
   - Fine (`1.5` logical px), Normal (`8.0` logical px), and Coarse (`24.0` logical px) nudges translate cleanly across fractional scales (e.g., `+1.5` logical px on `2.0x` Retina = exact `+3` physical px).
   - Under `NudgeClampMode::VirtualDesktop`, nudging into L-shaped multi-monitor dead space (such as `(-1500.0, -1000.0)` above `LEFT-1X` and left of `TOP-150`) projects cleanly onto the nearest valid monitor's interior.

---

## 4. Memory Hygiene & Privacy Security Audit Methodology

RatPoison intercepts global keyboard input and therefore enforces a strict **Zero-Keylogger / Zero-Persistence Memory Hygiene Architecture**, verified automatically in [`crates/ratpoison-tests/tests/memory_hygiene_audit.rs`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-tests/tests/memory_hygiene_audit.rs) and via manual release-audit runbooks below.

### 4.1 Automated Memory Hygiene Assertions (`memory_hygiene_audit.rs`)

| Security Invariant | Implementation Mechanism in `ratpoison-hid` | Automated Test Verification in `ratpoison-tests` |
| :--- | :--- | :--- |
| **Compile-Time Capacity Bound ($3 \le N \le 5$)** | `SecureKeyBuffer<N>::CAPACITY_INVARIANT` (`const` assertion) rejects $N < 3$ or $N > 5$ at compile time; zero heap `Vec` growth. | `secure_key_buffer_capacity_bounds_and_fifo_ring_eviction` tests $N \in \{3, 4, 5\}$ and verifies 12-key flood in `CommandInterceptor<5>` never exceeds `len() == 5`. |
| **Padding-Free Fixed Slot Layout** | `#[repr(C)] struct SecureKeySlot` is compile-time asserted to be exactly `16` bytes (`5 * 16 = 80` bytes total). | Raw byte slice inspection (`raw_memory_bytes(&buf)[..80]`) checks all 80 slot bytes directly in memory. |
| **Explicit Volatile Zeroization (`0x00`)** | `zeroize::Zeroize` + `compiler_fence(Ordering::SeqCst)` on FIFO eviction, `pop_oldest()`, `on_escape()`, `on_mode_exit()`, idle timeout (`750ms`), and `Drop`. | `volatile_zeroization_on_pop_escape_mode_exit_idle_timeout_and_drop` scans raw memory for both little-endian and big-endian `u16` byte patterns (`0x1122..0x99AA`, `0xBEEF`, `0xCAFE`, `0xDEAD`, `0xF00D`) and asserts all 80 slot bytes equal `0x00` after purge and after `ManuallyDrop::drop`. |
| **Pinned OS Page Locking (`mlock` / `VirtualLock`)** | `LockedSecureKeyBuffer<N>` pins the buffer in heap memory, applies `mlock` + Linux `MADV_DONTDUMP` (or Windows `VirtualLock`), and volatile-zeroes *before* unlocking on `Drop`. | `locked_secure_key_buffer_pins_memory_and_zeroizes_before_unlock` verifies pointer stability across mutations, valid `MemoryLockStatus`, and full `0x00` wipe on mode exit. |
| **Process Core-Dump Hardening** | `harden_process_memory()` sets `setrlimit(RLIMIT_CORE, 0)` on Unix and `prctl(PR_SET_DUMPABLE, 0)` on Linux; macOS `deny_debugger_attach()` (`PT_DENY_ATTACH`) available for release builds. | Asserts `report.core_dumps_disabled == true` on Unix targets. |
| **Strict Redaction & No Serialization** | `KeyCode`, `Modifiers`, `RawKeyEvent`, `SecureKeyEntry`, `SecureKeyBuffer`, `LockedSecureKeyBuffer`, `RedactedKey`, and `CommandInterceptor` redact all key values as `"[REDACTED]"` in `fmt::Debug` and omit `Display` / `Serialize`. | `debug_formatting_strictly_redacts_all_keystrokes_and_modifiers` formats all 8 types containing keycode `54321` (`0xD431`) and asserts `"[REDACTED]"` is present while neither `"54321"` nor `"d431"` appears. |
| **Zero-Leak Hold-For & Cycle Interceptor** | `SwallowedKeySet` tracks swallowed `KeyDown`s and swallows their `KeyRepeat`/`KeyUp` even after deactivation, while passing through `KeyUp` for modifiers pressed *before* activation. | `hold_for_and_cycle_interceptor_guarantees_zero_key_leakage_and_bounded_state` verifies pre-activation `Ctrl+Alt` releases pass through, post-deactivation `Return`/`M`/`Shift` releases are swallowed, and buffer is `0x00`-wiped upon `ExecuteClick`. |
| **Zero Disk-Write Guarantee** | Input interception and state-machine transitions operate strictly in RAM with zero file I/O or logging. | `zero_disk_write_guarantee_during_hid_interception_and_core_state_machine_pipeline` points `HOME`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, and `XDG_DATA_HOME` to a temporary sandbox, runs a full HID + CoreEngine session, and asserts zero files created or modified. |

### 4.2 Manual & Release-Gate Memory Hygiene Audit Runbook

Before cutting a signed release binary, QA executes the following four OS-level audits against the compiled release binary:

#### A. Live RAM / Heap Inspection (`lldb` / `gcore` / `procdump`)
1. Launch RatPoison in debug/audit mode (with `PT_DENY_ATTACH` disabled so the auditor can attach):
   - **macOS**: `lldb -p $(pgrep ratpoison)` $\rightarrow$ type distinct canary sequence in Grid Mode (`a`, `s`, `d`) and press `Escape`.
   - Dump process memory or inspect `SecureKeyBuffer` address:
     ```text
     (lldb) memory find -s "asd" -- <heap_start> <heap_end>
     (lldb) memory read --size 1 --format x --count 80 <secure_key_buffer_addr>
     ```
   - **Linux**: `gcore $(pgrep ratpoison)` (verifying first that `PR_SET_DUMPABLE=0` and `MADV_DONTDUMP` reject unprivileged dumps and omit the locked key-buffer page from privileged dumps).
   - **Windows**: `procdump64.exe -ma ratpoison.exe ratpoison_audit.dmp` and scan for canary keycode sequences.
2. **Pass Criteria**: All 80 slot bytes of `SecureKeyBuffer<5>` read `0x00` immediately after `Escape`, click execution, or `750ms` idle timeout.

#### B. Page-Lock & Anti-Core-Dump Verification (`ulimit` / `/proc/<pid>/smaps`)
1. **Unix (`macOS` / `Linux`)**:
   - Inspect resource limits of running process (`cat /proc/$(pgrep ratpoison)/limits` on Linux): verify `Max core file size` soft and hard limits are `0`.
   - On Linux, inspect `/proc/$(pgrep ratpoison)/smaps` for the pinned `LockedSecureKeyBuffer` page: verify `Locked: 4 kB` and `VmFlags` contains `lo` (locked) and `dd` (`MADV_DONTDUMP`).
   - Send `SIGABRT` (`kill -ABRT $(pgrep ratpoison)`) and verify zero `core.*` or `/cores/core.*` file is written.

#### C. Zero Disk-Write Verification (`fs_usage` / `strace` / `ProcMon`)
1. Attach filesystem syscall tracer while typing 500+ keystrokes across Normal Mode, Grid Mode, Subgrid Mode, Nudge Mode, Free Mode, and Label Mode:
   - **macOS**: `sudo fs_usage -w -f filesystem $(pgrep ratpoison)`
   - **Linux**: `strace -f -e trace=open,openat,creat,write,pwrite64,rename,unlink -p $(pgrep ratpoison)`
   - **Windows**: Sysinternals Process Monitor (`ProcMon.exe`) filtered to `Process Name is ratpoison.exe` and `Operation is WriteFile / CreateFile`.
2. **Pass Criteria**: Zero file writes occur during keyboard interception or cursor control. Disk writes are permitted **exclusively** when the user explicitly clicks Save in Settings (`RatPoisonConfig`) or activates/deactivates a license seat (`PersistedLicenseStore`).

#### D. Zero Network-Exfiltration Verification (`tcpdump` / Little Snitch)
1. Capture all outbound packets while exercising all keyboard modes:
   - **macOS / Linux**: `sudo tcpdump -i any -n "host not 127.0.0.1"` (or per-PID socket filter via `lsof -i -a -p $(pgrep ratpoison)` / `ss -tupn | grep ratpoison`).
2. **Pass Criteria**:
   - `ratpoison-core`, `ratpoison-hid`, and `ratpoison-a11y` have **zero network dependencies** in their `Cargo.toml` dependency trees and open zero sockets.
   - Network activity is strictly zero during keyboard/mouse operation (and zero during air-gapped offline Ed25519 challenge-response licensing).

---

## 5. Cross-Crate Integration Gaps & Contract Friction Observed

While wiring `crates/ratpoison-tests` across `ratpoison-core`, `ratpoison-hid`, `ratpoison-a11y`, and `ratpoison-license`, we identified and bridged the following architectural contract differences (implemented in [`crates/ratpoison-tests/src/lib.rs`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-tests/src/lib.rs) and tested in [`crates/ratpoison-tests/tests/end_to_end_pipeline.rs`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-tests/tests/end_to_end_pipeline.rs)):

1. **Duplicate Nominal Geometry Structs Across Three Crates**:
   - `LogicalPoint`, `LogicalRect`, and `PhysicalPoint` are defined independently in `ratpoison-core::geometry`, `ratpoison-hid::types`, and `ratpoison-a11y`.
   - **Good news**: All three crates use identical field names and primitive types (`f64` for `LogicalPoint { x, y }` and `LogicalRect { x, y, width, height }`; `i32` for `PhysicalPoint { x, y }`), so their JSON representations are 100% wire-compatible.
   - **Friction**: In Rust, passing a point or rectangle from `ratpoison-core` to `ratpoison-hid` or `ratpoison-a11y` requires explicit struct conversion (`core_to_hid_logical_point`, `core_to_a11y_rect`, etc.).
2. **Coordinate Origin Assumption in `ratpoison-hid::LogicalPoint::to_physical` vs `ratpoison-core::logical_to_physical`**:
   - `ratpoison-core::logical_to_physical(point, monitor)` uses the full multi-monitor affine transform `monitor.physical_rect.x + ((point.x - monitor.logical_rect.x) * scale_factor).round()`, which is exact even when a secondary monitor has a negative origin and a different DPI scale than the primary monitor.
   - `ratpoison-hid::LogicalPoint::to_physical(scale_factor)` and `ratpoison-a11y::LogicalPoint::to_physical(scale_factor)` multiply `point.x * scale_factor` directly without a per-monitor origin offset.
   - **Recommendation for Tauri App Shell (`src-tauri`)**: Always compute physical coordinates via `ratpoison_core::logical_to_physical_clamped(point, monitor)` (or pass the already-computed `physical: PhysicalPoint` emitted inside `ratpoison_core::EngineEffect::{MoveCursor, Click, MouseDown, MouseUp}`).
3. **`MouseButton` Serde Casing & Variant Mismatch (`ratpoison-core` vs `ratpoison-hid`)**:
   - `ratpoison_core::MouseButton` has 3 variants (`Left`, `Right`, `Middle`) annotated with `#[serde(rename_all = "snake_case")]` (serializing as `"left"`, `"right"`, `"middle"`).
   - `ratpoison_hid::MouseButton` has 5 variants (`Left`, `Right`, `Middle`, `Back`, `Forward`) with default serde PascalCase (serializing as `"Left"`, `"Right"`, `"Middle"`, etc.).
   - **Bridge**: Use direct Rust enum mapping (`core_to_hid_mouse_button`) rather than JSON deserialization punning.
4. **`LabelBadge` Schema Difference (`ratpoison-a11y::LabelBadge` vs `ratpoison-core::LabelBadge`)**:
   - `ratpoison_a11y::LabelBadge` outputs `{ hint: String, target: LogicalPoint, bounds: LogicalRect, element: AccessibleElement }`.
   - `ratpoison_core::LabelBadge` expects `{ id: String, label: String, title: String, role: String, rect: LogicalRect, center: LogicalPoint, monitor_id: String }`.
   - **Bridge**: [`ratpoison_tests::bridge_a11y_badge_to_core`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-tests/src/lib.rs#L51-L65) maps `badge.hint -> label`, `badge.target -> center`, `badge.bounds -> rect`, and attaches the active `monitor_id`.
5. **`KeyCode` to Character Decoding Bridge (`ratpoison-hid` $\rightarrow$ `ratpoison-core`)**:
   - `ratpoison-hid` emits `InterceptorAction::GridKeyInput { key: RedactedKey, buffer_len }` containing an uppercase ASCII virtual `KeyCode` (`KeyCode::from_ascii_upper(b'A')`), whereas `ratpoison-core` accepts `EngineEvent::KeyChar { ch: char }`.
   - **Bridge**: Convert `key.code().raw()` into a `char` (preserving Shift state when `case_sensitive` grid/label schemes are active, or lowercasing by default) before dispatching `EngineEvent::KeyChar`.

---

## 6. Test Execution Summary & Pass Counts

Executed in `/Users/abelmora/Documents/Avalon/RatPoison`:

```bash
cargo test -p ratpoison-tests
cargo clippy -p ratpoison-tests --all-targets -- -D warnings
```

| Test Target | File | Tests Passed | Tests Failed | Clippy Warnings | Coverage Focus |
| :--- | :--- | :---: | :---: | :---: | :--- |
| `multi_monitor_dpi` | [`crates/ratpoison-tests/tests/multi_monitor_dpi.rs`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-tests/tests/multi_monitor_dpi.rs) | **7** | 0 | 0 | Quad mixed-DPI matrix (`1.0x`, `1.25x` portrait, `1.5x` negative-Y, `2.0x` Retina), exact physical/logical round-trips ($\le 0.5$ px), grid/subgrid center precision, half-open seam containment, directional & cyclic monitor switching, L-shaped dead-space nudge clamping, 1,200-cell (`>676`) mixed-case grid. |
| `memory_hygiene_audit` | [`crates/ratpoison-tests/tests/memory_hygiene_audit.rs`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-tests/tests/memory_hygiene_audit.rs) | **6** | 0 | 0 | `SecureKeyBuffer` capacity bounds ($3..=5$), FIFO eviction, raw-memory volatile zeroization (`0x00`) on eviction/pop/escape/mode-exit/idle-timeout/`Drop`, `LockedSecureKeyBuffer` page-lock + address pinning + `RLIMIT_CORE=0`, `[REDACTED]` `Debug` output across 8 types, zero-leak Hold-For/Cycle `CommandInterceptor`, isolated sandbox zero disk-write verification. |
| `end_to_end_pipeline` | [`crates/ratpoison-tests/tests/end_to_end_pipeline.rs`](file:///Users/abelmora/Documents/Avalon/RatPoison/crates/ratpoison-tests/tests/end_to_end_pipeline.rs) | **6** | 0 | 0 | End-to-end `MockHidBackend` + `CommandInterceptor` + `CoreEngine` (`Grid -> Subgrid -> Nudge -> RightClick` + drag cancel), `FreeMode` speed tiers (`Slow < Normal < Fast < Turbo`) + Quick Select handoff, `MockAccessibilityProvider` $\rightarrow$ home-row prefix-free 1–2 char badges $\rightarrow$ `CoreEngine::LabelMode` auto-click, `LicenseManager` 14-day trial + clock rollback + 5-seat limit + deactivation swap + offline Ed25519 challenge-response, cross-crate serde/type bridge audit. |
| **Total (`ratpoison-tests`)** | **3 Integration Suites + Library Bridge** | **19** | **0** | **0** | **100% pass rate with zero clippy warnings** |
