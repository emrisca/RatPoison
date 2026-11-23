import { useMemo } from 'react';
import { useOverlayStore } from '../../store/useOverlayStore';
import { hexToRgba, resolveLabelCollisions } from './colorUtils';

function truncateTitle(text: string, maxChars = 30): string {
  const clean = text.trim();
  if (clean.length <= maxChars) return clean;
  return `${clean.slice(0, maxChars - 1)}…`;
}

function renderHighlightedTitle(title: string, query: string, selected: boolean) {
  const display = truncateTitle(title, 30);
  const trimmed = query.trim();
  if (!trimmed) return display;

  const firstToken = trimmed.split(/\s+/)[0];
  if (!firstToken) return display;

  const idx = display.toLowerCase().indexOf(firstToken.toLowerCase());
  if (idx === -1) return display;

  const before = display.slice(0, idx);
  const match = display.slice(idx, idx + firstToken.length);
  const after = display.slice(idx + firstToken.length);

  return (
    <>
      {before}
      <span className={selected ? 'rp-search-hl--selected' : 'rp-search-hl'}>{match}</span>
      {after}
    </>
  );
}

/**
 * Collision-aware Label Mode renderer supporting two seamless workflows:
 * 1. Compact Keycap Mode (Homerow/Vimium style) — dense, uncluttered keycap pills.
 * 2. Spotlight Text Search Mode (Shortcat style, `/` or `startInSearchMode`) — zero clutter
 *    until typing, ranked live text matching, Tab/↑↓ cycling, and Enter to click.
 */
export function LabelLayer() {
  const overlay = useOverlayStore((s) => s.overlay);
  const config = useOverlayStore((s) => s.config);

  const visible = Boolean(overlay && overlay.visible && overlay.mode === 'label');
  const typedPrefix = overlay?.typedPrefix ?? '';
  const badges = overlay?.labelBadges ?? [];
  const searchActive = Boolean(overlay?.labelSearchActive);
  const searchQuery = overlay?.labelSearchQuery ?? '';
  const searchSelectedIndex = overlay?.labelSearchSelectedIndex ?? 0;
  const app = config.appearance;
  const labelCfg = config.labelMode;

  const viewportW = overlay?.activeMonitor.bounds.width || window.innerWidth;
  const viewportH = overlay?.activeMonitor.bounds.height || window.innerHeight;

  const placedBadges = useMemo(() => {
    if (!visible || badges.length === 0) return [];
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

  // --- Mode 2: Spotlight Text Search (Shortcat-style) ---
  if (searchActive) {
    const trimmedQuery = searchQuery.trim();
    const hasQuery = trimmedQuery.length > 0;
    const matchCount = badges.length;
    const clampedIndex = matchCount > 0 ? Math.min(searchSelectedIndex, matchCount - 1) : 0;

    return (
      <div className="rp-label-layer" aria-hidden="true">
        {/* Crisp target outlines rendered ONLY once the user has typed a search query */}
        {hasQuery && matchCount > 0 && (
          <svg
            className="rp-label-svg"
            width={viewportW}
            height={viewportH}
            viewBox={`0 0 ${viewportW} ${viewportH}`}
          >
            {badges.map((badge, idx) => {
              const isSelected = idx === clampedIndex || badge.exactMatch;
              return (
                <g key={`search-svg-${badge.id}`}>
                  <rect
                    x={badge.rect.x}
                    y={badge.rect.y}
                    width={badge.rect.width}
                    height={badge.rect.height}
                    rx={6}
                    fill={hexToRgba(app.labelBorderColor, isSelected ? 0.18 : 0.06)}
                    stroke={
                      isSelected
                        ? app.labelBorderColor
                        : hexToRgba(app.labelBorderColor, 0.45)
                    }
                    strokeWidth={isSelected ? 2 : 1}
                  />
                </g>
              );
            })}
          </svg>
        )}

        {/* Floating target pills for matched accessibility elements */}
        {hasQuery &&
          badges.map((badge, idx) => {
            const isSelected = idx === clampedIndex || badge.exactMatch;
            const labelText = badge.title?.trim() || badge.role || badge.code.toUpperCase();
            const pillX = Math.max(8, Math.min(viewportW - 220, badge.rect.x));
            const pillY =
              badge.rect.y >= 30
                ? badge.rect.y - 24
                : Math.min(viewportH - 28, badge.rect.y + badge.rect.height + 4);

            return (
              <div
                key={`search-pill-${badge.id}`}
                className={`rp-search-match-pill${isSelected ? ' rp-search-match-pill--selected' : ''}`}
                style={{
                  transform: `translate3d(${Math.round(pillX)}px, ${Math.round(pillY)}px, 0)`,
                  zIndex: isSelected ? 20 : 10,
                }}
              >
                <span className="rp-search-match-title">
                  {renderHighlightedTitle(labelText, searchQuery, isSelected)}
                </span>
                {isSelected && <kbd className="rp-search-enter-kbd">↵</kbd>}
              </div>
            );
          })}

        {/* Apple-crafted floating Spotlight Search Bar (Top-Center, 0ms instant) */}
        <div className="rp-spotlight-bar" role="status">
          <svg
            className={`rp-spotlight-icon${hasQuery ? ' rp-spotlight-icon--active' : ''}`}
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="11" cy="11" r="7.5" />
            <path d="M20 20l-3.8-3.8" />
          </svg>

          <div className="rp-spotlight-input">
            {searchQuery.length > 0 ? (
              <span className="rp-spotlight-query">
                {searchQuery}
                <span className="rp-spotlight-caret" />
              </span>
            ) : (
              <span className="rp-spotlight-placeholder">
                <span className="rp-spotlight-caret rp-spotlight-caret--leading" />
                Search buttons, links, tabs…
              </span>
            )}
          </div>

          {hasQuery && (
            <span
              className={`rp-spotlight-count${matchCount === 0 ? ' rp-spotlight-count--empty' : ''}`}
            >
              {matchCount > 0 ? `${clampedIndex + 1} / ${matchCount}` : 'No matches'}
            </span>
          )}

          <div className="rp-spotlight-hints">
            {hasQuery && matchCount > 1 && (
              <span className="rp-spotlight-hint">
                <kbd className="rp-spotlight-kbd">Tab</kbd>
              </span>
            )}
            {hasQuery && matchCount > 0 && (
              <span className="rp-spotlight-hint">
                <kbd className="rp-spotlight-kbd">↵</kbd> Click
              </span>
            )}
            {!hasQuery && (
              <span className="rp-spotlight-hint">
                <kbd className="rp-spotlight-kbd">⌫</kbd> Keycaps
              </span>
            )}
            <span className="rp-spotlight-hint">
              <kbd className="rp-spotlight-kbd">Esc</kbd>
            </span>
          </div>
        </div>
      </div>
    );
  }

  // --- Mode 1: Compact Keycap Mode (Homerow/Vimium style) ---
  if (badges.length === 0) {
    return (
      <div className="rp-label-layer" aria-hidden="true">
        <div className="rp-label-empty-pill" role="status">
          <span className="rp-label-empty-dot" />
          <span className="rp-label-empty-title">Label Mode</span>
          <span className="rp-label-empty-sep" aria-hidden="true">
            ·
          </span>
          <span className="rp-label-empty-text">No clickable elements in active window</span>
          <span className="rp-label-empty-sep" aria-hidden="true">
            ·
          </span>
          <span className="rp-label-empty-hint">
            <kbd className="rp-label-empty-kbd">Esc</kbd> to close
          </span>
        </div>
      </div>
    );
  }

  const prefixLen = typedPrefix.length;
  const visiblePlaced =
    prefixLen > 0 ? placedBadges.filter(({ badge }) => badge.matchedPrefix) : placedBadges;
  // Compact Keycap Mode: suppress wireframe bounding boxes when the full page is labeled,
  // revealing crisp target outlines once narrowed to <= 6 matches.
  const showTargetFrames = visiblePlaced.length <= 6;

  return (
    <div className="rp-label-layer" aria-hidden="true">
      {/* Focused target outline layer — rendered only when narrowed or exact-matched */}
      {showTargetFrames && (
        <svg
          className="rp-label-svg"
          width={viewportW}
          height={viewportH}
          viewBox={`0 0 ${viewportW} ${viewportH}`}
        >
          {visiblePlaced.map(({ badge, pillRect, anchorPoint, wasShifted }) => {
            const pillCx = pillRect.x + pillRect.width / 2;
            const pillCy = pillRect.y + pillRect.height / 2;

            return (
              <g key={`svg-${badge.id}`}>
                <rect
                  x={badge.rect.x}
                  y={badge.rect.y}
                  width={badge.rect.width}
                  height={badge.rect.height}
                  rx={5}
                  fill={hexToRgba(app.labelBorderColor, badge.exactMatch ? 0.16 : 0.08)}
                  stroke={
                    badge.exactMatch
                      ? app.labelMatchPrefixColor
                      : hexToRgba(app.labelBorderColor, 0.65)
                  }
                  strokeWidth={badge.exactMatch ? 1.5 : 1}
                />
                {wasShifted && (
                  <>
                    <line
                      x1={anchorPoint.x}
                      y1={anchorPoint.y}
                      x2={pillCx}
                      y2={pillCy}
                      stroke={hexToRgba(app.labelBorderColor, 0.55)}
                      strokeWidth={1}
                      strokeDasharray="2 2"
                    />
                    <circle
                      cx={anchorPoint.x}
                      cy={anchorPoint.y}
                      r={2}
                      fill={app.labelBorderColor}
                    />
                  </>
                )}
              </g>
            );
          })}
        </svg>
      )}

      {/* Compact, high-contrast Homerow/Vimium-style keycap pills */}
      {visiblePlaced.map(({ badge, pillRect }) => {
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
