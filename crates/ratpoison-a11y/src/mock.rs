//! Deterministic mock Accessibility Tree provider for cross-platform unit and integration tests.

use crate::{
    A11yError, AccessibilityBudget, AccessibilityProvider, AccessibilitySnapshot,
    AccessibleElement, ElementRole, LogicalRect,
};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A node in a synthetic hierarchical accessibility tree used to test depth/node/time budget limits.
#[derive(Debug, Clone)]
pub struct MockTreeNode {
    pub element: AccessibleElement,
    pub simulated_visit_delay: Duration,
    pub children: Vec<MockTreeNode>,
}

impl MockTreeNode {
    pub fn new(element: AccessibleElement) -> Self {
        Self {
            element,
            simulated_visit_delay: Duration::ZERO,
            children: Vec::new(),
        }
    }

    pub fn with_children(mut self, children: Vec<MockTreeNode>) -> Self {
        self.children = children;
        self
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.simulated_visit_delay = delay;
        self
    }
}

/// Deterministic mock provider that supports both flat element fixtures and deep tree traversal
/// with strict budget enforcement (`max_depth`, `max_nodes`, `timeout`).
#[derive(Debug, Clone)]
pub struct MockAccessibilityProvider {
    pub trusted: bool,
    pub roots: Vec<MockTreeNode>,
}

impl Default for MockAccessibilityProvider {
    fn default() -> Self {
        Self::standard_desktop_fixture()
    }
}

impl MockAccessibilityProvider {
    /// Creates an empty trusted mock provider.
    pub fn new(trusted: bool, roots: Vec<MockTreeNode>) -> Self {
        Self { trusted, roots }
    }

    /// Creates a provider from a flat slice of `AccessibleElement`s (all at depth 1).
    pub fn from_flat_elements(elements: Vec<AccessibleElement>) -> Self {
        let roots = elements.into_iter().map(MockTreeNode::new).collect();
        Self {
            trusted: true,
            roots,
        }
    }

    /// Builds a realistic desktop fixture containing:
    /// - Valid interactive controls (toolbar buttons, links, text fields, tabs, menu items)
    /// - Off-screen elements
    /// - Zero-area / tiny hidden elements
    /// - Near-duplicate wrapper/inner button elements (IoU & centroid collision)
    /// - Occluded background window elements (`z_order = 1` covered by `z_order = 0`)
    pub fn standard_desktop_fixture() -> Self {
        let elements = vec![
            AccessibleElement::new(
                "btn-back",
                ElementRole::Button,
                Some("Back".to_string()),
                LogicalRect::new(24.0, 48.0, 36.0, 32.0),
                true,
                true,
                0,
                1001,
            ),
            // Near-duplicate inner span inside btn-back (should be deduplicated by IoU/centroid)
            AccessibleElement::new(
                "btn-back-inner-icon",
                ElementRole::Button,
                Some("Back Icon".to_string()),
                LogicalRect::new(25.0, 49.0, 34.0, 30.0),
                true,
                true,
                0,
                1001,
            ),
            AccessibleElement::new(
                "input-url",
                ElementRole::TextField,
                Some("Address and search bar".to_string()),
                LogicalRect::new(120.0, 48.0, 480.0, 32.0),
                true,
                true,
                0,
                1001,
            ),
            AccessibleElement::new(
                "tab-1",
                ElementRole::Tab,
                Some("RatPoison Docs".to_string()),
                LogicalRect::new(80.0, 8.0, 180.0, 32.0),
                true,
                true,
                0,
                1001,
            ),
            AccessibleElement::new(
                "link-docs",
                ElementRole::Link,
                Some("Getting Started".to_string()),
                LogicalRect::new(240.0, 180.0, 140.0, 24.0),
                true,
                true,
                0,
                1001,
            ),
            // Foreground modal covering (400, 300, 300, 200) at z_order 0
            AccessibleElement::new(
                "modal-confirm-btn",
                ElementRole::Button,
                Some("Confirm Action".to_string()),
                LogicalRect::new(400.0, 300.0, 160.0, 48.0),
                true,
                true,
                0,
                1001,
            ),
            // Background button at z_order 1 completely occluded by modal-confirm-btn
            AccessibleElement::new(
                "bg-occluded-btn",
                ElementRole::Button,
                Some("Behind Modal".to_string()),
                LogicalRect::new(410.0, 305.0, 140.0, 38.0),
                true,
                true,
                1,
                1002,
            ),
            // Off-screen element (negative coordinates outside 1920x1080)
            AccessibleElement::new(
                "offscreen-btn",
                ElementRole::Button,
                Some("Offscreen".to_string()),
                LogicalRect::new(-250.0, -100.0, 80.0, 30.0),
                true,
                true,
                0,
                1001,
            ),
            // Zero-area element
            AccessibleElement::new(
                "zero-area-btn",
                ElementRole::Button,
                Some("Collapsed".to_string()),
                LogicalRect::new(300.0, 300.0, 0.0, 24.0),
                true,
                true,
                0,
                1001,
            ),
            // Invisible element
            AccessibleElement::new(
                "hidden-menu-item",
                ElementRole::MenuItem,
                Some("Hidden Item".to_string()),
                LogicalRect::new(320.0, 220.0, 120.0, 28.0),
                true,
                false,
                0,
                1001,
            ),
        ];

        Self::from_flat_elements(elements)
    }
}

impl AccessibilityProvider for MockAccessibilityProvider {
    fn platform_name(&self) -> &'static str {
        "mock"
    }

    fn is_trusted(&self) -> bool {
        self.trusted
    }

    fn ensure_permissions(&self, _prompt: bool) -> Result<bool, A11yError> {
        if self.trusted {
            Ok(true)
        } else {
            Err(A11yError::PermissionDenied(
                "Mock accessibility provider is not trusted".to_string(),
            ))
        }
    }

    fn snapshot(
        &self,
        budget: &AccessibilityBudget,
    ) -> Result<AccessibilitySnapshot, A11yError> {
        if !self.trusted {
            return Err(A11yError::PermissionDenied(
                "Accessibility permissions not granted".to_string(),
            ));
        }

        let start = Instant::now();
        let mut elements = Vec::new();
        let mut nodes_visited = 0usize;
        let mut max_depth_reached = 0usize;
        let mut truncated_by_budget = false;

        // Iterative BFS queue: (node_ref, depth)
        let mut queue: VecDeque<(&MockTreeNode, usize)> =
            self.roots.iter().map(|node| (node, 1usize)).collect();

        while let Some((node, depth)) = queue.pop_front() {
            if nodes_visited >= budget.max_nodes || start.elapsed() >= budget.timeout {
                truncated_by_budget = true;
                break;
            }

            if !node.simulated_visit_delay.is_zero() {
                std::thread::sleep(node.simulated_visit_delay);
                if start.elapsed() >= budget.timeout {
                    truncated_by_budget = true;
                    break;
                }
            }

            nodes_visited += 1;
            if depth > max_depth_reached {
                max_depth_reached = depth;
            }

            elements.push(node.element.clone());

            if depth < budget.max_depth {
                for child in &node.children {
                    queue.push_back((child, depth + 1));
                }
            } else if !node.children.is_empty() {
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
