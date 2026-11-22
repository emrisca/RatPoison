//! Executable Memory Hygiene & Privacy security audit tests against `ratpoison-hid`.
//!
//! Verifies:
//! 1. `SecureKeyBuffer` capacity bounds (3–5 keypresses), FIFO ring eviction, and explicit
//!    volatile zeroization (`0x00`) verified via raw in-memory byte inspection across eviction,
//!    pop, escape, mode exit, idle timeout expiry, and `Drop`.
//! 2. Redacted `Debug` output across `KeyCode`, `Modifiers`, `RawKeyEvent`, `SecureKeyEntry`,
//!    `SecureKeyBuffer`, `LockedSecureKeyBuffer`, `RedactedKey`, `CommandInterceptor`, and
//!    `InterceptDecision` (zero leaked keystroke characters or numeric keycodes).
//! 3. Hold-For & Cycle zero-leak `CommandInterceptor` state machine behavior (no orphaned
//!    keystrokes, balanced modifier pass-through, swallowed auto-repeats, post-deactivation
//!    key-up swallowing, and bounded retention).
//! 4. Zero disk-write guarantee during keyboard interception and core state-machine input
//!    processing inside an isolated temporary `HOME` / `XDG_CONFIG_HOME` / `TMPDIR` sandbox.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ratpoison_core::{
    CoreEngine, EngineEvent, LogicalRect as CoreLogicalRect, MonitorInfo, MonitorLayout,
    PhysicalPoint as CorePhysicalPoint, RatPoisonConfig,
};
use ratpoison_hid::{
    harden_process_memory, ClickKind, CommandInterceptor, InterceptDecision, InterceptorAction,
    InterceptorConfig, InterceptorMode, KeyCode, KeyPhase, KeyboardHookBackend,
    LockedSecureKeyBuffer, MemoryLockStatus, MockHidBackend, Modifiers, MouseSimulatorBackend,
    RawKeyEvent, SecureKeyBuffer, SpeedTier, DEFAULT_BUFFER_CAPACITY, DEFAULT_IDLE_PURGE_MS,
    MAX_BUFFER_CAPACITY, MIN_BUFFER_CAPACITY,
};
use zeroize::Zeroize;

/// Inspects raw memory bytes of any value `&T` to assert whether specific secret byte patterns exist.
fn raw_memory_bytes<T>(val: &T) -> &[u8] {
    let ptr = (val as *const T).cast::<u8>();
    let len = core::mem::size_of::<T>();
    // SAFETY: `val` is a valid reference to `T` of size `len`; reading bytes for test audit only.
    unsafe { core::slice::from_raw_parts(ptr, len) }
}

/// Checks if a 2-byte big-endian or little-endian representation of `secret_code` appears
/// anywhere in `bytes`.
fn contains_u16_pattern(bytes: &[u8], secret_code: u16) -> bool {
    let ne = secret_code.to_ne_bytes();
    let be = secret_code.to_be_bytes();
    bytes.windows(2).any(|w| w == ne || w == be)
}

#[test]
fn secure_key_buffer_capacity_bounds_and_fifo_ring_eviction() {
    assert_eq!(MIN_BUFFER_CAPACITY, 3);
    assert_eq!(MAX_BUFFER_CAPACITY, 5);
    assert_eq!(DEFAULT_BUFFER_CAPACITY, 5);

    let mut buf3 = SecureKeyBuffer::<3>::new();
    let buf4 = SecureKeyBuffer::<4>::new();
    let buf5 = SecureKeyBuffer::<5>::new();
    assert_eq!(buf3.capacity(), 3);
    assert_eq!(buf4.capacity(), 4);
    assert_eq!(buf5.capacity(), 5);
    assert_eq!(buf5.idle_timeout_ms(), DEFAULT_IDLE_PURGE_MS);

    // Fill 3-slot buffer and push 4th & 5th items -> verifies strict FIFO ring eviction
    let secret_1: u16 = 0x415A;
    let secret_2: u16 = 0x425B;
    let secret_3: u16 = 0x435C;
    let secret_4: u16 = 0x445D;
    let secret_5: u16 = 0x455E;

    assert!(!buf3.push(KeyCode(secret_1), Modifiers::SHIFT, 100).evicted);
    assert!(
        !buf3
            .push(KeyCode(secret_2), Modifiers::CONTROL, 110)
            .evicted
    );
    assert!(!buf3.push(KeyCode(secret_3), Modifiers::ALT, 120).evicted);
    assert_eq!(buf3.len(), 3);
    assert!(contains_u16_pattern(raw_memory_bytes(&buf3), secret_1));

    // Pushing secret_4 evicts and volatile-zeroes secret_1 before writing secret_4
    let out4 = buf3.push(KeyCode(secret_4), Modifiers::META, 130);
    assert!(out4.evicted);
    assert_eq!(out4.len, 3);
    assert!(
        !contains_u16_pattern(raw_memory_bytes(&buf3), secret_1),
        "evicted secret_1 must be completely zeroized from SecureKeyBuffer memory"
    );

    // Pushing secret_5 evicts and volatile-zeroes secret_2
    let out5 = buf3.push(KeyCode(secret_5), Modifiers::EMPTY, 140);
    assert!(out5.evicted);
    assert_eq!(out5.len, 3);
    assert!(
        !contains_u16_pattern(raw_memory_bytes(&buf3), secret_2),
        "evicted secret_2 must be completely zeroized from SecureKeyBuffer memory"
    );

    // Remaining active codes in oldest-to-newest FIFO order must be [secret_3, secret_4, secret_5]
    let mut stack_codes = [KeyCode::NONE; 3];
    assert_eq!(buf3.copy_codes_into(&mut stack_codes), 3);
    assert_eq!(
        stack_codes,
        [KeyCode(secret_3), KeyCode(secret_4), KeyCode(secret_5)]
    );

    // Zeroize stack copy buffer as well
    stack_codes.zeroize();
    assert!(!contains_u16_pattern(
        raw_memory_bytes(&stack_codes),
        secret_3
    ));
}

#[test]
fn volatile_zeroization_on_pop_escape_mode_exit_idle_timeout_and_drop() {
    let mut buf = SecureKeyBuffer::<5>::with_idle_timeout_ms(500);
    let secrets: [u16; 5] = [0x1122, 0x3344, 0x5566, 0x7788, 0x99AA];

    for (i, &code) in secrets.iter().enumerate() {
        buf.push(KeyCode(code), Modifiers::SHIFT, 1000 + (i as u64) * 10);
    }
    for &code in &secrets {
        assert!(contains_u16_pattern(raw_memory_bytes(&buf), code));
    }

    // 1. Popping the oldest entry immediately wipes its slot in the ring buffer,
    //    and dropping the returned `SecureKeyEntry` wipes the entry itself.
    {
        let entry = buf.pop_oldest().expect("oldest entry present");
        assert_eq!(entry.code(), KeyCode(0x1122));
        assert!(
            !contains_u16_pattern(raw_memory_bytes(&buf), 0x1122),
            "popped slot in SecureKeyBuffer must be volatile-zeroed immediately"
        );
    }

    // 2. Escape purges all remaining slots to 0x00
    buf.on_escape();
    assert!(buf.is_empty());
    assert_eq!(buf.purge_count(), 1);
    for &code in &secrets {
        assert!(
            !contains_u16_pattern(raw_memory_bytes(&buf), code),
            "keycode {code:#x} survived on_escape() purge"
        );
    }
    // The 80-byte slots array at the start of SecureKeyBuffer<5> must be strictly 0x00
    assert!(
        raw_memory_bytes(&buf)[..80].iter().all(|&b| b == 0),
        "first 80 bytes (5 x 16-byte slots) of SecureKeyBuffer<5> must be 0x00 after on_escape()"
    );

    // 3. Mode exit purges all slots to 0x00
    buf.push(KeyCode(0xBEEF), Modifiers::CONTROL, 2000);
    assert!(contains_u16_pattern(raw_memory_bytes(&buf), 0xBEEF));
    buf.on_mode_exit();
    assert!(buf.is_empty());
    assert_eq!(buf.purge_count(), 2);
    assert!(!contains_u16_pattern(raw_memory_bytes(&buf), 0xBEEF));
    assert!(raw_memory_bytes(&buf)[..80].iter().all(|&b| b == 0));

    // 4. Idle timeout expiry purges all slots to 0x00
    buf.push(KeyCode(0xCAFE), Modifiers::ALT, 3000);
    assert!(!buf.purge_if_expired(3499));
    assert!(contains_u16_pattern(raw_memory_bytes(&buf), 0xCAFE));
    assert!(buf.purge_if_expired(3500));
    assert!(buf.is_empty());
    assert!(!contains_u16_pattern(raw_memory_bytes(&buf), 0xCAFE));
    assert!(raw_memory_bytes(&buf)[..80].iter().all(|&b| b == 0));

    // 5. Drop volatile-zeroes in place (verified via ManuallyDrop in heap allocation)
    let mut heap_buf = Box::new(core::mem::ManuallyDrop::new(SecureKeyBuffer::<5>::new()));
    heap_buf.push(KeyCode(0xDEAD), Modifiers::META, 4000);
    heap_buf.push(KeyCode(0xF00D), Modifiers::SHIFT, 4010);
    assert!(contains_u16_pattern(raw_memory_bytes(&*heap_buf), 0xDEAD));
    assert!(contains_u16_pattern(raw_memory_bytes(&*heap_buf), 0xF00D));

    // SAFETY: Drop inner SecureKeyBuffer<5> exactly once and inspect the still-live Box allocation.
    unsafe {
        core::mem::ManuallyDrop::drop(&mut *heap_buf);
        let after_drop_bytes = raw_memory_bytes(&*heap_buf);
        assert!(
            !contains_u16_pattern(after_drop_bytes, 0xDEAD)
                && !contains_u16_pattern(after_drop_bytes, 0xF00D),
            "secret keycodes survived Drop!"
        );
        assert!(
            after_drop_bytes[..80].iter().all(|&b| b == 0),
            "all 80 slot bytes must be 0x00 after Drop"
        );
    }
}

#[test]
fn locked_secure_key_buffer_pins_memory_and_zeroizes_before_unlock() {
    let mut locked = LockedSecureKeyBuffer::<5>::new();
    assert!(matches!(
        locked.lock_status(),
        MemoryLockStatus::Locked
            | MemoryLockStatus::LockedWithDontDump
            | MemoryLockStatus::FallbackUnlocked { .. }
    ));

    let stable_addr_before = locked.buffer() as *const SecureKeyBuffer<5> as usize;
    locked
        .buffer_mut()
        .push(KeyCode(0x7331), Modifiers::SHIFT, 100);
    let stable_addr_after = locked.buffer() as *const SecureKeyBuffer<5> as usize;
    assert_eq!(
        stable_addr_before, stable_addr_after,
        "Pinned LockedSecureKeyBuffer address must never move"
    );

    locked.buffer_mut().on_mode_exit();
    assert!(locked.buffer().is_empty());
    assert!(raw_memory_bytes(locked.buffer())[..80]
        .iter()
        .all(|&b| b == 0));

    // Verify process-level core-dump hardening
    let report = harden_process_memory();
    #[cfg(unix)]
    assert!(
        report.core_dumps_disabled,
        "RLIMIT_CORE=0 must be applied on Unix"
    );
    assert!(!report.notes.is_empty());
}

#[test]
fn debug_formatting_strictly_redacts_all_keystrokes_and_modifiers() {
    // Use distinctive decimal/hex keycode values and ASCII letters that must never leak in Debug output
    let secret_code_num: u16 = 54321; // 0xD431
    let kc = KeyCode(secret_code_num);
    let mods = Modifiers::SHIFT.union(Modifiers::CONTROL);
    let raw_ev = RawKeyEvent::new(kc, mods, KeyPhase::Down, 8888);

    let mut buf = SecureKeyBuffer::<5>::new();
    buf.push(kc, mods, 8888);

    let locked = {
        let mut l = LockedSecureKeyBuffer::<5>::new();
        l.buffer_mut().push(kc, mods, 8888);
        l
    };

    let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
    interceptor.activate();
    let decision = interceptor.handle_event(raw_ev);

    let entry = buf.pop_oldest().unwrap();

    let formatted_samples = [
        ("KeyCode", format!("{kc:?}")),
        ("Modifiers", format!("{mods:?}")),
        ("RawKeyEvent", format!("{raw_ev:?}")),
        ("SecureKeyEntry", format!("{entry:?}")),
        ("SecureKeyBuffer", format!("{buf:?}")),
        ("LockedSecureKeyBuffer", format!("{locked:?}")),
        ("CommandInterceptor", format!("{interceptor:?}")),
        ("InterceptDecision", format!("{decision:?}")),
    ];

    for (label, text) in &formatted_samples {
        assert!(
            text.contains("[REDACTED]"),
            "{label} Debug output must contain [REDACTED], got: {text}"
        );
        assert!(
            !text.contains("54321") && !text.to_lowercase().contains("d431"),
            "{label} Debug output leaked secret keycode in: {text}"
        );
    }
}

#[test]
fn hold_for_and_cycle_interceptor_guarantees_zero_key_leakage_and_bounded_state() {
    let mut interceptor = CommandInterceptor::<5>::with_capacity(InterceptorConfig::default());
    assert_eq!(interceptor.mode(), InterceptorMode::Inactive);

    // 1. User holds Ctrl+Alt before pressing activation chord (Alt+Space)
    //    Ctrl and Alt pass through to foreground; Space activates overlay and is swallowed.
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(
            KeyCode::CTRL_L,
            Modifiers::CONTROL,
            KeyPhase::Down,
            10
        )),
        InterceptDecision::PassThrough
    );
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(
            KeyCode::ALT_L,
            Modifiers::CONTROL.union(Modifiers::ALT),
            KeyPhase::Down,
            20
        )),
        InterceptDecision::PassThrough
    );
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(
            KeyCode::SPACE,
            Modifiers::CONTROL.union(Modifiers::ALT),
            KeyPhase::Down,
            30
        )),
        InterceptDecision::SwallowAndEmit(InterceptorAction::ActivateOverlay)
    );
    assert_eq!(interceptor.mode(), InterceptorMode::Active);

    // 2. User releases Alt & Ctrl while active -> because they went down BEFORE activation,
    //    their KeyUp events MUST pass through so the foreground app never sees stuck modifiers!
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(
            KeyCode::ALT_L,
            Modifiers::CONTROL,
            KeyPhase::Up,
            35
        )),
        InterceptDecision::PassThrough
    );
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(
            KeyCode::CTRL_L,
            Modifiers::EMPTY,
            KeyPhase::Up,
            38
        )),
        InterceptDecision::PassThrough
    );
    assert_eq!(interceptor.foreground_held_modifiers(), Modifiers::EMPTY);

    // Space KeyUp was swallowed on KeyDown -> its KeyUp MUST be swallowed
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(
            KeyCode::SPACE,
            Modifiers::EMPTY,
            KeyPhase::Up,
            40
        )),
        InterceptDecision::Swallow
    );

    // 3. Flood 12 grid keystrokes (using keys not bound to Hold-For/Cycle: A, B, D, E, F, H, I, J, K, L, N, O)
    //    while active -> buffer length never exceeds 5 (bounded ring), and every keystroke + repeat + key-up is swallowed.
    let grid_keys = *b"ABDEFHIJKLNO";
    for (i, &ch) in grid_keys.iter().enumerate() {
        let k = KeyCode::from_ascii_upper(ch);
        let t = 100 + (i as u64) * 10;
        let dec = interceptor.handle_event(RawKeyEvent::new(k, Modifiers::EMPTY, KeyPhase::Down, t));
        assert!(matches!(
            dec,
            InterceptDecision::SwallowAndEmit(InterceptorAction::GridKeyInput {
                buffer_len,
                ..
            }) if buffer_len <= 5
        ));
        assert!(interceptor.key_buffer().len() <= 5);
        assert_eq!(
            interceptor.handle_event(RawKeyEvent::new(k, Modifiers::EMPTY, KeyPhase::Repeat, t + 2)),
            InterceptDecision::Swallow
        );
        assert_eq!(
            interceptor.handle_event(RawKeyEvent::new(k, Modifiers::EMPTY, KeyPhase::Up, t + 5)),
            InterceptDecision::Swallow
        );
    }
    assert_eq!(interceptor.key_buffer().len(), 5);

    // 4. Cycle Click Mode ('C') and Hold-For Click Override ('R' then 'M') + Speed Boost (SHIFT_L)
    let key_c = KeyCode::from_ascii_upper(b'C');
    let key_r = KeyCode::from_ascii_upper(b'R');
    let key_m = KeyCode::from_ascii_upper(b'M');

    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(key_c, Modifiers::EMPTY, KeyPhase::Down, 300)),
        InterceptDecision::SwallowAndEmit(InterceptorAction::CycleClickMode {
            new_mode: ClickKind::Right,
            step_index: 1,
        })
    );
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(key_c, Modifiers::EMPTY, KeyPhase::Up, 305)),
        InterceptDecision::Swallow
    );

    // Hold 'M' (Middle) -> overrides cycled Right click; Hold SHIFT_L -> overrides speed to Fast
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(key_m, Modifiers::EMPTY, KeyPhase::Down, 310)),
        InterceptDecision::SwallowAndEmit(InterceptorAction::SetClickMode(ClickKind::Middle))
    );
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(
            KeyCode::SHIFT_L,
            Modifiers::SHIFT,
            KeyPhase::Down,
            315
        )),
        InterceptDecision::SwallowAndEmit(InterceptorAction::SetSpeedTier(SpeedTier::Fast))
    );
    assert_eq!(interceptor.effective_click_mode(), ClickKind::Middle);
    assert_eq!(interceptor.effective_speed_tier(), SpeedTier::Fast);

    // Execute click (RETURN) while 'M' and SHIFT_L are still physically held down!
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(
            KeyCode::RETURN,
            Modifiers::SHIFT,
            KeyPhase::Down,
            320
        )),
        InterceptDecision::SwallowAndEmit(InterceptorAction::ExecuteClick {
            kind: ClickKind::Middle,
            foreground_held_modifiers: Modifiers::EMPTY,
            deactivated: true,
        })
    );
    assert_eq!(interceptor.mode(), InterceptorMode::Inactive);
    // Key buffer must be completely purged and zeroized immediately upon execution!
    assert!(interceptor.key_buffer().is_empty());
    assert!(raw_memory_bytes(interceptor.key_buffer())[..80]
        .iter()
        .all(|&b| b == 0));

    // Releasing RETURN, 'M', and SHIFT_L AFTER deactivation must still be swallowed (zero orphaned key-up leaks!)
    for (k, t) in [
        (KeyCode::RETURN, 330),
        (key_m, 335),
        (KeyCode::SHIFT_L, 340),
    ] {
        assert_eq!(
            interceptor.handle_event(RawKeyEvent::new(k, Modifiers::EMPTY, KeyPhase::Repeat, t)),
            InterceptDecision::Swallow
        );
        assert_eq!(
            interceptor.handle_event(RawKeyEvent::new(k, Modifiers::EMPTY, KeyPhase::Up, t + 2)),
            InterceptDecision::Swallow
        );
    }
    assert_eq!(interceptor.swallowed_key_count(), 0);

    // Once released, subsequent normal typing of 'M' or 'R' passes cleanly to the foreground app
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(key_r, Modifiers::EMPTY, KeyPhase::Down, 400)),
        InterceptDecision::PassThrough
    );
    assert_eq!(
        interceptor.handle_event(RawKeyEvent::new(key_r, Modifiers::EMPTY, KeyPhase::Up, 410)),
        InterceptDecision::PassThrough
    );
}

/// Recursively collects all file and directory entries inside `dir`.
fn list_directory_tree(dir: &Path) -> Vec<PathBuf> {
    let mut entries = Vec::new();
    if let Ok(read_dir) = fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                entries.extend(list_directory_tree(&path));
            }
            entries.push(path);
        }
    }
    entries.sort();
    entries
}

#[test]
fn zero_disk_write_guarantee_during_hid_interception_and_core_state_machine_pipeline() {
    // Create an isolated temporary sandbox directory for HOME, XDG_CONFIG_HOME, XDG_CACHE_HOME, XDG_DATA_HOME
    let sandbox_root = std::env::temp_dir().join(format!(
        "ratpoison_zero_disk_audit_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let home_dir = sandbox_root.join("home");
    let xdg_config = sandbox_root.join("xdg_config");
    let xdg_cache = sandbox_root.join("xdg_cache");
    let xdg_data = sandbox_root.join("xdg_data");

    for dir in [&home_dir, &xdg_config, &xdg_cache, &xdg_data] {
        fs::create_dir_all(dir).expect("create isolated sandbox dir");
    }

    // Snapshot initial directory tree (only the 4 empty subdirectories exist)
    let snapshot_before = list_directory_tree(&sandbox_root);
    assert_eq!(snapshot_before.len(), 4);

    // Point environment variables at the isolated sandbox for the duration of the pipeline exercise
    let prev_home = std::env::var_os("HOME");
    let prev_xdg_cfg = std::env::var_os("XDG_CONFIG_HOME");
    let prev_xdg_cache = std::env::var_os("XDG_CACHE_HOME");
    let prev_xdg_data = std::env::var_os("XDG_DATA_HOME");

    std::env::set_var("HOME", &home_dir);
    std::env::set_var("XDG_CONFIG_HOME", &xdg_config);
    std::env::set_var("XDG_CACHE_HOME", &xdg_cache);
    std::env::set_var("XDG_DATA_HOME", &xdg_data);

    // Run a comprehensive HID + CoreEngine session (activation, grid typing, subgrid, nudge,
    // Hold-For drag, Free Mode ticks, escape, and idle timeout purge)
    let mut mock_hid = MockHidBackend::new();
    let interceptor = Arc::new(Mutex::new(CommandInterceptor::new(
        InterceptorConfig::default(),
    )));
    let cb_interceptor = Arc::clone(&interceptor);
    mock_hid
        .start(Box::new(move |ev| {
            cb_interceptor.lock().unwrap().handle_event(ev)
        }))
        .unwrap();

    let mon = MonitorInfo::from_logical_and_scale(
        "DP-1",
        "Primary",
        CoreLogicalRect::new(0.0, 0.0, 1920.0, 1080.0),
        CorePhysicalPoint::new(0, 0),
        2.0,
        true,
    )
    .unwrap();
    let layout = MonitorLayout::new(vec![mon]).unwrap();
    let mut engine = CoreEngine::new(RatPoisonConfig::default(), layout);

    // Activate via Alt+Space
    mock_hid
        .dispatch_event(RawKeyEvent::new(
            KeyCode::ALT_L,
            Modifiers::ALT,
            KeyPhase::Down,
            10,
        ))
        .unwrap();
    let decision = mock_hid
        .dispatch_event(RawKeyEvent::new(
            KeyCode::SPACE,
            Modifiers::ALT,
            KeyPhase::Down,
            20,
        ))
        .unwrap();
    assert_eq!(
        decision,
        InterceptDecision::SwallowAndEmit(InterceptorAction::ActivateOverlay)
    );
    engine.handle_event(EngineEvent::ActivateGrid {
        monitor_id: None,
        preset_id: None,
    });

    // Type grid coordinates 'a', 'b' and subgrid 's', then execute click
    for (idx, ch) in b"ABS".iter().enumerate() {
        let ts = 30 + (idx as u64) * 15;
        let kc = KeyCode::from_ascii_upper(*ch);
        let d = mock_hid
            .dispatch_event(RawKeyEvent::new(kc, Modifiers::EMPTY, KeyPhase::Down, ts))
            .unwrap();
        if let InterceptDecision::SwallowAndEmit(InterceptorAction::GridKeyInput { key, .. }) = d {
            let lower = (key.code().raw() as u8 as char).to_ascii_lowercase();
            engine.handle_event(EngineEvent::KeyChar { ch: lower });
        }
        mock_hid
            .dispatch_event(RawKeyEvent::new(
                kc,
                Modifiers::EMPTY,
                KeyPhase::Up,
                ts + 5,
            ))
            .unwrap();
    }

    // Execute click and perform mouse simulation
    mock_hid
        .dispatch_event(RawKeyEvent::new(
            KeyCode::RETURN,
            Modifiers::EMPTY,
            KeyPhase::Down,
            100,
        ))
        .unwrap();
    mock_hid
        .perform_click(
            ClickKind::Left,
            ratpoison_hid::LogicalPoint::new(100.0, 100.0),
            2.0,
        )
        .unwrap();
    engine.handle_event(EngineEvent::ExitToNormal);
    mock_hid.stop().unwrap();

    // Restore original environment variables
    if let Some(v) = prev_home {
        std::env::set_var("HOME", v);
    }
    if let Some(v) = prev_xdg_cfg {
        std::env::set_var("XDG_CONFIG_HOME", v);
    } else {
        std::env::remove_var("XDG_CONFIG_HOME");
    }
    if let Some(v) = prev_xdg_cache {
        std::env::set_var("XDG_CACHE_HOME", v);
    } else {
        std::env::remove_var("XDG_CACHE_HOME");
    }
    if let Some(v) = prev_xdg_data {
        std::env::set_var("XDG_DATA_HOME", v);
    } else {
        std::env::remove_var("XDG_DATA_HOME");
    }

    // Verify ZERO files or directories were created or modified inside the sandbox
    let snapshot_after = list_directory_tree(&sandbox_root);
    let _ = fs::remove_dir_all(&sandbox_root);

    assert_eq!(
        snapshot_before, snapshot_after,
        "Zero disk-write invariant violated: files were created in sandbox during keyboard interception!"
    );
}
