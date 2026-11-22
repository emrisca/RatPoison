//! Zero-leak Hold-For and Cycle command interceptor state machine.
//!
//! # Key-Leakage & Stuck-Modifier Invariants
//! 1. **Swallowed-key symmetry**: Every key whose `KeyDown` is swallowed by RatPoison is
//!    recorded in an internal fixed-capacity, zeroized `SwallowedKeySet`. All subsequent
//!    `KeyRepeat` and `KeyUp` events for that key are swallowed — *even if RatPoison
//!    deactivated before the user physically released the key*.
//! 2. **No stuck foreground modifiers**: If a key (such as `Ctrl`, `Alt`, `Shift`, or `Meta`)
//!    went `KeyDown` *before* RatPoison activated (meaning the foreground app saw it), its
//!    `KeyUp` is **not** in `SwallowedKeySet` and therefore passes through to the foreground
//!    app cleanly.
//! 3. **No repeat amplification or leaks**: OS auto-repeat (`KeyPhase::Repeat`) on Hold-For
//!    or Cycle keys is always swallowed silently, preventing accidental multi-cycling or
//!    text leakage into the focused window.
//! 4. **Bounded, zeroized key buffer**: Grid/navigation keys typed while active are pushed
//!    exclusively into an owned [`SecureKeyBuffer<5>`] and purged immediately on `Escape`,
//!    action execution, mode exit, idle timeout (default 750ms), or `Drop`.

use core::fmt;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::memory_hygiene::{SecureKeyBuffer, DEFAULT_BUFFER_CAPACITY, DEFAULT_IDLE_PURGE_MS};
use crate::types::{ClickKind, KeyCode, KeyPhase, Modifiers, MouseButton, RawKeyEvent, SpeedTier};

/// Maximum number of simultaneously swallowed physical keys tracked without heap allocation.
const MAX_SWALLOWED_KEYS: usize = 16;

/// Maximum nested Hold-For overrides active simultaneously.
const MAX_ACTIVE_HOLDS: usize = 8;

/// Redacted wrapper around a [`KeyCode`] for in-memory grid decoding without accidental logging.
#[derive(Clone, Copy, PartialEq, Eq, Zeroize)]
pub struct RedactedKey(KeyCode);

impl RedactedKey {
    #[inline]
    pub const fn code(self) -> KeyCode {
        self.0
    }
}

impl fmt::Debug for RedactedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactedKey([REDACTED])")
    }
}

/// Operating mode of the [`CommandInterceptor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterceptorMode {
    #[default]
    Inactive,
    Active,
}

/// Activation hotkey chord used to enter RatPoison overlay mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationChord {
    pub key: KeyCode,
    pub modifiers: Modifiers,
}

impl Default for ActivationChord {
    fn default() -> Self {
        Self {
            key: KeyCode::SPACE,
            modifiers: Modifiers::ALT,
        }
    }
}

/// Effect applied while a **Hold-For** key is physically held down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoldForTarget {
    /// Temporarily override the click type (e.g. right-click, middle-click, double-click).
    ClickOverride(ClickKind),
    /// Hold to perform a mouse drag (`mouse_down` on press, `mouse_up` on release).
    Drag(MouseButton),
    /// Temporarily override Free Mode cursor velocity tier.
    SpeedBoost(SpeedTier),
}

/// Configuration binding for a Hold-For modifier key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldForBinding {
    pub key: KeyCode,
    pub target: HoldForTarget,
}

/// Sequence advanced on each tap of a **Cycle** key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CycleTarget {
    ClickSequence(Vec<ClickKind>),
    SpeedSequence(Vec<SpeedTier>),
}

/// Configuration binding for a Cycle command key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleBinding {
    pub key: KeyCode,
    pub target: CycleTarget,
}

/// Serializable configuration for [`CommandInterceptor`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterceptorConfig {
    pub activation_chord: ActivationChord,
    pub execute_key: Option<KeyCode>,
    pub hold_for_bindings: Vec<HoldForBinding>,
    pub cycle_bindings: Vec<CycleBinding>,
    pub idle_purge_ms: u64,
    pub deactivate_on_execute: bool,
    pub reset_cycle_on_execute: bool,
}

impl Default for InterceptorConfig {
    fn default() -> Self {
        Self {
            activation_chord: ActivationChord::default(),
            execute_key: Some(KeyCode::RETURN),
            hold_for_bindings: vec![
                HoldForBinding {
                    key: KeyCode::from_ascii_upper(b'R'),
                    target: HoldForTarget::ClickOverride(ClickKind::Right),
                },
                HoldForBinding {
                    key: KeyCode::from_ascii_upper(b'M'),
                    target: HoldForTarget::ClickOverride(ClickKind::Middle),
                },
                HoldForBinding {
                    key: KeyCode::from_ascii_upper(b'G'),
                    target: HoldForTarget::Drag(MouseButton::Left),
                },
                HoldForBinding {
                    key: KeyCode::SHIFT_L,
                    target: HoldForTarget::SpeedBoost(SpeedTier::Fast),
                },
            ],
            cycle_bindings: vec![
                CycleBinding {
                    key: KeyCode::from_ascii_upper(b'C'),
                    target: CycleTarget::ClickSequence(ClickKind::DEFAULT_CYCLE.to_vec()),
                },
                CycleBinding {
                    key: KeyCode::from_ascii_upper(b'V'),
                    target: CycleTarget::SpeedSequence(SpeedTier::DEFAULT_CYCLE.to_vec()),
                },
            ],
            idle_purge_ms: DEFAULT_IDLE_PURGE_MS,
            deactivate_on_execute: true,
            reset_cycle_on_execute: true,
        }
    }
}

/// Semantic action emitted when an intercepted key triggers RatPoison behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterceptorAction {
    /// Overlay mode was entered via the activation hotkey chord.
    ActivateOverlay,
    /// Overlay mode was exited (e.g. via Escape); key buffer has been purged.
    DeactivateOverlay {
        purge_buffer: bool,
        cancel_drag: Option<MouseButton>,
    },
    /// A Hold-For key temporarily overrode the active click type.
    SetClickMode(ClickKind),
    /// A Hold-For click override was released; reverted to the effective underlying click type.
    RevertClickMode(ClickKind),
    /// A Hold-For drag key was pressed; caller should press `mouse_down(button)`.
    BeginDrag(MouseButton),
    /// A Hold-For drag key was released; caller should press `mouse_up(button)`.
    EndDrag(MouseButton),
    /// A Hold-For speed boost key temporarily overrode the Free Mode speed tier.
    SetSpeedTier(SpeedTier),
    /// A Hold-For speed boost key was released; reverted to the underlying speed tier.
    RevertSpeedTier(SpeedTier),
    /// A Cycle key advanced the click type.
    CycleClickMode {
        new_mode: ClickKind,
        step_index: usize,
    },
    /// A Cycle key advanced the Free Mode speed tier.
    CycleSpeedTier {
        new_tier: SpeedTier,
        step_index: usize,
    },
    /// Execute the effective mouse click action.
    ExecuteClick {
        kind: ClickKind,
        /// Modifiers currently held in the foreground app (for clean click synthesis).
        foreground_held_modifiers: Modifiers,
        /// True if the interceptor returned to `Inactive` after execution.
        deactivated: bool,
    },
    /// A grid/navigation key was accepted into the zeroized [`SecureKeyBuffer`].
    GridKeyInput {
        key: RedactedKey,
        buffer_len: usize,
    },
    /// Synthesize a balanced key-up if emergency modifier recovery is required.
    SynthesizeKeyUp(KeyCode),
}

/// Decision returned to the OS keyboard hook backend for every incoming [`RawKeyEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterceptDecision {
    /// Allow the event to continue untouched to the foreground application.
    PassThrough,
    /// Consume the event silently; foreground application never observes it.
    Swallow,
    /// Consume the event and execute the emitted [`InterceptorAction`].
    SwallowAndEmit(InterceptorAction),
}

/// Fixed-capacity stack set tracking swallowed keycodes with explicit volatile zeroization.
struct SwallowedKeySet {
    slots: [KeyCode; MAX_SWALLOWED_KEYS],
    len: usize,
}

impl SwallowedKeySet {
    const fn new() -> Self {
        Self {
            slots: [KeyCode::NONE; MAX_SWALLOWED_KEYS],
            len: 0,
        }
    }

    fn contains(&self, code: KeyCode) -> bool {
        self.slots[..self.len].contains(&code)
    }

    fn insert(&mut self, code: KeyCode) {
        if self.contains(code) {
            return;
        }
        if self.len < MAX_SWALLOWED_KEYS {
            self.slots[self.len] = code;
            self.len += 1;
        } else {
            self.slots[0].zeroize();
            for i in 1..MAX_SWALLOWED_KEYS {
                self.slots[i - 1] = self.slots[i];
            }
            self.slots[MAX_SWALLOWED_KEYS - 1] = code;
        }
    }

    fn remove(&mut self, code: KeyCode) -> bool {
        if let Some(pos) = self.slots[..self.len].iter().position(|&k| k == code) {
            for i in (pos + 1)..self.len {
                self.slots[i - 1] = self.slots[i];
            }
            self.len -= 1;
            self.slots[self.len].zeroize();
            true
        } else {
            false
        }
    }

    #[inline]
    fn len(&self) -> usize {
        self.len
    }
}

impl Zeroize for SwallowedKeySet {
    fn zeroize(&mut self) {
        for slot in &mut self.slots {
            slot.zeroize();
        }
        self.len.zeroize();
    }
}

impl Drop for SwallowedKeySet {
    fn drop(&mut self) {
        self.zeroize();
    }
}

#[derive(Clone, Copy)]
struct ActiveHoldEntry {
    key: KeyCode,
    target: HoldForTarget,
}

/// Zero-leak Hold-For and Cycle command interceptor coupled with [`SecureKeyBuffer`].
pub struct CommandInterceptor<const N: usize = DEFAULT_BUFFER_CAPACITY> {
    config: InterceptorConfig,
    mode: InterceptorMode,
    key_buffer: SecureKeyBuffer<N>,
    swallowed_keys: SwallowedKeySet,
    foreground_held_keys: SwallowedKeySet,
    active_holds: [Option<ActiveHoldEntry>; MAX_ACTIVE_HOLDS],
    active_holds_len: usize,
    active_drag: Option<MouseButton>,
    click_cycle_idx: usize,
    speed_cycle_idx: usize,
    cycled_click_mode: ClickKind,
    cycled_speed_tier: SpeedTier,
}

impl CommandInterceptor<DEFAULT_BUFFER_CAPACITY> {
    /// Creates a new interceptor with the default 5-slot [`SecureKeyBuffer`] and default bindings.
    pub fn new(config: InterceptorConfig) -> Self {
        Self::with_capacity(config)
    }
}

impl<const N: usize> CommandInterceptor<N> {
    /// Creates a new interceptor with compile-time validated buffer capacity `N` (`3 <= N <= 5`).
    pub fn with_capacity(config: InterceptorConfig) -> Self {
        let idle_purge_ms = config.idle_purge_ms;
        let initial_click = config
            .cycle_bindings
            .iter()
            .find_map(|b| match &b.target {
                CycleTarget::ClickSequence(seq) => seq.first().copied(),
                _ => None,
            })
            .unwrap_or(ClickKind::Left);
        let initial_speed = config
            .cycle_bindings
            .iter()
            .find_map(|b| match &b.target {
                CycleTarget::SpeedSequence(seq) => seq.first().copied(),
                _ => None,
            })
            .unwrap_or(SpeedTier::Normal);

        Self {
            config,
            mode: InterceptorMode::Inactive,
            key_buffer: SecureKeyBuffer::<N>::with_idle_timeout_ms(idle_purge_ms),
            swallowed_keys: SwallowedKeySet::new(),
            foreground_held_keys: SwallowedKeySet::new(),
            active_holds: [None; MAX_ACTIVE_HOLDS],
            active_holds_len: 0,
            active_drag: None,
            click_cycle_idx: 0,
            speed_cycle_idx: 0,
            cycled_click_mode: initial_click,
            cycled_speed_tier: initial_speed,
        }
    }

    /// Returns the current [`InterceptorMode`].
    #[inline]
    pub const fn mode(&self) -> InterceptorMode {
        self.mode
    }

    /// Programmatically activates overlay mode (e.g. from a tray menu or UI trigger).
    pub fn activate(&mut self) {
        self.key_buffer.purge();
        self.mode = InterceptorMode::Active;
    }

    /// Programmatically deactivates overlay mode and purges the key buffer.
    pub fn deactivate(&mut self) -> Option<MouseButton> {
        self.key_buffer.on_mode_exit();
        self.mode = InterceptorMode::Inactive;
        self.clear_non_drag_holds();
        self.reset_cycles();
        self.active_drag.take()
    }

    /// Returns the currently effective [`ClickKind`] (active Hold-For override wins over Cycle).
    pub fn effective_click_mode(&self) -> ClickKind {
        for idx in (0..self.active_holds_len).rev() {
            if let Some(entry) = self.active_holds[idx] {
                if let HoldForTarget::ClickOverride(kind) = entry.target {
                    return kind;
                }
            }
        }
        self.cycled_click_mode
    }

    /// Returns the currently effective [`SpeedTier`] (active Hold-For boost wins over Cycle).
    pub fn effective_speed_tier(&self) -> SpeedTier {
        for idx in (0..self.active_holds_len).rev() {
            if let Some(entry) = self.active_holds[idx] {
                if let HoldForTarget::SpeedBoost(tier) = entry.target {
                    return tier;
                }
            }
        }
        self.cycled_speed_tier
    }

    /// Returns the modifier bitmask currently physically held by the foreground application.
    pub fn foreground_held_modifiers(&self) -> Modifiers {
        let mut mods = Modifiers::EMPTY;
        for i in 0..self.foreground_held_keys.len() {
            mods.insert(self.foreground_held_keys.slots[i].as_modifier());
        }
        mods
    }

    /// Number of physical keys currently tracked as swallowed awaiting `KeyUp`.
    #[inline]
    pub fn swallowed_key_count(&self) -> usize {
        self.swallowed_keys.len()
    }

    /// Shared access to the owned [`SecureKeyBuffer`].
    #[inline]
    pub fn key_buffer(&self) -> &SecureKeyBuffer<N> {
        &self.key_buffer
    }

    /// Periodic tick hook to enforce idle keystroke buffer expiration (default 750ms).
    pub fn tick(&mut self, now_ms: u64) -> bool {
        self.key_buffer.purge_if_expired(now_ms)
    }

    /// Processes an incoming [`RawKeyEvent`] and returns a leak-free [`InterceptDecision`].
    pub fn handle_event(&mut self, event: RawKeyEvent) -> InterceptDecision {
        if event.is_injected {
            return InterceptDecision::PassThrough;
        }

        self.key_buffer.purge_if_expired(event.timestamp_ms);

        match self.mode {
            InterceptorMode::Inactive => self.handle_inactive(event),
            InterceptorMode::Active => self.handle_active(event),
        }
    }

    fn handle_inactive(&mut self, event: RawKeyEvent) -> InterceptDecision {
        if self.swallowed_keys.contains(event.code) {
            match event.phase {
                KeyPhase::Repeat => return InterceptDecision::Swallow,
                KeyPhase::Up => {
                    self.swallowed_keys.remove(event.code);
                    if let Some(HoldForTarget::Drag(btn)) = self.remove_active_hold(event.code) {
                        if self.active_drag == Some(btn) {
                            self.active_drag = None;
                            return InterceptDecision::SwallowAndEmit(InterceptorAction::EndDrag(
                                btn,
                            ));
                        }
                    }
                    return InterceptDecision::Swallow;
                }
                KeyPhase::Down => {
                    self.swallowed_keys.remove(event.code);
                }
            }
        }

        if event.phase == KeyPhase::Down
            && event.code == self.config.activation_chord.key
            && event
                .modifiers
                .contains(self.config.activation_chord.modifiers)
        {
            self.swallowed_keys.insert(event.code);
            self.key_buffer.purge();
            self.mode = InterceptorMode::Active;
            return InterceptDecision::SwallowAndEmit(InterceptorAction::ActivateOverlay);
        }

        match event.phase {
            KeyPhase::Down => {
                self.foreground_held_keys.insert(event.code);
            }
            KeyPhase::Up => {
                self.foreground_held_keys.remove(event.code);
            }
            KeyPhase::Repeat => {}
        }

        InterceptDecision::PassThrough
    }

    fn handle_active(&mut self, event: RawKeyEvent) -> InterceptDecision {
        match event.phase {
            KeyPhase::Up => self.handle_active_key_up(event),
            KeyPhase::Repeat => self.handle_active_key_repeat(event),
            KeyPhase::Down => self.handle_active_key_down(event),
        }
    }

    fn handle_active_key_up(&mut self, event: RawKeyEvent) -> InterceptDecision {
        if !self.swallowed_keys.remove(event.code) {
            self.foreground_held_keys.remove(event.code);
            return InterceptDecision::PassThrough;
        }

        if let Some(released_target) = self.remove_active_hold(event.code) {
            return match released_target {
                HoldForTarget::ClickOverride(_) => InterceptDecision::SwallowAndEmit(
                    InterceptorAction::RevertClickMode(self.effective_click_mode()),
                ),
                HoldForTarget::Drag(btn) => {
                    if self.active_drag == Some(btn) {
                        self.active_drag = None;
                        InterceptDecision::SwallowAndEmit(InterceptorAction::EndDrag(btn))
                    } else {
                        InterceptDecision::Swallow
                    }
                }
                HoldForTarget::SpeedBoost(_) => InterceptDecision::SwallowAndEmit(
                    InterceptorAction::RevertSpeedTier(self.effective_speed_tier()),
                ),
            };
        }

        InterceptDecision::Swallow
    }

    fn handle_active_key_repeat(&mut self, event: RawKeyEvent) -> InterceptDecision {
        if self.swallowed_keys.contains(event.code) {
            return InterceptDecision::Swallow;
        }
        if event.code.as_modifier().is_empty() {
            InterceptDecision::Swallow
        } else {
            InterceptDecision::PassThrough
        }
    }

    fn handle_active_key_down(&mut self, event: RawKeyEvent) -> InterceptDecision {
        if event.code == KeyCode::ESCAPE {
            self.swallowed_keys.insert(event.code);
            self.key_buffer.on_escape();
            let cancel_drag = self.active_drag.take();
            self.clear_all_holds();
            self.reset_cycles();
            self.mode = InterceptorMode::Inactive;
            return InterceptDecision::SwallowAndEmit(InterceptorAction::DeactivateOverlay {
                purge_buffer: true,
                cancel_drag,
            });
        }

        if let Some(target) = self.find_hold_for_target(event.code) {
            self.swallowed_keys.insert(event.code);
            self.push_active_hold(event.code, target);
            return match target {
                HoldForTarget::ClickOverride(kind) => {
                    InterceptDecision::SwallowAndEmit(InterceptorAction::SetClickMode(kind))
                }
                HoldForTarget::Drag(btn) => {
                    self.active_drag = Some(btn);
                    InterceptDecision::SwallowAndEmit(InterceptorAction::BeginDrag(btn))
                }
                HoldForTarget::SpeedBoost(tier) => {
                    InterceptDecision::SwallowAndEmit(InterceptorAction::SetSpeedTier(tier))
                }
            };
        }

        if let Some(action) = self.try_advance_cycle(event.code) {
            self.swallowed_keys.insert(event.code);
            return InterceptDecision::SwallowAndEmit(action);
        }

        if self.config.execute_key == Some(event.code) {
            self.swallowed_keys.insert(event.code);
            let kind = self.effective_click_mode();
            let foreground_held_modifiers = self.foreground_held_modifiers();
            self.key_buffer.on_mode_exit();

            let deactivated = if self.config.deactivate_on_execute {
                self.mode = InterceptorMode::Inactive;
                self.clear_non_drag_holds();
                if self.config.reset_cycle_on_execute {
                    self.reset_cycles();
                }
                true
            } else {
                if self.config.reset_cycle_on_execute {
                    self.reset_cycles();
                }
                false
            };

            return InterceptDecision::SwallowAndEmit(InterceptorAction::ExecuteClick {
                kind,
                foreground_held_modifiers,
                deactivated,
            });
        }

        self.swallowed_keys.insert(event.code);
        let outcome = self
            .key_buffer
            .push(event.code, event.modifiers, event.timestamp_ms);

        InterceptDecision::SwallowAndEmit(InterceptorAction::GridKeyInput {
            key: RedactedKey(event.code),
            buffer_len: outcome.len,
        })
    }

    fn find_hold_for_target(&self, key: KeyCode) -> Option<HoldForTarget> {
        self.config
            .hold_for_bindings
            .iter()
            .find(|b| b.key == key)
            .map(|b| b.target)
    }

    fn push_active_hold(&mut self, key: KeyCode, target: HoldForTarget) {
        for idx in 0..self.active_holds_len {
            if let Some(entry) = &mut self.active_holds[idx] {
                if entry.key == key {
                    entry.target = target;
                    return;
                }
            }
        }
        if self.active_holds_len < MAX_ACTIVE_HOLDS {
            self.active_holds[self.active_holds_len] = Some(ActiveHoldEntry { key, target });
            self.active_holds_len += 1;
        }
    }

    fn remove_active_hold(&mut self, key: KeyCode) -> Option<HoldForTarget> {
        for idx in 0..self.active_holds_len {
            if let Some(entry) = self.active_holds[idx] {
                if entry.key == key {
                    for shift in (idx + 1)..self.active_holds_len {
                        self.active_holds[shift - 1] = self.active_holds[shift];
                    }
                    self.active_holds_len -= 1;
                    self.active_holds[self.active_holds_len] = None;
                    return Some(entry.target);
                }
            }
        }
        None
    }

    fn clear_all_holds(&mut self) {
        self.active_holds.fill(None);
        self.active_holds_len = 0;
    }

    fn clear_non_drag_holds(&mut self) {
        let mut write = 0;
        for read in 0..self.active_holds_len {
            if let Some(entry) = self.active_holds[read] {
                if matches!(entry.target, HoldForTarget::Drag(_)) {
                    self.active_holds[write] = Some(entry);
                    write += 1;
                }
            }
        }
        self.active_holds[write..].fill(None);
        self.active_holds_len = write;
    }

    fn try_advance_cycle(&mut self, key: KeyCode) -> Option<InterceptorAction> {
        for binding in &self.config.cycle_bindings {
            if binding.key != key {
                continue;
            }
            match &binding.target {
                CycleTarget::ClickSequence(seq) if !seq.is_empty() => {
                    self.click_cycle_idx = (self.click_cycle_idx + 1) % seq.len();
                    let new_mode = seq[self.click_cycle_idx];
                    self.cycled_click_mode = new_mode;
                    return Some(InterceptorAction::CycleClickMode {
                        new_mode,
                        step_index: self.click_cycle_idx,
                    });
                }
                CycleTarget::SpeedSequence(seq) if !seq.is_empty() => {
                    self.speed_cycle_idx = (self.speed_cycle_idx + 1) % seq.len();
                    let new_tier = seq[self.speed_cycle_idx];
                    self.cycled_speed_tier = new_tier;
                    return Some(InterceptorAction::CycleSpeedTier {
                        new_tier,
                        step_index: self.speed_cycle_idx,
                    });
                }
                _ => {}
            }
        }
        None
    }

    fn reset_cycles(&mut self) {
        self.click_cycle_idx = 0;
        self.speed_cycle_idx = 0;
        self.cycled_click_mode = self
            .config
            .cycle_bindings
            .iter()
            .find_map(|b| match &b.target {
                CycleTarget::ClickSequence(seq) => seq.first().copied(),
                _ => None,
            })
            .unwrap_or(ClickKind::Left);
        self.cycled_speed_tier = self
            .config
            .cycle_bindings
            .iter()
            .find_map(|b| match &b.target {
                CycleTarget::SpeedSequence(seq) => seq.first().copied(),
                _ => None,
            })
            .unwrap_or(SpeedTier::Normal);
    }
}

impl<const N: usize> fmt::Debug for CommandInterceptor<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandInterceptor")
            .field("mode", &self.mode)
            .field("effective_click_mode", &self.effective_click_mode())
            .field("effective_speed_tier", &self.effective_speed_tier())
            .field("swallowed_key_count", &self.swallowed_keys.len())
            .field("key_buffer", &self.key_buffer)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_ev(code: KeyCode, mods: Modifiers, phase: KeyPhase, ts: u64) -> RawKeyEvent {
        RawKeyEvent::new(code, mods, phase, ts)
    }

    #[test]
    fn hold_for_swallows_down_repeat_up_with_zero_leakage() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();

        let key_r = KeyCode::from_ascii_upper(b'R');

        assert_eq!(
            interceptor.handle_event(key_ev(key_r, Modifiers::EMPTY, KeyPhase::Down, 100)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::SetClickMode(ClickKind::Right))
        );
        assert_eq!(interceptor.effective_click_mode(), ClickKind::Right);

        for t in [150, 200, 250] {
            assert_eq!(
                interceptor.handle_event(key_ev(key_r, Modifiers::EMPTY, KeyPhase::Repeat, t)),
                InterceptDecision::Swallow
            );
        }

        assert_eq!(
            interceptor.handle_event(key_ev(key_r, Modifiers::EMPTY, KeyPhase::Up, 300)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::RevertClickMode(ClickKind::Left))
        );
        assert_eq!(interceptor.effective_click_mode(), ClickKind::Left);
        assert_eq!(interceptor.swallowed_key_count(), 0);
    }

    #[test]
    fn hold_for_speed_boost_overrides_cycled_speed_and_reverts_cleanly() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();

        let key_v = KeyCode::from_ascii_upper(b'V');
        // Cycle speed: Normal -> Fast -> Turbo -> Slow
        assert_eq!(
            interceptor.handle_event(key_ev(key_v, Modifiers::EMPTY, KeyPhase::Down, 10)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::CycleSpeedTier {
                new_tier: SpeedTier::Fast,
                step_index: 1,
            })
        );
        assert_eq!(
            interceptor.handle_event(key_ev(key_v, Modifiers::EMPTY, KeyPhase::Up, 20)),
            InterceptDecision::Swallow
        );
        interceptor.handle_event(key_ev(key_v, Modifiers::EMPTY, KeyPhase::Down, 30));
        interceptor.handle_event(key_ev(key_v, Modifiers::EMPTY, KeyPhase::Up, 40));
        interceptor.handle_event(key_ev(key_v, Modifiers::EMPTY, KeyPhase::Down, 50));
        interceptor.handle_event(key_ev(key_v, Modifiers::EMPTY, KeyPhase::Up, 60));
        assert_eq!(interceptor.effective_speed_tier(), SpeedTier::Slow);

        // Hold SHIFT_L -> temporarily boosts to Fast, repeat swallowed, release reverts to Slow
        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::SHIFT_L, Modifiers::SHIFT, KeyPhase::Down, 70)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::SetSpeedTier(SpeedTier::Fast))
        );
        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::SHIFT_L, Modifiers::SHIFT, KeyPhase::Repeat, 80)),
            InterceptDecision::Swallow
        );
        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::SHIFT_L, Modifiers::EMPTY, KeyPhase::Up, 90)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::RevertSpeedTier(SpeedTier::Slow))
        );
        assert_eq!(interceptor.effective_speed_tier(), SpeedTier::Slow);
    }

    #[test]
    fn hold_for_key_released_after_execute_deactivation_still_swallowed() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();

        let key_r = KeyCode::from_ascii_upper(b'R');

        assert_eq!(
            interceptor.handle_event(key_ev(key_r, Modifiers::EMPTY, KeyPhase::Down, 100)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::SetClickMode(ClickKind::Right))
        );

        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::RETURN, Modifiers::EMPTY, KeyPhase::Down, 120)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::ExecuteClick {
                kind: ClickKind::Right,
                foreground_held_modifiers: Modifiers::EMPTY,
                deactivated: true,
            })
        );
        assert_eq!(interceptor.mode(), InterceptorMode::Inactive);

        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::RETURN, Modifiers::EMPTY, KeyPhase::Up, 140)),
            InterceptDecision::Swallow
        );
        assert_eq!(
            interceptor.handle_event(key_ev(key_r, Modifiers::EMPTY, KeyPhase::Repeat, 145)),
            InterceptDecision::Swallow
        );
        assert_eq!(
            interceptor.handle_event(key_ev(key_r, Modifiers::EMPTY, KeyPhase::Up, 150)),
            InterceptDecision::Swallow
        );
        assert_eq!(interceptor.swallowed_key_count(), 0);

        assert_eq!(
            interceptor.handle_event(key_ev(key_r, Modifiers::EMPTY, KeyPhase::Down, 200)),
            InterceptDecision::PassThrough
        );
    }

    #[test]
    fn nested_hold_for_stack_reverts_in_lifo_order() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();

        let key_r = KeyCode::from_ascii_upper(b'R');
        let key_m = KeyCode::from_ascii_upper(b'M');

        interceptor.handle_event(key_ev(key_r, Modifiers::EMPTY, KeyPhase::Down, 10));
        assert_eq!(interceptor.effective_click_mode(), ClickKind::Right);

        interceptor.handle_event(key_ev(key_m, Modifiers::EMPTY, KeyPhase::Down, 20));
        assert_eq!(interceptor.effective_click_mode(), ClickKind::Middle);

        assert_eq!(
            interceptor.handle_event(key_ev(key_m, Modifiers::EMPTY, KeyPhase::Up, 30)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::RevertClickMode(ClickKind::Right))
        );

        assert_eq!(
            interceptor.handle_event(key_ev(key_r, Modifiers::EMPTY, KeyPhase::Up, 40)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::RevertClickMode(ClickKind::Left))
        );
    }

    #[test]
    fn cycle_command_advances_click_modes_and_swallows_repeats() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();

        let key_c = KeyCode::from_ascii_upper(b'C');
        assert_eq!(interceptor.effective_click_mode(), ClickKind::Left);

        let expected = [
            (ClickKind::Right, 1),
            (ClickKind::DoubleLeft, 2),
            (ClickKind::Middle, 3),
            (ClickKind::Left, 0),
        ];

        let mut ts = 100;
        for (mode, step_index) in expected {
            assert_eq!(
                interceptor.handle_event(key_ev(key_c, Modifiers::EMPTY, KeyPhase::Down, ts)),
                InterceptDecision::SwallowAndEmit(InterceptorAction::CycleClickMode {
                    new_mode: mode,
                    step_index,
                })
            );
            assert_eq!(
                interceptor.handle_event(key_ev(key_c, Modifiers::EMPTY, KeyPhase::Repeat, ts + 20)),
                InterceptDecision::Swallow
            );
            assert_eq!(interceptor.effective_click_mode(), mode);
            assert_eq!(
                interceptor.handle_event(key_ev(key_c, Modifiers::EMPTY, KeyPhase::Up, ts + 40)),
                InterceptDecision::Swallow
            );
            ts += 100;
        }
    }

    #[test]
    fn hold_for_overrides_cycled_mode_then_reverts_to_cycled_mode() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();

        let key_c = KeyCode::from_ascii_upper(b'C');
        let key_m = KeyCode::from_ascii_upper(b'M');

        interceptor.handle_event(key_ev(key_c, Modifiers::EMPTY, KeyPhase::Down, 10));
        interceptor.handle_event(key_ev(key_c, Modifiers::EMPTY, KeyPhase::Up, 20));
        assert_eq!(interceptor.effective_click_mode(), ClickKind::Right);

        assert_eq!(
            interceptor.handle_event(key_ev(key_m, Modifiers::EMPTY, KeyPhase::Down, 30)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::SetClickMode(ClickKind::Middle))
        );

        assert_eq!(
            interceptor.handle_event(key_ev(key_m, Modifiers::EMPTY, KeyPhase::Up, 40)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::RevertClickMode(ClickKind::Right))
        );
    }

    #[test]
    fn activation_chord_modifier_held_before_activation_never_sticks() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());

        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::ALT_L, Modifiers::ALT, KeyPhase::Down, 10)),
            InterceptDecision::PassThrough
        );
        assert_eq!(interceptor.foreground_held_modifiers(), Modifiers::ALT);

        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::SPACE, Modifiers::ALT, KeyPhase::Down, 20)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::ActivateOverlay)
        );
        assert_eq!(interceptor.mode(), InterceptorMode::Active);

        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::SPACE, Modifiers::ALT, KeyPhase::Up, 30)),
            InterceptDecision::Swallow
        );

        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::ALT_L, Modifiers::EMPTY, KeyPhase::Up, 40)),
            InterceptDecision::PassThrough
        );
        assert_eq!(interceptor.foreground_held_modifiers(), Modifiers::EMPTY);
    }

    #[test]
    fn escape_purges_secure_key_buffer_and_cancels_active_drag() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();

        let key_a = KeyCode::from_ascii_upper(b'A');
        let key_g = KeyCode::from_ascii_upper(b'G');

        let decision =
            interceptor.handle_event(key_ev(key_a, Modifiers::EMPTY, KeyPhase::Down, 100));
        assert!(matches!(
            decision,
            InterceptDecision::SwallowAndEmit(InterceptorAction::GridKeyInput { buffer_len: 1, .. })
        ));
        assert_eq!(interceptor.key_buffer().len(), 1);

        assert_eq!(
            interceptor.handle_event(key_ev(key_g, Modifiers::EMPTY, KeyPhase::Down, 120)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::BeginDrag(MouseButton::Left))
        );

        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::ESCAPE, Modifiers::EMPTY, KeyPhase::Down, 140)),
            InterceptDecision::SwallowAndEmit(InterceptorAction::DeactivateOverlay {
                purge_buffer: true,
                cancel_drag: Some(MouseButton::Left),
            })
        );
        assert_eq!(interceptor.mode(), InterceptorMode::Inactive);
        assert!(interceptor.key_buffer().is_empty());

        assert_eq!(
            interceptor.handle_event(key_ev(KeyCode::ESCAPE, Modifiers::EMPTY, KeyPhase::Up, 150)),
            InterceptDecision::Swallow
        );
        assert_eq!(
            interceptor.handle_event(key_ev(key_g, Modifiers::EMPTY, KeyPhase::Up, 160)),
            InterceptDecision::Swallow
        );
        assert_eq!(
            interceptor.handle_event(key_ev(key_a, Modifiers::EMPTY, KeyPhase::Up, 170)),
            InterceptDecision::Swallow
        );
        assert_eq!(interceptor.swallowed_key_count(), 0);
    }

    #[test]
    fn injected_events_always_pass_through_to_prevent_loops() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();

        let ev = RawKeyEvent::injected(
            KeyCode::from_ascii_upper(b'A'),
            Modifiers::EMPTY,
            KeyPhase::Down,
            50,
        );
        assert_eq!(interceptor.handle_event(ev), InterceptDecision::PassThrough);
        assert!(interceptor.key_buffer().is_empty());
    }

    #[test]
    fn tick_purges_idle_key_buffer_after_timeout() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();

        interceptor.handle_event(key_ev(
            KeyCode::from_ascii_upper(b'A'),
            Modifiers::EMPTY,
            KeyPhase::Down,
            1_000,
        ));
        assert_eq!(interceptor.key_buffer().len(), 1);
        assert!(!interceptor.tick(1_700));
        assert_eq!(interceptor.key_buffer().len(), 1);
        assert!(interceptor.tick(1_750));
        assert!(interceptor.key_buffer().is_empty());
    }

    #[test]
    fn interceptor_debug_redacts_buffered_keys() {
        let mut interceptor = CommandInterceptor::new(InterceptorConfig::default());
        interceptor.activate();
        let decision = interceptor.handle_event(key_ev(KeyCode(7777), Modifiers::SHIFT, KeyPhase::Down, 10));
        let decision_dbg = format!("{decision:?}");
        let interceptor_dbg = format!("{interceptor:?}");
        assert!(decision_dbg.contains("[REDACTED]"));
        assert!(!decision_dbg.contains("7777"));
        assert!(interceptor_dbg.contains("[REDACTED]"));
        assert!(!interceptor_dbg.contains("7777"));
    }
}
