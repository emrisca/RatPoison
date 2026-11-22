import { useMemo } from 'react';
import { useOverlayStore } from '../../store/useOverlayStore';
import { hexToRgba, resolveLabelCollisions } from './colorUtils';

/**
 * Collision-aware Label Mode renderer.
 * Draws crisp SVG target outlines + leader lines for shifted badges, paired with
 * GPU-composited high-contrast hint pills that filter live as the user types.
 */
export function LabelLayer() {
  const overlay = useOverlayStore((s) => s.overlay);
  const config = useOverlayStore((s) => s.config);

  const visible = Boolean(overlay && overlay.visible && overlay.mode === 'label');
  const typedPrefix = overlay?.typedPrefix ?? '';
  const badges = overlay?.labelBadges ?? [];
  const app = config.appearance;
  const labelCfg = config.labelMode;

  const viewportW = overlay?.activeMonitor.bounds.width || window.innerWidth;
  const viewportH = overlay?.activeMonitor.bounds.height || window.innerHeight;

  const placedBadges = useMemo(() => {
    if (!visible) return [];
    const capped = badges.slice(0, labelCfg.maxBadges);
    return resolveLabelCollisions(
      capped,
      viewportW,
      viewportH,
      app.labelFontSize,
      labelCfg.showElementRoleHint,
      labelCfg.badgePlacement,
      labelCfg.collisionAvoidance,
    );
  }, [
    visible,
    badges,
    labelCfg.maxBadges,
    labelCfg.showElementRoleHint,
    labelCfg.badgePlacement,
    labelCfg.collisionAvoidance,
    viewportW,
    viewportH,
    app.labelFontSize,
  ]);

  if (!visible) return null;

  const prefixLen = typedPrefix.length;

  return (
    <div className="rp-label-layer" aria-hidden="true">
      {/* SVG layer for subtle accessibility bounding-box frames & collision leader lines */}
      <svg
        className="rp-label-svg"
        width={viewportW}
        height={viewportH}
        viewBox={`0 0 ${viewportW} ${viewportH}`}
      >
        {placedBadges.map(({ badge, pillRect, anchorPoint, wasShifted }) => {
          if (!badge.matchedPrefix && prefixLen > 0) return null;
          const pillCx = pillRect.x + pillRect.width / 2;
          const pillCy = pillRect.y + pillRect.height / 2;

          return (
            <g key={`svg-${badge.id}`}>
              {/* Target bounding box hairline */}
              <rect
                x={badge.rect.x}
                y={badge.rect.y}
                width={badge.rect.width}
                height={badge.rect.height}
                rx={3}
                fill={badge.exactMatch ? hexToRgba(app.labelBgColor, 0.22) : 'transparent'}
                stroke={
                  badge.exactMatch
                    ? app.labelMatchPrefixColor
                    : hexToRgba(app.labelBorderColor, 0.55)
                }
                strokeWidth={badge.exactMatch ? 2 : 1}
              />
              {/* Leader line when collision avoidance nudged the pill */}
              {wasShifted && (
                <>
                  <line
                    x1={anchorPoint.x}
                    y1={anchorPoint.y}
                    x2={pillCx}
                    y2={pillCy}
                    stroke={hexToRgba(app.labelBorderColor, 0.85)}
                    strokeWidth={1.25}
                    strokeDasharray="3 2"
                  />
                  <circle
                    cx={anchorPoint.x}
                    cy={anchorPoint.y}
                    r={2.5}
                    fill={app.labelBorderColor}
                  />
                </>
              )}
            </g>
          );
        })}
      </svg>

      {/* High-contrast composited hint pills */}
      {placedBadges.map(({ badge, pillRect }) => {
        const isFilteredOut = prefixLen > 0 && !badge.matchedPrefix;
        if (isFilteredOut) return null;

        const matchedChars = prefixLen > 0 ? badge.code.slice(0, prefixLen) : '';
        const remainingChars = prefixLen > 0 ? badge.code.slice(prefixLen) : badge.code;

        return (
          <div
            key={badge.id}
            className={`rp-label-pill${badge.exactMatch ? ' rp-label-pill--exact' : ''}`}
            style={{
              transform: `translate3d(${pillRect.x}px, ${pillRect.y}px, 0)`,
              backgroundColor: hexToRgba(app.labelBgColor, app.labelBgOpacity),
              borderColor: badge.exactMatch ? app.labelMatchPrefixColor : app.labelBorderColor,
              color: app.labelTextColor,
              fontFamily: app.labelFontFamily,
              fontSize: `${app.labelFontSize}px`,
              borderRadius: `${app.labelBorderRadius}px`,
            }}
          >
            <span className="rp-label-code">
              {matchedChars && (
                <span style={{ color: app.labelMatchPrefixColor }}>{matchedChars}</span>
              )}
              <span>{remainingChars}</span>
            </span>
            {labelCfg.showElementRoleHint && badge.role && (
              <span className="rp-label-role">{badge.role}</span>
            )}
          </div>
        );
      })}
    </div>
  );
}
