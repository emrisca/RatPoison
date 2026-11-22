# Sub-Agent 3 Deliverable: Input/HID & Security (`crates/ratpoison-hid`)

## 1. Architecture Overview

`crates/ratpoison-hid` provides RatPoison's zero-dependency-bloat OS keyboard interception, synthetic mouse simulation, leak-free Hold-For / Cycle command state machine, and hardened Memory Hygiene keystroke ring buffer.

```
OS Hardware Key Event
  │
  ▼
KeyboardHookBackend (macOS CGEventTap | Windows WH_KEYBOARD_LL | Linux evdev+uinput | MockHidBackend)
  ├─ [Secure Input active?] ────────────────────────► PassThrough (never inspect/swallow)
  ├─ [RATPOISON_EVENT_MAGIC / LLKHF_INJECTED?] ─────► PassThrough (loop prevention)
  │
  ▼
CommandInterceptor<5> (zero-allocation hot path, owns SecureKeyBuffer<5> + SwallowedKeySet)
  ├─ Inactive + not swallowed ──────────────────────► PassThrough (tracks foreground modifiers)
  ├─ Active + KeyUp of pre-existing key ────────────► PassThrough (prevents stuck modifiers)
  ├─ Activation / Hold-For / Cycle / Execute / Esc ─► Swallow / SwallowAndEmit(InterceptorAction)
  └─ Grid / Nav Key ────────────────────────────────► Push into SecureKeyBuffer<5> (auto-zeroized)
  │
  ▼
MouseSimulatorBackend (macOS CGEventPost | Windows SendInput | Linux uinput / XTest | MockHidBackend)
```

### Module Layout (`crates/ratpoison-hid/src/`)

| File | Responsibility |
| --- | --- |
| `types.rs` | `LogicalPoint`, `PhysicalPoint`, `LogicalRect` (aligned with `SWARM_CONTRACT.md`), `KeyCode`, `Modifiers`, `KeyPhase`, `RawKeyEvent` (redacted `Debug`, no `Serialize`/`Display`), `MouseButton`, `ClickKind`, `SpeedTier`, `ScrollDelta`, `PermissionReport`, `HidError`, `RATPOISON_EVENT_MAGIC`. |
| `memory_hygiene.rs` | `SecureKeyBuffer<const N: usize = 5>` (`3 <= N <= 5` compile-time enforced), `SecureKeyEntry` (`ZeroizeOnDrop`), `LockedSecureKeyBuffer<N>` (`Pin<Box<...>>` + `mlock`/`MADV_DONTDUMP`/`VirtualLock` with graceful fallback), `disable_core_dumps()`, `harden_process_memory()`, macOS release opt-in `deny_debugger_attach()`. |
| `interceptor.rs` | `CommandInterceptor<N>`, `InterceptorConfig`, `HoldForBinding`, `CycleBinding`, `InterceptDecision`, `InterceptorAction`, `RedactedKey`, zeroized stack-allocated `SwallowedKeySet`. |
| `platform/mod.rs` | `KeyboardHookBackend`, `MouseSimulatorBackend`, `normalize_virtual_desktop_coords()`, OS factory dispatchers. |
| `platform/mock.rs` | `MockHidBackend` implementing both hook and mouse traits for deterministic headless tests. |
| `platform/macos.rs` | `#[cfg(target_os = "macos")]` FFI bindings to `CoreGraphics`, `CoreFoundation`, `ApplicationServices`, and `Carbon`. |
| `platform/windows.rs` | `#[cfg(target_os = "windows")]` FFI bindings to `user32`, `kernel32`, and `advapi32`. |
| `platform/linux.rs` | `#[cfg(target_os = "linux")]` `evdev` (`EVIOCGRAB`) + `uinput` virtual keyboard/mouse + dynamic `dlopen` X11 `XTest` fallback. |

### Shared Coordinate Contract
- All public mouse simulation methods accept `LogicalPoint { x: f64, y: f64 }` paired with `scale_factor: f64`.
- Invalid (`<= 0.0` or non-finite) `scale_factor` values safely clamp to `1.0` in `LogicalPoint::to_physical` and `PhysicalPoint::to_logical`.

---

## 2. OS-Specific Input & Mouse Simulation Reference

### 2.1 macOS (`platform/macos.rs`)

| Capability | Native OS API | Notes |
| --- | --- | --- |
| Global keyboard hook | `CGEventTapCreate(kCGHIDEventTap, kCGHeadInsertEventTap, kCGEventTapOptionDefault, ...)` | Runs on a dedicated `CFRunLoop` thread (`ratpoison-macos-cgeventtap`). Returning `NULL` from the callback swallows the event before any application observes it. |
| Modifier-only keys | `kCGEventFlagsChanged` | Compares current `CGEventFlags` against `prev_modifiers` so Hold-For modifier keys (e.g. `Shift_L`) synthesize clean `Down`/`Up` transitions. |
| Watchdog recovery | `kCGEventTapDisabledByTimeout` / `kCGEventTapDisabledByUserInput` | Immediately re-enables the tap via `CGEventTapEnable(tap, 1)` so brief system stalls never leave the user stranded. |
| Permission checks | `AXIsProcessTrusted()` + `CGPreflightListenEventAccess()` | Non-prompting read-only checks. Both Accessibility and Input Monitoring are surfaced via `PermissionReport`. |
| **Secure Input detection** | `IsSecureEventInputEnabled()` (`Carbon` / HIToolbox) | Checked inside the event tap trampoline AND exposed via `is_secure_input_active()`. Whenever macOS enables Secure Event Input (password fields, 1Password, Terminal Secure Keyboard Entry), macOS withholds key events from taps and our trampoline unconditionally passes through. UI should surface `PermissionReport.guidance` when active. |
| Mouse simulation | `CGWarpMouseCursorPosition`, `CGEventCreateMouseEvent`, `CGEventCreateScrollWheelEvent2`, `CGEventPost(kCGHIDEventTap, ...)` | Uses `CGPoint` directly in **logical points** (no double scaling on Retina displays). Uses non-variadic `CGEventCreateScrollWheelEvent2` to avoid Apple Silicon (`arm64`) C-variadic ABI traps. Double-clicks set `kCGMouseEventClickState = 1` then `2`. |
| Loop prevention | `CGEventSetIntegerValueField(..., kCGEventSourceUserData, RATPOISON_EVENT_MAGIC)` | All synthetic events carry `RATPOISON_EVENT_MAGIC`; the tap checks this field and marks matching events `is_injected = true` (`PassThrough`). |
| Memory safety | `CfOwned` RAII wrapper | Every `CFMachPortRef`, `CFRunLoopSourceRef`, and `CGEventRef` is deterministically released via `CFRelease`. |

### 2.2 Windows (`platform/windows.rs`)

| Capability | Native OS API | Notes |
| --- | --- | --- |
| Global keyboard hook | `SetWindowsHookExW(WH_KEYBOARD_LL, ll_keyboard_proc, NULL, 0)` | Installed on a dedicated Win32 message-pump thread (`GetMessageW` loop; stopped cleanly via `PostThreadMessageW(tid, WM_QUIT, 0, 0)` + `UnhookWindowsHookEx`). Returning `1` swallows the event; otherwise `CallNextHookEx` forwards it. |
| Latency budget | `LowLevelHooksTimeout` (default ~300ms) | Callback performs zero heap allocations, zero disk I/O, and zero logging so Windows never silently drops the hook. |
| Auto-repeat detection | Per-hook-thread `held_keys: [u32; 16]` | `WH_KEYBOARD_LL` delivers repeated `WM_KEYDOWN` messages; tracking active `vk_code`s classifies repeated downs as `KeyPhase::Repeat`. |
| Mouse simulation | `SendInput` (`INPUT_MOUSE`) | Multi-monitor absolute movement converts `LogicalPoint -> PhysicalPoint` and normalizes across `(SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN)` into `0..=65535` with `MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` (handles negative left/top monitor origins). |
| Loop prevention | `LLKHF_INJECTED` + `dwExtraInfo = RATPOISON_EVENT_MAGIC` | Incoming injected events always pass through untouched. |
| **UIPI / Elevated Windows Caveat** | `OpenProcessToken` + `GetTokenInformation(TokenElevation)` | Under Windows User Interface Privilege Isolation (UIPI), a Medium-integrity process cannot intercept keys or deliver `SendInput` (`ERROR_ACCESS_DENIED = 5` → `HidError::ElevatedWindowBlocked`) while a High-integrity window (Task Manager, UAC, admin PowerShell) has focus. **Mitigation**: Release installers should code-sign the binary, install under `Program Files`, and declare `uiAccess="true"` in the Win32 application manifest (documented in `PermissionReport.elevated_target_caveat`). |

### 2.3 Linux (`platform/linux.rs`)

| Capability | Native OS API | Notes |
| --- | --- | --- |
| Wayland + X11 keyboard hook | `evdev` (`/dev/input/event*` + `ioctl(EVIOCGRAB, 1)`) + `uinput` virtual keyboard | Exclusively grabs the physical keyboard at the kernel input layer (works identically on Wayland compositors and X11). `PassThrough` events are re-emitted immediately through a companion `uinput` virtual keyboard (`RatPoison Virtual Keyboard`); swallowed events are dropped before any display server sees them. Releases `EVIOCGRAB` and calls `UI_DEV_DESTROY` on stop/drop. |
| Wayland + X11 mouse simulation | `/dev/uinput` (`UI_DEV_SETUP`, `UI_DEV_CREATE`) | Registers `EV_KEY` (`BTN_LEFT/RIGHT/MIDDLE/SIDE/EXTRA`), `EV_REL` (`REL_X/Y/WHEEL/HWHEEL`), and `EV_ABS` (`ABS_X/Y`), emitting `EV_SYN` (`SYN_REPORT`) after every action. |
| **Permissions & udev rules** | `/dev/input/event*` + `/dev/uinput` | Requires user membership in the `input` group (`sudo usermod -aG input "$USER"`) plus a udev rule (`/etc/udev/rules.d/99-ratpoison-uinput.rules`): `KERNEL=="uinput", SUBSYSTEM=="misc", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"`. Note for Flatpak: sandboxed Flatpaks cannot open `/dev/uinput` directly without host device policy or a host helper daemon (coordinate with Sub-Agent 4). |
| **X11 `XTest` fallback** | Dynamic `dlopen` of `libX11.so.6` + `libXtst.so.6` | Automatically used when `/dev/uinput` is unavailable and `DISPLAY` is set (`XTestFakeMotionEvent`, `XTestFakeRelativeMotionEvent`, `XTestFakeButtonEvent`, `XFlush`). Dynamic loading ensures pure Wayland builds carry zero compile-time X11 link dependency. |

---

## 3. Strict Memory Hygiene Specification

RatPoison hooks global keyboard events solely to drive pointer navigation—never to record user text. `crates/ratpoison-hid/src/memory_hygiene.rs` enforces defense-in-depth guarantees:

### 3.1 Compile-Time Bounded Ring Buffer (`SecureKeyBuffer<const N: usize = 5>`)
- **Bounds**: `MIN_BUFFER_CAPACITY = 3`, `MAX_BUFFER_CAPACITY = 5`, `DEFAULT_BUFFER_CAPACITY = 5`.
- **Enforcement**: `const CAPACITY_INVARIANT: () = assert!(N >= 3 && N <= 5, ...)` causes `SecureKeyBuffer::<2>::new()` and `SecureKeyBuffer::<6>::new()` to fail at **compile time** (verified by `compile_fail` doctests).
- **Fixed Padding-Free Storage**: Slots are stored inline as `[SecureKeySlot; N]` with `#[repr(C)]` exact 16-byte layout (`code: u16`, `modifiers: u8`, `occupied: u8`, `_reserved: u32`, `timestamp_ms: u64`; statically asserted `size_of == 16`). Zero dynamic `Vec`/`String` heap allocations and zero hidden struct padding bytes that could retain stale key material.

### 3.2 Deterministic Zeroization Lifecycle

Every zeroization call uses `zeroize::Zeroize` (`core::ptr::write_volatile`) followed by `core::sync::atomic::compiler_fence(Ordering::SeqCst)` so LLVM dead-store elimination can never strip the wipe:

| Lifecycle Trigger | Action Performed |
| --- | --- |
| **Ring eviction (`push` when `len == N`)** | Volatile-zeroes `self.slots[evict_idx]` *before* writing the new keystroke. |
| **Slot extraction (`pop_oldest`)** | Extracts a `ZeroizeOnDrop` `SecureKeyEntry` (non-`Copy`) and immediately volatile-zeroes the vacated ring slot. |
| **Idle timeout (`DEFAULT_IDLE_PURGE_MS = 750`)** | `purge_if_expired(now_ms)` (invoked on `tick()`, `handle_event()`, and before every `push()`) volatile-zeroes all `N` slots, `head`, `len`, and `last_activity_ms`. |
| **Mode exit / `Escape` / Execute** | `on_escape()` / `on_mode_exit()` volatile-zeroes all `N` slots unconditionally. |
| **Transient hook stack event** | OS hook callbacks volatile-zero their stack `RawKeyEvent` immediately after the interceptor callback returns. |
| **Drop / Shutdown** | `Drop::drop` volatile-zeroes all `N` slots and metadata. In `LockedSecureKeyBuffer<N>`, zeroization completes **before** `munlock`/`VirtualUnlock` so dirty keystroke pages can never be paged to swap. |

### 3.3 Logging, Formatting & Serialization Isolation
- `SecureKeyBuffer<N>`, `SecureKeyEntry`, and `RawKeyEvent` intentionally do **not** implement `Display`, `serde::Serialize`, or `serde::Deserialize` (enforced via `compile_fail` doctests).
- Custom `fmt::Debug` implementations on `SecureKeyBuffer`, `SecureKeyEntry`, `RawKeyEvent`, `KeyCode`, `Modifiers`, and `RedactedKey` strictly emit `"[REDACTED]"` so accidental `tracing::debug!` or `dbg!` calls can never leak keystrokes.

### 3.4 Anti-Swap & Anti-Core-Dump OS Hardening
- **`LockedSecureKeyBuffer<N>`**: Pins `Box<SecureKeyBuffer<N>>` at a fixed virtual address and locks pages into physical RAM:
  - **Unix (macOS / Linux)**: `mlock(ptr, len)` (plus `madvise(ptr, len, MADV_DONTDUMP)` on Linux).
  - **Windows**: `VirtualLock(ptr, len)`.
  - **Graceful fallback**: If `RLIMIT_MEMLOCK` is tight or denied, returns `MemoryLockStatus::FallbackUnlocked { errno }` instead of crashing, while retaining full volatile zeroization guarantees.
- **Process-level hardening (`harden_process_memory()`)**:
  - **Unix**: `setrlimit(RLIMIT_CORE, {0, 0})` disables core dumps.
  - **Linux**: `prctl(PR_SET_DUMPABLE, 0)` blocks core dumps and non-root `ptrace` inspection of `/proc/self/mem`.
  - **macOS**: Explicit release-only opt-in `deny_debugger_attach()` invokes `ptrace(PT_DENY_ATTACH, 0, NULL, 0)` (intentionally not invoked automatically in debug/test builds so `lldb` and `cargo test` continue working).
  - **Windows**: Installer/deployment should exclude RatPoison from Windows Error Reporting (`WER\LocalDumps`) crash-dump capture.

---

## 4. Hold-For & Cycle Command Interceptor (`interceptor.rs`)

### 4.1 Decision Model
Every raw keyboard event yields one of three explicit outcomes:
- `InterceptDecision::PassThrough` — forward untouched to the foreground app.
- `InterceptDecision::Swallow` — drop silently; foreground app never observes it.
- `InterceptDecision::SwallowAndEmit(InterceptorAction)` — drop silently and emit a structured RatPoison action (`ActivateOverlay`, `DeactivateOverlay`, `SetClickMode`, `RevertClickMode`, `BeginDrag`, `EndDrag`, `SetSpeedTier`, `RevertSpeedTier`, `CycleClickMode`, `CycleSpeedTier`, `ExecuteClick`, `GridKeyInput`, `SynthesizeKeyUp`).

### 4.2 Zero-Leakage & Stuck-Modifier Invariants
1. **Swallowed-key symmetry (`SwallowedKeySet`)**: Fixed 16-slot zeroized stack set recording every key whose `KeyDown` was swallowed. Its `KeyRepeat` and `KeyUp` are **always** swallowed—even if RatPoison deactivated (e.g., after clicking or pressing `Escape`) before the user physically lifted their finger.
2. **Pre-existing foreground keys never stick**: If a user holds `Alt` *before* pressing `Space` to trigger `Alt+Space`, `Alt`'s `KeyDown` passed through to the foreground app (recorded in `foreground_held_keys`, *not* in `swallowed_keys`). When `Alt` is later released while RatPoison is active, `KeyUp(Alt)` returns `PassThrough` so the foreground app never gets a stuck modifier.
3. **Hold-For nested LIFO stack**: Holding `R` (Right) then `M` (Middle) switches to `Middle`; releasing `M` reverts to `Right`; releasing `R` reverts to the underlying Cycle mode (not blindly `Left`). Auto-repeats are strictly swallowed.
4. **Cycle precedence & repeat suppression**: Tapping a Cycle key (default `C`: `Left → Right → DoubleLeft → Middle`; default `V`: `Normal → Fast → Turbo → Slow`) advances once per `KeyDown`, swallows all `KeyRepeat` and `KeyUp` events, and yields temporarily to any active Hold-For override before resuming upon release.

---

## 5. Security Audit Checklist (Keyhook & HID Subsystem)

Use this checklist before every release or modification to `crates/ratpoison-hid/`:

### Memory Hygiene & Zeroization
- [ ] `SecureKeyBuffer<N>` capacity remains strictly compile-time bounded to `3 <= N <= 5`.
- [ ] `SecureKeySlot` remains `#[repr(C)]` with `size_of == 16` and zero implicit padding bytes.
- [ ] Eviction (`push` at capacity), extraction (`pop_oldest`), `Escape`, mode exit, idle timeout (default ≤ 750ms), and `Drop` all invoke `zeroize()` + `SeqCst` compiler fence.
- [ ] `LockedSecureKeyBuffer::drop` always zeroizes memory *before* calling `munlock` / `VirtualUnlock`.
- [ ] Transient `RawKeyEvent` structs inside OS hook callbacks are explicitly zeroized before returning from the callback.
- [ ] `SwallowedKeySet` zeroes removed slots and wipes all slots on `Drop`.

### Anti-Keylogger, Logging & Persistence Isolation
- [ ] Zero `println!`, `eprintln!`, `dbg!`, `log::*`, or `tracing::*` calls exist inside `crates/ratpoison-hid` that inspect raw keycodes or modifiers.
- [ ] `SecureKeyBuffer`, `SecureKeyEntry`, and `RawKeyEvent` do **not** implement `Display`, `Serialize`, or `Deserialize` (verified by `compile_fail` doctests).
- [ ] `Debug` formatting on all key/buffer types redacts keycodes and modifiers as `"[REDACTED]"` (verified by unit tests).
- [ ] Keystrokes are never written to disk, SQLite, clipboard, crash logs, IPC telemetry, or Tauri frontend events outside ephemeral in-memory grid matching.

### Hook Privacy, Permissions & OS Hardening
- [ ] **macOS**: `IsSecureEventInputEnabled()` is checked on every tap invocation; keystrokes pass through untouched whenever Secure Event Input is active.
- [ ] **macOS**: `AXIsProcessTrusted()` and `CGPreflightListenEventAccess()` are checked before installing the tap; no unsolicited privilege escalation.
- [ ] **macOS**: Release binary invokes `harden_process_memory()` at startup and optionally `deny_debugger_attach()` (`PT_DENY_ATTACH`) in signed release builds.
- [ ] **Windows**: `WH_KEYBOARD_LL` callback executes in microseconds with zero blocking calls or heap allocation (staying well under `LowLevelHooksTimeout`).
- [ ] **Windows**: UIPI elevation status is reported accurately; no unsafe token stealing or driver-level kernel bypasses.
- [ ] **Linux**: `EVIOCGRAB` is always released on `stop()` and `Drop` (and `UI_DEV_DESTROY` called) so a crash/exit never locks out the user's keyboard.
- [ ] **Linux**: `harden_process_memory()` applies `RLIMIT_CORE = 0` and `prctl(PR_SET_DUMPABLE, 0)`; `LockedSecureKeyBuffer` applies `mlock` + `MADV_DONTDUMP`.

### Loop Prevention & Input Safety
- [ ] All synthetic mouse/keyboard events carry `RATPOISON_EVENT_MAGIC` (macOS `kCGEventSourceUserData`, Windows `dwExtraInfo`) and injected events always return `PassThrough`.
- [ ] `KeyUp` events for keys pressed *before* overlay activation always return `PassThrough` (no stuck modifiers).
- [ ] `KeyRepeat` and `KeyUp` events for keys swallowed *during* overlay activation always return `Swallow` even after deactivation (zero stray character/key-up leaks).
- [ ] `Escape` cancels any active Hold-For drag (`cancel_drag: Some(btn)`) so mouse buttons never stay stuck down.

### FFI & Supply-Chain Hygiene
- [ ] Zero third-party unvetted FFI crates added; only workspace-pinned `serde`, `thiserror`, and `zeroize` are used.
- [ ] Every `unsafe` FFI block documents its safety invariants and wraps CoreFoundation/Win32/Unix handles in RAII drop guards (`CfOwned`, `File`, `XTestFallback`).
- [ ] `cargo clippy -p ratpoison-hid --all-targets -- -D warnings` and `cargo test -p ratpoison-hid` pass cleanly.

---

## 6. Integration Notes & Risks for Swarm Orchestrator

1. **Zero external crate additions / clean workspace isolation**: `crates/ratpoison-hid` compiles with only the existing workspace dependencies (`serde`, `thiserror`, `zeroize`) and zero edits to root `Cargo.toml` or other crates.
2. **Wiring `src-tauri` & `ratpoison-core`**:
   - Call `harden_process_memory()` early in `src-tauri`'s `main()` before spawning the keyboard hook.
   - Drive `CommandInterceptor<5>` inside the `KeyboardHookBackend::start` callback, forwarding `InterceptorAction::GridKeyInput { key, .. }` in-memory to `ratpoison-core`'s grid resolver using `key.code()` without logging `key`.
   - Call `CommandInterceptor::tick(now_ms)` on a lightweight timer (e.g., every 100–250ms while active) so idle keystrokes purge promptly at 750ms even if no further keypress arrives.
   - Emit `ratpoison://overlay-state` whenever `InterceptorAction` changes mode, click kind, or speed tier.
3. **Cross-platform compilation & testing caveats**:
   - Verified natively on **macOS** (`cargo test -p ratpoison-hid` + `cargo clippy -p ratpoison-hid --all-targets -- -D warnings`).
   - `platform/windows.rs` and `platform/linux.rs` are gated with `#[cfg(target_os = "windows")]` and `#[cfg(target_os = "linux")]` and have **not** yet been compiled or runtime-tested on native Windows/Linux CI runners (recommend adding a GitHub Actions matrix job for `windows-latest` and `ubuntu-latest` via Sub-Agent 5).
   - Live macOS `CGEventTapCreate` and `CGEventPost` end-to-end execution requires interactive Accessibility + Input Monitoring permissions on the host machine and therefore was not exercised in headless `cargo test` (non-prompting permission and Secure Input queries were verified live on macOS).
   - **Linux `evdev` crash-safety risk**: If a process holding `EVIOCGRAB` receives `SIGKILL` (`-9`), the kernel automatically closes the fd and releases the grab, which is safe; however, `evdev_file.read_exact()` blocks until the next hardware key event during `stop()`. For instant non-blocking shutdown on Linux, switch the evdev reader loop to `poll(2)` with a 50ms timeout or an `eventfd` wakeup pipe before shipping Linux beta.
   - **Keyboard layout normalization**: Current `macos_keycode_to_normalized` and `linux_evdev_key_to_normalized` map US ANSI QWERTY hardware scan codes; non-QWERTY layouts (AZERTY/QWERTZ/Dvorak/Colemak) should be layered via OS layout translation (`UCKeyTranslate` / `ToUnicodeEx` / `xkbcommon`) in a follow-up integration pass with `ratpoison-core`.
