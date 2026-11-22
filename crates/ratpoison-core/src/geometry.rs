//! Multi-monitor mixed-DPI geometry and deterministic coordinate mapping.
//!
//! # Coordinate System Conventions
//! - Logical coordinates use `f64` (`LogicalPoint`, `LogicalRect`), representing
//!   device-independent points in the virtual desktop space. Monitors placed left
//!   or above the primary display have negative `x` or `y` origins.
//! - Physical coordinates use `i32` (`PhysicalPoint`, `PhysicalRect`), representing
//!   device pixels in OS virtual-screen space.
//! - Each monitor carries its own `logical_rect`, `physical_rect`, and `scale_factor`.
//!   Conversions are always performed relative to the owning monitor's origin so
//!   mixed-DPI neighbors (e.g., 2.0x next to 1.25x) never accumulate cross-monitor
//!   scaling drift.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Cardinal direction for nudging, free-mode steering, and directional monitor switching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    /// Unit vector `(dx, dy)` in screen coordinates (positive `y` points downward).
    pub const fn unit_vector(self) -> (f64, f64) {
        match self {
            Self::Up => (0.0, -1.0),
            Self::Down => (0.0, 1.0),
            Self::Left => (-1.0, 0.0),
            Self::Right => (1.0, 0.0),
        }
    }
}

/// Physical/logical display orientation derived from dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Landscape,
    Portrait,
}

/// A 2D point in logical (device-independent) desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

impl LogicalPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Squared Euclidean distance to another logical point.
    pub fn distance_squared(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }
}

/// A 2D point in physical OS pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhysicalPoint {
    pub x: i32,
    pub y: i32,
}

impl PhysicalPoint {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle in logical (device-independent) desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LogicalRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(self) -> f64 {
        self.y + self.height
    }

    pub fn center(self) -> LogicalPoint {
        LogicalPoint::new(self.x + self.width * 0.5, self.y + self.height * 0.5)
    }

    /// Half-open containment `[x, x + width) x [y, y + height)` so adjacent monitors
    /// never double-claim shared boundary lines.
    pub fn contains_point(self, point: LogicalPoint) -> bool {
        point.x >= self.x
            && point.x < self.right()
            && point.y >= self.y
            && point.y < self.bottom()
    }

    /// Closed containment `[x, x + width] x [y, y + height]`.
    pub fn contains_point_inclusive(self, point: LogicalPoint) -> bool {
        point.x >= self.x
            && point.x <= self.right()
            && point.y >= self.y
            && point.y <= self.bottom()
    }

    pub fn orientation(self) -> Orientation {
        if self.height > self.width {
            Orientation::Portrait
        } else {
            Orientation::Landscape
        }
    }
}

/// An axis-aligned rectangle in physical OS pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysicalRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Exclusive right edge in `i64` to prevent `i32` overflow.
    pub const fn right_i64(self) -> i64 {
        self.x as i64 + self.width as i64
    }

    /// Exclusive bottom edge in `i64` to prevent `i32` overflow.
    pub const fn bottom_i64(self) -> i64 {
        self.y as i64 + self.height as i64
    }

    /// Inclusive maximum physical X coordinate within this monitor.
    pub const fn max_x_inclusive(self) -> i32 {
        if self.width == 0 {
            self.x
        } else {
            let max = self.x as i64 + self.width as i64 - 1;
            if max > i32::MAX as i64 {
                i32::MAX
            } else {
                max as i32
            }
        }
    }

    /// Inclusive maximum physical Y coordinate within this monitor.
    pub const fn max_y_inclusive(self) -> i32 {
        if self.height == 0 {
            self.y
        } else {
            let max = self.y as i64 + self.height as i64 - 1;
            if max > i32::MAX as i64 {
                i32::MAX
            } else {
                max as i32
            }
        }
    }

    /// Half-open containment `[x, x + width) x [y, y + height)`.
    pub const fn contains_point(self, point: PhysicalPoint) -> bool {
        let px = point.x as i64;
        let py = point.y as i64;
        px >= self.x as i64
            && px < self.right_i64()
            && py >= self.y as i64
            && py < self.bottom_i64()
    }
}

/// Complete description of a display in a multi-monitor virtual desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: String,
    pub name: String,
    pub logical_rect: LogicalRect,
    pub physical_rect: PhysicalRect,
    pub scale_factor: f64,
    pub is_primary: bool,
}

impl MonitorInfo {
    /// Construct a `MonitorInfo` from logical bounds, explicit physical origin, and scale factor.
    pub fn from_logical_and_scale(
        id: impl Into<String>,
        name: impl Into<String>,
        logical_rect: LogicalRect,
        physical_origin: PhysicalPoint,
        scale_factor: f64,
        is_primary: bool,
    ) -> Result<Self, CoreError> {
        let id = id.into();
        if !scale_factor.is_finite() || scale_factor <= 0.0 {
            return Err(CoreError::InvalidMonitor {
                id,
                reason: format!("scale_factor must be finite and > 0, got {scale_factor}"),
            });
        }
        let phys_w = (logical_rect.width * scale_factor).round().max(0.0) as u32;
        let phys_h = (logical_rect.height * scale_factor).round().max(0.0) as u32;
        let info = Self {
            id,
            name: name.into(),
            logical_rect,
            physical_rect: PhysicalRect::new(physical_origin.x, physical_origin.y, phys_w, phys_h),
            scale_factor,
            is_primary,
        };
        info.validate()?;
        Ok(info)
    }

    /// Validate that monitor geometry and scale factor are finite and strictly positive.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.id.trim().is_empty() {
            return Err(CoreError::InvalidMonitor {
                id: self.id.clone(),
                reason: "monitor id must not be empty".to_string(),
            });
        }
        if !self.scale_factor.is_finite() || self.scale_factor <= 0.0 {
            return Err(CoreError::InvalidMonitor {
                id: self.id.clone(),
                reason: format!(
                    "scale_factor must be finite and > 0, got {}",
                    self.scale_factor
                ),
            });
        }
        let r = self.logical_rect;
        if !r.x.is_finite()
            || !r.y.is_finite()
            || !r.width.is_finite()
            || !r.height.is_finite()
            || r.width <= 0.0
            || r.height <= 0.0
        {
            return Err(CoreError::InvalidMonitor {
                id: self.id.clone(),
                reason: format!(
                    "logical_rect must have finite coordinates and positive dimensions, got {r:?}"
                ),
            });
        }
        if self.physical_rect.width == 0 || self.physical_rect.height == 0 {
            return Err(CoreError::InvalidMonitor {
                id: self.id.clone(),
                reason: format!(
                    "physical_rect must have positive dimensions, got {:?}",
                    self.physical_rect
                ),
            });
        }
        Ok(())
    }

    /// Display orientation (landscape or portrait).
    pub fn orientation(&self) -> Orientation {
        self.logical_rect.orientation()
    }

    /// Center of this monitor in logical coordinates.
    pub fn logical_center(&self) -> LogicalPoint {
        self.logical_rect.center()
    }

    /// Maximum logical X coordinate that still maps to the monitor's final physical pixel
    /// (`physical_rect.x + physical_rect.width - 1`) without spilling onto an adjacent monitor.
    pub fn max_logical_x_inclusive(&self) -> f64 {
        let last_px_offset = self.physical_rect.width.saturating_sub(1) as f64;
        let from_phys = self.logical_rect.x + last_px_offset / self.scale_factor;
        from_phys.min(self.logical_rect.right())
    }

    /// Maximum logical Y coordinate that still maps to the monitor's final physical pixel
    /// (`physical_rect.y + physical_rect.height - 1`) without spilling onto an adjacent monitor.
    pub fn max_logical_y_inclusive(&self) -> f64 {
        let last_px_offset = self.physical_rect.height.saturating_sub(1) as f64;
        let from_phys = self.logical_rect.y + last_px_offset / self.scale_factor;
        from_phys.min(self.logical_rect.bottom())
    }
}

/// Convert a logical coordinate to a physical OS pixel coordinate on a specific monitor.
///
/// Uses per-monitor local affine transform:
/// `physical = physical_origin + round((logical - logical_origin) * scale_factor)`.
/// This handles negative monitor origins, fractional DPIs (1.25x, 1.5x, 1.75x, 2.0x),
/// and portrait/landscape orientations identically without cross-monitor drift.
pub fn logical_to_physical(point: LogicalPoint, monitor: &MonitorInfo) -> PhysicalPoint {
    let dx = (point.x - monitor.logical_rect.x) * monitor.scale_factor;
    let dy = (point.y - monitor.logical_rect.y) * monitor.scale_factor;
    let px = (monitor.physical_rect.x as f64 + dx.round())
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    let py = (monitor.physical_rect.y as f64 + dy.round())
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    PhysicalPoint::new(px, py)
}

/// Convert a logical coordinate to a physical pixel coordinate clamped to the monitor's
/// valid inclusive pixel range `[physical_rect.x, max_x_inclusive] x [physical_rect.y, max_y_inclusive]`.
pub fn logical_to_physical_clamped(point: LogicalPoint, monitor: &MonitorInfo) -> PhysicalPoint {
    let raw = logical_to_physical(point, monitor);
    PhysicalPoint::new(
        raw.x
            .clamp(monitor.physical_rect.x, monitor.physical_rect.max_x_inclusive()),
        raw.y
            .clamp(monitor.physical_rect.y, monitor.physical_rect.max_y_inclusive()),
    )
}

/// Convert a physical OS pixel coordinate on a specific monitor back to logical coordinates.
///
/// Exact left-inverse of `logical_to_physical`: for every physical pixel `p` on `monitor`,
/// `logical_to_physical(physical_to_logical(p, monitor), monitor) == p`.
pub fn physical_to_logical(point: PhysicalPoint, monitor: &MonitorInfo) -> LogicalPoint {
    let dx_p = (point.x as i64 - monitor.physical_rect.x as i64) as f64;
    let dy_p = (point.y as i64 - monitor.physical_rect.y as i64) as f64;
    LogicalPoint::new(
        monitor.logical_rect.x + dx_p / monitor.scale_factor,
        monitor.logical_rect.y + dy_p / monitor.scale_factor,
    )
}

/// Clamp a logical point cleanly inside a single monitor's valid pixel domain.
///
/// The upper bound uses `max_logical_x_inclusive()` / `max_logical_y_inclusive()` so the
/// clamped point stays strictly inside the monitor's half-open logical rect and maps
/// to a valid in-bounds physical pixel.
pub fn clamp_logical_to_monitor(point: LogicalPoint, monitor: &MonitorInfo) -> LogicalPoint {
    LogicalPoint::new(
        point
            .x
            .clamp(monitor.logical_rect.x, monitor.max_logical_x_inclusive()),
        point
            .y
            .clamp(monitor.logical_rect.y, monitor.max_logical_y_inclusive()),
    )
}

/// Ordered collection of displays forming the user's virtual desktop.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MonitorLayout {
    monitors: Vec<MonitorInfo>,
}

impl MonitorLayout {
    /// Create a validated `MonitorLayout` from a list of monitors.
    pub fn new(monitors: Vec<MonitorInfo>) -> Result<Self, CoreError> {
        let mut seen = HashSet::with_capacity(monitors.len());
        for m in &monitors {
            m.validate()?;
            if !seen.insert(m.id.as_str()) {
                return Err(CoreError::DuplicateMonitorId(m.id.clone()));
            }
        }
        Ok(Self { monitors })
    }

    /// Access the underlying slice of monitors.
    pub fn monitors(&self) -> &[MonitorInfo] {
        &self.monitors
    }

    pub fn is_empty(&self) -> bool {
        self.monitors.is_empty()
    }

    pub fn len(&self) -> usize {
        self.monitors.len()
    }

    /// Return the primary monitor, falling back to the first monitor if none is flagged primary.
    pub fn primary(&self) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|m| m.is_primary)
            .or_else(|| self.monitors.first())
    }

    /// Look up a monitor by its ID.
    pub fn by_id(&self, id: &str) -> Option<&MonitorInfo> {
        self.monitors.iter().find(|m| m.id == id)
    }

    /// Look up a monitor by 0-based index.
    pub fn by_index(&self, index: usize) -> Option<&MonitorInfo> {
        self.monitors.get(index)
    }

    /// Find the monitor containing `point` using half-open logical bounds.
    pub fn monitor_at_logical(&self, point: LogicalPoint) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|m| m.logical_rect.contains_point(point))
    }

    /// Find the monitor containing `point` using half-open physical bounds.
    pub fn monitor_at_physical(&self, point: PhysicalPoint) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|m| m.physical_rect.contains_point(point))
    }

    /// Find the monitor containing `point`, or the nearest monitor by clamped projection distance.
    pub fn nearest_monitor_logical(&self, point: LogicalPoint) -> Option<&MonitorInfo> {
        if let Some(m) = self.monitor_at_logical(point) {
            return Some(m);
        }
        let mut best: Option<(&MonitorInfo, f64)> = None;
        for m in &self.monitors {
            let clamped = clamp_logical_to_monitor(point, m);
            let dist_sq = point.distance_squared(clamped);
            match best {
                None => best = Some((m, dist_sq)),
                Some((best_m, best_dist)) => {
                    let strictly_closer = dist_sq < best_dist - 1e-9;
                    let tie_prefer_primary =
                        (dist_sq - best_dist).abs() <= 1e-9 && m.is_primary && !best_m.is_primary;
                    if strictly_closer || tie_prefer_primary {
                        best = Some((m, dist_sq));
                    }
                }
            }
        }
        best.map(|(m, _)| m)
    }

    /// Convert a global logical point into `(monitor_id, PhysicalPoint)` using the owning
    /// (or nearest) monitor.
    pub fn logical_to_physical(&self, point: LogicalPoint) -> Option<(String, PhysicalPoint)> {
        let monitor = self.nearest_monitor_logical(point)?;
        Some((monitor.id.clone(), logical_to_physical(point, monitor)))
    }

    /// Convert a global physical point into `(monitor_id, LogicalPoint)` using the owning
    /// (or nearest) monitor.
    pub fn physical_to_logical(&self, point: PhysicalPoint) -> Option<(String, LogicalPoint)> {
        let monitor = self.monitor_at_physical(point).or_else(|| {
            // Fall back to nearest monitor in physical space.
            self.monitors.iter().min_by_key(|m| {
                let cx = point
                    .x
                    .clamp(m.physical_rect.x, m.physical_rect.max_x_inclusive())
                    as i64;
                let cy = point
                    .y
                    .clamp(m.physical_rect.y, m.physical_rect.max_y_inclusive())
                    as i64;
                let dx = point.x as i64 - cx;
                let dy = point.y as i64 - cy;
                dx * dx + dy * dy
            })
        })?;
        Some((monitor.id.clone(), physical_to_logical(point, monitor)))
    }

    /// Axis-aligned bounding box of all monitors in logical coordinates.
    pub fn virtual_logical_bounds(&self) -> Option<LogicalRect> {
        let first = self.monitors.first()?;
        let mut min_x = first.logical_rect.x;
        let mut min_y = first.logical_rect.y;
        let mut max_x = first.logical_rect.right();
        let mut max_y = first.logical_rect.bottom();

        for m in self.monitors.iter().skip(1) {
            min_x = min_x.min(m.logical_rect.x);
            min_y = min_y.min(m.logical_rect.y);
            max_x = max_x.max(m.logical_rect.right());
            max_y = max_y.max(m.logical_rect.bottom());
        }
        Some(LogicalRect::new(min_x, min_y, max_x - min_x, max_y - min_y))
    }

    /// Clamp a logical point to the union of all monitors (avoiding dead space in L-shaped
    /// or mismatched-height multi-monitor setups). Returns the clamped point and owning monitor ID.
    pub fn clamp_logical_to_virtual_desktop(
        &self,
        point: LogicalPoint,
    ) -> Option<(String, LogicalPoint)> {
        let monitor = self.nearest_monitor_logical(point)?;
        Some((monitor.id.clone(), clamp_logical_to_monitor(point, monitor)))
    }

    /// Cycle to the next monitor in deterministic order (sorted by `logical_rect.x`, then `y`, then `id`).
    pub fn next_monitor_id(&self, current_id: &str) -> Option<&str> {
        let ordered = self.spatially_ordered_monitors();
        if ordered.is_empty() {
            return None;
        }
        let pos = ordered
            .iter()
            .position(|m| m.id == current_id)
            .unwrap_or(ordered.len() - 1);
        Some(ordered[(pos + 1) % ordered.len()].id.as_str())
    }

    /// Cycle to the previous monitor in deterministic spatial order.
    pub fn prev_monitor_id(&self, current_id: &str) -> Option<&str> {
        let ordered = self.spatially_ordered_monitors();
        if ordered.is_empty() {
            return None;
        }
        let pos = ordered.iter().position(|m| m.id == current_id).unwrap_or(0);
        let prev = if pos == 0 { ordered.len() - 1 } else { pos - 1 };
        Some(ordered[prev].id.as_str())
    }

    /// Find the closest monitor strictly in `direction` from `current_id`.
    ///
    /// A candidate monitor qualifies if it lies inside the ±45° forward cone (`perpendicular <= forward`)
    /// OR is strictly separated beyond `current`'s edge along `direction` (handling offset stacked displays
    /// while preventing tall side-by-side portrait displays from matching Up/Down).
    pub fn neighbor_in_direction(&self, current_id: &str, direction: Direction) -> Option<&str> {
        let current = self.by_id(current_id)?;
        let c = current.logical_center();
        let cr = current.logical_rect;
        let (ux, uy) = direction.unit_vector();

        let mut best: Option<(&MonitorInfo, f64)> = None;
        for m in &self.monitors {
            if m.id == current.id {
                continue;
            }
            let mc = m.logical_center();
            let mr = m.logical_rect;
            let dx = mc.x - c.x;
            let dy = mc.y - c.y;
            let forward = dx * ux + dy * uy;
            if forward <= 1.0 {
                continue;
            }
            let perpendicular = (dx * uy - dy * ux).abs();
            let strictly_beyond_edge = match direction {
                Direction::Up => mr.bottom() <= cr.y + 1e-6,
                Direction::Down => mr.y >= cr.bottom() - 1e-6,
                Direction::Left => mr.right() <= cr.x + 1e-6,
                Direction::Right => mr.x >= cr.right() - 1e-6,
            };
            if perpendicular > forward && !strictly_beyond_edge {
                continue;
            }

            // Weight perpendicular offset more heavily so aligned neighbors win cleanly.
            let score = forward + perpendicular * 2.0;
            match best {
                None => best = Some((m, score)),
                Some((best_m, best_score)) => {
                    if score < best_score - 1e-9
                        || ((score - best_score).abs() <= 1e-9 && m.id < best_m.id)
                    {
                        best = Some((m, score));
                    }
                }
            }
        }
        best.map(|(m, _)| m.id.as_str())
    }

    /// Deterministic spatial order: left-to-right, top-to-bottom, tie-breaking by `id`.
    pub fn spatially_ordered_monitors(&self) -> Vec<&MonitorInfo> {
        let mut refs: Vec<&MonitorInfo> = self.monitors.iter().collect();
        refs.sort_by(|a, b| {
            a.logical_rect
                .x
                .total_cmp(&b.logical_rect.x)
                .then_with(|| a.logical_rect.y.total_cmp(&b.logical_rect.y))
                .then_with(|| a.id.cmp(&b.id))
        });
        refs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mixed_dpi_layout() -> MonitorLayout {
        // Left monitor: negative origin, 1.25x fractional DPI, landscape (1920x1080 physical -> 1536x864 logical)
        let left = MonitorInfo::from_logical_and_scale(
            "HDMI-1",
            "Left 125%",
            LogicalRect::new(-1536.0, 100.0, 1536.0, 864.0),
            PhysicalPoint::new(-1920, 125),
            1.25,
            false,
        )
        .unwrap();

        // Primary center monitor: Retina 2.0x (1440x900 logical -> 2880x1800 physical)
        let primary = MonitorInfo::from_logical_and_scale(
            "DP-1",
            "Retina Primary",
            LogicalRect::new(0.0, 0.0, 1440.0, 900.0),
            PhysicalPoint::new(0, 0),
            2.0,
            true,
        )
        .unwrap();

        // Top monitor: negative Y origin, 1.5x fractional DPI (1280x720 logical -> 1920x1080 physical)
        let top = MonitorInfo::from_logical_and_scale(
            "DP-2",
            "Top 150%",
            LogicalRect::new(80.0, -720.0, 1280.0, 720.0),
            PhysicalPoint::new(160, -1080),
            1.5,
            false,
        )
        .unwrap();

        // Right portrait monitor: 1.0x DPI, portrait (1080x1920)
        let right = MonitorInfo::from_logical_and_scale(
            "DP-3",
            "Portrait Right",
            LogicalRect::new(1440.0, -300.0, 1080.0, 1920.0),
            PhysicalPoint::new(2880, -300),
            1.0,
            false,
        )
        .unwrap();

        MonitorLayout::new(vec![left, primary, top, right]).unwrap()
    }

    #[test]
    fn mixed_dpi_roundtrip_all_monitors() {
        let layout = sample_mixed_dpi_layout();
        for m in layout.monitors() {
            // Test origin, center, quarter-points, and inclusive max corner
            let samples = [
                PhysicalPoint::new(m.physical_rect.x, m.physical_rect.y),
                PhysicalPoint::new(
                    m.physical_rect.x + (m.physical_rect.width / 2) as i32,
                    m.physical_rect.y + (m.physical_rect.height / 2) as i32,
                ),
                PhysicalPoint::new(m.physical_rect.x + 17, m.physical_rect.y + 31),
                PhysicalPoint::new(
                    m.physical_rect.max_x_inclusive(),
                    m.physical_rect.max_y_inclusive(),
                ),
            ];
            for p in samples {
                let l = physical_to_logical(p, m);
                let roundtrip = logical_to_physical(l, m);
                assert_eq!(roundtrip, p, "roundtrip failed on monitor {}", m.id);
            }
        }
    }

    #[test]
    fn negative_origin_125_percent_exact_mapping() {
        let layout = sample_mixed_dpi_layout();
        let left = layout.by_id("HDMI-1").unwrap();
        // Origin of left monitor maps exactly to (-1920, 125)
        assert_eq!(
            logical_to_physical(LogicalPoint::new(-1536.0, 100.0), left),
            PhysicalPoint::new(-1920, 125)
        );
        // Logical center (-768.0, 532.0) -> offset (768, 432) * 1.25 = (960, 540) -> (-960, 665)
        assert_eq!(
            logical_to_physical(left.logical_center(), left),
            PhysicalPoint::new(-960, 665)
        );
    }

    #[test]
    fn clamped_monitor_edge_never_spills_onto_adjacent_monitor() {
        let layout = sample_mixed_dpi_layout();
        let left = layout.by_id("HDMI-1").unwrap();
        let clamped = clamp_logical_to_monitor(LogicalPoint::new(500.0, 500.0), left);
        assert!(left.logical_rect.contains_point(clamped));
        assert_eq!(layout.monitor_at_logical(clamped).unwrap().id, "HDMI-1");
        let phys = logical_to_physical(clamped, left);
        assert_eq!(phys.x, left.physical_rect.max_x_inclusive());
    }

    #[test]
    fn orientation_detection_landscape_and_portrait() {
        let layout = sample_mixed_dpi_layout();
        assert_eq!(
            layout.by_id("DP-1").unwrap().orientation(),
            Orientation::Landscape
        );
        assert_eq!(
            layout.by_id("DP-3").unwrap().orientation(),
            Orientation::Portrait
        );
    }

    #[test]
    fn l_shaped_desktop_clamps_out_of_dead_space() {
        let layout = sample_mixed_dpi_layout();
        // (-1000, -500) is in dead space above Left and left of Top
        let (mon_id, clamped) = layout
            .clamp_logical_to_virtual_desktop(LogicalPoint::new(-1000.0, -500.0))
            .unwrap();
        let m = layout.by_id(&mon_id).unwrap();
        assert!(m.logical_rect.contains_point(clamped));
    }

    #[test]
    fn directional_monitor_switching_works() {
        let layout = sample_mixed_dpi_layout();
        assert_eq!(
            layout.neighbor_in_direction("DP-1", Direction::Left),
            Some("HDMI-1")
        );
        assert_eq!(
            layout.neighbor_in_direction("DP-1", Direction::Right),
            Some("DP-3")
        );
        assert_eq!(
            layout.neighbor_in_direction("DP-1", Direction::Up),
            Some("DP-2")
        );
        assert_eq!(layout.neighbor_in_direction("DP-1", Direction::Down), None);
    }

    #[test]
    fn rejects_invalid_monitor_and_duplicates() {
        let bad_scale = MonitorInfo::from_logical_and_scale(
            "bad",
            "Bad",
            LogicalRect::new(0.0, 0.0, 100.0, 100.0),
            PhysicalPoint::new(0, 0),
            0.0,
            true,
        );
        assert!(matches!(bad_scale, Err(CoreError::InvalidMonitor { .. })));

        let m1 = MonitorInfo::from_logical_and_scale(
            "dup",
            "A",
            LogicalRect::new(0.0, 0.0, 100.0, 100.0),
            PhysicalPoint::new(0, 0),
            1.0,
            true,
        )
        .unwrap();
        let m2 = m1.clone();
        assert!(matches!(
            MonitorLayout::new(vec![m1, m2]),
            Err(CoreError::DuplicateMonitorId(_))
        ));
    }
}
