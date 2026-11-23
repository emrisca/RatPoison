import { useOverlayStore } from '../../store/useOverlayStore';
import { hexToRgba } from './colorUtils';

/**
 * Minimal, non-focusable Free Mode HUD pill displaying speed profile,
 * directional key activity (I/J/K/L), and A/S/D/F modifier state.
 */
export function FreeModeHud() {
  const overlay = useOverlayStore((s) => s.overlay);
  const config = useOverlayStore((s) => s.config);

  if (
    !overlay ||
    !overlay.visible ||
    overlay.mode !== 'free' ||
    !overlay.freeMode ||
    !config.freeMode.showHudBadge
  ) {
    return null;
  }

  const free = overlay.freeMode;
  const fmCfg = config.freeMode;
  const app = config.appearance;
  const pos = fmCfg.hudPosition;

  const isCursorFollow = pos === 'cursor-follow';
  const followStyle = isCursorFollow
    ? {
        left: 0,
        top: 0,
        transform: `translate3d(${Math.min(window.innerWidth - 220, free.cursor.x + 18)}px, ${Math.min(
          window.innerHeight - 60,
          free.cursor.y + 18,
        )}px, 0)`,
      }
    : undefined;

  const modeBadge = free.scrollMode ? 'SCROLL' : free.dragActive ? 'DRAG' : 'FREE';
  const speedLabel = free.speedProfile.toUpperCase();

  const modifiers = [
    { key: fmCfg.modDrag, label: 'DRAG', active: free.dragActive || free.heldModifiers.includes(fmCfg.modDrag) },
    { key: fmCfg.modSlow, label: 'SLOW', active: free.speedProfile === 'slow' || free.heldModifiers.includes(fmCfg.modSlow) },
    { key: fmCfg.modScroll, label: 'SCRL', active: free.scrollMode || free.heldModifiers.includes(fmCfg.modScroll) },
    { key: fmCfg.modFast, label: 'FAST', active: free.speedProfile === 'fast' || free.heldModifiers.includes(fmCfg.modFast) },
  ];

  const directions: Array<{ dir: 'up' | 'left' | 'down' | 'right'; key: string }> = [
    { dir: 'left', key: fmCfg.keyLeft },
    { dir: 'up', key: fmCfg.keyUp },
    { dir: 'down', key: fmCfg.keyDown },
    { dir: 'right', key: fmCfg.keyRight },
  ];

  return (
    <div
      className={`rp-free-hud rp-free-hud--${pos}`}
      style={{
        ...followStyle,
        backgroundColor: hexToRgba(app.hudBgColor, app.hudOpacity),
        color: app.hudTextColor,
        borderColor: hexToRgba(app.hudAccentColor, 0.45),
      }}
      aria-hidden="true"
    >
      <span
        className="rp-free-hud-mode"
        style={{ backgroundColor: app.hudAccentColor, color: '#0a0a0a' }}
      >
        {modeBadge}
      </span>
      <span className="rp-free-hud-speed">{speedLabel}</span>

      <span className="rp-free-hud-divider" />

      <div className="rp-free-hud-keys">
        {directions.map((d) => {
          const active = free.activeDirections.includes(d.dir);
          return (
            <kbd
              key={d.dir}
              className="rp-hud-kbd"
              style={
                active
                  ? { backgroundColor: app.hudAccentColor, color: '#0a0a0a', borderColor: app.hudAccentColor }
                  : undefined
              }
            >
              {d.key}
            </kbd>
          );
        })}
      </div>

      <span className="rp-free-hud-divider" />

      <div className="rp-free-hud-mods">
        {modifiers.map((m) => (
          <span
            key={m.label}
            className={`rp-hud-mod${m.active ? ' rp-hud-mod--active' : ''}`}
            style={m.active ? { color: app.hudAccentColor, borderColor: app.hudAccentColor } : undefined}
          >
            {m.key}
          </span>
        ))}
      </div>
    </div>
  );
}
