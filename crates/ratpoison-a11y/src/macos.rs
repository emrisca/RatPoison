//! macOS Accessibility Tree adapter using `ApplicationServices` (`AXUIElement`).
//!
//! Features:
//! - Permission verification and prompting via `AXIsProcessTrusted` / `AXIsProcessTrustedWithOptions`.
//! - System-wide focus discovery (`AXUIElementCreateSystemWide` -> `AXFocusedApplication` -> `AXWindows`).
//! - Automatic Chromium / Electron / WebKit accessibility activation (`AXEnhancedUserInterface = kCFBooleanTrue`,
//!   `AXManualAccessibility = kCFBooleanTrue`).
//! - Strict depth, node-count, and wall-clock time budgets (`<120ms` default) to prevent stalling on massive DOMs.

use crate::{
    A11yError, AccessibilityBudget, AccessibilityProvider, AccessibilitySnapshot,
    AccessibleElement, ElementRole, LogicalRect,
};
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;
use std::time::Instant;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFBooleanRef = *const c_void;
type CFAllocatorRef = *const c_void;
type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;
type AXError = i32;
type CFIndex = isize;
type CFStringEncoding = u32;

const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;
const K_AX_ERROR_SUCCESS: AXError = 0;
const K_AX_VALUE_CGPOINT_TYPE: u32 = 1;
const K_AX_VALUE_CGSIZE_TYPE: u32 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFBooleanRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;

    fn CFRelease(cf: CFTypeRef);
    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: CFStringEncoding,
    ) -> CFStringRef;
    fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
    fn CFStringGetMaximumSizeForEncoding(
        length: CFIndex,
        encoding: CFStringEncoding,
    ) -> CFIndex;
    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: CFStringEncoding,
    ) -> bool;
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;

    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementCopyActionNames(
        element: AXUIElementRef,
        names: *mut CFArrayRef,
    ) -> AXError;
    fn AXValueGetValue(
        value: AXValueRef,
        the_type: u32,
        value_ptr: *mut c_void,
    ) -> bool;
}

/// RAII wrapper for CoreFoundation references.
struct ScopedCFRef(CFTypeRef);

impl ScopedCFRef {
    fn new(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for ScopedCFRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CFRelease(self.0);
            }
        }
    }
}

fn cfstring_from_str(s: &str) -> Option<ScopedCFRef> {
    let c_str = CString::new(s).ok()?;
    let ptr = unsafe {
        CFStringCreateWithCString(ptr::null(), c_str.as_ptr(), K_CF_STRING_ENCODING_UTF8)
    };
    ScopedCFRef::new(ptr)
}

unsafe fn cfstring_to_string(cf_str: CFStringRef) -> Option<String> {
    if cf_str.is_null() {
        return None;
    }
    let len = CFStringGetLength(cf_str);
    if len <= 0 {
        return Some(String::new());
    }
    let max_size = CFStringGetMaximumSizeForEncoding(len, K_CF_STRING_ENCODING_UTF8) + 1;
    let mut buffer = vec![0u8; max_size as usize];
    let ok = CFStringGetCString(
        cf_str,
        buffer.as_mut_ptr() as *mut c_char,
        max_size,
        K_CF_STRING_ENCODING_UTF8,
    );
    if !ok {
        return None;
    }
    let c_str = CStr::from_ptr(buffer.as_ptr() as *const c_char);
    Some(c_str.to_string_lossy().into_owned())
}

/// Maps a macOS `AXRole` (and optional `AXSubrole`) string to RatPoison's unified `ElementRole`.
pub fn map_ax_role(ax_role: &str, ax_subrole: Option<&str>) -> ElementRole {
    if let Some(sub) = ax_subrole {
        match sub {
            "AXCloseButton" | "AXMinimizeButton" | "AXZoomButton" | "AXFullScreenButton" => {
                return ElementRole::WindowControl;
            }
            "AXSearchField" | "AXSecureTextField" => {
                return ElementRole::TextField;
            }
            "AXTabButton" => {
                return ElementRole::Tab;
            }
            _ => {}
        }
    }

    match ax_role {
        "AXButton" | "AXPopUpButton" | "AXMenuButton" | "AXDisclosureTriangle" => {
            ElementRole::Button
        }
        "AXLink" => ElementRole::Link,
        "AXTextField" | "AXTextArea" | "AXComboBox" => ElementRole::TextField,
        "AXMenuItem" | "AXMenuBarItem" => ElementRole::MenuItem,
        "AXRadioButton" => ElementRole::Radio,
        "AXCheckBox" => ElementRole::Checkbox,
        "AXSlider" => ElementRole::Slider,
        "AXTabGroup" | "AXTab" => ElementRole::Tab,
        "AXRow" | "AXCell" | "AXOutlineRow" => ElementRole::ListItem,
        "AXToolbar" => ElementRole::Toolbar,
        "AXWindow" | "AXSheet" | "AXDrawer" => ElementRole::Window,
        "AXGroup" | "AXScrollArea" | "AXSplitGroup" | "AXWebArea" | "AXLayoutArea" => {
            ElementRole::Container
        }
        "AXStaticText" => ElementRole::StaticText,
        "AXImage" => ElementRole::Image,
        _ => ElementRole::Unknown,
    }
}

/// macOS `AXUIElement` Accessibility Tree provider.
#[derive(Debug, Clone, Default)]
pub struct MacOSAccessibilityProvider {
    /// Optional target application PID override; when `None`, queries the system-wide focused application.
    pub target_pid: Option<i32>,
    /// Whether to set `AXEnhancedUserInterface` and `AXManualAccessibility` on the target application
    /// so Chromium, Electron, and Firefox expose their internal web accessibility trees.
    pub enable_enhanced_ui: bool,
}

impl MacOSAccessibilityProvider {
    pub fn new() -> Self {
        Self {
            target_pid: None,
            enable_enhanced_ui: true,
        }
    }

    pub fn with_pid(pid: i32) -> Self {
        Self {
            target_pid: Some(pid),
            enable_enhanced_ui: true,
        }
    }

    /// Checks whether the current process is trusted for macOS Accessibility (`AXIsProcessTrustedWithOptions`).
    pub fn check_trusted(prompt: bool) -> bool {
        unsafe {
            if !prompt {
                return AXIsProcessTrusted();
            }
            let keys: [*const c_void; 1] = [kAXTrustedCheckOptionPrompt];
            let values: [*const c_void; 1] = [kCFBooleanTrue];
            let dict = CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            if dict.is_null() {
                return AXIsProcessTrusted();
            }
            let _guard = ScopedCFRef::new(dict);
            AXIsProcessTrustedWithOptions(dict)
        }
    }

    /// Activates Chromium / Electron / WebKit accessibility tree exposure on an application element.
    fn activate_chromium_electron_a11y(app_element: AXUIElementRef) {
        unsafe {
            for attr_name in ["AXEnhancedUserInterface", "AXManualAccessibility"] {
                if let Some(attr) = cfstring_from_str(attr_name) {
                    let _ = AXUIElementSetAttributeValue(
                        app_element,
                        attr.as_ptr(),
                        kCFBooleanTrue,
                    );
                }
            }
        }
    }

    fn copy_attr(element: AXUIElementRef, attr_name: &str) -> Option<ScopedCFRef> {
        let attr = cfstring_from_str(attr_name)?;
        let mut value: CFTypeRef = ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(element, attr.as_ptr(), &mut value) };
        if err == K_AX_ERROR_SUCCESS && !value.is_null() {
            ScopedCFRef::new(value)
        } else {
            None
        }
    }

    fn copy_string_attr(element: AXUIElementRef, attr_name: &str) -> Option<String> {
        let val = Self::copy_attr(element, attr_name)?;
        unsafe { cfstring_to_string(val.as_ptr()) }
    }

    fn copy_bounds(element: AXUIElementRef) -> Option<LogicalRect> {
        let pos_ref = Self::copy_attr(element, "AXPosition")?;
        let size_ref = Self::copy_attr(element, "AXSize")?;

        let mut pt = CGPoint::default();
        let mut sz = CGSize::default();

        let ok_pt = unsafe {
            AXValueGetValue(
                pos_ref.as_ptr(),
                K_AX_VALUE_CGPOINT_TYPE,
                &mut pt as *mut CGPoint as *mut c_void,
            )
        };
        let ok_sz = unsafe {
            AXValueGetValue(
                size_ref.as_ptr(),
                K_AX_VALUE_CGSIZE_TYPE,
                &mut sz as *mut CGSize as *mut c_void,
            )
        };

        if ok_pt && ok_sz {
            Some(LogicalRect::new(pt.x, pt.y, sz.width, sz.height))
        } else {
            None
        }
    }

    fn has_press_or_confirm_action(element: AXUIElementRef) -> bool {
        let mut names_array: CFArrayRef = ptr::null();
        let err = unsafe { AXUIElementCopyActionNames(element, &mut names_array) };
        if err != K_AX_ERROR_SUCCESS || names_array.is_null() {
            return false;
        }
        let guard = match ScopedCFRef::new(names_array) {
            Some(g) => g,
            None => return false,
        };
        let count = unsafe { CFArrayGetCount(guard.as_ptr()) };
        for i in 0..count {
            let item = unsafe { CFArrayGetValueAtIndex(guard.as_ptr(), i) };
            if let Some(action) = unsafe { cfstring_to_string(item) } {
                if matches!(
                    action.as_str(),
                    "AXPress" | "AXConfirm" | "AXShowMenu" | "AXPick" | "AXOpen"
                ) {
                    return true;
                }
            }
        }
        false
    }
}

impl AccessibilityProvider for MacOSAccessibilityProvider {
    fn platform_name(&self) -> &'static str {
        "macos-axuielement"
    }

    fn is_trusted(&self) -> bool {
        Self::check_trusted(false)
    }

    fn ensure_permissions(&self, prompt: bool) -> Result<bool, A11yError> {
        let trusted = Self::check_trusted(prompt);
        if trusted {
            Ok(true)
        } else {
            Err(A11yError::PermissionDenied(
                "macOS Accessibility permission (AXIsProcessTrusted) is not granted. Enable RatPoison in System Settings -> Privacy & Security -> Accessibility.".to_string(),
            ))
        }
    }

    fn snapshot(
        &self,
        budget: &AccessibilityBudget,
    ) -> Result<AccessibilitySnapshot, A11yError> {
        if !self.is_trusted() {
            return Err(A11yError::PermissionDenied(
                "macOS Accessibility permission not granted".to_string(),
            ));
        }

        let start = Instant::now();

        let app_ref = if let Some(pid) = self.target_pid {
            let ptr = unsafe { AXUIElementCreateApplication(pid) };
            ScopedCFRef::new(ptr).ok_or_else(|| {
                A11yError::PlatformError(format!("Failed to create AXUIElement for PID {pid}"))
            })?
        } else {
            let sys_ptr = unsafe { AXUIElementCreateSystemWide() };
            let sys_ref = ScopedCFRef::new(sys_ptr).ok_or_else(|| {
                A11yError::PlatformError("AXUIElementCreateSystemWide returned null".to_string())
            })?;
            Self::copy_attr(sys_ref.as_ptr(), "AXFocusedApplication").ok_or_else(|| {
                A11yError::PlatformError("Could not query AXFocusedApplication".to_string())
            })?
        };

        if self.enable_enhanced_ui {
            Self::activate_chromium_electron_a11y(app_ref.as_ptr());
        }

        let mut app_pid: i32 = self.target_pid.unwrap_or(0);
        if app_pid == 0 {
            unsafe {
                let _ = AXUIElementGetPid(app_ref.as_ptr(), &mut app_pid);
            }
        }

        let mut elements = Vec::new();
        let mut nodes_visited = 0usize;
        let mut max_depth_reached = 0usize;
        let mut truncated_by_budget = false;

        // Iterative traversal stack holding `(element_ptr, depth, z_order)`.
        // Keep parent CFArrayRefs / AXUIElementRefs alive in `cf_arena` while traversing.
        let mut cf_arena: Vec<ScopedCFRef> = Vec::new();
        let mut stack: Vec<(AXUIElementRef, usize, u32)> = Vec::new();

        // Collect root windows from AXWindows (CFArray), falling back to AXFocusedWindow (single element).
        if let Some(win_arr) = Self::copy_attr(app_ref.as_ptr(), "AXWindows") {
            let win_count = unsafe { CFArrayGetCount(win_arr.as_ptr()) };
            for w_idx in (0..win_count).rev() {
                let win_ptr = unsafe { CFArrayGetValueAtIndex(win_arr.as_ptr(), w_idx) };
                if !win_ptr.is_null() {
                    let win_title = Self::copy_string_attr(win_ptr, "AXTitle");
                    if win_title.as_deref() == Some("RatPoison Overlay") {
                        continue;
                    }
                    stack.push((win_ptr, 1, w_idx as u32));
                }
            }
            cf_arena.push(win_arr);
        } else if let Some(focused_win) = Self::copy_attr(app_ref.as_ptr(), "AXFocusedWindow") {
            let win_title = Self::copy_string_attr(focused_win.as_ptr(), "AXTitle");
            if win_title.as_deref() != Some("RatPoison Overlay") {
                stack.push((focused_win.as_ptr(), 1, 0));
                cf_arena.push(focused_win);
            }
        }

        if stack.is_empty() {
            return Ok(AccessibilitySnapshot {
                platform: self.platform_name().to_string(),
                elements,
                nodes_visited,
                max_depth_reached,
                elapsed: start.elapsed(),
                truncated_by_budget,
            });
        }

        while let Some((elem_ptr, depth, z_order)) = stack.pop() {
            if nodes_visited >= budget.max_nodes || start.elapsed() >= budget.timeout {
                truncated_by_budget = true;
                break;
            }

            nodes_visited += 1;
            if depth > max_depth_reached {
                max_depth_reached = depth;
            }

            let ax_role =
                Self::copy_string_attr(elem_ptr, "AXRole").unwrap_or_else(|| "AXUnknown".into());
            let ax_subrole = Self::copy_string_attr(elem_ptr, "AXSubrole");
            let role = map_ax_role(&ax_role, ax_subrole.as_deref());

            if let Some(bounds) = Self::copy_bounds(elem_ptr) {
                let is_clickable =
                    role.is_default_interactive() || Self::has_press_or_confirm_action(elem_ptr);
                let is_visible = bounds.width > 1.0 && bounds.height > 1.0;

                if is_visible && is_clickable {
                    let title = Self::copy_string_attr(elem_ptr, "AXTitle")
                        .filter(|s| !s.is_empty())
                        .or_else(|| {
                            Self::copy_string_attr(elem_ptr, "AXDescription")
                                .filter(|s| !s.is_empty())
                        });

                    let id = format!("macos-ax-{app_pid}-{nodes_visited}");
                    elements.push(AccessibleElement::new(
                        id,
                        role,
                        title,
                        bounds,
                        is_clickable,
                        is_visible,
                        z_order,
                        app_pid as u32,
                    ));
                }
            }

            if depth < budget.max_depth {
                // Prefer non-empty AXVisibleChildren to avoid walking thousands of off-screen rows,
                // but fall back to AXChildren whenever AXVisibleChildren is absent OR returns an empty array
                // (common for AXWindow, AXScrollArea, and WKWebView AXWebArea containers on macOS).
                let children_ref = Self::copy_attr(elem_ptr, "AXVisibleChildren")
                    .filter(|arr| unsafe { CFArrayGetCount(arr.as_ptr()) } > 0)
                    .or_else(|| Self::copy_attr(elem_ptr, "AXChildren"));

                if let Some(children_arr) = children_ref {
                    let child_count = unsafe { CFArrayGetCount(children_arr.as_ptr()) };
                    for c_idx in (0..child_count).rev() {
                        let child_ptr =
                            unsafe { CFArrayGetValueAtIndex(children_arr.as_ptr(), c_idx) };
                        if !child_ptr.is_null() {
                            stack.push((child_ptr, depth + 1, z_order));
                        }
                    }
                    cf_arena.push(children_arr);
                }
            } else {
                truncated_by_budget = true;
            }
        }

        Ok(AccessibilitySnapshot {
            platform: self.platform_name().to_string(),
            elements,
            nodes_visited,
            max_depth_reached,
            elapsed: start.elapsed(),
            truncated_by_budget,
        })
    }
}
