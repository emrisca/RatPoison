//! Deterministic in-memory mock HID backend for unit and integration tests.

use crate::interceptor::InterceptDecision;
use crate::platform::{KeyEventCallback, KeyboardHookBackend, MouseSimulatorBackend};
use crate::types::{
    ClickKind, HidError, KeyPhase, LogicalPoint, MouseButton, PermissionReport, RawKeyEvent,
    ScrollDelta,
};

/// Recorded synthetic mouse operation performed against [`MockHidBackend`].
#[derive(Debug, Clone, PartialEq)]
pub enum MockMouseOp {
    MoveTo {
        point: LogicalPoint,
        scale_factor: f64,
    },
    MoveRelative {
        dx: f64,
        dy: f64,
    },
    MouseDown {
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    },
    MouseUp {
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    },
    Scroll(ScrollDelta),
}

/// Non-sensitive summary of an event that reached the simulated foreground application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForegroundEventRecord {
    pub phase: KeyPhase,
    pub timestamp_ms: u64,
    pub was_injected: bool,
}

/// Deterministic mock implementing both [`KeyboardHookBackend`] and [`MouseSimulatorBackend`].
pub struct MockHidBackend {
    running: bool,
    accessibility_granted: bool,
    secure_input_active: bool,
    callback: Option<KeyEventCallback>,
    cursor: LogicalPoint,
    pressed_buttons: Vec<MouseButton>,
    mouse_ops: Vec<MockMouseOp>,
    foreground_received: Vec<ForegroundEventRecord>,
    swallowed_count: usize,
}

impl MockHidBackend {
    pub fn new() -> Self {
        Self {
            running: false,
            accessibility_granted: true,
            secure_input_active: false,
            callback: None,
            cursor: LogicalPoint::new(0.0, 0.0),
            pressed_buttons: Vec::new(),
            mouse_ops: Vec::new(),
            foreground_received: Vec::new(),
            swallowed_count: 0,
        }
    }

    /// Configures whether the mock simulates granted OS Accessibility permissions.
    pub fn set_accessibility_granted(&mut self, granted: bool) {
        self.accessibility_granted = granted;
    }

    /// Configures whether the mock simulates OS Secure Keyboard Input being active.
    pub fn set_secure_input_active(&mut self, active: bool) {
        self.secure_input_active = active;
    }

    /// Injects a deterministic [`RawKeyEvent`] through the installed callback and tracks
    /// whether the event leaked through to the simulated foreground application.
    pub fn dispatch_event(&mut self, event: RawKeyEvent) -> Result<InterceptDecision, HidError> {
        if !self.running {
            return Err(HidError::HookNotRunning);
        }
        if self.secure_input_active {
            // Mirror OS Secure Input behavior: never intercept keystrokes while active.
            self.foreground_received.push(ForegroundEventRecord {
                phase: event.phase,
                timestamp_ms: event.timestamp_ms,
                was_injected: event.is_injected,
            });
            return Ok(InterceptDecision::PassThrough);
        }

        let decision = match self.callback.as_mut() {
            Some(cb) => cb(event),
            None => return Err(HidError::HookNotRunning),
        };

        match &decision {
            InterceptDecision::PassThrough => {
                self.foreground_received.push(ForegroundEventRecord {
                    phase: event.phase,
                    timestamp_ms: event.timestamp_ms,
                    was_injected: event.is_injected,
                });
            }
            InterceptDecision::Swallow | InterceptDecision::SwallowAndEmit(_) => {
                self.swallowed_count += 1;
            }
        }

        Ok(decision)
    }

    /// Returns all simulated mouse operations recorded so far.
    pub fn mouse_ops(&self) -> &[MockMouseOp] {
        &self.mouse_ops
    }

    /// Returns all events that passed through to the simulated foreground application.
    pub fn foreground_received(&self) -> &[ForegroundEventRecord] {
        &self.foreground_received
    }

    /// Returns how many key events were swallowed.
    pub fn swallowed_count(&self) -> usize {
        self.swallowed_count
    }

    /// Returns the currently held synthetic mouse buttons.
    pub fn pressed_buttons(&self) -> &[MouseButton] {
        &self.pressed_buttons
    }
}

impl Default for MockHidBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardHookBackend for MockHidBackend {
    fn backend_name(&self) -> &'static str {
        "MockHidBackend"
    }

    fn check_permissions(&self) -> PermissionReport {
        PermissionReport {
            platform: "mock",
            accessibility_granted: self.accessibility_granted,
            input_monitoring_granted: self.accessibility_granted,
            secure_input_active: self.secure_input_active,
            elevated_target_caveat: false,
            uinput_available: true,
            xtest_available: true,
            guidance: if self.accessibility_granted {
                None
            } else {
                Some("Enable mock accessibility permission for test".to_string())
            },
        }
    }

    fn start(&mut self, callback: KeyEventCallback) -> Result<(), HidError> {
        if !self.accessibility_granted {
            return Err(HidError::PermissionDenied {
                permission: "Accessibility",
                detail: "Mock accessibility permission denied".to_string(),
            });
        }
        self.callback = Some(callback);
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), HidError> {
        self.running = false;
        self.callback = None;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_secure_input_active(&self) -> bool {
        self.secure_input_active
    }
}

impl MouseSimulatorBackend for MockHidBackend {
    fn backend_name(&self) -> &'static str {
        "MockHidBackend"
    }

    fn move_to(&mut self, point: LogicalPoint, scale_factor: f64) -> Result<(), HidError> {
        self.cursor = point;
        self.mouse_ops.push(MockMouseOp::MoveTo {
            point,
            scale_factor,
        });
        Ok(())
    }

    fn move_relative(&mut self, dx: f64, dy: f64) -> Result<(), HidError> {
        self.cursor = LogicalPoint::new(self.cursor.x + dx, self.cursor.y + dy);
        self.mouse_ops.push(MockMouseOp::MoveRelative { dx, dy });
        Ok(())
    }

    fn mouse_down(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError> {
        self.cursor = point;
        if !self.pressed_buttons.contains(&button) {
            self.pressed_buttons.push(button);
        }
        self.mouse_ops.push(MockMouseOp::MouseDown {
            button,
            point,
            scale_factor,
        });
        Ok(())
    }

    fn mouse_up(
        &mut self,
        button: MouseButton,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError> {
        self.cursor = point;
        self.pressed_buttons.retain(|&b| b != button);
        self.mouse_ops.push(MockMouseOp::MouseUp {
            button,
            point,
            scale_factor,
        });
        Ok(())
    }

    fn perform_click(
        &mut self,
        kind: ClickKind,
        point: LogicalPoint,
        scale_factor: f64,
    ) -> Result<(), HidError> {
        match kind {
            ClickKind::Left => {
                self.move_to(point, scale_factor)?;
                self.mouse_down(MouseButton::Left, point, scale_factor)?;
                self.mouse_up(MouseButton::Left, point, scale_factor)?;
            }
            ClickKind::Right => {
                self.move_to(point, scale_factor)?;
                self.mouse_down(MouseButton::Right, point, scale_factor)?;
                self.mouse_up(MouseButton::Right, point, scale_factor)?;
            }
            ClickKind::Middle => {
                self.move_to(point, scale_factor)?;
                self.mouse_down(MouseButton::Middle, point, scale_factor)?;
                self.mouse_up(MouseButton::Middle, point, scale_factor)?;
            }
            ClickKind::DoubleLeft => {
                self.move_to(point, scale_factor)?;
                for _ in 0..2 {
                    self.mouse_down(MouseButton::Left, point, scale_factor)?;
                    self.mouse_up(MouseButton::Left, point, scale_factor)?;
                }
            }
            ClickKind::DragStart => {
                self.move_to(point, scale_factor)?;
                self.mouse_down(MouseButton::Left, point, scale_factor)?;
            }
            ClickKind::DragEnd => {
                self.move_to(point, scale_factor)?;
                self.mouse_up(MouseButton::Left, point, scale_factor)?;
            }
        }
        Ok(())
    }

    fn scroll(&mut self, delta: ScrollDelta) -> Result<(), HidError> {
        self.mouse_ops.push(MockMouseOp::Scroll(delta));
        Ok(())
    }

    fn cursor_position(&self) -> Result<LogicalPoint, HidError> {
        Ok(self.cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interceptor::{CommandInterceptor, InterceptorAction, InterceptorConfig};
    use crate::types::{KeyCode, Modifiers};
    use std::sync::{Arc, Mutex};

    #[test]
    fn mock_end_to_end_hold_for_and_click_simulation_zero_leak() {
        let mut backend = MockHidBackend::new();
        let interceptor = Arc::new(Mutex::new(CommandInterceptor::new(
            InterceptorConfig::default(),
        )));

        let interceptor_cb = Arc::clone(&interceptor);
        backend
            .start(Box::new(move |ev| {
                interceptor_cb.lock().unwrap().handle_event(ev)
            }))
            .unwrap();

        // 1. Alt down (foreground sees it), Space down (activates & swallowed)
        backend
            .dispatch_event(RawKeyEvent::new(
                KeyCode::ALT_L,
                Modifiers::ALT,
                KeyPhase::Down,
                10,
            ))
            .unwrap();
        let act = backend
            .dispatch_event(RawKeyEvent::new(
                KeyCode::SPACE,
                Modifiers::ALT,
                KeyPhase::Down,
                20,
            ))
            .unwrap();
        assert_eq!(
            act,
            InterceptDecision::SwallowAndEmit(InterceptorAction::ActivateOverlay)
        );

        // 2. Release Space (swallowed) and Alt (passes through -> balanced!)
        backend
            .dispatch_event(RawKeyEvent::new(
                KeyCode::SPACE,
                Modifiers::ALT,
                KeyPhase::Up,
                30,
            ))
            .unwrap();
        backend
            .dispatch_event(RawKeyEvent::new(
                KeyCode::ALT_L,
                Modifiers::EMPTY,
                KeyPhase::Up,
                40,
            ))
            .unwrap();

        // 3. Hold 'R' + press Return -> executes Right click
        let key_r = KeyCode::from_ascii_upper(b'R');
        backend
            .dispatch_event(RawKeyEvent::new(key_r, Modifiers::EMPTY, KeyPhase::Down, 50))
            .unwrap();
        backend
            .dispatch_event(RawKeyEvent::new(
                key_r,
                Modifiers::EMPTY,
                KeyPhase::Repeat,
                60,
            ))
            .unwrap();
        let exec = backend
            .dispatch_event(RawKeyEvent::new(
                KeyCode::RETURN,
                Modifiers::EMPTY,
                KeyPhase::Down,
                70,
            ))
            .unwrap();

        if let InterceptDecision::SwallowAndEmit(InterceptorAction::ExecuteClick { kind, .. }) =
            exec
        {
            assert_eq!(kind, ClickKind::Right);
            backend
                .perform_click(kind, LogicalPoint::new(640.0, 360.0), 2.0)
                .unwrap();
        } else {
            panic!("expected ExecuteClick");
        }

        // 4. Release Return & R after deactivation -> both swallowed
        backend
            .dispatch_event(RawKeyEvent::new(
                KeyCode::RETURN,
                Modifiers::EMPTY,
                KeyPhase::Up,
                80,
            ))
            .unwrap();
        backend
            .dispatch_event(RawKeyEvent::new(key_r, Modifiers::EMPTY, KeyPhase::Up, 90))
            .unwrap();

        // Foreground ONLY saw Alt Down + Alt Up (2 balanced events), ZERO leaks of Space/R/Return!
        assert_eq!(backend.foreground_received().len(), 2);
        assert_eq!(backend.foreground_received()[0].phase, KeyPhase::Down);
        assert_eq!(backend.foreground_received()[1].phase, KeyPhase::Up);
        assert_eq!(backend.swallowed_count(), 7);
        assert_eq!(backend.cursor_position().unwrap(), LogicalPoint::new(640.0, 360.0));
        assert!(backend.pressed_buttons().is_empty());
        assert_eq!(backend.mouse_ops().len(), 3); // MoveTo, MouseDown(Right), MouseUp(Right)
    }

    #[test]
    fn mock_respects_permission_denied_and_secure_input() {
        let mut backend = MockHidBackend::new();
        backend.set_accessibility_granted(false);
        assert!(matches!(
            backend.start(Box::new(|_| InterceptDecision::Swallow)),
            Err(HidError::PermissionDenied { .. })
        ));

        backend.set_accessibility_granted(true);
        backend
            .start(Box::new(|_| InterceptDecision::Swallow))
            .unwrap();
        backend.set_secure_input_active(true);

        // When Secure Input is active, keystrokes MUST pass through untouched
        let decision = backend
            .dispatch_event(RawKeyEvent::new(
                KeyCode::from_ascii_upper(b'P'),
                Modifiers::EMPTY,
                KeyPhase::Down,
                100,
            ))
            .unwrap();
        assert_eq!(decision, InterceptDecision::PassThrough);
        assert_eq!(backend.swallowed_count(), 0);
        assert_eq!(backend.foreground_received().len(), 1);
    }
}
