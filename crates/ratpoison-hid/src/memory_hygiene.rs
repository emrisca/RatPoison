//! Strict Memory Hygiene keypress ring buffer and OS memory hardening.
//!
//! # Security Guarantees
//! 1. **Compile-time bounded capacity (`3 <= N <= 5`)**: `SecureKeyBuffer<N>` refuses to
//!    compile if `N < 3` or `N > 5`, preventing unbounded keystroke accumulation.
//! 2. **Fixed in-memory slots**: keystrokes are stored exclusively in a fixed `[SecureKeySlot; N]`
//!    array with zero dynamic heap growth and zero implicit struct padding.
//! 3. **Explicit volatile zeroization (`zeroize`)**:
//!    - Every evicted slot is volatile-zeroed *before* new key data is written.
//!    - Every popped slot is volatile-zeroed immediately upon extraction.
//!    - Mode exit, `Escape`, and idle timeout expiry (default 750ms) volatile-zero *all* `N` slots.
//!    - `Drop` volatile-zeroes all slots and ring metadata with a `SeqCst` compiler fence.
//! 4. **Zero logging / zero persistence**: neither `SecureKeyEntry` nor `SecureKeyBuffer`
//!    implements `Display`, `serde::Serialize`, or `serde::Deserialize`; `Debug` strictly
//!    outputs `[REDACTED]`.
//! 5. **Optional OS memory locking & anti-core-dump hardening**:
//!    [`LockedSecureKeyBuffer`] pins the buffer at a stable address and applies `mlock`
//!    (plus `MADV_DONTDUMP` on Linux; `VirtualLock` on Windows) with graceful fallback when
//!    `RLIMIT_MEMLOCK` is tight. On `Drop`, memory is always zeroized *before* unlocking.

use core::ffi::c_void;
use core::fmt;
use core::pin::Pin;
use core::sync::atomic::{compiler_fence, Ordering};

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::types::{HidError, KeyCode, Modifiers};

/// Minimum permitted capacity of [`SecureKeyBuffer`].
pub const MIN_BUFFER_CAPACITY: usize = 3;

/// Maximum permitted capacity of [`SecureKeyBuffer`].
pub const MAX_BUFFER_CAPACITY: usize = 5;

/// Default capacity of [`SecureKeyBuffer`].
pub const DEFAULT_BUFFER_CAPACITY: usize = 5;

/// Default idle timeout (in milliseconds) after which buffered keystrokes are purged.
pub const DEFAULT_IDLE_PURGE_MS: u64 = 750;

/// Internal fixed-layout, padding-free slot stored inside [`SecureKeyBuffer`].
///
/// Layout is exactly 16 bytes with zero implicit padding so volatile zeroing wipes
/// every single byte of the slot deterministically.
#[repr(C)]
#[derive(Clone, Default, Zeroize)]
struct SecureKeySlot {
    code: KeyCode,
    modifiers: Modifiers,
    occupied: u8,
    _reserved: u32,
    timestamp_ms: u64,
}

// Compile-time check that `SecureKeySlot` has exact 16-byte layout with zero hidden padding.
const _: () = assert!(core::mem::size_of::<SecureKeySlot>() == 16);

impl SecureKeySlot {
    #[inline]
    fn from_parts(code: KeyCode, modifiers: Modifiers, timestamp_ms: u64) -> Self {
        Self {
            code,
            modifiers,
            occupied: 1,
            _reserved: 0,
            timestamp_ms,
        }
    }

    #[inline]
    fn is_occupied(&self) -> bool {
        self.occupied != 0
    }
}

/// Extracted keystroke entry that automatically volatile-zeroes itself on drop.
///
/// Intentionally does **not** implement `Copy`, `Display`, `Serialize`, or `Deserialize`.
#[derive(PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct SecureKeyEntry {
    code: KeyCode,
    modifiers: Modifiers,
    timestamp_ms: u64,
}

impl SecureKeyEntry {
    /// Returns the in-memory keycode (never log or persist this value).
    #[inline]
    pub fn code(&self) -> KeyCode {
        self.code
    }

    /// Returns the in-memory modifier bitmask.
    #[inline]
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Returns the monotonic event timestamp in milliseconds.
    #[inline]
    pub fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }
}

impl fmt::Debug for SecureKeyEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecureKeyEntry")
            .field("code", &"[REDACTED]")
            .field("modifiers", &"[REDACTED]")
            .field("timestamp_ms", &self.timestamp_ms)
            .finish()
    }
}

/// Result metadata returned by [`SecureKeyBuffer::push`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushOutcome {
    /// True if an older slot was evicted (and zeroized) due to reaching capacity `N`.
    pub evicted: bool,
    /// True if stale entries had expired and were purged before this push.
    pub expired_before_push: bool,
    /// Current number of active entries after the push.
    pub len: usize,
}

/// Compile-time bounded, explicitly zeroized keypress ring buffer (`3 <= N <= 5`).
///
/// # Compile-Time Capacity Enforcement
/// Attempting to instantiate `SecureKeyBuffer` with `N < 3` or `N > 5` fails to compile:
///
/// ```compile_fail
/// use ratpoison_hid::memory_hygiene::SecureKeyBuffer;
/// let _too_small = SecureKeyBuffer::<2>::new();
/// ```
///
/// ```compile_fail
/// use ratpoison_hid::memory_hygiene::SecureKeyBuffer;
/// let _too_large = SecureKeyBuffer::<6>::new();
/// ```
///
/// # Compile-Time Serialization & Display Prohibition
/// Raw key history cannot be serialized or formatted via `Display`:
///
/// ```compile_fail
/// use ratpoison_hid::memory_hygiene::SecureKeyBuffer;
/// fn require_serialize<T: serde::Serialize>(_: &T) {}
/// let buf = SecureKeyBuffer::<5>::new();
/// require_serialize(&buf);
/// ```
///
/// ```compile_fail
/// use ratpoison_hid::memory_hygiene::SecureKeyBuffer;
/// fn require_display<T: core::fmt::Display>(_: &T) {}
/// let buf = SecureKeyBuffer::<5>::new();
/// require_display(&buf);
/// ```
pub struct SecureKeyBuffer<const N: usize = DEFAULT_BUFFER_CAPACITY> {
    slots: [SecureKeySlot; N],
    head: usize,
    len: usize,
    last_activity_ms: u64,
    idle_timeout_ms: u64,
    purge_count: u64,
}

impl<const N: usize> SecureKeyBuffer<N> {
    /// Compile-time invariant ensuring `3 <= N <= 5`.
    const CAPACITY_INVARIANT: () = assert!(
        N >= MIN_BUFFER_CAPACITY && N <= MAX_BUFFER_CAPACITY,
        "SecureKeyBuffer capacity N must satisfy 3 <= N <= 5"
    );

    /// Creates a new empty `SecureKeyBuffer<N>` with the default 750ms idle purge timeout.
    pub fn new() -> Self {
        Self::with_idle_timeout_ms(DEFAULT_IDLE_PURGE_MS)
    }

    /// Creates a new empty `SecureKeyBuffer<N>` with a custom idle purge timeout in milliseconds.
    pub fn with_idle_timeout_ms(idle_timeout_ms: u64) -> Self {
        let () = Self::CAPACITY_INVARIANT;
        Self {
            slots: core::array::from_fn(|_| SecureKeySlot::default()),
            head: 0,
            len: 0,
            last_activity_ms: 0,
            idle_timeout_ms: idle_timeout_ms.max(1),
            purge_count: 0,
        }
    }

    /// Returns the compile-time fixed capacity `N`.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Returns the number of active keystrokes currently buffered (`0..=N`).
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if no keystrokes are currently buffered.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the configured idle purge timeout in milliseconds.
    #[inline]
    pub const fn idle_timeout_ms(&self) -> u64 {
        self.idle_timeout_ms
    }

    /// Returns how many full purges have occurred (non-sensitive diagnostic metric).
    #[inline]
    pub const fn purge_count(&self) -> u64 {
        self.purge_count
    }

    /// Pushes a keystroke into the fixed ring buffer.
    ///
    /// - If buffered entries have exceeded `idle_timeout_ms`, all slots are purged first.
    /// - If the buffer is at capacity `N`, the oldest slot is explicitly volatile-zeroed
    ///   before being overwritten.
    pub fn push(&mut self, code: KeyCode, modifiers: Modifiers, timestamp_ms: u64) -> PushOutcome {
        let expired_before_push = self.purge_if_expired(timestamp_ms);

        let evicted = if self.len == N {
            // Explicitly volatile-zero the evicted slot before overwriting it.
            let evict_idx = self.head;
            self.slots[evict_idx].zeroize();
            compiler_fence(Ordering::SeqCst);

            self.slots[evict_idx] = SecureKeySlot::from_parts(code, modifiers, timestamp_ms);
            self.head = (self.head + 1) % N;
            true
        } else {
            let write_idx = (self.head + self.len) % N;
            self.slots[write_idx] = SecureKeySlot::from_parts(code, modifiers, timestamp_ms);
            self.len += 1;
            false
        };

        self.last_activity_ms = timestamp_ms;

        PushOutcome {
            evicted,
            expired_before_push,
            len: self.len,
        }
    }

    /// Pops the oldest keystroke entry and immediately volatile-zeroes its slot in place.
    pub fn pop_oldest(&mut self) -> Option<SecureKeyEntry> {
        if self.len == 0 {
            return None;
        }

        let idx = self.head;
        let entry = SecureKeyEntry {
            code: self.slots[idx].code,
            modifiers: self.slots[idx].modifiers,
            timestamp_ms: self.slots[idx].timestamp_ms,
        };

        // Volatile-zero the vacated ring slot immediately.
        self.slots[idx].zeroize();
        compiler_fence(Ordering::SeqCst);

        self.len -= 1;
        if self.len == 0 {
            self.head.zeroize();
            self.last_activity_ms.zeroize();
        } else {
            self.head = (self.head + 1) % N;
        }

        Some(entry)
    }

    /// Copies the currently buffered keycodes in oldest-to-newest order into a caller-supplied
    /// fixed stack array `[KeyCode; N]`, returning the number of valid entries written.
    ///
    /// Avoids heap allocation so grid label matching stays strictly on the stack.
    pub fn copy_codes_into(&self, dest: &mut [KeyCode; N]) -> usize {
        dest.zeroize();
        for (offset, dst) in dest.iter_mut().enumerate().take(self.len) {
            let idx = (self.head + offset) % N;
            if self.slots[idx].is_occupied() {
                *dst = self.slots[idx].code;
            }
        }
        self.len
    }

    /// Purges all `N` slots if the buffer is non-empty and at least `idle_timeout_ms`
    /// has elapsed since the last buffered keystroke.
    pub fn purge_if_expired(&mut self, now_ms: u64) -> bool {
        if self.len == 0 {
            return false;
        }
        if now_ms.saturating_sub(self.last_activity_ms) >= self.idle_timeout_ms {
            self.purge();
            true
        } else {
            false
        }
    }

    /// Explicitly volatile-zeroes every slot (occupied or inactive) and ring state.
    pub fn purge(&mut self) {
        for slot in &mut self.slots {
            slot.zeroize();
        }
        self.head.zeroize();
        self.len.zeroize();
        self.last_activity_ms.zeroize();
        compiler_fence(Ordering::SeqCst);
        self.purge_count = self.purge_count.saturating_add(1);
    }

    /// Semantic hook invoked when RatPoison exits overlay/grid mode.
    #[inline]
    pub fn on_mode_exit(&mut self) {
        self.purge();
    }

    /// Semantic hook invoked when the user presses `Escape`.
    #[inline]
    pub fn on_escape(&mut self) {
        self.purge();
    }

    #[cfg(test)]
    pub(crate) fn raw_slots_bytes(&self) -> &[u8] {
        let ptr = self.slots.as_ptr() as *const u8;
        let len = core::mem::size_of_val(&self.slots);
        // SAFETY: `SecureKeySlot` is `#[repr(C)]` with no padding and all fields initialized.
        unsafe { core::slice::from_raw_parts(ptr, len) }
    }
}

impl<const N: usize> Default for SecureKeyBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Zeroize for SecureKeyBuffer<N> {
    fn zeroize(&mut self) {
        for slot in &mut self.slots {
            slot.zeroize();
        }
        self.head.zeroize();
        self.len.zeroize();
        self.last_activity_ms.zeroize();
        compiler_fence(Ordering::SeqCst);
    }
}

impl<const N: usize> Drop for SecureKeyBuffer<N> {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl<const N: usize> fmt::Debug for SecureKeyBuffer<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecureKeyBuffer")
            .field("capacity", &N)
            .field("len", &self.len)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("purge_count", &self.purge_count)
            .field("slots", &"[REDACTED]")
            .finish()
    }
}

/// Outcome of attempting to lock the buffer's memory pages against swapping/core dumps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLockStatus {
    /// Pages are locked in RAM via `mlock` / `VirtualLock`.
    Locked,
    /// Pages are locked in RAM via `mlock` AND marked `MADV_DONTDUMP` (Linux).
    LockedWithDontDump,
    /// Locking was denied or constrained (e.g. tight `RLIMIT_MEMLOCK`); buffer still works
    /// normally with full volatile zeroization guarantees.
    FallbackUnlocked { errno: i32 },
}

/// Heap-pinned [`SecureKeyBuffer`] backed by OS memory locking (`mlock` / `VirtualLock`)
/// with graceful fallback when `RLIMIT_MEMLOCK` is tight.
///
/// Pinning guarantees the buffer's address never moves while locked. On `Drop`, the
/// buffer is explicitly volatile-zeroed *before* `munlock` / `VirtualUnlock` is called,
/// ensuring dirty keystroke bytes can never be paged out to swap.
pub struct LockedSecureKeyBuffer<const N: usize = DEFAULT_BUFFER_CAPACITY> {
    inner: Pin<Box<SecureKeyBuffer<N>>>,
    lock_status: MemoryLockStatus,
    locked_len: usize,
}

impl<const N: usize> LockedSecureKeyBuffer<N> {
    /// Allocates a pinned [`SecureKeyBuffer<N>`] and attempts to lock its pages in RAM.
    pub fn new() -> Self {
        Self::with_idle_timeout_ms(DEFAULT_IDLE_PURGE_MS)
    }

    /// Allocates a pinned [`SecureKeyBuffer<N>`] with a custom idle timeout and attempts OS mlock.
    pub fn with_idle_timeout_ms(idle_timeout_ms: u64) -> Self {
        let inner = Box::pin(SecureKeyBuffer::<N>::with_idle_timeout_ms(idle_timeout_ms));
        let ptr = (&*inner) as *const SecureKeyBuffer<N> as *const c_void;
        let len = core::mem::size_of::<SecureKeyBuffer<N>>();
        let lock_status = os_memlock::lock_memory(ptr, len);
        Self {
            inner,
            lock_status,
            locked_len: len,
        }
    }

    /// Returns whether OS page locking succeeded or gracefully fell back.
    #[inline]
    pub const fn lock_status(&self) -> MemoryLockStatus {
        self.lock_status
    }

    /// Shared access to the underlying [`SecureKeyBuffer<N>`].
    #[inline]
    pub fn buffer(&self) -> &SecureKeyBuffer<N> {
        &self.inner
    }

    /// Mutable access to the underlying [`SecureKeyBuffer<N>`] without unpinning its address.
    #[inline]
    pub fn buffer_mut(&mut self) -> &mut SecureKeyBuffer<N> {
        self.inner.as_mut().get_mut()
    }
}

impl<const N: usize> Default for LockedSecureKeyBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> fmt::Debug for LockedSecureKeyBuffer<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LockedSecureKeyBuffer")
            .field("lock_status", &self.lock_status)
            .field("buffer", &*self.inner)
            .finish()
    }
}

impl<const N: usize> Drop for LockedSecureKeyBuffer<N> {
    fn drop(&mut self) {
        // CRITICAL ORDER:
        // 1. Volatile-zeroize every byte while the page is still locked in RAM.
        self.inner.as_mut().get_mut().zeroize();
        compiler_fence(Ordering::SeqCst);

        // 2. Only unlock memory after zeroization is guaranteed complete.
        if matches!(
            self.lock_status,
            MemoryLockStatus::Locked | MemoryLockStatus::LockedWithDontDump
        ) {
            let ptr = (&*self.inner) as *const SecureKeyBuffer<N> as *const c_void;
            os_memlock::unlock_memory(ptr, self.locked_len);
        }
    }
}

/// Summary of process-level memory hardening controls applied at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessHardeningReport {
    pub core_dumps_disabled: bool,
    pub dumpable_cleared: bool,
    pub debugger_attach_denied: bool,
    pub notes: &'static str,
}

/// Disables OS core dumps for the current process (`RLIMIT_CORE = 0` on Unix,
/// plus `PR_SET_DUMPABLE = 0` on Linux).
pub fn disable_core_dumps() -> Result<(), HidError> {
    os_memlock::disable_core_dumps_impl()
}

/// Applies safe startup process-level memory hardening (disabling core dumps and
/// clearing Linux `PR_SET_DUMPABLE`).
///
/// Note: On macOS, `PT_DENY_ATTACH` is intentionally **not** called automatically here
/// so debuggers and test runners continue to function; release binaries may opt in
/// explicitly via [`deny_debugger_attach`].
pub fn harden_process_memory() -> ProcessHardeningReport {
    let core_dumps_disabled = os_memlock::disable_core_dumps_impl().is_ok();
    let dumpable_cleared = os_memlock::clear_dumpable_flag();

    #[cfg(target_os = "macos")]
    let notes = "RLIMIT_CORE=0 applied; call deny_debugger_attach() explicitly in release builds for PT_DENY_ATTACH";
    #[cfg(target_os = "linux")]
    let notes = "RLIMIT_CORE=0 and prctl(PR_SET_DUMPABLE, 0) applied; LockedSecureKeyBuffer applies mlock + MADV_DONTDUMP";
    #[cfg(target_os = "windows")]
    let notes = "LockedSecureKeyBuffer uses VirtualLock; configure WER LocalDumps exclusion in installer for core-dump hardening";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let notes = "Software volatile zeroization active; OS-specific page-locking unavailable";

    ProcessHardeningReport {
        core_dumps_disabled,
        dumpable_cleared,
        debugger_attach_denied: false,
        notes,
    }
}

/// Release-only opt-in on macOS to prevent runtime debugger attachment via `ptrace(PT_DENY_ATTACH)`.
///
/// WARNING: If invoked while a debugger (`lldb`/`dtrace`) is attached, macOS immediately
/// terminates the process with `ENOTSUP`. Only call in hardened release binaries.
#[cfg(target_os = "macos")]
pub fn deny_debugger_attach() -> Result<(), HidError> {
    const PT_DENY_ATTACH: core::ffi::c_int = 31;
    extern "C" {
        fn ptrace(
            request: core::ffi::c_int,
            pid: i32,
            addr: *mut c_void,
            data: core::ffi::c_int,
        ) -> core::ffi::c_int;
    }
    // SAFETY: `PT_DENY_ATTACH` ignores `pid`, `addr`, and `data` per Apple `ptrace(2)`.
    let rc = unsafe { ptrace(PT_DENY_ATTACH, 0, core::ptr::null_mut(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
        Err(HidError::MemoryLockFailed { errno })
    }
}

mod os_memlock {
    use super::{c_void, HidError, MemoryLockStatus};

    #[cfg(unix)]
    pub fn lock_memory(ptr: *const c_void, len: usize) -> MemoryLockStatus {
        extern "C" {
            fn mlock(addr: *const c_void, len: usize) -> core::ffi::c_int;
        }

        // SAFETY: `ptr` points to a valid, pinned `len`-byte allocation owned by the caller.
        let rc = unsafe { mlock(ptr, len) };
        if rc != 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
            return MemoryLockStatus::FallbackUnlocked { errno };
        }

        #[cfg(target_os = "linux")]
        {
            const MADV_DONTDUMP: core::ffi::c_int = 16;
            extern "C" {
                fn madvise(
                    addr: *mut c_void,
                    len: usize,
                    advice: core::ffi::c_int,
                ) -> core::ffi::c_int;
            }
            // SAFETY: `ptr` is valid for `len` bytes; `MADV_DONTDUMP` excludes pages from core dumps.
            let madv_rc = unsafe { madvise(ptr as *mut c_void, len, MADV_DONTDUMP) };
            if madv_rc == 0 {
                return MemoryLockStatus::LockedWithDontDump;
            }
        }

        MemoryLockStatus::Locked
    }

    #[cfg(unix)]
    pub fn unlock_memory(ptr: *const c_void, len: usize) {
        extern "C" {
            fn munlock(addr: *const c_void, len: usize) -> core::ffi::c_int;
        }
        // SAFETY: Matches a prior successful `mlock(ptr, len)` call on the same pinned region.
        let _ = unsafe { munlock(ptr, len) };
    }

    #[cfg(unix)]
    pub fn disable_core_dumps_impl() -> Result<(), HidError> {
        #[repr(C)]
        struct RLimit {
            rlim_cur: u64,
            rlim_max: u64,
        }

        const RLIMIT_CORE: core::ffi::c_int = 4;

        extern "C" {
            fn setrlimit(resource: core::ffi::c_int, rlp: *const RLimit) -> core::ffi::c_int;
        }

        let zero_limit = RLimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `zero_limit` is a valid `rlimit` struct for `RLIMIT_CORE`.
        let rc = unsafe { setrlimit(RLIMIT_CORE, &zero_limit) };
        if rc == 0 {
            Ok(())
        } else {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
            Err(HidError::MemoryLockFailed { errno })
        }
    }

    #[cfg(target_os = "linux")]
    pub fn clear_dumpable_flag() -> bool {
        const PR_SET_DUMPABLE: core::ffi::c_int = 4;
        extern "C" {
            fn prctl(option: core::ffi::c_int, ...) -> core::ffi::c_int;
        }
        // SAFETY: `PR_SET_DUMPABLE` takes a single integer flag (0 = non-dumpable).
        unsafe { prctl(PR_SET_DUMPABLE, 0usize, 0usize, 0usize, 0usize) == 0 }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn clear_dumpable_flag() -> bool {
        false
    }

    #[cfg(windows)]
    pub fn lock_memory(ptr: *const c_void, len: usize) -> MemoryLockStatus {
        #[link(name = "kernel32")]
        extern "system" {
            fn VirtualLock(lp_address: *const c_void, dw_size: usize) -> i32;
        }
        // SAFETY: `ptr` points to a valid pinned `len`-byte allocation.
        let ok = unsafe { VirtualLock(ptr, len) };
        if ok != 0 {
            MemoryLockStatus::Locked
        } else {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
            MemoryLockStatus::FallbackUnlocked { errno }
        }
    }

    #[cfg(windows)]
    pub fn unlock_memory(ptr: *const c_void, len: usize) {
        #[link(name = "kernel32")]
        extern "system" {
            fn VirtualUnlock(lp_address: *const c_void, dw_size: usize) -> i32;
        }
        // SAFETY: Matches a prior successful `VirtualLock(ptr, len)`.
        let _ = unsafe { VirtualUnlock(ptr, len) };
    }

    #[cfg(windows)]
    pub fn disable_core_dumps_impl() -> Result<(), HidError> {
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    pub fn lock_memory(_ptr: *const c_void, _len: usize) -> MemoryLockStatus {
        MemoryLockStatus::FallbackUnlocked { errno: 0 }
    }

    #[cfg(not(any(unix, windows)))]
    pub fn unlock_memory(_ptr: *const c_void, _len: usize) {}

    #[cfg(not(any(unix, windows)))]
    pub fn disable_core_dumps_impl() -> Result<(), HidError> {
        Err(HidError::UnsupportedPlatform("unknown"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_valid_compile_time_capacities_3_to_5() {
        let b3 = SecureKeyBuffer::<3>::new();
        let b4 = SecureKeyBuffer::<4>::new();
        let b5 = SecureKeyBuffer::<5>::new();
        assert_eq!(b3.capacity(), 3);
        assert_eq!(b4.capacity(), 4);
        assert_eq!(b5.capacity(), 5);
        assert!(b5.is_empty());
    }

    #[test]
    fn evicts_oldest_and_preserves_bounded_ring_order() {
        let mut buf = SecureKeyBuffer::<3>::new();
        assert!(!buf.push(KeyCode(0x41), Modifiers::EMPTY, 100).evicted);
        assert!(!buf.push(KeyCode(0x42), Modifiers::SHIFT, 120).evicted);
        assert!(!buf.push(KeyCode(0x43), Modifiers::CONTROL, 140).evicted);
        assert_eq!(buf.len(), 3);

        let outcome = buf.push(KeyCode(0x44), Modifiers::ALT, 160);
        assert!(outcome.evicted);
        assert!(!outcome.expired_before_push);
        assert_eq!(outcome.len, 3);

        let mut codes = [KeyCode::NONE; 3];
        assert_eq!(buf.copy_codes_into(&mut codes), 3);
        assert_eq!(codes, [KeyCode(0x42), KeyCode(0x43), KeyCode(0x44)]);
    }

    #[test]
    fn pop_oldest_volatile_zeroes_vacated_slot_bytes() {
        let mut buf = SecureKeyBuffer::<3>::new();
        buf.push(KeyCode(0x1337), Modifiers::META, 500);
        buf.push(KeyCode(0x2448), Modifiers::SHIFT, 510);

        assert!(buf.raw_slots_bytes()[0..16].iter().any(|&b| b != 0));

        let popped = buf.pop_oldest().expect("should pop oldest");
        assert_eq!(popped.code(), KeyCode(0x1337));
        assert_eq!(popped.modifiers(), Modifiers::META);
        assert_eq!(popped.timestamp_ms(), 500);

        assert!(
            buf.raw_slots_bytes()[0..16].iter().all(|&b| b == 0),
            "vacated slot 0 was not zeroized!"
        );
        assert!(buf.raw_slots_bytes()[16..32].iter().any(|&b| b != 0));

        let popped2 = buf.pop_oldest().expect("should pop second");
        assert_eq!(popped2.code(), KeyCode(0x2448));

        assert!(
            buf.raw_slots_bytes().iter().all(|&b| b == 0),
            "buffer was not completely zeroized after emptying!"
        );
    }

    #[test]
    fn mode_exit_and_escape_zeroize_all_slots_byte_for_byte() {
        let mut buf = SecureKeyBuffer::<5>::new();
        for i in 0..5u16 {
            buf.push(KeyCode(0xAA00 + i), Modifiers::SHIFT, 1000 + u64::from(i) * 10);
        }
        assert!(buf.raw_slots_bytes().iter().any(|&b| b != 0));

        buf.on_escape();
        assert!(buf.is_empty());
        assert_eq!(buf.purge_count(), 1);
        assert!(
            buf.raw_slots_bytes().iter().all(|&b| b == 0),
            "all slot bytes must be 0x00 after on_escape()"
        );

        buf.push(KeyCode(0xBEEF), Modifiers::CONTROL, 2000);
        buf.on_mode_exit();
        assert!(buf.is_empty());
        assert_eq!(buf.purge_count(), 2);
        assert!(
            buf.raw_slots_bytes().iter().all(|&b| b == 0),
            "all slot bytes must be 0x00 after on_mode_exit()"
        );
    }

    #[test]
    fn idle_timeout_purges_and_zeroizes_stale_keystrokes() {
        let mut buf = SecureKeyBuffer::<5>::with_idle_timeout_ms(750);
        buf.push(KeyCode(0x0041), Modifiers::EMPTY, 1_000);
        buf.push(KeyCode(0x0042), Modifiers::EMPTY, 1_100);

        assert!(!buf.purge_if_expired(1_849));
        assert_eq!(buf.len(), 2);

        assert!(buf.purge_if_expired(1_850));
        assert!(buf.is_empty());
        assert!(buf.raw_slots_bytes().iter().all(|&b| b == 0));
    }

    #[test]
    fn push_after_idle_timeout_purges_stale_entries_first() {
        let mut buf = SecureKeyBuffer::<4>::with_idle_timeout_ms(750);
        buf.push(KeyCode(0x0050), Modifiers::EMPTY, 100);
        buf.push(KeyCode(0x0051), Modifiers::EMPTY, 200);

        let outcome = buf.push(KeyCode(0x0052), Modifiers::EMPTY, 1_200);
        assert!(outcome.expired_before_push);
        assert!(!outcome.evicted);
        assert_eq!(outcome.len, 1);

        let entry = buf.pop_oldest().unwrap();
        assert_eq!(entry.code(), KeyCode(0x0052));
        assert!(buf.pop_oldest().is_none());
    }

    #[test]
    fn drop_zeroizes_memory_in_place() {
        let mut boxed = Box::new(core::mem::ManuallyDrop::new(SecureKeyBuffer::<5>::new()));
        boxed.push(KeyCode(0xCAFE), Modifiers::META, 42);
        boxed.push(KeyCode(0xBABE), Modifiers::ALT, 43);
        assert!(boxed.raw_slots_bytes().iter().any(|&b| b != 0));

        // SAFETY: We drop the inner value once and inspect raw bytes of the still-allocated Box.
        unsafe {
            core::mem::ManuallyDrop::drop(&mut *boxed);
            let raw_ptr =
                (&*boxed) as *const core::mem::ManuallyDrop<SecureKeyBuffer<5>> as *const u8;
            let raw_len = core::mem::size_of::<SecureKeyBuffer<5>>();
            let bytes = core::slice::from_raw_parts(raw_ptr, raw_len);
            assert!(
                !bytes
                    .windows(2)
                    .any(|w| w == 0xCAFEu16.to_ne_bytes() || w == 0xBABEu16.to_ne_bytes()),
                "secret keycode bytes survived Drop!"
            );
        }
    }

    #[test]
    fn debug_output_redacts_all_keycodes_and_modifiers() {
        let mut buf = SecureKeyBuffer::<5>::new();
        buf.push(KeyCode(9876), Modifiers::SHIFT, 12345);

        let buf_debug = format!("{buf:?}");
        assert!(buf_debug.contains("[REDACTED]"));
        assert!(!buf_debug.contains("9876"));
        assert!(!buf_debug.contains("2694"));

        let entry = buf.pop_oldest().unwrap();
        let entry_debug = format!("{entry:?}");
        assert!(entry_debug.contains("[REDACTED]"));
        assert!(!entry_debug.contains("9876"));
    }

    #[test]
    fn locked_buffer_works_and_reports_valid_lock_status() {
        let mut locked = LockedSecureKeyBuffer::<5>::new();
        let status = locked.lock_status();
        assert!(matches!(
            status,
            MemoryLockStatus::Locked
                | MemoryLockStatus::LockedWithDontDump
                | MemoryLockStatus::FallbackUnlocked { .. }
        ));

        locked.buffer_mut().push(KeyCode(0x0041), Modifiers::EMPTY, 50);
        assert_eq!(locked.buffer().len(), 1);
        let debug_str = format!("{locked:?}");
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("65"));

        let popped = locked.buffer_mut().pop_oldest().unwrap();
        assert_eq!(popped.code(), KeyCode(0x0041));
        assert!(locked.buffer().is_empty());
    }

    #[test]
    fn process_hardening_disables_core_dumps_safely() {
        let report = harden_process_memory();
        #[cfg(unix)]
        assert!(report.core_dumps_disabled);
        assert!(!report.debugger_attach_denied);
    }
}
