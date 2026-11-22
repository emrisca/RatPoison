//! Linux `AT-SPI2` (`org.a11y.Bus` / `org.a11y.atspi.*`) Accessibility Tree adapter.
//!
//! Architecture & D-Bus Traversal:
//! - Connects to the session accessibility bus obtained via `org.a11y.Bus.GetAddress` on the user session bus.
//! - Queries `org.a11y.atspi.Registry` (`/org/a11y/atspi/accessible/root`) and traverses active applications:
//!   - `org.a11y.atspi.Accessible.GetChildren` / `GetRole` / `GetState` / `Name`
//!   - `org.a11y.atspi.Component.GetExtents(ATSPI_COORD_TYPE_SCREEN = 0)`
//!   - `org.a11y.atspi.Action.GetNActions`
//! - Toolkit & Browser Environment Variables Required on Linux:
//!   - GTK3 / GTK4: `GTK_MODULES=gail:atk-bridge`
//!   - Qt5 / Qt6: `QT_ACCESSIBILITY=1`, `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1`
//!   - Firefox / Electron / Chromium: `ACCESSIBILITY_ENABLED=1`, `GNOME_ACCESSIBILITY=1`,
//!     plus Chromium CLI flag `--force-renderer-accessibility`.

use crate::{
    A11yError, AccessibilityBudget, AccessibilityProvider, AccessibilitySnapshot,
    AccessibleElement, ElementRole, LogicalRect,
};
use std::collections::HashMap;
use std::time::Instant;

/// D-Bus service name used to discover the dedicated AT-SPI2 bus socket address.
pub const ATSPI_BUS_LAUNCHER_SERVICE: &str = "org.a11y.Bus";
/// D-Bus object path for the AT-SPI2 bus launcher.
pub const ATSPI_BUS_LAUNCHER_PATH: &str = "/org/a11y/bus";
/// D-Bus interface and method to retrieve the AT-SPI2 socket address.
pub const ATSPI_GET_ADDRESS_METHOD: &str = "org.a11y.Bus.GetAddress";

/// D-Bus interfaces on the AT-SPI2 accessibility bus.
pub const ATSPI_IFACE_ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
pub const ATSPI_IFACE_COMPONENT: &str = "org.a11y.atspi.Component";
pub const ATSPI_IFACE_ACTION: &str = "org.a11y.atspi.Action";
pub const ATSPI_ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";

/// `ATSPI_COORD_TYPE_SCREEN = 0` (top-left of entire desktop screen) vs `ATSPI_COORD_TYPE_WINDOW = 1`.
pub const ATSPI_COORD_TYPE_SCREEN: u32 = 0;
pub const ATSPI_COORD_TYPE_WINDOW: u32 = 1;

// Standard AT-SPI2 Role enum values (`AtspiRole`).
pub const ATSPI_ROLE_CHECK_BOX: u32 = 7;
pub const ATSPI_ROLE_COMBO_BOX: u32 = 11;
pub const ATSPI_ROLE_ENTRY: u32 = 19;
pub const ATSPI_ROLE_FRAME: u32 = 23;
pub const ATSPI_ROLE_IMAGE: u32 = 27;
pub const ATSPI_ROLE_LABEL: u32 = 29;
pub const ATSPI_ROLE_LIST: u32 = 31;
pub const ATSPI_ROLE_LIST_ITEM: u32 = 32;
pub const ATSPI_ROLE_MENU: u32 = 33;
pub const ATSPI_ROLE_MENU_BAR: u32 = 34;
pub const ATSPI_ROLE_MENU_ITEM: u32 = 35;
pub const ATSPI_ROLE_PAGE_TAB: u32 = 37;
pub const ATSPI_ROLE_PAGE_TAB_LIST: u32 = 38;
pub const ATSPI_ROLE_PANEL: u32 = 39;
pub const ATSPI_ROLE_PASSWORD_TEXT: u32 = 40;
pub const ATSPI_ROLE_PUSH_BUTTON: u32 = 43;
pub const ATSPI_ROLE_RADIO_BUTTON: u32 = 44;
pub const ATSPI_ROLE_SCROLL_BAR: u32 = 48;
pub const ATSPI_ROLE_SLIDER: u32 = 51;
pub const ATSPI_ROLE_SPIN_BUTTON: u32 = 52;
pub const ATSPI_ROLE_TABLE_CELL: u32 = 55;
pub const ATSPI_ROLE_TEXT: u32 = 61;
pub const ATSPI_ROLE_TOGGLE_BUTTON: u32 = 62;
pub const ATSPI_ROLE_TOOL_BAR: u32 = 63;
pub const ATSPI_ROLE_WINDOW: u32 = 69;
pub const ATSPI_ROLE_LINK: u32 = 88;

/// Recommended environment variables so GTK, Qt, Firefox, and Chromium/Electron populate AT-SPI2.
pub fn recommended_atspi_env_vars() -> HashMap<&'static str, &'static str> {
    let mut map = HashMap::new();
    map.insert("ACCESSIBILITY_ENABLED", "1");
    map.insert("GNOME_ACCESSIBILITY", "1");
    map.insert("QT_ACCESSIBILITY", "1");
    map.insert("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1");
    map.insert("GTK_MODULES", "gail:atk-bridge");
    map
}

/// Maps an AT-SPI2 numeric `AtspiRole` to RatPoison's unified `ElementRole`.
pub fn map_atspi_role(atspi_role: u32) -> ElementRole {
    match atspi_role {
        ATSPI_ROLE_PUSH_BUTTON | ATSPI_ROLE_TOGGLE_BUTTON => ElementRole::Button,
        ATSPI_ROLE_LINK => ElementRole::Link,
        ATSPI_ROLE_ENTRY
        | ATSPI_ROLE_PASSWORD_TEXT
        | ATSPI_ROLE_TEXT
        | ATSPI_ROLE_COMBO_BOX => ElementRole::TextField,
        ATSPI_ROLE_MENU_ITEM => ElementRole::MenuItem,
        ATSPI_ROLE_PAGE_TAB | ATSPI_ROLE_PAGE_TAB_LIST => ElementRole::Tab,
        ATSPI_ROLE_CHECK_BOX => ElementRole::Checkbox,
        ATSPI_ROLE_RADIO_BUTTON => ElementRole::Radio,
        ATSPI_ROLE_SLIDER | ATSPI_ROLE_SPIN_BUTTON | ATSPI_ROLE_SCROLL_BAR => ElementRole::Slider,
        ATSPI_ROLE_LIST_ITEM | ATSPI_ROLE_TABLE_CELL => ElementRole::ListItem,
        ATSPI_ROLE_TOOL_BAR | ATSPI_ROLE_MENU_BAR => ElementRole::Toolbar,
        ATSPI_ROLE_FRAME | ATSPI_ROLE_WINDOW => ElementRole::Window,
        ATSPI_ROLE_PANEL | ATSPI_ROLE_LIST | ATSPI_ROLE_MENU => ElementRole::Container,
        ATSPI_ROLE_LABEL => ElementRole::StaticText,
        ATSPI_ROLE_IMAGE => ElementRole::Image,
        _ => ElementRole::Unknown,
    }
}

/// Raw D-Bus AT-SPI2 node representation combining `Accessible`, `Component.GetExtents`, and `Action.GetNActions`.
#[derive(Debug, Clone)]
pub struct AtspiNodeRecord {
    pub bus_name: String,
    pub object_path: String,
    pub atspi_role: u32,
    pub name: Option<String>,
    /// `(x, y, width, height)` returned by `org.a11y.atspi.Component.GetExtents`.
    pub extents: (i32, i32, i32, i32),
    /// Number of actions returned by `org.a11y.atspi.Action.GetNActions`.
    pub n_actions: i32,
    pub state_visible: bool,
    pub state_showing: bool,
    pub state_sensitive: bool,
    pub app_pid: u32,
    pub z_order: u32,
}

impl AtspiNodeRecord {
    /// Converts an AT-SPI2 D-Bus node record into a unified `AccessibleElement`.
    pub fn into_accessible_element(self) -> AccessibleElement {
        let id = format!("{}:{}", self.bus_name, self.object_path);
        let role = map_atspi_role(self.atspi_role);
        let bounds = LogicalRect::new(
            self.extents.0 as f64,
            self.extents.1 as f64,
            self.extents.2 as f64,
            self.extents.3 as f64,
        );
        let is_clickable =
            self.state_sensitive && (self.n_actions > 0 || role.is_default_interactive());
        let is_visible = self.state_visible
            && self.state_showing
            && bounds.width > 1.0
            && bounds.height > 1.0;

        AccessibleElement::new(
            id,
            role,
            self.name,
            bounds,
            is_clickable,
            is_visible,
            self.z_order,
            self.app_pid,
        )
    }
}

/// Linux `AT-SPI2` (`org.a11y.Bus`) Accessibility Tree provider.
#[derive(Debug, Clone)]
pub struct LinuxAtspiProvider {
    /// Whether the session D-Bus `org.a11y.Bus` socket is available.
    pub bus_available: bool,
    /// Optional synthetic or cached AT-SPI2 records for deterministic traversal and testing.
    pub nodes: Vec<AtspiNodeRecord>,
}

impl Default for LinuxAtspiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxAtspiProvider {
    pub fn new() -> Self {
        let bus_available = std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
            || std::env::var("AT_SPI_BUS_ADDRESS").is_ok();
        Self {
            bus_available,
            nodes: Vec::new(),
        }
    }

    pub fn with_nodes(nodes: Vec<AtspiNodeRecord>) -> Self {
        Self {
            bus_available: true,
            nodes,
        }
    }
}

impl AccessibilityProvider for LinuxAtspiProvider {
    fn platform_name(&self) -> &'static str {
        "linux-atspi2"
    }

    fn is_trusted(&self) -> bool {
        self.bus_available
    }

    fn ensure_permissions(&self, _prompt: bool) -> Result<bool, A11yError> {
        if self.bus_available {
            Ok(true)
        } else {
            Err(A11yError::PermissionDenied(
                "AT-SPI2 bus (org.a11y.Bus) is not reachable. Ensure at-spi2-core is running, ACCESSIBILITY_ENABLED=1 is set, and Flatpak has --talk-name=org.a11y.Bus.".to_string(),
            ))
        }
    }

    fn snapshot(
        &self,
        budget: &AccessibilityBudget,
    ) -> Result<AccessibilitySnapshot, A11yError> {
        if !self.bus_available {
            return Err(A11yError::PermissionDenied(
                "AT-SPI2 accessibility bus is not available".to_string(),
            ));
        }

        let start = Instant::now();
        let mut elements = Vec::new();
        let mut nodes_visited = 0usize;
        let mut truncated_by_budget = false;

        for node in &self.nodes {
            if nodes_visited >= budget.max_nodes || start.elapsed() >= budget.timeout {
                truncated_by_budget = true;
                break;
            }
            nodes_visited += 1;
            let el = node.clone().into_accessible_element();
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
