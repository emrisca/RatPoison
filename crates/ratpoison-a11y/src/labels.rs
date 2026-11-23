//! Deterministic Label Mode badge generator for RatPoison.
//!
//! Performs spatial filtering, occlusion pruning, bounding-box IoU & centroid deduplication,
//! and assigns prefix-free ergonomic home-row-first 1–2 character hint labels.

use crate::{AccessibleElement, ElementRole, LogicalPoint, LogicalRect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default home-row-first key alphabet used for prefix-free badge generation.
pub const DEFAULT_HINT_KEYS: &str = "asdfghjklqwertyuiopzxcvbnm";

/// Configuration for the Label Mode badge generator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelConfig {
    /// Viewport bounds of the active monitor in logical points.
    pub viewport: LogicalRect,
    /// Minimum width or height (in logical points) for an element to be considered actionable.
    pub min_dimension: f64,
    /// Maximum Intersection-over-Union (IoU) threshold before two overlapping boxes are considered duplicates.
    pub iou_threshold: f64,
    /// Maximum Euclidean distance (in logical points) between centroids before two overlapping elements are merged.
    pub centroid_distance_threshold: f64,
    /// Key characters ordered by ergonomic preference (home-row first).
    pub hint_alphabet: String,
    /// Optional cursor focus point; when present, elements can be sorted by proximity or reading order.
    pub cursor_position: Option<LogicalPoint>,
    /// Whether to sort primarily by proximity to `cursor_position` (if provided) before reading order.
    pub sort_by_cursor_proximity: bool,
    /// Maximum number of badges to emit.
    pub max_badges: usize,
}

impl Default for LabelConfig {
    fn default() -> Self {
        Self {
            viewport: LogicalRect::new(0.0, 0.0, 1920.0, 1080.0),
            min_dimension: 6.0,
            iou_threshold: 0.65,
            centroid_distance_threshold: 8.0,
            hint_alphabet: DEFAULT_HINT_KEYS.to_string(),
            cursor_position: None,
            sort_by_cursor_proximity: false,
            max_badges: 500,
        }
    }
}

/// A rendered Label Mode badge associated with an accessible UI element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelBadge {
    /// Prefix-free 1- or 2-character keyboard hint (e.g., `"a"`, `"sd"`).
    pub hint: String,
    /// Target click coordinate in logical screen points.
    pub target: LogicalPoint,
    /// Clipped bounding rectangle in logical screen points.
    pub bounds: LogicalRect,
    /// Underlying accessible element metadata.
    pub element: AccessibleElement,
}

/// Generates prefix-free 1–2 character hint strings for `count` items using `alphabet`.
///
/// To guarantee prefix-freedom:
/// - If `count <= K` (where `K` is the number of unique characters in `alphabet`),
///   we emit 1-character hints directly (`a`, `s`, `d`, ...).
/// - If `count > K`, we reserve the first `S` characters as single-key hints and the remaining
///   `P = K - S` characters as first-key prefixes for 2-character hints (`P * K` combinations),
///   choosing the largest `S` such that `S + (K - S) * K >= count`.
/// - If `count > K * K`, we use all 2-character combinations (`K * K`).
pub fn generate_prefix_free_hints(count: usize, alphabet: &str) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }

    let mut chars = Vec::new();
    for c in alphabet.chars() {
        let lower = c.to_ascii_lowercase();
        if !chars.contains(&lower) && !lower.is_whitespace() {
            chars.push(lower);
        }
    }
    if chars.is_empty() {
        chars.extend("asdfghjklqwertyuiopzxcvbnm".chars());
    }

    let k = chars.len();
    if count <= k {
        return chars.into_iter().take(count).map(|c| c.to_string()).collect();
    }

    // Solve for largest number of single-char keys `s` in 0..k such that:
    // s + (k - s) * k >= count
    // => s * (k - 1) <= k * k - count
    let max_capacity = k * k;
    let target = count.min(max_capacity);
    let single_count = if k > 1 {
        (k * k - target) / (k - 1)
    } else {
        0
    };

    let mut hints = Vec::with_capacity(target);

    // Emit single-character hints first (most ergonomic home-row keys).
    for &c in chars.iter().take(single_count) {
        hints.push(c.to_string());
    }

    // Emit two-character hints using the remaining prefix characters.
    'outer: for &prefix in chars.iter().skip(single_count) {
        for &suffix in &chars {
            if hints.len() >= target {
                break 'outer;
            }
            let mut s = String::with_capacity(2);
            s.push(prefix);
            s.push(suffix);
            hints.push(s);
        }
    }

    hints
}

/// Filters, deduplicates, sorts, and assigns prefix-free Label Mode badges to accessible elements.
pub fn generate_label_badges(
    elements: &[AccessibleElement],
    config: &LabelConfig,
) -> Vec<LabelBadge> {
    // 1. Basic visibility, actionability, and viewport intersection filter.
    // Drop oversized structural boxes (>30% of viewport area) that represent whole panels/containers.
    let max_element_area = (config.viewport.area() * 0.30).max(1.0);
    let mut candidates: Vec<(AccessibleElement, LogicalRect)> = elements
        .iter()
        .filter(|el| el.is_visible && (el.is_clickable || el.role.is_default_interactive()))
        .filter(|el| !matches!(el.role, ElementRole::Window | ElementRole::Toolbar))
        .filter_map(|el| {
            let clipped = el.bounds.intersection(&config.viewport)?;
            if clipped.width >= config.min_dimension
                && clipped.height >= config.min_dimension
                && clipped.area() <= max_element_area
            {
                Some((el.clone(), clipped))
            } else {
                None
            }
        })
        .collect();

    // 2. Occlusion & deduplication pass.
    // Sort by z_order ascending (0 = frontmost window/layer), then smaller area (more specific inner control
    // wins over a broad container wrapper with similar bounds), then stable ID.
    candidates.sort_by(|(a, rect_a), (b, rect_b)| {
        a.z_order
            .cmp(&b.z_order)
            .then_with(|| {
                rect_a
                    .area()
                    .partial_cmp(&rect_b.area())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut deduplicated: Vec<(AccessibleElement, LogicalRect)> = Vec::new();

    'candidate_loop: for (cand_el, cand_rect) in candidates {
        let cand_center = cand_rect.center();

        for (kept_el, kept_rect) in &deduplicated {
            let kept_center = kept_rect.center();

            // Check 2a: Near-duplicate bounding box IoU or very close centroids.
            let iou = cand_rect.iou(kept_rect);
            let center_dist = cand_center.distance_to(&kept_center);
            if iou >= config.iou_threshold || center_dist <= config.centroid_distance_threshold {
                continue 'candidate_loop;
            }

            // Check 2b: Occlusion by a strictly frontmost opaque window/container from another window layer.
            // If a frontmost element (`kept_el.z_order < cand_el.z_order`) completely covers `cand_rect`'s center
            // and covers >90% of `cand_rect`, `cand_el` is occluded.
            if kept_el.z_order < cand_el.z_order && kept_rect.contains_point(&cand_center) {
                if let Some(inter) = cand_rect.intersection(kept_rect) {
                    if cand_rect.area() > 0.0 && (inter.area() / cand_rect.area()) >= 0.90 {
                        continue 'candidate_loop;
                    }
                }
            }

            // Check 2c: Suppress redundant wrapper containers (`Container`, `Unknown`, `ListItem`)
            // that enclose a smaller, already-kept specific interactive child (e.g. a `Link` or `Button`
            // inside a `ListItem` row or `AXGroup` card). Because candidates are sorted by area ascending,
            // inner interactive controls are always evaluated first.
            if matches!(
                cand_el.role,
                ElementRole::Container | ElementRole::Unknown | ElementRole::ListItem
            ) && kept_el.z_order == cand_el.z_order
                && cand_rect.contains_point(&kept_center)
                && kept_rect.area() < cand_rect.area()
            {
                continue 'candidate_loop;
            }
        }

        deduplicated.push((cand_el, cand_rect));
    }

    // 3. Sort accepted elements in deterministic ergonomic assignment order:
    // If cursor proximity sorting is enabled and a cursor position is provided, closest elements
    // receive the single-key/home-row hints first, with reading order as tie-breaker.
    // Otherwise, sort by spatial reading order (top-to-bottom row bands of 12pt, then left-to-right).
    deduplicated.sort_by(|(a, rect_a), (b, rect_b)| {
        let center_a = rect_a.center();
        let center_b = rect_b.center();

        if config.sort_by_cursor_proximity {
            if let Some(cursor) = config.cursor_position {
                let dist_a = (center_a.x - cursor.x).hypot(center_a.y - cursor.y);
                let dist_b = (center_b.x - cursor.x).hypot(center_b.y - cursor.y);
                let dist_cmp = dist_a
                    .partial_cmp(&dist_b)
                    .unwrap_or(std::cmp::Ordering::Equal);
                if (dist_a - dist_b).abs() > 4.0 {
                    return dist_cmp;
                }
            }
        }

        // Quantize Y into 12-logical-point horizontal reading bands for clean left-to-right scanning.
        let row_a = (rect_a.y / 12.0).floor() as i64;
        let row_b = (rect_b.y / 12.0).floor() as i64;
        row_a
            .cmp(&row_b)
            .then_with(|| {
                rect_a
                    .x
                    .partial_cmp(&rect_b.x)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.id.cmp(&b.id))
    });

    if deduplicated.len() > config.max_badges {
        deduplicated.truncate(config.max_badges);
    }

    // 4. Generate prefix-free hints and pair them with the sorted elements.
    let hints = generate_prefix_free_hints(deduplicated.len(), &config.hint_alphabet);

    deduplicated
        .into_iter()
        .zip(hints)
        .map(|((mut element, clipped_bounds), hint)| {
            let target = clipped_bounds.center();
            element.center = target;
            LabelBadge {
                hint,
                target,
                bounds: clipped_bounds,
                element,
            }
        })
        .collect()
}

/// Interactive prefix-matcher state helper for resolving typed Label Mode key sequences.
#[derive(Debug, Clone, PartialEq)]
pub enum HintMatchResult<'a> {
    /// No badge starts with the typed buffer.
    NoMatch,
    /// One or more badges share this strict prefix; wait for next keystroke.
    PartialMatch(Vec<&'a LabelBadge>),
    /// Exact prefix-free match found; trigger click immediately.
    ExactMatch(&'a LabelBadge),
}

/// Evaluates a typed key buffer against an active list of `LabelBadge`s.
pub fn match_typed_hint<'a>(badges: &'a [LabelBadge], typed: &str) -> HintMatchResult<'a> {
    let normalized = typed.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return HintMatchResult::NoMatch;
    }

    let mut partials = Vec::new();
    for badge in badges {
        if badge.hint == normalized {
            return HintMatchResult::ExactMatch(badge);
        }
        if badge.hint.starts_with(&normalized) {
            partials.push(badge);
        }
    }

    if partials.is_empty() {
        HintMatchResult::NoMatch
    } else {
        HintMatchResult::PartialMatch(partials)
    }
}

/// Builds a fast lookup index from hint string to badge index.
pub fn index_badges(badges: &[LabelBadge]) -> HashMap<String, usize> {
    badges
        .iter()
        .enumerate()
        .map(|(idx, b)| (b.hint.clone(), idx))
        .collect()
}
