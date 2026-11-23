//! Cross-platform OS Accessibility Tree parser and Label Mode badge generator for RatPoison.
//!
//! Provides:
//! - Unified `AccessibleElement`, `ElementRole`, `LogicalPoint`, `LogicalRect`, and `PhysicalPoint` types
//!   matching the RatPoison Swarm Contract.
//! - `AccessibilityProvider` trait with strict depth (`max_depth`), node-count (`max_nodes`), and
//!   wall-clock timeout budgets so massive Electron/browser DOM trees never stall Label Mode.
//! - Deterministic Label Mode badge generator (`labels`) with off-screen/zero-area/occlusion pruning,
//!   bounding-box IoU & centroid deduplication, and prefix-free home-row-first 1–2 character hint assignment.
//! - Platform adapters for macOS (`AXUIElement`), Windows (`UIAutomation`), Linux (`AT-SPI2`), and
//!   a deterministic `mock` provider.

pub mod labels;
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod mock;
pub mod windows;

pub use labels::{
    generate_label_badges, generate_prefix_free_hints, index_badges, match_typed_hint,
    HintMatchResult, LabelBadge, LabelConfig, DEFAULT_HINT_KEYS,
};
pub use mock::{MockAccessibilityProvider, MockTreeNode};

use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

/// Logical 2D coordinate in screen points (`f64`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

impl LogicalPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Euclidean distance to another `LogicalPoint`.
    pub fn distance_to(&self, other: &Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    /// Converts logical coordinates to physical screen pixels using the monitor `scale_factor`.
    pub fn to_physical(&self, scale_factor: f64) -> PhysicalPoint {
        PhysicalPoint {
            x: (self.x * scale_factor).round() as i32,
            y: (self.y * scale_factor).round() as i32,
        }
    }
}

/// Physical 2D pixel coordinate (`i32`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PhysicalPoint {
    pub x: i32,
    pub y: i32,
}

impl PhysicalPoint {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// Logical axis-aligned bounding rectangle in screen points (`f64`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Type alias matching the `bounds: Rect` field convention in `AccessibleElement`.
pub type Rect = LogicalRect;

impl LogicalRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Area of the rectangle (`0.0` if width or height is non-positive).
    pub fn area(&self) -> f64 {
        if self.width <= 0.0 || self.height <= 0.0 {
            0.0
        } else {
            self.width * self.height
        }
    }

    /// Center point of the rectangle.
    pub fn center(&self) -> LogicalPoint {
        LogicalPoint {
            x: self.x + self.width * 0.5,
            y: self.y + self.height * 0.5,
        }
    }

    /// Returns `true` if `point` lies inside or on the boundary of this rectangle.
    pub fn contains_point(&self, point: &LogicalPoint) -> bool {
        if self.width <= 0.0 || self.height <= 0.0 {
            return false;
        }
        point.x >= self.x
            && point.x <= (self.x + self.width)
            && point.y >= self.y
            && point.y <= (self.y + self.height)
    }

    /// Computes the overlapping rectangle between `self` and `other`, if any.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        if self.width <= 0.0 || self.height <= 0.0 || other.width <= 0.0 || other.height <= 0.0 {
            return None;
        }
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width).min(other.x + other.width);
        let y2 = (self.y + self.height).min(other.y + other.height);

        let w = x2 - x1;
        let h = y2 - y1;
        if w > 0.0 && h > 0.0 {
            Some(Self::new(x1, y1, w, h))
        } else {
            None
        }
    }

    /// Computes the Intersection-over-Union (IoU) ratio in `[0.0, 1.0]`.
    pub fn iou(&self, other: &Self) -> f64 {
        let Some(inter) = self.intersection(other) else {
            return 0.0;
        };
        let inter_area = inter.area();
        let union_area = self.area() + other.area() - inter_area;
        if union_area <= 0.0 {
            0.0
        } else {
            (inter_area / union_area).clamp(0.0, 1.0)
        }
    }
}

/// Unified semantic role of an OS or browser accessibility element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ElementRole {
    Button,
    Link,
    TextField,
    MenuItem,
    Tab,
    Checkbox,
    Radio,
    Slider,
    ListItem,
    WindowControl,
    Toolbar,
    Window,
    Container,
    StaticText,
    Image,
    Unknown,
}

impl ElementRole {
    /// Returns `true` if this role is intrinsically interactive/clickable in Label Mode.
    pub const fn is_default_interactive(&self) -> bool {
        matches!(
            self,
            Self::Button
                | Self::Link
                | Self::TextField
                | Self::MenuItem
                | Self::Tab
                | Self::Checkbox
                | Self::Radio
                | Self::Slider
                | Self::ListItem
                | Self::WindowControl
        )
    }
}

/// Unified representation of an actionable element discovered in the OS Accessibility Tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessibleElement {
    /// Platform-unique or snapshot-unique identifier.
    pub id: String,
    /// Unified semantic role.
    pub role: ElementRole,
    /// Human-readable label, title, or accessible description.
    pub title: Option<String>,
    /// Logical screen bounding box (`Rect`).
    pub bounds: Rect,
    /// Logical center point for click targeting.
    pub center: LogicalPoint,
    /// Whether the element exposes a click/press/invoke pattern or interactive role.
    pub is_clickable: bool,
    /// Whether the element is currently visible on screen.
    pub is_visible: bool,
    /// Window stacking order (`0` = frontmost active window; higher values = background layers).
    pub z_order: u32,
    /// Owning application process ID.
    pub app_pid: u32,
}

impl AccessibleElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        role: ElementRole,
        title: Option<String>,
        bounds: Rect,
        is_clickable: bool,
        is_visible: bool,
        z_order: u32,
        app_pid: u32,
    ) -> Self {
        let center = bounds.center();
        Self {
            id: id.into(),
            role,
            title,
            bounds,
            center,
            is_clickable,
            is_visible,
            z_order,
            app_pid,
        }
    }
}

/// Strict traversal budget to guarantee Label Mode never stalls on huge Electron/browser DOMs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessibilityBudget {
    /// Maximum tree depth to descend from the root/window node.
    pub max_depth: usize,
    /// Maximum number of accessibility nodes to inspect before halting traversal.
    pub max_nodes: usize,
    /// Maximum wall-clock duration allowed for the snapshot (`450ms` default).
    pub timeout: Duration,
}

impl Default for AccessibilityBudget {
    fn default() -> Self {
        Self {
            max_depth: 28,
            max_nodes: 3000,
            timeout: Duration::from_millis(450),
        }
    }
}

/// Result of an `AccessibilityProvider::snapshot` traversal, including telemetry metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessibilitySnapshot {
    /// Identifier of the platform backend (`"macos-axuielement"`, `"windows-uiautomation"`, `"linux-atspi2"`, `"mock"`).
    pub platform: String,
    /// Discovered accessible elements.
    pub elements: Vec<AccessibleElement>,
    /// Total number of raw tree nodes visited during traversal.
    pub nodes_visited: usize,
    /// Deepest tree level reached during traversal.
    pub max_depth_reached: usize,
    /// Wall-clock duration spent collecting the snapshot.
    pub elapsed: Duration,
    /// `true` if traversal stopped early due to hitting `max_depth`, `max_nodes`, or `timeout`.
    pub truncated_by_budget: bool,
}

/// Errors produced by OS Accessibility Tree providers.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum A11yError {
    #[error("Accessibility permissions not granted: {0}")]
    PermissionDenied(String),
    #[error("Accessibility query timed out after {0:?}")]
    Timeout(Duration),
    #[error("Platform accessibility error: {0}")]
    PlatformError(String),
    #[error("Unsupported platform or display server: {0}")]
    Unsupported(String),
}

/// Trait implemented by OS Accessibility Tree adapters (`AXUIElement`, `UIAutomation`, `AT-SPI2`, and `Mock`).
pub trait AccessibilityProvider: Send + Sync {
    /// Short identifier for the platform adapter.
    fn platform_name(&self) -> &'static str;

    /// Returns `true` if the process currently holds required OS accessibility permissions.
    fn is_trusted(&self) -> bool;

    /// Verifies (and optionally prompts the user for) OS accessibility permissions.
    fn ensure_permissions(&self, prompt: bool) -> Result<bool, A11yError>;

    /// Traverses the active application/window accessibility tree within the given `budget`.
    fn snapshot(
        &self,
        budget: &AccessibilityBudget,
    ) -> Result<AccessibilitySnapshot, A11yError>;

    /// Convenience helper that snapshots the tree under `budget` and generates prefix-free `LabelBadge`s.
    fn collect_label_badges(
        &self,
        budget: &AccessibilityBudget,
        label_config: &LabelConfig,
    ) -> Result<(AccessibilitySnapshot, Vec<LabelBadge>), A11yError> {
        let snapshot = self.snapshot(budget)?;
        let badges = generate_label_badges(&snapshot.elements, label_config);
        Ok((snapshot, badges))
    }
}

/// Returns the default native `AccessibilityProvider` for the current host OS.
pub fn default_platform_provider() -> Box<dyn AccessibilityProvider> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOSAccessibilityProvider::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsUiaProvider::default())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxAtspiProvider::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Box::new(mock::MockAccessibilityProvider::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::{
        map_atspi_role, recommended_atspi_env_vars, AtspiNodeRecord, LinuxAtspiProvider,
        ATSPI_ROLE_LINK, ATSPI_ROLE_PUSH_BUTTON,
    };
    use crate::windows::{
        map_uia_control_type, UiaElementRecord, WindowsUiaProvider, UIA_BUTTON_CONTROL_TYPE_ID,
        UIA_EDIT_CONTROL_TYPE_ID,
    };

    #[test]
    fn test_default_accessibility_budget() {
        let budget = AccessibilityBudget::default();
        assert_eq!(budget.max_depth, 28);
        assert_eq!(budget.max_nodes, 3000);
        assert_eq!(budget.timeout, Duration::from_millis(450));
    }

    #[test]
    fn test_prefix_free_hints_small_and_large() {
        // 1. Small count <= alphabet length (26): all single-char hints, prefix-free
        let small = generate_prefix_free_hints(8, DEFAULT_HINT_KEYS);
        assert_eq!(small, vec!["a", "s", "d", "f", "g", "h", "j", "k"]);

        // 2. Large count > 26 (e.g. 60 and 200 elements): mix of 1-char and 2-char hints, strictly prefix-free!
        for count in [60, 200] {
            let large = generate_prefix_free_hints(count, DEFAULT_HINT_KEYS);
            assert_eq!(large.len(), count);

            for (i, h1) in large.iter().enumerate() {
                for (j, h2) in large.iter().enumerate() {
                    if i != j {
                        assert!(
                            !h2.starts_with(h1),
                            "Hint '{h1}' (index {i}) is a prefix of '{h2}' (index {j})"
                        );
                    }
                }
            }
        }

        // 3. Maximum capacity (26 * 26 = 676): all 2-char hints, strictly prefix-free
        let max_set = generate_prefix_free_hints(676, DEFAULT_HINT_KEYS);
        assert_eq!(max_set.len(), 676);
        assert!(max_set.iter().all(|h| h.len() == 2));
    }

    #[test]
    fn test_badge_filtering_occlusion_and_iou_deduplication() {
        let provider = MockAccessibilityProvider::standard_desktop_fixture();
        let budget = AccessibilityBudget::default();
        let config = LabelConfig::default();

        let (snapshot, badges) = provider.collect_label_badges(&budget, &config).unwrap();
        assert_eq!(snapshot.elements.len(), 10);

        // From the 10 fixture elements:
        // - "btn-back" (kept)
        // - "btn-back-inner-icon" (deduplicated via high IoU & close centroid to btn-back -> keep smaller or first z=0)
        // - "input-url" (kept)
        // - "tab-1" (kept)
        // - "link-docs" (kept)
        // - "modal-confirm-btn" (kept, z_order = 0)
        // - "bg-occluded-btn" (pruned because occluded by modal-confirm-btn at z_order = 0)
        // - "offscreen-btn" (pruned: outside viewport)
        // - "zero-area-btn" (pruned: width 0)
        // - "hidden-menu-item" (pruned: is_visible = false)
        assert_eq!(badges.len(), 5);

        let ids: Vec<&str> = badges.iter().map(|b| b.element.id.as_str()).collect();
        // Verify occluded, offscreen, zero-area, and hidden elements were filtered out
        assert!(!ids.contains(&"bg-occluded-btn"));
        assert!(!ids.contains(&"offscreen-btn"));
        assert!(!ids.contains(&"zero-area-btn"));
        assert!(!ids.contains(&"hidden-menu-item"));

        // Verify only ONE of btn-back or btn-back-inner-icon survived IoU deduplication
        let back_count = ids
            .iter()
            .filter(|&&id| id == "btn-back" || id == "btn-back-inner-icon")
            .count();
        assert_eq!(back_count, 1);

        // Verify spatial reading order: tab-1 is at y=8 (top row), so it receives the first home-row key "a"
        assert_eq!(badges[0].element.id, "tab-1");
        assert_eq!(badges[0].hint, "a");
    }

    #[test]
    fn test_bfs_traversal_discovers_shallow_controls_before_deep_containers() {
        // Construct a tree where branch 1 plunges 10 levels deep while branch 2 has a shallow toolbar button at depth 2.
        let mut deep_branch = MockTreeNode::new(AccessibleElement::new(
            "deep-leaf",
            ElementRole::Button,
            Some("Deep Button".into()),
            LogicalRect::new(100.0, 400.0, 60.0, 24.0),
            true,
            true,
            0,
            100,
        ));
        for d in (2..=8).rev() {
            deep_branch = MockTreeNode::new(AccessibleElement::new(
                format!("deep-container-{d}"),
                ElementRole::Container,
                None,
                LogicalRect::new(0.0, 0.0, 800.0, 600.0),
                false,
                true,
                0,
                100,
            ))
            .with_children(vec![deep_branch]);
        }

        let shallow_toolbar_btn = MockTreeNode::new(AccessibleElement::new(
            "shallow-toolbar-btn",
            ElementRole::Button,
            Some("Toolbar Action".into()),
            LogicalRect::new(40.0, 20.0, 80.0, 28.0),
            true,
            true,
            0,
            100,
        ));

        let root = MockTreeNode::new(AccessibleElement::new(
            "window-root",
            ElementRole::Window,
            Some("Main Window".into()),
            LogicalRect::new(0.0, 0.0, 800.0, 600.0),
            false,
            true,
            0,
            100,
        ))
        .with_children(vec![deep_branch, shallow_toolbar_btn]);

        let provider = MockAccessibilityProvider::new(true, vec![root]);
        // Budget of only 3 nodes: BFS must visit window-root (depth 1), deep-container-2 (depth 2),
        // and shallow-toolbar-btn (depth 2) before descending into depth 3+.
        let budget = AccessibilityBudget {
            max_depth: 28,
            max_nodes: 3,
            timeout: Duration::from_millis(450),
        };
        let snap = provider.snapshot(&budget).unwrap();
        let visited_ids: Vec<&str> = snap.elements.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            visited_ids,
            vec!["window-root", "deep-container-2", "shallow-toolbar-btn"]
        );
    }

    #[test]
    fn test_cursor_proximity_sorting() {
        let provider = MockAccessibilityProvider::standard_desktop_fixture();
        let snapshot = provider.snapshot(&AccessibilityBudget::default()).unwrap();

        let config = LabelConfig {
            cursor_position: Some(LogicalPoint::new(450.0, 320.0)), // Right on top of modal-confirm-btn
            sort_by_cursor_proximity: true,
            ..LabelConfig::default()
        };

        let badges = generate_label_badges(&snapshot.elements, &config);
        assert!(!badges.is_empty());
        assert_eq!(badges[0].element.id, "modal-confirm-btn");
        assert_eq!(badges[0].hint, "a");
    }

    #[test]
    fn test_budget_enforcement_depth_nodes_and_timeout() {
        // Build a deep synthetic tree of depth 10
        let mut current = MockTreeNode::new(AccessibleElement::new(
            "leaf-10",
            ElementRole::Button,
            Some("Deep Button".into()),
            LogicalRect::new(100.0, 100.0, 40.0, 20.0),
            true,
            true,
            0,
            42,
        ));

        for d in (1..10).rev() {
            current = MockTreeNode::new(AccessibleElement::new(
                format!("node-{d}"),
                ElementRole::Button,
                Some(format!("Node {d}")),
                LogicalRect::new(10.0 * d as f64, 10.0 * d as f64, 40.0, 20.0),
                true,
                true,
                0,
                42,
            ))
            .with_children(vec![current]);
        }

        let provider = MockAccessibilityProvider::new(true, vec![current]);

        // 1. Depth limit = 4
        let depth_budget = AccessibilityBudget {
            max_depth: 4,
            max_nodes: 100,
            timeout: Duration::from_millis(450),
        };
        let snap_depth = provider.snapshot(&depth_budget).unwrap();
        assert_eq!(snap_depth.max_depth_reached, 4);
        assert_eq!(snap_depth.nodes_visited, 4);
        assert!(snap_depth.truncated_by_budget);

        // 2. Node limit = 2
        let node_budget = AccessibilityBudget {
            max_depth: 20,
            max_nodes: 2,
            timeout: Duration::from_millis(450),
        };
        let snap_nodes = provider.snapshot(&node_budget).unwrap();
        assert_eq!(snap_nodes.nodes_visited, 2);
        assert!(snap_nodes.truncated_by_budget);

        // 3. Timeout budget
        let slow_node = MockTreeNode::new(AccessibleElement::new(
            "slow-root",
            ElementRole::Button,
            None,
            LogicalRect::new(10.0, 10.0, 50.0, 30.0),
            true,
            true,
            0,
            99,
        ))
        .with_delay(Duration::from_millis(25))
        .with_children(vec![MockTreeNode::new(AccessibleElement::new(
            "slow-child",
            ElementRole::Button,
            None,
            LogicalRect::new(80.0, 10.0, 50.0, 30.0),
            true,
            true,
            0,
            99,
        ))
        .with_delay(Duration::from_millis(25))]);

        let slow_provider = MockAccessibilityProvider::new(true, vec![slow_node]);
        let time_budget = AccessibilityBudget {
            max_depth: 10,
            max_nodes: 100,
            timeout: Duration::from_millis(15),
        };
        let snap_time = slow_provider.snapshot(&time_budget).unwrap();
        assert!(snap_time.truncated_by_budget);
        assert!(snap_time.elapsed < Duration::from_millis(120));
    }

    #[test]
    fn test_interactive_hint_matching() {
        let provider = MockAccessibilityProvider::standard_desktop_fixture();
        let (_, badges) = provider
            .collect_label_badges(&AccessibilityBudget::default(), &LabelConfig::default())
            .unwrap();

        let exact = match_typed_hint(&badges, "a");
        assert!(matches!(exact, HintMatchResult::ExactMatch(b) if b.hint == "a"));

        let none = match_typed_hint(&badges, "zz");
        assert_eq!(none, HintMatchResult::NoMatch);

        let idx = index_badges(&badges);
        assert_eq!(idx.get("a"), Some(&0));
    }

    #[test]
    fn test_windows_uia_and_linux_atspi_adapters() {
        // Test Windows UIAutomation role mapping & DPI scaling
        assert_eq!(
            map_uia_control_type(UIA_BUTTON_CONTROL_TYPE_ID),
            ElementRole::Button
        );
        assert_eq!(
            map_uia_control_type(UIA_EDIT_CONTROL_TYPE_ID),
            ElementRole::TextField
        );

        let uia_provider = WindowsUiaProvider::with_synthetic_records(
            1.5,
            vec![UiaElementRecord {
                runtime_id: "uia-42-1".into(),
                control_type_id: UIA_BUTTON_CONTROL_TYPE_ID,
                name: Some("OK".into()),
                bounding_rect: [150.0, 300.0, 90.0, 45.0],
                is_offscreen: false,
                is_enabled: true,
                is_invoke_pattern_available: true,
                is_toggle_pattern_available: false,
                is_selection_item_pattern_available: false,
                is_expand_collapse_pattern_available: false,
                process_id: 4200,
                z_order: 0,
            }],
        );
        let uia_snap = uia_provider
            .snapshot(&AccessibilityBudget::default())
            .unwrap();
        assert_eq!(uia_snap.elements.len(), 1);
        assert_eq!(
            uia_snap.elements[0].bounds,
            LogicalRect::new(100.0, 200.0, 60.0, 30.0)
        );

        // Test Linux AT-SPI2 role mapping & node conversion
        assert_eq!(map_atspi_role(ATSPI_ROLE_PUSH_BUTTON), ElementRole::Button);
        assert_eq!(map_atspi_role(ATSPI_ROLE_LINK), ElementRole::Link);
        assert_eq!(
            recommended_atspi_env_vars().get("ACCESSIBILITY_ENABLED"),
            Some(&"1")
        );

        let atspi_provider = LinuxAtspiProvider::with_nodes(vec![AtspiNodeRecord {
            bus_name: ":1.84".into(),
            object_path: "/org/a11y/atspi/accessible/204".into(),
            atspi_role: ATSPI_ROLE_PUSH_BUTTON,
            name: Some("Submit".into()),
            extents: (50, 80, 120, 36),
            n_actions: 1,
            state_visible: true,
            state_showing: true,
            state_sensitive: true,
            app_pid: 8400,
            z_order: 0,
        }]);
        let atspi_snap = atspi_provider
            .snapshot(&AccessibilityBudget::default())
            .unwrap();
        assert_eq!(atspi_snap.elements.len(), 1);
        assert_eq!(
            atspi_snap.elements[0].bounds,
            LogicalRect::new(50.0, 80.0, 120.0, 36.0)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_ax_role_mapping_and_provider_construction() {
        use crate::macos::{
            is_pure_structural_ax_role, map_ax_role, select_target_pids_from_windows,
            should_query_ax_subrole, MacOSAccessibilityProvider, OnScreenWindowCandidate,
            RATPOISON_OVERLAY_TITLE,
        };

        assert_eq!(map_ax_role("AXButton", None), ElementRole::Button);
        assert_eq!(
            map_ax_role("AXButton", Some("AXCloseButton")),
            ElementRole::WindowControl
        );
        assert_eq!(
            map_ax_role("AXTextField", Some("AXSearchField")),
            ElementRole::TextField
        );

        // Role / container IPC optimization helpers
        assert!(should_query_ax_subrole("AXButton"));
        assert!(should_query_ax_subrole("AXTextField"));
        assert!(!should_query_ax_subrole("AXScrollArea"));
        assert!(!should_query_ax_subrole("AXStaticText"));

        assert!(is_pure_structural_ax_role("AXWindow"));
        assert!(is_pure_structural_ax_role("AXScrollArea"));
        assert!(is_pure_structural_ax_role("AXSplitGroup"));
        assert!(is_pure_structural_ax_role("AXWebArea"));
        assert!(!is_pure_structural_ax_role("AXButton"));
        assert!(!is_pure_structural_ax_role("AXGroup"));

        // Window list PID discovery logic:
        // Case 1: RatPoison Overlay is on top, followed by Safari (PID 500) and Terminal (PID 600),
        // with RatPoison Settings (self_pid = 999) behind Safari.
        let windows = vec![
            OnScreenWindowCandidate {
                owner_pid: 999,
                window_layer: 0,
                window_name: Some(RATPOISON_OVERLAY_TITLE.to_string()),
                owner_name: Some("RatPoison".to_string()),
            },
            OnScreenWindowCandidate {
                owner_pid: 500,
                window_layer: 0,
                window_name: Some("Apple".to_string()),
                owner_name: Some("Safari".to_string()),
            },
            OnScreenWindowCandidate {
                owner_pid: 999,
                window_layer: 0,
                window_name: Some("RatPoison Settings".to_string()),
                owner_name: Some("RatPoison".to_string()),
            },
            OnScreenWindowCandidate {
                owner_pid: 600,
                window_layer: 0,
                window_name: Some("zsh".to_string()),
                owner_name: Some("Terminal".to_string()),
            },
        ];
        assert_eq!(select_target_pids_from_windows(&windows, 999), vec![500, 600]);

        // Case 2: RatPoison Settings (self_pid = 999) is genuinely the frontmost layer-0 window
        // underneath the RatPoison Overlay.
        let settings_front_windows = vec![
            OnScreenWindowCandidate {
                owner_pid: 999,
                window_layer: 0,
                window_name: Some(RATPOISON_OVERLAY_TITLE.to_string()),
                owner_name: Some("RatPoison".to_string()),
            },
            OnScreenWindowCandidate {
                owner_pid: 999,
                window_layer: 0,
                window_name: Some("RatPoison Settings".to_string()),
                owner_name: Some("RatPoison".to_string()),
            },
            OnScreenWindowCandidate {
                owner_pid: 500,
                window_layer: 0,
                window_name: Some("Apple".to_string()),
                owner_name: Some("Safari".to_string()),
            },
        ];
        assert_eq!(
            select_target_pids_from_windows(&settings_front_windows, 999),
            vec![999, 500]
        );

        let provider = MacOSAccessibilityProvider::new();
        assert_eq!(provider.platform_name(), "macos-axuielement");
        // Verify `is_trusted()` and `query_on_screen_windows()` FFI calls execute cleanly
        let _trusted = provider.is_trusted();
        let live_windows = MacOSAccessibilityProvider::query_on_screen_windows();
        assert!(!live_windows.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_live_snapshot_discovers_interactive_elements() {
        use crate::macos::MacOSAccessibilityProvider;

        let provider = MacOSAccessibilityProvider::new();
        if !provider.is_trusted() {
            println!("Skipping live macOS AX snapshot assertion because AXIsProcessTrusted() == false");
            return;
        }

        let budget = AccessibilityBudget::default();
        let (snap, badges) = provider
            .collect_label_badges(&budget, &LabelConfig::default())
            .expect("live macOS snapshot should succeed");

        println!(
            "Live macOS AX snapshot: elements={}, badges={}, nodes_visited={}, max_depth_reached={}, elapsed_ms={:.2}, truncated={}",
            snap.elements.len(),
            badges.len(),
            snap.nodes_visited,
            snap.max_depth_reached,
            snap.elapsed.as_secs_f64() * 1000.0,
            snap.truncated_by_budget
        );
        assert!(
            !snap.elements.is_empty(),
            "Expected >0 interactive elements from live macOS snapshot, visited {} nodes",
            snap.nodes_visited
        );
        assert!(
            !badges.is_empty(),
            "Expected >0 generated LabelBadges from live macOS snapshot"
        );
    }
}
