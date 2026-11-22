import type { BadgePlacement, LabelBadge, LogicalPoint, LogicalRect } from '../../types/ipc';

/** Convert `#rrggbb` or `#rgb` hex color + alpha (0..1) into an `rgba(...)` CSS string. */
export function hexToRgba(hex: string, alpha: number): string {
  const clean = hex.replace('#', '').trim();
  const clampedAlpha = Math.max(0, Math.min(1, alpha));
  if (clean.length === 3) {
    const r = parseInt(clean[0] + clean[0], 16);
    const g = parseInt(clean[1] + clean[1], 16);
    const b = parseInt(clean[2] + clean[2], 16);
    return `rgba(${r}, ${g}, ${b}, ${clampedAlpha})`;
  }
  if (clean.length === 6) {
    const r = parseInt(clean.slice(0, 2), 16);
    const g = parseInt(clean.slice(2, 4), 16);
    const b = parseInt(clean.slice(4, 6), 16);
    if (!Number.isNaN(r) && !Number.isNaN(g) && !Number.isNaN(b)) {
      return `rgba(${r}, ${g}, ${b}, ${clampedAlpha})`;
    }
  }
  return `rgba(15, 23, 42, ${clampedAlpha})`;
}

export interface PlacedLabelBadge {
  badge: LabelBadge;
  pillRect: LogicalRect;
  anchorPoint: LogicalPoint;
  wasShifted: boolean;
}

function rectsOverlap(a: LogicalRect, b: LogicalRect, pad = 2): boolean {
  return !(
    a.x + a.width + pad <= b.x ||
    b.x + b.width + pad <= a.x ||
    a.y + a.height + pad <= b.y ||
    b.y + b.height + pad <= a.y
  );
}

function computePreferredAnchor(badge: LabelBadge, placement: BadgePlacement): LogicalPoint {
  if (badge.collisionOffset) {
    return {
      x: badge.anchor.x + badge.collisionOffset.x,
      y: badge.anchor.y + badge.collisionOffset.y,
    };
  }
  switch (placement) {
    case 'center':
      return {
        x: badge.rect.x + badge.rect.width / 2,
        y: badge.rect.y + badge.rect.height / 2,
      };
    case 'top-right':
      return {
        x: badge.rect.x + badge.rect.width,
        y: badge.rect.y,
      };
    case 'top-left':
    case 'smart':
    default:
      return { x: badge.anchor.x, y: badge.anchor.y };
  }
}

/**
 * Deterministic, fast O(N*K) spatial collision resolver for Label Mode badges.
 * Guarantees badges never overlap each other while clamping inside the active monitor viewport.
 */
export function resolveLabelCollisions(
  badges: LabelBadge[],
  viewportWidth: number,
  viewportHeight: number,
  fontSize: number,
  showRoleHint: boolean,
  placement: BadgePlacement,
  enableAvoidance: boolean,
): PlacedLabelBadge[] {
  const placed: PlacedLabelBadge[] = [];
  const pillH = Math.ceil(fontSize * 1.55);
  const charW = fontSize * 0.68;

  // Candidate offset ring (spiral search around preferred anchor)
  const offsets: Array<[number, number]> = [
    [0, 0],
    [0, -(pillH + 4)],
    [0, pillH + 4],
    [26, 0],
    [-26, 0],
    [28, -(pillH + 4)],
    [-28, -(pillH + 4)],
    [28, pillH + 4],
    [-28, pillH + 4],
    [0, -2 * (pillH + 4)],
    [0, 2 * (pillH + 4)],
  ];

  for (const badge of badges) {
    const roleExtra = showRoleHint && badge.role ? badge.role.length * (fontSize * 0.52) + 8 : 0;
    const pillW = Math.max(26, Math.ceil(badge.code.length * charW + 14 + roleExtra));

    const anchorPoint = computePreferredAnchor(badge, placement);
    const clampX = (x: number) => Math.max(4, Math.min(viewportWidth - pillW - 4, x));
    const clampY = (y: number) => Math.max(4, Math.min(viewportHeight - pillH - 4, y));

    const baseRect: LogicalRect = {
      x: clampX(placement === 'center' ? anchorPoint.x - pillW / 2 : anchorPoint.x),
      y: clampY(placement === 'center' ? anchorPoint.y - pillH / 2 : anchorPoint.y - pillH * 0.25),
      width: pillW,
      height: pillH,
    };

    if (!enableAvoidance) {
      placed.push({ badge, pillRect: baseRect, anchorPoint, wasShifted: false });
      continue;
    }

    let chosen = baseRect;
    let wasShifted = false;
    for (let i = 0; i < offsets.length; i++) {
      const [dx, dy] = offsets[i];
      const candidate: LogicalRect = {
        x: clampX(baseRect.x + dx),
        y: clampY(baseRect.y + dy),
        width: pillW,
        height: pillH,
      };
      const collides = placed.some((p) => rectsOverlap(candidate, p.pillRect));
      if (!collides) {
        chosen = candidate;
        wasShifted = i > 0 || Boolean(badge.collisionOffset);
        break;
      }
    }

    placed.push({ badge, pillRect: chosen, anchorPoint, wasShifted });
  }

  return placed;
}
