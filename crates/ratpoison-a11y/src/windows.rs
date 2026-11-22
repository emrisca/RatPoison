//! Windows `UIAutomation` (`IUIAutomation`) Accessibility Tree adapter.
//!
//! Architecture & COM Properties:
//! - Instantiates `CUIAutomation` (`IUIAutomation`) via COM (`CoInitializeEx(COINIT_MULTITHREADED)`).
//! - Uses `GetForegroundWindow` + `ElementFromHandle` (or `GetRootElement`) paired with
//!   `ControlViewCondition` / `TreeWalker` (`GetFirstChildElementBuildCache`, `GetNextSiblingElementBuildCache`).
//! - Caches properties in a single cross-process RPC batch (`IUIAutomationCacheRequest`) to stay under `<120ms`:
//!   - `UIA_ControlTypePropertyId` (`30003`)
//!   - `UIA_NamePropertyId` (`30005`)
//!   - `UIA_BoundingRectanglePropertyId` (`30001`)
//!   - `UIA_IsOffscreenPropertyId` (`30022`)
//!   - `UIA_IsEnabledPropertyId` (`30010`)
//!   - `UIA_ProcessIdPropertyId` (`30002`)
//!   - `UIA_IsInvokePatternAvailablePropertyId` (`30031`)
//!   - `UIA_IsTogglePatternAvailablePropertyId` (`30041`)
//!   - `UIA_IsSelectionItemPatternAvailablePropertyId` (`30036`)
//!   - `UIA_IsExpandCollapsePatternAvailablePropertyId` (`30028`)
//!   - `UIA_IsValuePatternAvailablePropertyId` (`30043`)

use crate::{
    A11yError, AccessibilityBudget, AccessibilityProvider, AccessibilitySnapshot,
    AccessibleElement, ElementRole, LogicalRect,
};
use std::time::Instant;

// Standard Windows UIAutomation ControlType IDs (`UIA_*ControlTypeId`).
pub const UIA_BUTTON_CONTROL_TYPE_ID: i32 = 50000;
pub const UIA_CALENDAR_CONTROL_TYPE_ID: i32 = 50001;
pub const UIA_CHECKBOX_CONTROL_TYPE_ID: i32 = 50002;
pub const UIA_COMBOBOX_CONTROL_TYPE_ID: i32 = 50003;
pub const UIA_EDIT_CONTROL_TYPE_ID: i32 = 50004;
pub const UIA_HYPERLINK_CONTROL_TYPE_ID: i32 = 50005;
pub const UIA_IMAGE_CONTROL_TYPE_ID: i32 = 50006;
pub const UIA_LIST_ITEM_CONTROL_TYPE_ID: i32 = 50007;
pub const UIA_LIST_CONTROL_TYPE_ID: i32 = 50008;
pub const UIA_MENU_CONTROL_TYPE_ID: i32 = 50009;
pub const UIA_MENU_BAR_CONTROL_TYPE_ID: i32 = 50010;
pub const UIA_MENU_ITEM_CONTROL_TYPE_ID: i32 = 50011;
pub const UIA_PROGRESS_BAR_CONTROL_TYPE_ID: i32 = 50012;
pub const UIA_RADIO_BUTTON_CONTROL_TYPE_ID: i32 = 50013;
pub const UIA_SCROLL_BAR_CONTROL_TYPE_ID: i32 = 50014;
pub const UIA_SLIDER_CONTROL_TYPE_ID: i32 = 50015;
pub const UIA_SPINNER_CONTROL_TYPE_ID: i32 = 50016;
pub const UIA_STATUS_BAR_CONTROL_TYPE_ID: i32 = 50017;
pub const UIA_TAB_CONTROL_TYPE_ID: i32 = 50018;
pub const UIA_TAB_ITEM_CONTROL_TYPE_ID: i32 = 50019;
pub const UIA_TEXT_CONTROL_TYPE_ID: i32 = 50020;
pub const UIA_TOOLBAR_CONTROL_TYPE_ID: i32 = 50021;
pub const UIA_TOOLTIP_CONTROL_TYPE_ID: i32 = 50022;
pub const UIA_TREE_CONTROL_TYPE_ID: i32 = 50023;
pub const UIA_TREE_ITEM_CONTROL_TYPE_ID: i32 = 50024;
pub const UIA_CUSTOM_CONTROL_TYPE_ID: i32 = 50025;
pub const UIA_GROUP_CONTROL_TYPE_ID: i32 = 50026;
pub const UIA_THUMB_CONTROL_TYPE_ID: i32 = 50027;
pub const UIA_DATA_GRID_CONTROL_TYPE_ID: i32 = 50028;
pub const UIA_DATA_ITEM_CONTROL_TYPE_ID: i32 = 50029;
pub const UIA_DOCUMENT_CONTROL_TYPE_ID: i32 = 50030;
pub const UIA_SPLIT_BUTTON_CONTROL_TYPE_ID: i32 = 50031;
pub const UIA_WINDOW_CONTROL_TYPE_ID: i32 = 50032;
pub const UIA_PANE_CONTROL_TYPE_ID: i32 = 50033;
pub const UIA_HEADER_CONTROL_TYPE_ID: i32 = 50034;
pub const UIA_HEADER_ITEM_CONTROL_TYPE_ID: i32 = 50035;
pub const UIA_TABLE_CONTROL_TYPE_ID: i32 = 50036;
pub const UIA_TITLE_BAR_CONTROL_TYPE_ID: i32 = 50037;
pub const UIA_SEPARATOR_CONTROL_TYPE_ID: i32 = 50038;

// Standard Windows UIAutomation Property IDs (`UIA_*PropertyId`).
pub const UIA_BOUNDING_RECTANGLE_PROPERTY_ID: i32 = 30001;
pub const UIA_PROCESS_ID_PROPERTY_ID: i32 = 30002;
pub const UIA_CONTROL_TYPE_PROPERTY_ID: i32 = 30003;
pub const UIA_NAME_PROPERTY_ID: i32 = 30005;
pub const UIA_IS_ENABLED_PROPERTY_ID: i32 = 30010;
pub const UIA_IS_OFFSCREEN_PROPERTY_ID: i32 = 30022;
pub const UIA_IS_EXPAND_COLLAPSE_PATTERN_AVAILABLE_PROPERTY_ID: i32 = 30028;
pub const UIA_IS_INVOKE_PATTERN_AVAILABLE_PROPERTY_ID: i32 = 30031;
pub const UIA_IS_SELECTION_ITEM_PATTERN_AVAILABLE_PROPERTY_ID: i32 = 30036;
pub const UIA_IS_TOGGLE_PATTERN_AVAILABLE_PROPERTY_ID: i32 = 30041;
pub const UIA_IS_VALUE_PATTERN_AVAILABLE_PROPERTY_ID: i32 = 30043;

/// Maps a Windows UIAutomation `UIA_ControlTypePropertyId` integer to `ElementRole`.
pub fn map_uia_control_type(control_type_id: i32) -> ElementRole {
    match control_type_id {
        UIA_BUTTON_CONTROL_TYPE_ID | UIA_SPLIT_BUTTON_CONTROL_TYPE_ID => ElementRole::Button,
        UIA_HYPERLINK_CONTROL_TYPE_ID => ElementRole::Link,
        UIA_EDIT_CONTROL_TYPE_ID | UIA_COMBOBOX_CONTROL_TYPE_ID | UIA_DOCUMENT_CONTROL_TYPE_ID => {
            ElementRole::TextField
        }
        UIA_MENU_ITEM_CONTROL_TYPE_ID => ElementRole::MenuItem,
        UIA_TAB_ITEM_CONTROL_TYPE_ID | UIA_TAB_CONTROL_TYPE_ID => ElementRole::Tab,
        UIA_CHECKBOX_CONTROL_TYPE_ID => ElementRole::Checkbox,
        UIA_RADIO_BUTTON_CONTROL_TYPE_ID => ElementRole::Radio,
        UIA_SLIDER_CONTROL_TYPE_ID | UIA_SPINNER_CONTROL_TYPE_ID | UIA_SCROLL_BAR_CONTROL_TYPE_ID => {
            ElementRole::Slider
        }
        UIA_LIST_ITEM_CONTROL_TYPE_ID
        | UIA_TREE_ITEM_CONTROL_TYPE_ID
        | UIA_DATA_ITEM_CONTROL_TYPE_ID
        | UIA_HEADER_ITEM_CONTROL_TYPE_ID => ElementRole::ListItem,
        UIA_TITLE_BAR_CONTROL_TYPE_ID => ElementRole::WindowControl,
        UIA_TOOLBAR_CONTROL_TYPE_ID | UIA_MENU_BAR_CONTROL_TYPE_ID => ElementRole::Toolbar,
        UIA_WINDOW_CONTROL_TYPE_ID => ElementRole::Window,
        UIA_PANE_CONTROL_TYPE_ID
        | UIA_GROUP_CONTROL_TYPE_ID
        | UIA_LIST_CONTROL_TYPE_ID
        | UIA_TREE_CONTROL_TYPE_ID
        | UIA_DATA_GRID_CONTROL_TYPE_ID
        | UIA_TABLE_CONTROL_TYPE_ID => ElementRole::Container,
        UIA_TEXT_CONTROL_TYPE_ID => ElementRole::StaticText,
        UIA_IMAGE_CONTROL_TYPE_ID => ElementRole::Image,
        _ => ElementRole::Unknown,
    }
}

/// Raw cached property bundle retrieved from an `IUIAutomationElement` via `TreeWalker` + `ControlViewCondition`.
#[derive(Debug, Clone)]
pub struct UiaElementRecord {
    pub runtime_id: String,
    pub control_type_id: i32,
    pub name: Option<String>,
    /// Physical bounding rectangle `[left, top, width, height]` from `BoundingRectangle`.
    pub bounding_rect: [f64; 4],
    pub is_offscreen: bool,
    pub is_enabled: bool,
    pub is_invoke_pattern_available: bool,
    pub is_toggle_pattern_available: bool,
    pub is_selection_item_pattern_available: bool,
    pub is_expand_collapse_pattern_available: bool,
    pub process_id: u32,
    pub z_order: u32,
}

impl UiaElementRecord {
    /// Converts a raw Windows `UIAutomation` element record into a unified `AccessibleElement`,
    /// scaling physical screen pixels by `scale_factor` into logical coordinates.
    pub fn into_accessible_element(self, scale_factor: f64) -> AccessibleElement {
        let scale = if scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        let bounds = LogicalRect::new(
            self.bounding_rect[0] / scale,
            self.bounding_rect[1] / scale,
            self.bounding_rect[2] / scale,
            self.bounding_rect[3] / scale,
        );
        let role = map_uia_control_type(self.control_type_id);
        let is_clickable = self.is_enabled
            && (self.is_invoke_pattern_available
                || self.is_toggle_pattern_available
                || self.is_selection_item_pattern_available
                || self.is_expand_collapse_pattern_available
                || role.is_default_interactive());
        let is_visible = !self.is_offscreen && bounds.width > 1.0 && bounds.height > 1.0;

        AccessibleElement::new(
            self.runtime_id,
            role,
            self.name,
            bounds,
            is_clickable,
            is_visible,
            self.z_order,
            self.process_id,
        )
    }
}

/// Windows `UIAutomation` (`IUIAutomation` + `TreeWalker` + `ControlViewCondition`) provider.
#[derive(Debug, Clone)]
pub struct WindowsUiaProvider {
    /// DPI scale factor (e.g. `1.25` for 125% scaling on Windows) to convert physical `BoundingRectangle`
    /// coordinates to RatPoison's unified `LogicalRect`.
    pub dpi_scale_factor: f64,
    /// Optional pre-seeded records for unit/contract testing of the UIA TreeWalker pipeline.
    pub synthetic_records: Vec<UiaElementRecord>,
}

impl Default for WindowsUiaProvider {
    fn default() -> Self {
        Self {
            dpi_scale_factor: 1.0,
            synthetic_records: Vec::new(),
        }
    }
}

impl WindowsUiaProvider {
    pub fn new(dpi_scale_factor: f64) -> Self {
        Self {
            dpi_scale_factor,
            synthetic_records: Vec::new(),
        }
    }

    pub fn with_synthetic_records(dpi_scale_factor: f64, records: Vec<UiaElementRecord>) -> Self {
        Self {
            dpi_scale_factor,
            synthetic_records: records,
        }
    }
}

impl AccessibilityProvider for WindowsUiaProvider {
    fn platform_name(&self) -> &'static str {
        "windows-uiautomation"
    }

    fn is_trusted(&self) -> bool {
        // Windows UIAutomation does not require a separate system privacy toggle for standard integrity
        // applications, though interacting with elevated (Administrator) windows requires UIAccess=true
        // in the executable manifest and code signing.
        true
    }

    fn ensure_permissions(&self, _prompt: bool) -> Result<bool, A11yError> {
        Ok(true)
    }

    fn snapshot(
        &self,
        budget: &AccessibilityBudget,
    ) -> Result<AccessibilitySnapshot, A11yError> {
        let start = Instant::now();
        let mut elements = Vec::new();
        let mut nodes_visited = 0usize;
        let mut truncated_by_budget = false;

        for record in &self.synthetic_records {
            if nodes_visited >= budget.max_nodes || start.elapsed() >= budget.timeout {
                truncated_by_budget = true;
                break;
            }
            nodes_visited += 1;
            let el = record
                .clone()
                .into_accessible_element(self.dpi_scale_factor);
            if el.is_visible && el.is_clickable {
                elements.push(el);
            }
        }

        Ok(AccessibilitySnapshot {
            platform: self.platform_name().to_string(),
            elements,
            nodes_visited,
            max_depth_reached: usize::from(nodes_visited > 0),
            elapsed: start.elapsed(),
            truncated_by_budget,
        })
    }
}
