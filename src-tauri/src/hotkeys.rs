//! Global hotkey parsing and microsecond-fast keyboard routing for RatPoison.
//!
//! Designed so the OS event-tap callback (`CGEventTap` / `WH_KEYBOARD_LL` / `evdev`) can decide
//! `PassThrough` vs `Swallow` in microseconds with zero blocking I/O, while emitting semantic
//! [`HookAction`]s onto a channel consumed by the runtime worker thread.

use ratpoison_core::{Direction, EngineEvent, EngineMode, FreeKey, MonitorSwitch, MouseActionKind, NudgeStep};
use ratpoison_hid::{InterceptDecision, KeyCode, KeyPhase, Modifiers, RawKeyEvent};
use serde_json::Value;
use std::collections::HashSet;

const RELEVANT_MODS: Modifiers = Modifiers::from_bits_truncate(
    Modifiers::SHIFT.bits()
        | Modifiers::CONTROL.bits()
        | Modifiers::ALT.bits()
        | Modifiers::META.bits(),
);

#[inline]
fn mask_relevant_modifiers(m: Modifiers) -> Modifiers {
    Modifiers::from_bits_truncate(m.bits() & RELEVANT_MODS.bits())
}

/// A parsed keyboard shortcut (e.g. `Option+Space`, `Ctrl+Alt+Space`, `Shift+Space`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: Modifiers,
}

impl KeyChord {
    pub const fn new(code: KeyCode, modifiers: Modifiers) -> Self {
        Self { code, modifiers }
    }

    /// Parses a human-readable shortcut string into a normalized [`KeyChord`].
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return None;
        }

        let mut mods = Modifiers::EMPTY;
        let mut key: Option<KeyCode> = None;

        // Handle trailing literal "+" if someone ever binds "+"
        let tokens: Vec<&str> = trimmed
            .split('+')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() {
            return None;
        }

        for (idx, token) in tokens.iter().enumerate() {
            let lower = token.to_ascii_lowercase();
            let is_last = idx + 1 == tokens.len();

            if !is_last {
                match lower.as_str() {
                    "cmd" | "command" | "meta" | "super" | "win" | "⌘" => {
                        mods.insert(Modifiers::META);
                        continue;
                    }
                    "opt" | "option" | "alt" | "⌥" => {
                        mods.insert(Modifiers::ALT);
                        continue;
                    }
                    "ctrl" | "control" | "⌃" => {
                        mods.insert(Modifiers::CONTROL);
                        continue;
                    }
                    "shift" | "⇧" => {
                        mods.insert(Modifiers::SHIFT);
                        continue;
                    }
                    _ => {}
                }
            }

            key = parse_single_keycode(token);
        }

        key.map(|code| Self {
            code,
            modifiers: mask_relevant_modifiers(mods),
        })
    }

    #[inline]
    pub fn matches(&self, ev: &RawKeyEvent) -> bool {
        ev.code == self.code && mask_relevant_modifiers(ev.modifiers) == self.modifiers
    }
}

pub fn parse_single_keycode(token: &str) -> Option<KeyCode> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "space" | "spacebar" => Some(KeyCode::SPACE),
        "enter" | "return" => Some(KeyCode::RETURN),
        "tab" => Some(KeyCode::TAB),
        "esc" | "escape" => Some(KeyCode::ESCAPE),
        "backspace" | "delete" => Some(KeyCode::BACKSPACE),
        "left" | "arrowleft" => Some(KeyCode::ARROW_LEFT),
        "right" | "arrowright" => Some(KeyCode::ARROW_RIGHT),
        "up" | "arrowup" => Some(KeyCode::ARROW_UP),
        "down" | "arrowdown" => Some(KeyCode::ARROW_DOWN),
        "," | "comma" => Some(KeyCode::COMMA),
        "." | "period" => Some(KeyCode::PERIOD),
        "/" | "slash" => Some(KeyCode::SLASH),
        ";" | "semicolon" => Some(KeyCode::SEMICOLON),
        "'" | "quote" | "apostrophe" => Some(KeyCode::APOSTROPHE),
        "[" | "bracketleft" => Some(KeyCode::BRACKET_LEFT),
        "]" | "bracketright" => Some(KeyCode::BRACKET_RIGHT),
        "-" | "minus" => Some(KeyCode::MINUS),
        "=" | "equal" => Some(KeyCode::EQUAL),
        "`" | "backquote" | "grave" => Some(KeyCode::GRAVE),
        "\\" | "backslash" => Some(KeyCode::BACKSLASH),
        _ => {
            let mut chars = trimmed.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            if ch.is_ascii_alphabetic() {
                Some(KeyCode::from_ascii_upper(ch.to_ascii_uppercase() as u8))
            } else if ch.is_ascii_digit() {
                Some(KeyCode::from_ascii_digit(ch as u8))
            } else {
                None
            }
        }
    }
}

/// Active keybinding configuration snapshot used by [`HookRouter`].
#[derive(Debug, Clone)]
pub struct HookRouteConfig {
    pub toggle_grid: Option<KeyChord>,
    pub toggle_label_mode: Option<KeyChord>,
    pub toggle_free_mode: Option<KeyChord>,
    pub left_click: Option<KeyChord>,
    pub right_click: Option<KeyChord>,
    pub middle_click: Option<KeyChord>,
    pub double_click: Option<KeyChord>,
    pub hold_drag: Option<KeyChord>,
    pub cycle_monitor: Option<KeyChord>,
    pub open_settings: Option<KeyChord>,
    pub grid_case_sensitive: bool,
    pub free_up: KeyCode,
    pub free_down: KeyCode,
    pub free_left: KeyCode,
    pub free_right: KeyCode,
    pub free_slow: KeyCode,
    pub free_fast: KeyCode,
    pub free_scroll: KeyCode,
    pub free_drag: KeyCode,
}

impl HookRouteConfig {
    pub fn from_config_json(cfg: &Value) -> Self {
        let kb = |key: &str, fallback: &str| -> Option<KeyChord> {
            let raw = cfg
                .get("keybindings")
                .and_then(|v| v.get(key))
                .and_then(Value::as_str)
                .unwrap_or(fallback);
            KeyChord::parse(raw)
        };

        let fm_key = |key: &str, fallback: KeyCode| -> KeyCode {
            cfg.get("freeMode")
                .and_then(|v| v.get(key))
                .and_then(Value::as_str)
                .and_then(parse_single_keycode)
                .unwrap_or(fallback)
        };

        let grid_case_sensitive = cfg
            .get("grid")
            .and_then(|v| v.get("caseSensitive"))
            .and_then(Value::as_bool)
            .unwrap_or(true);

        Self {
            toggle_grid: kb("toggleGrid", "Option+Space"),
            toggle_label_mode: kb("toggleLabelMode", "Option+Shift+Space"),
            toggle_free_mode: kb("toggleFreeMode", "Option+F"),
            left_click: kb("leftClick", "Space"),
            right_click: kb("rightClick", "Shift+Space"),
            middle_click: kb("middleClick", "Option+M"),
            double_click: kb("doubleClick", "Enter"),
            hold_drag: kb("holdDrag", "D"),
            cycle_monitor: kb("cycleMonitor", "Tab"),
            open_settings: kb("openSettings", "Cmd+,"),
            grid_case_sensitive,
            free_up: fm_key("keyUp", KeyCode::from_ascii_upper(b'I')),
            free_down: fm_key("keyDown", KeyCode::from_ascii_upper(b'K')),
            free_left: fm_key("keyLeft", KeyCode::from_ascii_upper(b'J')),
            free_right: fm_key("keyRight", KeyCode::from_ascii_upper(b'L')),
            free_slow: fm_key("modSlow", KeyCode::from_ascii_upper(b'S')),
            free_fast: fm_key("modFast", KeyCode::from_ascii_upper(b'F')),
            free_scroll: fm_key("modScroll", KeyCode::from_ascii_upper(b'D')),
            free_drag: fm_key("modDrag", KeyCode::from_ascii_upper(b'A')),
        }
    }
}

/// High-level action sent from the fast OS hook callback to the RatPoison worker thread.
#[derive(Debug, Clone)]
pub enum HookAction {
    ToggleGrid,
    ToggleLabelMode,
    ToggleFreeMode,
    OpenSettings,
    Engine(EngineEvent),
}

/// Fast, non-blocking keyboard router shared between the OS hook thread and worker thread.
pub struct HookRouter {
    config: HookRouteConfig,
    mode: EngineMode,
    dragging: bool,
    scroll_held: bool,
    swallowed_keys: HashSet<KeyCode>,
    active_free_directions: HashSet<Direction>,
}

impl HookRouter {
    pub fn new(config: HookRouteConfig) -> Self {
        Self {
            config,
            mode: EngineMode::Normal,
            dragging: false,
            scroll_held: false,
            swallowed_keys: HashSet::new(),
            active_free_directions: HashSet::new(),
        }
    }

    pub fn update_config(&mut self, config: HookRouteConfig) {
        self.config = config;
    }

    pub fn sync_engine_state(&mut self, mode: EngineMode, dragging: bool) {
        self.mode = mode;
        self.dragging = dragging;
        if mode != EngineMode::FreeMode {
            self.scroll_held = false;
            self.active_free_directions.clear();
        }
    }

    pub fn mode(&self) -> EngineMode {
        self.mode
    }

    pub fn is_scroll_held(&self) -> bool {
        self.scroll_held
    }

    pub fn active_directions_strings(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.active_free_directions.len());
        for dir in [
            Direction::Up,
            Direction::Down,
            Direction::Left,
            Direction::Right,
        ] {
            if self.active_free_directions.contains(&dir) {
                out.push(
                    match dir {
                        Direction::Up => "up",
                        Direction::Down => "down",
                        Direction::Left => "left",
                        Direction::Right => "right",
                    }
                    .to_string(),
                );
            }
        }
        out
    }

    fn direction_for_free_key(&self, code: KeyCode) -> Option<Direction> {
        if code == self.config.free_up || code == KeyCode::ARROW_UP {
            Some(Direction::Up)
        } else if code == self.config.free_down || code == KeyCode::ARROW_DOWN {
            Some(Direction::Down)
        } else if code == self.config.free_left || code == KeyCode::ARROW_LEFT {
            Some(Direction::Left)
        } else if code == self.config.free_right || code == KeyCode::ARROW_RIGHT {
            Some(Direction::Right)
        } else {
            None
        }
    }

    fn map_direction_to_free_key(dir: Direction, scroll: bool) -> FreeKey {
        match (dir, scroll) {
            (Direction::Up, false) => FreeKey::MoveUp,
            (Direction::Down, false) => FreeKey::MoveDown,
            (Direction::Left, false) => FreeKey::MoveLeft,
            (Direction::Right, false) => FreeKey::MoveRight,
            (Direction::Up, true) => FreeKey::ScrollUp,
            (Direction::Down, true) => FreeKey::ScrollDown,
            (Direction::Left, true) => FreeKey::ScrollLeft,
            (Direction::Right, true) => FreeKey::ScrollRight,
        }
    }

    fn match_mouse_click_chord(&self, ev: &RawKeyEvent) -> Option<MouseActionKind> {
        let candidates = [
            (self.config.right_click, MouseActionKind::RightClick),
            (self.config.middle_click, MouseActionKind::MiddleClick),
            (self.config.double_click, MouseActionKind::DoubleClick),
            (self.config.left_click, MouseActionKind::LeftClick),
        ];
        // Check chords with modifiers first so `Shift+Space` wins over `Space`.
        for (chord_opt, action) in candidates {
            if let Some(chord) = chord_opt {
                if !chord.modifiers.is_empty() && chord.matches(ev) {
                    return Some(action);
                }
            }
        }
        for (chord_opt, action) in candidates {
            if let Some(chord) = chord_opt {
                if chord.modifiers.is_empty() && chord.matches(ev) {
                    return Some(action);
                }
            }
        }
        None
    }

    /// Processes a single raw OS keyboard event and returns whether to swallow it plus
    /// any actions for the worker thread.
    pub fn route_event(&mut self, ev: RawKeyEvent) -> (InterceptDecision, Vec<HookAction>) {
        // Never intercept synthetic events we injected ourselves.
        if ev.is_injected {
            return (InterceptDecision::PassThrough, Vec::new());
        }

        // Never swallow pure modifier flag changes so OS modifier state stays balanced.
        if !ev.code.as_modifier().is_empty() {
            return (InterceptDecision::PassThrough, Vec::new());
        }

        let mut actions = Vec::new();

        // Key-up handling: release Free Mode keys & swallow key-ups of previously swallowed keys.
        if ev.phase == KeyPhase::Up {
            if self.mode == EngineMode::FreeMode {
                if ev.code == self.config.free_scroll {
                    self.scroll_held = false;
                    // Switch any held directions back from Scroll to Move.
                    for &dir in &self.active_free_directions {
                        actions.push(HookAction::Engine(EngineEvent::FreeModeKeyUp {
                            key: Self::map_direction_to_free_key(dir, true),
                        }));
                        actions.push(HookAction::Engine(EngineEvent::FreeModeKeyDown {
                            key: Self::map_direction_to_free_key(dir, false),
                        }));
                    }
                    self.swallowed_keys.remove(&ev.code);
                    return (InterceptDecision::Swallow, actions);
                }
                if let Some(dir) = self.direction_for_free_key(ev.code) {
                    self.active_free_directions.remove(&dir);
                    actions.push(HookAction::Engine(EngineEvent::FreeModeKeyUp {
                        key: Self::map_direction_to_free_key(dir, false),
                    }));
                    actions.push(HookAction::Engine(EngineEvent::FreeModeKeyUp {
                        key: Self::map_direction_to_free_key(dir, true),
                    }));
                    self.swallowed_keys.remove(&ev.code);
                    return (InterceptDecision::Swallow, actions);
                }
                if ev.code == self.config.free_slow {
                    actions.push(HookAction::Engine(EngineEvent::FreeModeKeyUp {
                        key: FreeKey::SpeedSlow,
                    }));
                    self.swallowed_keys.remove(&ev.code);
                    return (InterceptDecision::Swallow, actions);
                }
                if ev.code == self.config.free_fast {
                    actions.push(HookAction::Engine(EngineEvent::FreeModeKeyUp {
                        key: FreeKey::SpeedFast,
                    }));
                    self.swallowed_keys.remove(&ev.code);
                    return (InterceptDecision::Swallow, actions);
                }
            }

            if self.swallowed_keys.remove(&ev.code) {
                return (InterceptDecision::Swallow, actions);
            }
            return (InterceptDecision::PassThrough, actions);
        }

        // Global mode toggle shortcuts (checked in all modes).
        if self
            .config
            .toggle_label_mode
            .is_some_and(|c| c.matches(&ev))
        {
            self.swallowed_keys.insert(ev.code);
            if ev.phase == KeyPhase::Down {
                self.mode = if self.mode == EngineMode::LabelMode {
                    EngineMode::Normal
                } else {
                    EngineMode::LabelMode
                };
                actions.push(HookAction::ToggleLabelMode);
            }
            return (InterceptDecision::Swallow, actions);
        }

        if self.config.toggle_grid.is_some_and(|c| c.matches(&ev)) {
            self.swallowed_keys.insert(ev.code);
            if ev.phase == KeyPhase::Down {
                if matches!(
                    self.mode,
                    EngineMode::Grid | EngineMode::Subgrid | EngineMode::Nudge
                ) {
                    self.mode = EngineMode::Normal;
                    actions.push(HookAction::Engine(EngineEvent::ExitToNormal));
                } else {
                    self.mode = EngineMode::Grid;
                    actions.push(HookAction::ToggleGrid);
                }
            }
            return (InterceptDecision::Swallow, actions);
        }

        if self
            .config
            .toggle_free_mode
            .is_some_and(|c| c.matches(&ev))
        {
            self.swallowed_keys.insert(ev.code);
            if ev.phase == KeyPhase::Down {
                self.mode = if self.mode == EngineMode::FreeMode {
                    EngineMode::Normal
                } else {
                    EngineMode::FreeMode
                };
                self.scroll_held = false;
                self.active_free_directions.clear();
                actions.push(HookAction::ToggleFreeMode);
            }
            return (InterceptDecision::Swallow, actions);
        }

        // In Normal mode, every other key passes straight through untouched.
        if self.mode == EngineMode::Normal {
            return (InterceptDecision::PassThrough, actions);
        }

        // --- Active overlay / cursor modes below ---

        if ev.code == KeyCode::ESCAPE {
            self.swallowed_keys.insert(ev.code);
            if ev.phase == KeyPhase::Down {
                self.mode = EngineMode::Normal;
                self.scroll_held = false;
                self.active_free_directions.clear();
                actions.push(HookAction::Engine(EngineEvent::ExitToNormal));
            }
            return (InterceptDecision::Swallow, actions);
        }

        if self.config.open_settings.is_some_and(|c| c.matches(&ev)) {
            self.swallowed_keys.insert(ev.code);
            if ev.phase == KeyPhase::Down {
                self.mode = EngineMode::Normal;
                actions.push(HookAction::Engine(EngineEvent::ExitToNormal));
                actions.push(HookAction::OpenSettings);
            }
            return (InterceptDecision::Swallow, actions);
        }

        if ev.code == KeyCode::BACKSPACE {
            self.swallowed_keys.insert(ev.code);
            actions.push(HookAction::Engine(EngineEvent::Backspace));
            return (InterceptDecision::Swallow, actions);
        }

        if self.config.cycle_monitor.is_some_and(|c| c.matches(&ev)) {
            self.swallowed_keys.insert(ev.code);
            if ev.phase == KeyPhase::Down {
                actions.push(HookAction::Engine(EngineEvent::SwitchMonitor {
                    switch: MonitorSwitch::Next,
                }));
            }
            return (InterceptDecision::Swallow, actions);
        }

        if let Some(mouse_action) = self.match_mouse_click_chord(&ev) {
            self.swallowed_keys.insert(ev.code);
            if ev.phase == KeyPhase::Down {
                actions.push(HookAction::Engine(EngineEvent::MouseAction {
                    action: mouse_action,
                }));
            }
            return (InterceptDecision::Swallow, actions);
        }

        match self.mode {
            EngineMode::FreeMode => {
                self.swallowed_keys.insert(ev.code);
                if ev.code == self.config.free_scroll {
                    if ev.phase == KeyPhase::Down && !self.scroll_held {
                        self.scroll_held = true;
                        for &dir in &self.active_free_directions {
                            actions.push(HookAction::Engine(EngineEvent::FreeModeKeyUp {
                                key: Self::map_direction_to_free_key(dir, false),
                            }));
                            actions.push(HookAction::Engine(EngineEvent::FreeModeKeyDown {
                                key: Self::map_direction_to_free_key(dir, true),
                            }));
                        }
                    }
                    return (InterceptDecision::Swallow, actions);
                }

                if ev.code == self.config.free_drag
                    || self.config.hold_drag.is_some_and(|c| c.matches(&ev))
                {
                    if ev.phase == KeyPhase::Down {
                        let next_action = if self.dragging {
                            MouseActionKind::DragEnd
                        } else {
                            MouseActionKind::DragStart
                        };
                        self.dragging = !self.dragging;
                        actions.push(HookAction::Engine(EngineEvent::MouseAction {
                            action: next_action,
                        }));
                    }
                    return (InterceptDecision::Swallow, actions);
                }

                if let Some(dir) = self.direction_for_free_key(ev.code) {
                    if ev.phase == KeyPhase::Down {
                        self.active_free_directions.insert(dir);
                        actions.push(HookAction::Engine(EngineEvent::FreeModeKeyDown {
                            key: Self::map_direction_to_free_key(dir, self.scroll_held),
                        }));
                    }
                    return (InterceptDecision::Swallow, actions);
                }

                if ev.code == self.config.free_slow {
                    if ev.phase == KeyPhase::Down {
                        actions.push(HookAction::Engine(EngineEvent::FreeModeKeyDown {
                            key: FreeKey::SpeedSlow,
                        }));
                    }
                    return (InterceptDecision::Swallow, actions);
                }

                if ev.code == self.config.free_fast {
                    if ev.phase == KeyPhase::Down {
                        actions.push(HookAction::Engine(EngineEvent::FreeModeKeyDown {
                            key: FreeKey::SpeedFast,
                        }));
                    }
                    return (InterceptDecision::Swallow, actions);
                }

                (InterceptDecision::Swallow, actions)
            }
            EngineMode::Grid | EngineMode::Subgrid | EngineMode::Nudge | EngineMode::LabelMode => {
                // Arrow keys nudge in Grid / Subgrid / Nudge
                if let Some(dir) = match ev.code {
                    KeyCode::ARROW_UP => Some(Direction::Up),
                    KeyCode::ARROW_DOWN => Some(Direction::Down),
                    KeyCode::ARROW_LEFT => Some(Direction::Left),
                    KeyCode::ARROW_RIGHT => Some(Direction::Right),
                    _ => None,
                } {
                    let step = if ev.modifiers.contains(Modifiers::SHIFT) {
                        NudgeStep::Coarse
                    } else if ev.modifiers.contains(Modifiers::ALT) {
                        NudgeStep::Fine
                    } else {
                        NudgeStep::Normal
                    };
                    self.swallowed_keys.insert(ev.code);
                    actions.push(HookAction::Engine(EngineEvent::Nudge {
                        direction: dir,
                        step,
                    }));
                    return (InterceptDecision::Swallow, actions);
                }

                // In Nudge mode, support HJKL nudging and Drag toggle
                if self.mode == EngineMode::Nudge {
                    if self.config.hold_drag.is_some_and(|c| c.matches(&ev)) {
                        self.swallowed_keys.insert(ev.code);
                        if ev.phase == KeyPhase::Down {
                            let next_action = if self.dragging {
                                MouseActionKind::DragEnd
                            } else {
                                MouseActionKind::DragStart
                            };
                            self.dragging = !self.dragging;
                            actions.push(HookAction::Engine(EngineEvent::MouseAction {
                                action: next_action,
                            }));
                        }
                        return (InterceptDecision::Swallow, actions);
                    }

                    let hjkl_dir = match ev.code {
                        c if c == KeyCode::from_ascii_upper(b'H') => Some(Direction::Left),
                        c if c == KeyCode::from_ascii_upper(b'J') => Some(Direction::Down),
                        c if c == KeyCode::from_ascii_upper(b'K') => Some(Direction::Up),
                        c if c == KeyCode::from_ascii_upper(b'L') => Some(Direction::Right),
                        _ => None,
                    };
                    if let Some(dir) = hjkl_dir {
                        let step = if ev.modifiers.contains(Modifiers::SHIFT) {
                            NudgeStep::Coarse
                        } else if ev.modifiers.contains(Modifiers::ALT) {
                            NudgeStep::Fine
                        } else {
                            NudgeStep::Normal
                        };
                        self.swallowed_keys.insert(ev.code);
                        actions.push(HookAction::Engine(EngineEvent::Nudge {
                            direction: dir,
                            step,
                        }));
                        return (InterceptDecision::Swallow, actions);
                    }
                }

                // Printable letters & digits target Grid / Subgrid / LabelMode codes
                let non_shift = Modifiers::from_bits_truncate(
                    mask_relevant_modifiers(ev.modifiers).bits() & !Modifiers::SHIFT.bits(),
                );
                if non_shift.is_empty() {
                    let use_shift = ev.modifiers.contains(Modifiers::SHIFT)
                        && self.mode == EngineMode::Grid
                        && self.config.grid_case_sensitive;
                    if let Some(ch) = ev.code.to_ascii_char(use_shift) {
                        self.swallowed_keys.insert(ev.code);
                        if ev.phase == KeyPhase::Down {
                            actions.push(HookAction::Engine(EngineEvent::KeyChar { ch }));
                        }
                        return (InterceptDecision::Swallow, actions);
                    }
                }

                // Allow Cmd+Tab / Cmd+Q etc. through if user holds Cmd/Ctrl while overlay is up,
                // otherwise swallow stray unmodified keys.
                if non_shift.is_empty() {
                    self.swallowed_keys.insert(ev.code);
                    (InterceptDecision::Swallow, actions)
                } else {
                    (InterceptDecision::PassThrough, actions)
                }
            }
            EngineMode::Normal => (InterceptDecision::PassThrough, actions),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::default_frontend_config_json_for_os;

    fn ev(code: KeyCode, mods: Modifiers, phase: KeyPhase) -> RawKeyEvent {
        RawKeyEvent::new(code, mods, phase, 1)
    }

    #[test]
    fn parses_cross_platform_shortcut_strings() {
        assert_eq!(
            KeyChord::parse("Option+Space"),
            Some(KeyChord::new(KeyCode::SPACE, Modifiers::ALT))
        );
        assert_eq!(
            KeyChord::parse("Option+Shift+Space"),
            Some(KeyChord::new(KeyCode::SPACE, Modifiers::ALT.union(Modifiers::SHIFT)))
        );
        assert_eq!(
            KeyChord::parse("Ctrl+Alt+Space"),
            Some(KeyChord::new(
                KeyCode::SPACE,
                Modifiers::CONTROL.union(Modifiers::ALT)
            ))
        );
        assert_eq!(
            KeyChord::parse("Super+Alt+Space"),
            Some(KeyChord::new(
                KeyCode::SPACE,
                Modifiers::META.union(Modifiers::ALT)
            ))
        );
        assert_eq!(
            KeyChord::parse("Cmd+,"),
            Some(KeyChord::new(KeyCode::COMMA, Modifiers::META))
        );
    }

    #[test]
    fn option_space_toggles_grid_and_routes_grid_typing_and_clicks() {
        let cfg = HookRouteConfig::from_config_json(&default_frontend_config_json_for_os("macos"));
        let mut router = HookRouter::new(cfg);

        // Normal key passes through in Normal mode
        let (dec, actions) = router.route_event(ev(
            KeyCode::from_ascii_upper(b'A'),
            Modifiers::EMPTY,
            KeyPhase::Down,
        ));
        assert_eq!(dec, InterceptDecision::PassThrough);
        assert!(actions.is_empty());

        // Option+Space activates Grid and swallows both Down and Up
        let (dec, actions) = router.route_event(ev(KeyCode::SPACE, Modifiers::ALT, KeyPhase::Down));
        assert_eq!(dec, InterceptDecision::Swallow);
        assert!(matches!(actions.as_slice(), [HookAction::ToggleGrid]));
        assert_eq!(router.mode(), EngineMode::Grid);

        let (dec, _) = router.route_event(ev(KeyCode::SPACE, Modifiers::ALT, KeyPhase::Up));
        assert_eq!(dec, InterceptDecision::Swallow);

        // Typing 'a' in Grid mode emits KeyChar('a') and swallows
        let (dec, actions) = router.route_event(ev(
            KeyCode::from_ascii_upper(b'A'),
            Modifiers::EMPTY,
            KeyPhase::Down,
        ));
        assert_eq!(dec, InterceptDecision::Swallow);
        assert!(matches!(
            actions.as_slice(),
            [HookAction::Engine(EngineEvent::KeyChar { ch: 'a' })]
        ));

        // Shift+Space triggers RightClick (not LeftClick)
        let (dec, actions) = router.route_event(ev(KeyCode::SPACE, Modifiers::SHIFT, KeyPhase::Down));
        assert_eq!(dec, InterceptDecision::Swallow);
        assert!(matches!(
            actions.as_slice(),
            [HookAction::Engine(EngineEvent::MouseAction {
                action: MouseActionKind::RightClick
            })]
        ));

        // Pressing Option+Space again exits Grid to Normal
        let (dec, actions) = router.route_event(ev(KeyCode::SPACE, Modifiers::ALT, KeyPhase::Down));
        assert_eq!(dec, InterceptDecision::Swallow);
        assert!(matches!(
            actions.as_slice(),
            [HookAction::Engine(EngineEvent::ExitToNormal)]
        ));
        assert_eq!(router.mode(), EngineMode::Normal);
    }

    #[test]
    fn free_mode_routes_ijkl_speed_and_scroll_keys() {
        let cfg = HookRouteConfig::from_config_json(&default_frontend_config_json_for_os("macos"));
        let mut router = HookRouter::new(cfg);

        // Option+F activates FreeMode
        let (dec, actions) = router.route_event(ev(
            KeyCode::from_ascii_upper(b'F'),
            Modifiers::ALT,
            KeyPhase::Down,
        ));
        assert_eq!(dec, InterceptDecision::Swallow);
        assert!(matches!(actions.as_slice(), [HookAction::ToggleFreeMode]));
        assert_eq!(router.mode(), EngineMode::FreeMode);

        // Holding I moves up
        let (dec, actions) = router.route_event(ev(
            KeyCode::from_ascii_upper(b'I'),
            Modifiers::EMPTY,
            KeyPhase::Down,
        ));
        assert_eq!(dec, InterceptDecision::Swallow);
        assert!(matches!(
            actions.as_slice(),
            [HookAction::Engine(EngineEvent::FreeModeKeyDown {
                key: FreeKey::MoveUp
            })]
        ));
        assert_eq!(router.active_directions_strings(), vec!["up"]);

        // Releasing I stops moving up
        let (dec, actions) = router.route_event(ev(
            KeyCode::from_ascii_upper(b'I'),
            Modifiers::EMPTY,
            KeyPhase::Up,
        ));
        assert_eq!(dec, InterceptDecision::Swallow);
        assert!(!actions.is_empty());
        assert!(router.active_directions_strings().is_empty());
    }
}
