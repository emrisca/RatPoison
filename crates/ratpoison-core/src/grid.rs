//! Configurable NxM grid generation, nested subgrid subdivision, label matching,
//! and precision cursor nudging.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::geometry::{
    clamp_logical_to_monitor, Direction, LogicalPoint, LogicalRect, MonitorInfo, MonitorLayout,
};

/// Ordering strategy for assigning 2-character labels to `(row, col)` cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelOrdering {
    /// When `rows <= first_keys.len()` and `cols <= second_keys.len()`, cell `(r, c)` receives
    /// `(first_keys[r], second_keys[c])`. Automatically falls back to `Sequential` for grids
    /// larger than `first_keys.len()` rows or `second_keys.len()` cols.
    #[default]
    RowColIndexed,
    /// Assigns labels sequentially in row-major order across the Cartesian product
    /// `first_keys x second_keys`, utilizing the entire capacity up to `|first| * |second|`.
    Sequential,
}

/// Configurable keyboard alphabet & key-row layout for grid/subgrid labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelScheme {
    /// Primary key alphabet (first character of 2-char labels, or sole character of 1-char labels).
    #[serde(default = "default_lowercase_alphabet")]
    pub alphabet: Vec<char>,
    /// Optional distinct second-character alphabet for 2-char labels. When `None`, `alphabet` is reused.
    #[serde(default)]
    pub second_alphabet: Option<Vec<char>>,
    /// When `true`, uppercase and lowercase keys are treated as distinct labels (e.g. `a` != `A`),
    /// expanding `a-zA-Z` to 52 keys (2,704 two-character labels, > 676).
    #[serde(default)]
    pub case_sensitive: bool,
    /// Optional custom key-row layout (e.g. QWERTY/Colemak/Dvorak rows) preserved for UI & spatial mapping.
    #[serde(default)]
    pub key_rows: Option<Vec<String>>,
    /// Label assignment strategy for 2-character grids.
    #[serde(default)]
    pub ordering: LabelOrdering,
    /// Preserves unknown/future keys during config load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

fn default_lowercase_alphabet() -> Vec<char> {
    ('a'..='z').collect()
}

impl Default for LabelScheme {
    fn default() -> Self {
        Self::lowercase_az()
    }
}

impl LabelScheme {
    /// Standard 26-letter lowercase alphabet (`a`..=`z`, 676 two-char labels, case-insensitive).
    pub fn lowercase_az() -> Self {
        Self {
            alphabet: ('a'..='z').collect(),
            second_alphabet: None,
            case_sensitive: false,
            key_rows: Some(vec![
                "qwertyuiop".to_string(),
                "asdfghjkl".to_string(),
                "zxcvbnm".to_string(),
            ]),
            ordering: LabelOrdering::RowColIndexed,
            unknown_fields: BTreeMap::new(),
        }
    }

    /// 52-letter mixed-case alphabet (`a`..=`z` + `A`..=`Z`, 2,704 two-char labels, case-sensitive).
    pub fn mixed_case_az() -> Self {
        let mut alphabet: Vec<char> = ('a'..='z').collect();
        alphabet.extend('A'..='Z');
        Self {
            alphabet,
            second_alphabet: None,
            case_sensitive: true,
            key_rows: Some(vec![
                "qwertyuiop".to_string(),
                "asdfghjkl".to_string(),
                "zxcvbnm".to_string(),
                "QWERTYUIOP".to_string(),
                "ASDFGHJKL".to_string(),
                "ZXCVBNM".to_string(),
            ]),
            ordering: LabelOrdering::RowColIndexed,
            unknown_fields: BTreeMap::new(),
        }
    }

    /// Construct a scheme from custom keyboard row strings (e.g. `["qwertyuiop", "asdfghjkl;", "zxcvbnm,./"]`).
    pub fn from_key_rows(rows: &[&str], case_sensitive: bool) -> Result<Self, CoreError> {
        let mut alphabet = Vec::new();
        let mut key_rows = Vec::with_capacity(rows.len());
        for row in rows {
            key_rows.push((*row).to_string());
            for ch in row.chars() {
                alphabet.push(ch);
            }
        }
        let scheme = Self {
            alphabet,
            second_alphabet: None,
            case_sensitive,
            key_rows: Some(key_rows),
            ordering: LabelOrdering::RowColIndexed,
            unknown_fields: BTreeMap::new(),
        };
        scheme.validate()?;
        Ok(scheme)
    }

    /// Standard 30-key QWERTY layout (60 keys when `case_sensitive` is true -> 3,600 2-char labels).
    pub fn qwerty_rows(case_sensitive: bool) -> Self {
        if case_sensitive {
            Self::from_key_rows(
                &[
                    "qwertyuiop",
                    "asdfghjkl;",
                    "zxcvbnm,./",
                    "QWERTYUIOP",
                    "ASDFGHJKL:",
                    "ZXCVBNM<>?",
                ],
                true,
            )
            .expect("built-in QWERTY rows are valid")
        } else {
            Self::from_key_rows(&["qwertyuiop", "asdfghjkl;", "zxcvbnm,./"], false)
                .expect("built-in QWERTY rows are valid")
        }
    }

    /// First-character alphabet slice.
    pub fn first_keys(&self) -> &[char] {
        &self.alphabet
    }

    /// Second-character alphabet slice (falls back to `alphabet` when `second_alphabet` is `None`).
    pub fn second_keys(&self) -> &[char] {
        self.second_alphabet.as_deref().unwrap_or(&self.alphabet)
    }

    /// Validate that alphabets are non-empty and contain no duplicate characters under `case_sensitive`.
    pub fn validate(&self) -> Result<(), CoreError> {
        validate_key_slice(&self.alphabet, self.case_sensitive, "alphabet")?;
        if let Some(ref second) = self.second_alphabet {
            validate_key_slice(second, self.case_sensitive, "second_alphabet")?;
        }
        Ok(())
    }

    /// Maximum number of unique labels representable with `label_length` (1 or 2).
    pub fn capacity(&self, label_length: u8) -> usize {
        match label_length {
            1 => self.first_keys().len(),
            2 => self.first_keys().len().saturating_mul(self.second_keys().len()),
            _ => 0,
        }
    }

    /// Generate `cols * rows` deterministic, unique labels in row-major order.
    pub fn generate_labels(
        &self,
        cols: u16,
        rows: u16,
        label_length: u8,
    ) -> Result<Vec<String>, CoreError> {
        self.validate()?;
        if cols == 0 || rows == 0 {
            return Err(CoreError::InvalidGridDimensions {
                cols,
                rows,
                reason: "cols and rows must both be >= 1".to_string(),
            });
        }
        if label_length != 1 && label_length != 2 {
            return Err(CoreError::InvalidLabelScheme(format!(
                "label_length must be 1 or 2, got {label_length}"
            )));
        }

        let total = (cols as usize) * (rows as usize);
        let available = self.capacity(label_length);
        if total > available {
            return Err(CoreError::InsufficientLabelCapacity {
                required: total,
                available,
            });
        }

        let first = self.first_keys();
        let second = self.second_keys();
        let mut labels = Vec::with_capacity(total);

        if label_length == 1 {
            for &ch in first.iter().take(total) {
                labels.push(ch.to_string());
            }
            return Ok(labels);
        }

        let use_row_col = matches!(self.ordering, LabelOrdering::RowColIndexed)
            && (rows as usize) <= first.len()
            && (cols as usize) <= second.len();

        if use_row_col {
            for &c1 in first.iter().take(rows as usize) {
                for &c2 in second.iter().take(cols as usize) {
                    let mut s = String::with_capacity(c1.len_utf8() + c2.len_utf8());
                    s.push(c1);
                    s.push(c2);
                    labels.push(s);
                }
            }
        } else {
            let s_len = second.len();
            for idx in 0..total {
                let c1 = first[idx / s_len];
                let c2 = second[idx % s_len];
                let mut s = String::with_capacity(c1.len_utf8() + c2.len_utf8());
                s.push(c1);
                s.push(c2);
                labels.push(s);
            }
        }

        Ok(labels)
    }
}

fn validate_key_slice(keys: &[char], case_sensitive: bool, name: &str) -> Result<(), CoreError> {
    if keys.is_empty() {
        return Err(CoreError::InvalidLabelScheme(format!(
            "{name} must not be empty"
        )));
    }
    let mut seen = HashSet::with_capacity(keys.len());
    for &ch in keys {
        let norm = normalize_char(ch, case_sensitive);
        if !seen.insert(norm) {
            return Err(CoreError::InvalidLabelScheme(format!(
                "{name} contains duplicate key '{ch}' (case_sensitive={case_sensitive})"
            )));
        }
    }
    Ok(())
}

pub(crate) fn normalize_char(ch: char, case_sensitive: bool) -> String {
    if case_sensitive {
        ch.to_string()
    } else {
        ch.to_lowercase().collect()
    }
}

pub(crate) fn normalize_str(s: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        s.to_string()
    } else {
        s.to_lowercase()
    }
}

/// Configuration for the primary NxM grid and nested subgrid subdivision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridConfig {
    #[serde(default = "default_grid_cols")]
    pub cols: u16,
    #[serde(default = "default_grid_rows")]
    pub rows: u16,
    #[serde(default = "default_label_length")]
    pub label_length: u8,
    #[serde(default)]
    pub label_scheme: LabelScheme,
    #[serde(default = "default_subgrid_cols")]
    pub subgrid_cols: u16,
    #[serde(default = "default_subgrid_rows")]
    pub subgrid_rows: u16,
    /// Keys assigned to subgrid cells (defaults to 3x3 left-hand cluster `qwe/asd/zxc`).
    #[serde(default = "default_subgrid_keys")]
    pub subgrid_keys: Vec<char>,
    /// Preserves unknown/future keys during config load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

fn default_grid_cols() -> u16 {
    16
}
fn default_grid_rows() -> u16 {
    12
}
fn default_label_length() -> u8 {
    2
}
fn default_subgrid_cols() -> u16 {
    3
}
fn default_subgrid_rows() -> u16 {
    3
}
pub fn default_subgrid_keys() -> Vec<char> {
    vec!['q', 'w', 'e', 'a', 's', 'd', 'z', 'x', 'c']
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            cols: default_grid_cols(),
            rows: default_grid_rows(),
            label_length: default_label_length(),
            label_scheme: LabelScheme::default(),
            subgrid_cols: default_subgrid_cols(),
            subgrid_rows: default_subgrid_rows(),
            subgrid_keys: default_subgrid_keys(),
            unknown_fields: BTreeMap::new(),
        }
    }
}

/// A single cell in a primary grid or nested subgrid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridCell {
    pub label: String,
    pub row: u16,
    pub col: u16,
    pub rect: LogicalRect,
    pub center: LogicalPoint,
}

/// Result of matching a typed key prefix against a `Grid`.
#[derive(Debug, Clone, PartialEq)]
pub enum PrefixMatch<'a> {
    /// Exact unique match ready for activation.
    Exact(&'a GridCell),
    /// One or more cells share the typed prefix, waiting for further input.
    Candidates(Vec<&'a GridCell>),
    /// No cell matches the typed prefix.
    NoMatch,
}

/// A generated primary grid or subgrid covering a `LogicalRect`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    pub monitor_id: String,
    pub bounds: LogicalRect,
    pub cols: u16,
    pub rows: u16,
    pub case_sensitive: bool,
    pub cells: Vec<GridCell>,
}

impl Grid {
    /// Match a typed key prefix against all cells in this grid.
    pub fn match_prefix(&self, prefix: &str) -> PrefixMatch<'_> {
        if prefix.is_empty() {
            return PrefixMatch::Candidates(self.cells.iter().collect());
        }
        let norm_prefix = normalize_str(prefix, self.case_sensitive);
        let mut exact: Option<&GridCell> = None;
        let mut candidates = Vec::new();

        for cell in &self.cells {
            let norm_label = normalize_str(&cell.label, self.case_sensitive);
            if norm_label == norm_prefix {
                exact = Some(cell);
                candidates.push(cell);
            } else if norm_label.starts_with(&norm_prefix) {
                candidates.push(cell);
            }
        }

        if let Some(cell) = exact {
            if candidates.len() == 1 {
                return PrefixMatch::Exact(cell);
            }
        }
        if candidates.is_empty() {
            PrefixMatch::NoMatch
        } else {
            PrefixMatch::Candidates(candidates)
        }
    }

    /// Look up a cell by exact label (respecting `self.case_sensitive`).
    pub fn cell_by_label(&self, label: &str) -> Option<&GridCell> {
        let norm = normalize_str(label, self.case_sensitive);
        self.cells
            .iter()
            .find(|c| normalize_str(&c.label, self.case_sensitive) == norm)
    }

    /// Find the cell containing `point` (clamping exact right/bottom edges into the final cell).
    pub fn cell_at_point(&self, point: LogicalPoint) -> Option<&GridCell> {
        if !self.bounds.contains_point_inclusive(point) || self.cols == 0 || self.rows == 0 {
            return None;
        }
        let rel_x = ((point.x - self.bounds.x) / self.bounds.width).clamp(0.0, 1.0);
        let rel_y = ((point.y - self.bounds.y) / self.bounds.height).clamp(0.0, 1.0);
        let col = ((rel_x * self.cols as f64).floor() as u16).min(self.cols - 1);
        let row = ((rel_y * self.rows as f64).floor() as u16).min(self.rows - 1);
        let idx = (row as usize) * (self.cols as usize) + (col as usize);
        self.cells.get(idx)
    }
}

/// Generate an NxM primary grid across `monitor.logical_rect`.
pub fn generate_grid(monitor: &MonitorInfo, config: &GridConfig) -> Result<Grid, CoreError> {
    monitor.validate()?;
    generate_grid_in_rect(monitor.logical_rect, &monitor.id, config)
}

/// Generate an NxM grid covering an arbitrary `LogicalRect`.
pub fn generate_grid_in_rect(
    bounds: LogicalRect,
    monitor_id: impl Into<String>,
    config: &GridConfig,
) -> Result<Grid, CoreError> {
    if !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err(CoreError::InvalidGridDimensions {
            cols: config.cols,
            rows: config.rows,
            reason: "bounds width and height must be finite and positive".to_string(),
        });
    }

    let labels = config
        .label_scheme
        .generate_labels(config.cols, config.rows, config.label_length)?;

    let cells = tile_rect_with_labels(bounds, config.cols, config.rows, labels);

    Ok(Grid {
        monitor_id: monitor_id.into(),
        bounds,
        cols: config.cols,
        rows: config.rows,
        case_sensitive: config.label_scheme.case_sensitive,
        cells,
    })
}

/// Generate a nested `subgrid_cols x subgrid_rows` subgrid inside `parent_cell.rect`.
///
/// Uses single-character `config.subgrid_keys` when they cover all subgrid cells, and
/// falls back cleanly to `config.label_scheme` (1-char if sufficient, else 2-char) for
/// larger custom subgrids.
pub fn generate_subgrid(
    parent_cell: &GridCell,
    monitor_id: impl Into<String>,
    config: &GridConfig,
) -> Result<Grid, CoreError> {
    generate_subgrid_in_rect(parent_cell.rect, monitor_id, config)
}

/// Generate a nested subgrid inside any `LogicalRect`.
pub fn generate_subgrid_in_rect(
    bounds: LogicalRect,
    monitor_id: impl Into<String>,
    config: &GridConfig,
) -> Result<Grid, CoreError> {
    let cols = config.subgrid_cols;
    let rows = config.subgrid_rows;
    if cols == 0 || rows == 0 {
        return Err(CoreError::InvalidGridDimensions {
            cols,
            rows,
            reason: "subgrid cols and rows must both be >= 1".to_string(),
        });
    }
    if !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err(CoreError::InvalidGridDimensions {
            cols,
            rows,
            reason: "subgrid bounds width and height must be finite and positive".to_string(),
        });
    }

    let total = (cols as usize) * (rows as usize);
    let case_sensitive = config.label_scheme.case_sensitive;

    let labels = if !config.subgrid_keys.is_empty() && config.subgrid_keys.len() >= total {
        validate_key_slice(&config.subgrid_keys, case_sensitive, "subgrid_keys")?;
        config
            .subgrid_keys
            .iter()
            .take(total)
            .map(|c| c.to_string())
            .collect()
    } else if config.label_scheme.capacity(1) >= total {
        config.label_scheme.generate_labels(cols, rows, 1)?
    } else {
        config.label_scheme.generate_labels(cols, rows, 2)?
    };

    let cells = tile_rect_with_labels(bounds, cols, rows, labels);

    Ok(Grid {
        monitor_id: monitor_id.into(),
        bounds,
        cols,
        rows,
        case_sensitive,
        cells,
    })
}

/// Deterministically subdivide `bounds` into `cols x rows` edge-exact cells.
///
/// Computing edges via `bounds.x + col * width / cols` and `bounds.x + (col + 1) * width / cols`
/// guarantees zero gaps between adjacent cells and exact boundary alignment at `col == cols - 1`.
fn tile_rect_with_labels(
    bounds: LogicalRect,
    cols: u16,
    rows: u16,
    labels: Vec<String>,
) -> Vec<GridCell> {
    let cols_f = cols as f64;
    let rows_f = rows as f64;
    let mut cells = Vec::with_capacity(labels.len());

    for row in 0..rows {
        let y0 = bounds.y + (row as f64) * bounds.height / rows_f;
        let y1 = if row + 1 == rows {
            bounds.bottom()
        } else {
            bounds.y + ((row + 1) as f64) * bounds.height / rows_f
        };
        let cell_h = y1 - y0;

        for col in 0..cols {
            let x0 = bounds.x + (col as f64) * bounds.width / cols_f;
            let x1 = if col + 1 == cols {
                bounds.right()
            } else {
                bounds.x + ((col + 1) as f64) * bounds.width / cols_f
            };
            let cell_w = x1 - x0;
            let idx = (row as usize) * (cols as usize) + (col as usize);
            let rect = LogicalRect::new(x0, y0, cell_w, cell_h);
            let center = LogicalPoint::new((x0 + x1) * 0.5, (y0 + y1) * 0.5);

            cells.push(GridCell {
                label: labels[idx].clone(),
                row,
                col,
                rect,
                center,
            });
        }
    }

    cells
}

/// Precision tier for keyboard nudging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NudgeStep {
    Coarse,
    #[default]
    Normal,
    Fine,
}

/// Clamping boundary policy during Nudge mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NudgeClampMode {
    /// Keep nudging strictly inside the active monitor's valid pixel bounds.
    #[default]
    ActiveMonitor,
    /// Allow nudging across adjacent monitors while clamping cleanly to the virtual desktop union.
    VirtualDesktop,
    /// Do not clamp coordinates (primarily useful for tests or virtual canvases).
    Unclamped,
}

/// Nudge distances (in logical pixels) and clamping policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NudgeConfig {
    #[serde(default = "default_coarse_px")]
    pub coarse_px: f64,
    #[serde(default = "default_normal_px")]
    pub normal_px: f64,
    #[serde(default = "default_fine_px")]
    pub fine_px: f64,
    #[serde(default)]
    pub clamp_mode: NudgeClampMode,
    /// Preserves unknown/future keys during config load/save round-trips.
    #[serde(flatten, default)]
    pub unknown_fields: BTreeMap<String, serde_json::Value>,
}

fn default_coarse_px() -> f64 {
    24.0
}
fn default_normal_px() -> f64 {
    8.0
}
fn default_fine_px() -> f64 {
    1.5
}

impl Default for NudgeConfig {
    fn default() -> Self {
        Self {
            coarse_px: default_coarse_px(),
            normal_px: default_normal_px(),
            fine_px: default_fine_px(),
            clamp_mode: NudgeClampMode::ActiveMonitor,
            unknown_fields: BTreeMap::new(),
        }
    }
}

impl NudgeConfig {
    /// Logical pixel magnitude for a `NudgeStep`.
    pub fn step_distance(&self, step: NudgeStep) -> f64 {
        match step {
            NudgeStep::Coarse => self.coarse_px.max(0.0),
            NudgeStep::Normal => self.normal_px.max(0.0),
            NudgeStep::Fine => self.fine_px.max(0.0),
        }
    }
}

/// Apply a directional nudge step to `current`, clamping according to `config.clamp_mode`.
///
/// Returns `(resulting_monitor_id, resulting_logical_point)`.
pub fn nudge_in_direction(
    current: LogicalPoint,
    direction: Direction,
    step: NudgeStep,
    config: &NudgeConfig,
    active_monitor: &MonitorInfo,
    layout: &MonitorLayout,
) -> (String, LogicalPoint) {
    let (dx, dy) = direction.unit_vector();
    nudge_by_vector(current, dx, dy, step, config, active_monitor, layout)
}

/// Apply an arbitrary direction vector `(dx, dy)` scaled by `step` distance and clamped cleanly.
pub fn nudge_by_vector(
    current: LogicalPoint,
    dx: f64,
    dy: f64,
    step: NudgeStep,
    config: &NudgeConfig,
    active_monitor: &MonitorInfo,
    layout: &MonitorLayout,
) -> (String, LogicalPoint) {
    let dist = config.step_distance(step);
    let candidate = LogicalPoint::new(current.x + dx * dist, current.y + dy * dist);

    match config.clamp_mode {
        NudgeClampMode::ActiveMonitor => (
            active_monitor.id.clone(),
            clamp_logical_to_monitor(candidate, active_monitor),
        ),
        NudgeClampMode::VirtualDesktop => layout
            .clamp_logical_to_virtual_desktop(candidate)
            .unwrap_or_else(|| {
                (
                    active_monitor.id.clone(),
                    clamp_logical_to_monitor(candidate, active_monitor),
                )
            }),
        NudgeClampMode::Unclamped => {
            let mon_id = layout
                .nearest_monitor_logical(candidate)
                .map(|m| m.id.clone())
                .unwrap_or_else(|| active_monitor.id.clone());
            (mon_id, candidate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysicalPoint;

    fn test_monitor() -> MonitorInfo {
        MonitorInfo::from_logical_and_scale(
            "DP-1",
            "Primary",
            LogicalRect::new(0.0, 0.0, 1440.0, 900.0),
            PhysicalPoint::new(0, 0),
            2.0,
            true,
        )
        .unwrap()
    }

    #[test]
    fn generates_large_case_sensitive_grid_over_676_cells() {
        let mon = test_monitor();
        let cfg = GridConfig {
            cols: 30,
            rows: 30, // 900 cells > 676!
            label_length: 2,
            label_scheme: LabelScheme::mixed_case_az(),
            ..Default::default()
        };
        let grid = generate_grid(&mon, &cfg).unwrap();
        assert_eq!(grid.cells.len(), 900);
        assert!(grid.case_sensitive);

        // Verify all 900 labels are strictly unique
        let unique: HashSet<&str> = grid.cells.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(unique.len(), 900);

        // Case-sensitive matching distinguishes "aa" from "AA"
        assert!(matches!(grid.match_prefix("aa"), PrefixMatch::Exact(c) if c.label == "aa"));
        assert!(matches!(grid.match_prefix("AA"), PrefixMatch::Exact(c) if c.label == "AA"));
    }

    #[test]
    fn case_insensitive_scheme_rejects_over_676_cells() {
        let mon = test_monitor();
        let cfg = GridConfig {
            cols: 27,
            rows: 26, // 702 > 676
            label_length: 2,
            label_scheme: LabelScheme::lowercase_az(),
            ..Default::default()
        };
        let err = generate_grid(&mon, &cfg).unwrap_err();
        assert_eq!(
            err,
            CoreError::InsufficientLabelCapacity {
                required: 702,
                available: 676
            }
        );
    }

    #[test]
    fn grid_cells_tile_bounds_with_zero_gaps_on_negative_origin() {
        let left = MonitorInfo::from_logical_and_scale(
            "HDMI-1",
            "Left",
            LogicalRect::new(-1920.0, -540.0, 1920.0, 1080.0),
            PhysicalPoint::new(-2400, -675),
            1.25,
            false,
        )
        .unwrap();
        let cfg = GridConfig {
            cols: 7,
            rows: 5,
            ..Default::default()
        };
        let grid = generate_grid(&left, &cfg).unwrap();
        assert_eq!(grid.cells.len(), 35);

        // Top-left cell starts exactly at monitor origin
        assert_eq!(grid.cells[0].rect.x, -1920.0);
        assert_eq!(grid.cells[0].rect.y, -540.0);

        // Bottom-right cell ends exactly at monitor right/bottom
        let last = grid.cells.last().unwrap();
        assert!((last.rect.right() - 0.0).abs() < 1e-12);
        assert!((last.rect.bottom() - 540.0).abs() < 1e-12);

        // Adjacent cells share exact boundaries
        for row in 0..5 {
            for col in 0..6 {
                let a = &grid.cells[row * 7 + col];
                let b = &grid.cells[row * 7 + col + 1];
                assert!((a.rect.right() - b.rect.x).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn subgrid_subdivides_parent_cell_precisely() {
        let mon = test_monitor();
        let cfg = GridConfig {
            cols: 12,
            rows: 9,
            subgrid_cols: 3,
            subgrid_rows: 3,
            ..Default::default()
        };
        let grid = generate_grid(&mon, &cfg).unwrap();
        let parent = &grid.cells[0]; // 120 x 100 rect at (0, 0)
        let sub = generate_subgrid(parent, &mon.id, &cfg).unwrap();
        assert_eq!(sub.cells.len(), 9);
        assert_eq!(sub.cells[0].label, "q");
        assert_eq!(sub.cells[4].label, "s"); // exact center of parent cell
        assert!((sub.cells[4].center.x - parent.center.x).abs() < 1e-9);
        assert!((sub.cells[4].center.y - parent.center.y).abs() < 1e-9);
    }

    #[test]
    fn custom_key_rows_work_and_reject_duplicates() {
        let colemak = LabelScheme::from_key_rows(&["qwfpgjluy;", "arstdhneio", "zxcvbkm,./"], false)
            .unwrap();
        assert_eq!(colemak.capacity(2), 900);

        let dup = LabelScheme::from_key_rows(&["abc", "Aef"], false);
        assert!(matches!(dup, Err(CoreError::InvalidLabelScheme(_))));

        // Same rows succeed when case_sensitive = true
        let ok = LabelScheme::from_key_rows(&["abc", "Aef"], true).unwrap();
        assert_eq!(ok.capacity(1), 6);
    }

    #[test]
    fn nudge_clamps_to_active_monitor_or_crosses_virtual_desktop() {
        let left = MonitorInfo::from_logical_and_scale(
            "LEFT",
            "Left",
            LogicalRect::new(-1920.0, 0.0, 1920.0, 1080.0),
            PhysicalPoint::new(-1920, 0),
            1.0,
            false,
        )
        .unwrap();
        let primary = MonitorInfo::from_logical_and_scale(
            "PRIMARY",
            "Primary",
            LogicalRect::new(0.0, 0.0, 1440.0, 900.0),
            PhysicalPoint::new(0, 0),
            2.0,
            true,
        )
        .unwrap();
        let layout = MonitorLayout::new(vec![left, primary.clone()]).unwrap();

        // ActiveMonitor clamp prevents crossing left edge of PRIMARY
        let cfg_mon = NudgeConfig {
            coarse_px: 50.0,
            clamp_mode: NudgeClampMode::ActiveMonitor,
            ..Default::default()
        };
        let (mid, pt) = nudge_in_direction(
            LogicalPoint::new(10.0, 100.0),
            Direction::Left,
            NudgeStep::Coarse,
            &cfg_mon,
            &primary,
            &layout,
        );
        assert_eq!(mid, "PRIMARY");
        assert_eq!(pt, LogicalPoint::new(0.0, 100.0));

        // VirtualDesktop clamp allows seamless crossing onto LEFT monitor
        let cfg_vd = NudgeConfig {
            coarse_px: 50.0,
            clamp_mode: NudgeClampMode::VirtualDesktop,
            ..Default::default()
        };
        let (mid2, pt2) = nudge_in_direction(
            LogicalPoint::new(10.0, 100.0),
            Direction::Left,
            NudgeStep::Coarse,
            &cfg_vd,
            &primary,
            &layout,
        );
        assert_eq!(mid2, "LEFT");
        assert_eq!(pt2, LogicalPoint::new(-40.0, 100.0));
    }
}
