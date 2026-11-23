//! macOS Accessibility Tree adapter using `ApplicationServices` (`AXUIElement`) and `CoreGraphics`.
//!
//! Features:
//! - Permission verification and prompting via `AXIsProcessTrusted` / `AXIsProcessTrustedWithOptions`.
//! - Resilient multi-stage target application discovery:
//!   1. `AXUIElementCreateSystemWide` -> `AXFocusedApplication` (when it succeeds and resolves to a
//!      non-overlay window of a foreground application).
//!   2. `CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements)`
//!      fallback scanning normal-layer (`kCGWindowLayer == 0`) on-screen windows top-to-bottom when
//!      `AXFocusedApplication` returns `-25204` (`kAXErrorCannotComplete`) or points to RatPoison's own
//!      overlay process.
//! - Per-element messaging timeout (`AXUIElementSetMessagingTimeout`) so an unresponsive background
//!   helper or slow menu bar never stalls snapshot collection.
//! - Root collection across `AXWindows`, with automatic fallback to `AXFocusedWindow` and `AXMainWindow`
//!   when `AXWindows` is empty or contains only `"RatPoison Overlay"`, plus `AXMenuBar` scanning.
//! - Breadth-First Search (`VecDeque` BFS) traversal with reduced per-node Mach IPC overhead so top-level
//!   toolbars, tabs, and buttons are discovered before deep web/DOM subtrees exhaust the time budget.
//! - Automatic Chromium / Electron / WebKit accessibility activation (`AXEnhancedUserInterface = kCFBooleanTrue`,
//!   `AXManualAccessibility = kCFBooleanTrue`).

use crate::{
    A11yError, AccessibilityBudget, AccessibilityProvider, AccessibilitySnapshot,
    AccessibleElement, ElementRole, LogicalRect,
};
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;
use std::time::Instant;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFBooleanRef = *const c_void;
type CFNumberRef = *const c_void;
type CFAllocatorRef = *const c_void;
type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;
type AXError = i32;
type CFIndex = isize;
type CFStringEncoding = u32;
type CFNumberType = CFIndex;
type CGWindowListOption = u32;
type CGWindowID = u32;

const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;
const K_CF_NUMBER_SINT32_TYPE: CFNumberType = 3;
const K_CF_NUMBER_SINT64_TYPE: CFNumberType = 4;
const K_AX_ERROR_SUCCESS: AXError = 0;
const K_AX_VALUE_CGPOINT_TYPE: u32 = 1;
const K_AX_VALUE_CGSIZE_TYPE: u32 = 2;

const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: CGWindowListOption = 1 << 0;
const K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS: CGWindowListOption = 1 << 4;
const K_CG_NULL_WINDOW_ID: CGWindowID = 0;

/// Title of RatPoison's transparent click-through overlay window, which must never be scanned or block underlying apps.
pub const RATPOISON_OVERLAY_TITLE: &str = "RatPoison Overlay";

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
    fn CFDictionaryGetValue(the_dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(
        number: CFNumberRef,
        the_type: CFNumberType,
        value_ptr: *mut c_void,
    ) -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(
        option: CGWindowListOption,
        relative_to_window: CGWindowID,
    ) -> CFArrayRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;

    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
    fn AXUIElementSetMessagingTimeout(
        element: AXUIElementRef,
        timeout_in_seconds: f32,
    ) -> AXError;
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

unsafe fn cfdict_get_i32(dict: CFDictionaryRef, key: CFStringRef) -> Option<i32> {
    if dict.is_null() || key.is_null() {
        return None;
    }
    let val = CFDictionaryGetValue(dict, key);
    if val.is_null() {
        return None;
    }
    let mut out: i32 = 0;
    if CFNumberGetValue(
        val,
        K_CF_NUMBER_SINT32_TYPE,
        &mut out as *mut i32 as *mut c_void,
    ) {
        return Some(out);
    }
    let mut out64: i64 = 0;
    if CFNumberGetValue(
        val,
        K_CF_NUMBER_SINT64_TYPE,
        &mut out64 as *mut i64 as *mut c_void,
    ) {
        return Some(out64 as i32);
    }
    None
}

unsafe fn cfdict_get_string(dict: CFDictionaryRef, key: CFStringRef) -> Option<String> {
    if dict.is_null() || key.is_null() {
        return None;
    }
    let val = CFDictionaryGetValue(dict, key);
    if val.is_null() {
        return None;
    }
    cfstring_to_string(val)
}

/// Returns `true` if `ax_role` may use `AXSubrole` to refine its unified `ElementRole`
/// (e.g., `AXCloseButton`, `AXSearchField`, `AXTabButton`).
///
/// Skipping `AXSubrole` on roles that never refine `map_ax_role` saves one Mach IPC call per node.
pub fn should_query_ax_subrole(ax_role: &str) -> bool {
    matches!(
        ax_role,
        "AXButton" | "AXTextField" | "AXGroup" | "AXWindow" | "AXUnknown" | "AXRadioButton"
    )
}

/// Returns `true` if `ax_role` is a pure structural container that should not be treated as a
/// standalone clickable target and does not need an `AXUIElementCopyActionNames` Mach IPC call.
pub fn is_pure_structural_ax_role(ax_role: &str) -> bool {
    matches!(
        ax_role,
        "AXWindow"
            | "AXApplication"
            | "AXScrollArea"
            | "AXSplitGroup"
            | "AXToolbar"
            | "AXLayoutArea"
            | "AXWebArea"
            | "AXSheet"
            | "AXDrawer"
            | "AXGrowArea"
            | "AXScrollBar"
            | "AXSplitter"
            | "AXRuler"
            | "AXMenuBar"
            | "AXMenu"
            | "AXList"
            | "AXOutline"
            | "AXTable"
            | "AXBrowser"
    )
}

/// Returns `true` if `ax_role` is an atomic leaf control or text/image node that never hosts
/// separate actionable child controls in Label Mode.
///
/// Skipping `AXVisibleChildren` + `AXChildren` on leaf controls eliminates 2 wasted Mach IPC calls
/// per button, link, text field, static text, or image node.
pub fn should_skip_ax_children(ax_role: &str) -> bool {
    matches!(
        ax_role,
        "AXStaticText"
            | "AXImage"
            | "AXButton"
            | "AXCheckBox"
            | "AXRadioButton"
            | "AXTextField"
            | "AXTextArea"
            | "AXSlider"
            | "AXMenuItem"
            | "AXMenuBarItem"
            | "AXScrollBar"
            | "AXSplitter"
            | "AXGrowArea"
            | "AXValueIndicator"
            | "AXBusyIndicator"
            | "AXProgressIndicator"
    )
}

/// Returns `true` if `ax_role` generally requires querying `AXVisibleChildren` before `AXChildren`
/// (e.g., virtualized tables, lists, outlines, where `AXChildren` could contain thousands of off-screen rows).
/// For standard containers (`AXGroup`, `AXWindow`, `AXWebArea`, `AXToolbar`, `AXSplitGroup`),
/// querying `AXChildren` directly avoids a redundant failing `AXVisibleChildren` Mach IPC call.
pub fn should_prefer_visible_children(ax_role: &str) -> bool {
    matches!(
        ax_role,
        "AXList" | "AXOutline" | "AXTable" | "AXBrowser" | "AXGrid"
    )
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

/// Lightweight summary of an on-screen CoreGraphics window entry used for resilient PID discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnScreenWindowCandidate {
    pub owner_pid: i32,
    pub window_layer: i32,
    pub window_name: Option<String>,
    pub owner_name: Option<String>,
}

/// Selects ordered candidate application PIDs from a top-to-bottom list of on-screen windows.
///
/// Rules:
/// - Only normal desktop window layers (`window_layer == 0`) are considered.
/// - Windows titled `"RatPoison Overlay"` are always ignored.
/// - Windows belonging to `self_pid` are skipped unless a non-overlay window of `self_pid`
///   (such as RatPoison Settings) is genuinely the very first layer-0 window on screen.
/// - Preserves front-to-back order without duplicate PIDs.
pub fn select_target_pids_from_windows(
    windows: &[OnScreenWindowCandidate],
    self_pid: i32,
) -> Vec<i32> {
    let mut pids = Vec::new();
    let mut seen_first_layer0 = false;

    for win in windows {
        if win.window_layer != 0 || win.owner_pid <= 0 {
            continue;
        }
        if win.window_name.as_deref() == Some(RATPOISON_OVERLAY_TITLE) {
            continue;
        }

        let is_first_layer0 = !seen_first_layer0;
        seen_first_layer0 = true;

        if win.owner_pid == self_pid && !is_first_layer0 {
            continue;
        }

        if !pids.contains(&win.owner_pid) {
            pids.push(win.owner_pid);
        }
    }

    pids
}

/// macOS `AXUIElement` Accessibility Tree provider.
#[derive(Debug, Clone, Default)]
pub struct MacOSAccessibilityProvider {
    /// Optional target application PID override; when `None`, discovers the active foreground application(s).
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

    /// Queries CoreGraphics `CGWindowListCopyWindowInfo` for all on-screen windows in front-to-back order.
    pub fn query_on_screen_windows() -> Vec<OnScreenWindowCandidate> {
        let mut results = Vec::new();
        let options = K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY
            | K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS;
        let info_arr = unsafe { CGWindowListCopyWindowInfo(options, K_CG_NULL_WINDOW_ID) };
        let Some(arr_guard) = ScopedCFRef::new(info_arr) else {
            return results;
        };

        let Some(key_pid) = cfstring_from_str("kCGWindowOwnerPID") else {
            return results;
        };
        let Some(key_layer) = cfstring_from_str("kCGWindowLayer") else {
            return results;
        };
        let Some(key_name) = cfstring_from_str("kCGWindowName") else {
            return results;
        };
        let Some(key_owner) = cfstring_from_str("kCGWindowOwnerName") else {
            return results;
        };

        let count = unsafe { CFArrayGetCount(arr_guard.as_ptr()) };
        for idx in 0..count {
            let dict_ptr = unsafe { CFArrayGetValueAtIndex(arr_guard.as_ptr(), idx) };
            if dict_ptr.is_null() {
                continue;
            }
            let owner_pid = unsafe { cfdict_get_i32(dict_ptr, key_pid.as_ptr()).unwrap_or(0) };
            let window_layer =
                unsafe { cfdict_get_i32(dict_ptr, key_layer.as_ptr()).unwrap_or(-1) };
            let window_name = unsafe { cfdict_get_string(dict_ptr, key_name.as_ptr()) };
            let owner_name = unsafe { cfdict_get_string(dict_ptr, key_owner.as_ptr()) };

            results.push(OnScreenWindowCandidate {
                owner_pid,
                window_layer,
                window_name,
                owner_name,
            });
        }
        results
    }

    /// Discovers ordered candidate application PIDs to scan for Label Mode.
    ///
    /// 1. If `self.target_pid` is explicitly set, uses that PID.
    /// 2. Tries `AXUIElementCreateSystemWide()` -> `AXFocusedApplication`. If it succeeds and
    ///    resolves to a PID other than our own process (or our process has a visible non-overlay window),
    ///    places it first.
    /// 3. Queries `CGWindowListCopyWindowInfo` for top-to-bottom normal-layer (`kCGWindowLayer == 0`)
    ///    on-screen windows so that when `AXFocusedApplication` returns `-25204` (`kAXErrorCannotComplete`)
    ///    or points to RatPoison's overlay, we reliably target the frontmost active application(s).
    pub fn discover_candidate_pids(&self) -> Vec<i32> {
        if let Some(pid) = self.target_pid {
            return vec![pid];
        }

        let self_pid = std::process::id() as i32;
        let mut candidate_pids = Vec::new();

        // Stage 1: Try AXFocusedApplication via SystemWide element.
        let sys_ptr = unsafe { AXUIElementCreateSystemWide() };
        if let Some(sys_ref) = ScopedCFRef::new(sys_ptr) {
            unsafe {
                let _ = AXUIElementSetMessagingTimeout(sys_ref.as_ptr(), 0.04);
            }
            if let Some(focused_app) = Self::copy_attr(sys_ref.as_ptr(), "AXFocusedApplication") {
                let mut focused_pid: i32 = 0;
                unsafe {
                    let _ = AXUIElementGetPid(focused_app.as_ptr(), &mut focused_pid);
                }
                if focused_pid > 0
                    && (focused_pid != self_pid
                        || Self::app_has_non_overlay_window(focused_app.as_ptr()))
                {
                    candidate_pids.push(focused_pid);
                }
            }
        }

        // Stage 2: Discover top-to-bottom layer-0 on-screen application PIDs via CoreGraphics.
        let cg_windows = Self::query_on_screen_windows();
        for pid in select_target_pids_from_windows(&cg_windows, self_pid) {
            if !candidate_pids.contains(&pid) {
                candidate_pids.push(pid);
            }
        }

        candidate_pids
    }

    /// Checks whether an `AXApplication` element has at least one window that is not `"RatPoison Overlay"`.
    fn app_has_non_overlay_window(app_element: AXUIElementRef) -> bool {
        if let Some(win_arr) = Self::copy_attr(app_element, "AXWindows") {
            let count = unsafe { CFArrayGetCount(win_arr.as_ptr()) };
            for i in 0..count {
                let win_ptr = unsafe { CFArrayGetValueAtIndex(win_arr.as_ptr(), i) };
                if !win_ptr.is_null()
                    && Self::copy_string_attr(win_ptr, "AXTitle").as_deref()
                        != Some(RATPOISON_OVERLAY_TITLE)
                {
                    return true;
                }
            }
        }
        for attr in ["AXFocusedWindow", "AXMainWindow"] {
            if let Some(win_ref) = Self::copy_attr(app_element, attr) {
                if Self::copy_string_attr(win_ref.as_ptr(), "AXTitle").as_deref()
                    != Some(RATPOISON_OVERLAY_TITLE)
                {
                    return true;
                }
            }
        }
        false
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

    /// Collects root windows and menu bar elements for `app_ref` into the BFS queue.
    fn enqueue_app_roots(
        app_ref: AXUIElementRef,
        app_pid: i32,
        base_z_order: u32,
        include_menu_bar: bool,
        queue: &mut VecDeque<(AXUIElementRef, usize, u32, i32)>,
        cf_arena: &mut Vec<ScopedCFRef>,
    ) -> u32 {
        let mut window_roots_added = 0u32;

        // 1. Inspect AXWindows array in front-to-back order.
        if let Some(win_arr) = Self::copy_attr(app_ref, "AXWindows") {
            let win_count = unsafe { CFArrayGetCount(win_arr.as_ptr()) };
            for w_idx in 0..win_count {
                let win_ptr = unsafe { CFArrayGetValueAtIndex(win_arr.as_ptr(), w_idx) };
                if !win_ptr.is_null() {
                    let win_title = Self::copy_string_attr(win_ptr, "AXTitle");
                    if win_title.as_deref() == Some(RATPOISON_OVERLAY_TITLE) {
                        continue;
                    }
                    queue.push_back((
                        win_ptr,
                        1,
                        base_z_order + window_roots_added,
                        app_pid,
                    ));
                    window_roots_added += 1;
                }
            }
            cf_arena.push(win_arr);
        }

        // 2. Fallback: If AXWindows was absent, empty, or only contained "RatPoison Overlay",
        //    check AXFocusedWindow and then AXMainWindow.
        if window_roots_added == 0 {
            for fallback_attr in ["AXFocusedWindow", "AXMainWindow"] {
                if let Some(win_ref) = Self::copy_attr(app_ref, fallback_attr) {
                    let win_title = Self::copy_string_attr(win_ref.as_ptr(), "AXTitle");
                    if win_title.as_deref() != Some(RATPOISON_OVERLAY_TITLE) {
                        queue.push_back((
                            win_ref.as_ptr(),
                            1,
                            base_z_order + window_roots_added,
                            app_pid,
                        ));
                        cf_arena.push(win_ref);
                        window_roots_added += 1;
                        break;
                    }
                }
            }
        }

        // 3. Also scan the frontmost application's AXMenuBar so menu bar items are labeled.
        if include_menu_bar {
            if let Some(menu_bar) = Self::copy_attr(app_ref, "AXMenuBar") {
                queue.push_back((menu_bar.as_ptr(), 1, base_z_order, app_pid));
                cf_arena.push(menu_bar);
            }
        }

        window_roots_added
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
        let candidate_pids = self.discover_candidate_pids();

        let mut elements = Vec::new();
        let mut nodes_visited = 0usize;
        let mut max_depth_reached = 0usize;
        let mut truncated_by_budget = false;

        if candidate_pids.is_empty() {
            return Ok(AccessibilitySnapshot {
                platform: self.platform_name().to_string(),
                elements,
                nodes_visited,
                max_depth_reached,
                elapsed: start.elapsed(),
                truncated_by_budget,
            });
        }

        // Keep parent CFArrayRefs / AXUIElementRefs alive in `cf_arena` during traversal.
        let mut cf_arena: Vec<ScopedCFRef> = Vec::new();
        // Breadth-first queue holding `(element_ptr, depth, z_order, app_pid)`.
        let mut queue: VecDeque<(AXUIElementRef, usize, u32, i32)> = VecDeque::new();
        let mut running_z_order = 0u32;

        // Inspect up to the top 2 visible candidate applications (frontmost app + next visible app for
        // split-screen or desktop fallback when the frontmost app has no standard windows).
        for (app_idx, &app_pid) in candidate_pids.iter().take(2).enumerate() {
            if app_idx > 0 && !elements.is_empty() && elements.len() >= 12 {
                // The primary frontmost application already yielded plenty of interactive elements.
                break;
            }

            let app_ptr = unsafe { AXUIElementCreateApplication(app_pid) };
            let Some(app_ref) = ScopedCFRef::new(app_ptr) else {
                continue;
            };

            // Cap individual AX IPC calls to 40ms so a slow/hung element never stalls the traversal.
            unsafe {
                let _ = AXUIElementSetMessagingTimeout(app_ref.as_ptr(), 0.04);
            }

            if self.enable_enhanced_ui {
                Self::activate_chromium_electron_a11y(app_ref.as_ptr());
            }

            let added = Self::enqueue_app_roots(
                app_ref.as_ptr(),
                app_pid,
                running_z_order,
                app_idx == 0,
                &mut queue,
                &mut cf_arena,
            );
            running_z_order = running_z_order.saturating_add(added.max(1));
            cf_arena.push(app_ref);

            while let Some((elem_ptr, depth, z_order, owner_pid)) = queue.pop_front() {
                if nodes_visited >= budget.max_nodes || start.elapsed() >= budget.timeout {
                    truncated_by_budget = true;
                    break;
                }

                nodes_visited += 1;
                if depth > max_depth_reached {
                    max_depth_reached = depth;
                }

                let ax_role = Self::copy_string_attr(elem_ptr, "AXRole")
                    .unwrap_or_else(|| "AXUnknown".into());
                let ax_subrole = if should_query_ax_subrole(&ax_role) {
                    Self::copy_string_attr(elem_ptr, "AXSubrole")
                } else {
                    None
                };
                let role = map_ax_role(&ax_role, ax_subrole.as_deref());

                // Avoid calling AXUIElementCopyActionNames on pure structural containers
                // (AXWindow, AXScrollArea, AXSplitGroup, AXToolbar, AXWebArea, etc.) or static text/images,
                // and avoid querying AXPosition + AXSize unless the node is actually clickable.
                let is_clickable = if role.is_default_interactive() {
                    true
                } else if is_pure_structural_ax_role(&ax_role)
                    || matches!(ax_role.as_str(), "AXStaticText" | "AXImage")
                {
                    false
                } else {
                    Self::has_press_or_confirm_action(elem_ptr)
                };

                if is_clickable {
                    if let Some(bounds) = Self::copy_bounds(elem_ptr) {
                        let is_visible = bounds.width > 1.0 && bounds.height > 1.0;
                        if is_visible {
                            let title = Self::copy_string_attr(elem_ptr, "AXTitle")
                                .filter(|s| !s.is_empty())
                                .or_else(|| {
                                    Self::copy_string_attr(elem_ptr, "AXDescription")
                                        .filter(|s| !s.is_empty())
                                });

                            let id = format!("macos-ax-{owner_pid}-{nodes_visited}");
                            elements.push(AccessibleElement::new(
                                id,
                                role,
                                title,
                                bounds,
                                is_clickable,
                                is_visible,
                                z_order,
                                owner_pid as u32,
                            ));
                        }
                    }
                }

                // Skip expanding children on leaf controls (`AXButton`, `AXStaticText`, `AXMenuBarItem`, etc.).
                if depth < budget.max_depth && !should_skip_ax_children(&ax_role) {
                    let children_ref = if should_prefer_visible_children(&ax_role) {
                        Self::copy_attr(elem_ptr, "AXVisibleChildren")
                            .filter(|arr| unsafe { CFArrayGetCount(arr.as_ptr()) } > 0)
                            .or_else(|| Self::copy_attr(elem_ptr, "AXChildren"))
                    } else {
                        Self::copy_attr(elem_ptr, "AXChildren")
                    };

                    if let Some(children_arr) = children_ref {
                        let child_count = unsafe { CFArrayGetCount(children_arr.as_ptr()) };
                        for c_idx in 0..child_count {
                            let child_ptr =
                                unsafe { CFArrayGetValueAtIndex(children_arr.as_ptr(), c_idx) };
                            if !child_ptr.is_null() {
                                queue.push_back((child_ptr, depth + 1, z_order, owner_pid));
                            }
                        }
                        cf_arena.push(children_arr);
                    }
                } else if depth >= budget.max_depth {
                    truncated_by_budget = true;
                }
            }

            if truncated_by_budget {
                break;
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
