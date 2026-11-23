import type { MouseEvent, ReactNode } from 'react';
import { SETTING_MAP, isModifiedPath } from '../../config/defaults';
import { useSettingsStore } from '../../store/useSettingsStore';

interface SettingRowProps {
  path: string;
  children: ReactNode;
  highlightQuery?: string;
  badgeText?: string;
}

function HighlightText({ text, query }: { text: string; query?: string }) {
  const trimmed = query?.trim();
  if (!trimmed) return <>{text}</>;
  const idx = text.toLowerCase().indexOf(trimmed.toLowerCase());
  if (idx === -1) return <>{text}</>;
  return (
    <>
      {text.slice(0, idx)}
      <mark className="rp-search-mark">{text.slice(idx, idx + trimmed.length)}</mark>
      {text.slice(idx + trimmed.length)}
    </>
  );
}

/**
 * Standard wrapper for every Settings control.
 * - Shows a subtle "modified" dot whenever the current value differs from DEFAULT_CONFIG.
 * - Attaches right-click context menu for quiet "Restore Default" (`reset_setting_key`).
 */
export function SettingRow({ path, children, highlightQuery, badgeText }: SettingRowProps) {
  const config = useSettingsStore((s) => s.config);
  const defaultConfig = useSettingsStore((s) => s.defaultConfig);
  const openContextMenu = useSettingsStore((s) => s.openContextMenu);
  const resetSettingKey = useSettingsStore((s) => s.resetSettingKey);

  const descriptor = SETTING_MAP[path];
  const label = descriptor?.label ?? path;
  const description = descriptor?.description ?? '';
  const modified = isModifiedPath(config, path, defaultConfig);

  const handleContextMenu = (e: MouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    openContextMenu(e.clientX, e.clientY, path, label);
  };

  return (
    <div
      className={`rp-setting-row${modified ? ' rp-setting-row--modified' : ''}`}
      onContextMenu={handleContextMenu}
      data-setting-path={path}
    >
      <div className="rp-setting-meta">
        <div className="rp-setting-title-line">
          {modified && (
            <button
              type="button"
              className="rp-modified-dot"
              title="Modified — click to reset"
              aria-label={`Restore ${label} to default`}
              onClick={() => void resetSettingKey(path)}
            />
          )}
          <span className="rp-setting-label">
            <HighlightText text={label} query={highlightQuery} />
          </span>
          {badgeText && <span className="rp-setting-tab-pill">{badgeText}</span>}
        </div>
        {description && (
          <p className="rp-setting-desc">
            <HighlightText text={description} query={highlightQuery} />
          </p>
        )}
      </div>
      <div className="rp-setting-control">{children}</div>
    </div>
  );
}
