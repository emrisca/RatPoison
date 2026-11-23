import { useEffect, useRef } from 'react';
import { Copy, RotateCcw } from 'lucide-react';
import {
  formatSettingValue,
  getByPath,
  isModifiedPath,
} from '../../config/defaults';
import { useSettingsStore } from '../../store/useSettingsStore';

/**
 * Right-click "Restore Default" floating context menu bound to every SettingRow.
 */
export function RestoreContextMenu() {
  const contextMenu = useSettingsStore((s) => s.contextMenu);
  const config = useSettingsStore((s) => s.config);
  const defaultConfig = useSettingsStore((s) => s.defaultConfig);
  const closeContextMenu = useSettingsStore((s) => s.closeContextMenu);
  const resetSettingKey = useSettingsStore((s) => s.resetSettingKey);
  const menuRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!contextMenu.open) return;

    const handlePointerDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        closeContextMenu();
      }
    };
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') closeContextMenu();
    };
    const handleScroll = () => closeContextMenu();

    window.addEventListener('mousedown', handlePointerDown);
    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('scroll', handleScroll, true);

    return () => {
      window.removeEventListener('mousedown', handlePointerDown);
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('scroll', handleScroll, true);
    };
  }, [contextMenu.open, closeContextMenu]);

  if (!contextMenu.open) return null;

  const { settingPath, settingLabel } = contextMenu;
  const currentValue = getByPath(config, settingPath);
  const defaultValue = getByPath(defaultConfig, settingPath);
  const modified = isModifiedPath(config, settingPath, defaultConfig);

  // Clamp coordinates so menu never overflows right/bottom window edges
  const menuW = 248;
  const menuH = 148;
  const left = Math.max(8, Math.min(window.innerWidth - menuW - 8, contextMenu.x));
  const top = Math.max(8, Math.min(window.innerHeight - menuH - 8, contextMenu.y));

  const handleRestore = async () => {
    closeContextMenu();
    await resetSettingKey(settingPath);
  };

  const handleCopyKey = async () => {
    try {
      await navigator.clipboard.writeText(settingPath);
    } catch {
      // Clipboard may be unavailable in restricted contexts
    }
    closeContextMenu();
  };

  return (
    <div
      ref={menuRef}
      className="rp-context-menu"
      style={{ left: `${left}px`, top: `${top}px` }}
      role="menu"
      aria-label={`Actions for ${settingLabel}`}
    >
      <div className="rp-context-menu-header">
        <span className="rp-context-menu-title">{settingLabel}</span>
        <code className="rp-context-menu-key">{settingPath}</code>
      </div>

      <div className="rp-context-menu-diff">
        <div>
          <span className="rp-diff-label">Current:</span>{' '}
          <span className="rp-diff-val">{formatSettingValue(currentValue)}</span>
        </div>
        <div>
          <span className="rp-diff-label">Default:</span>{' '}
          <span className="rp-diff-val">{formatSettingValue(defaultValue)}</span>
        </div>
      </div>

      <div className="rp-context-menu-divider" />

      <button
        type="button"
        className="rp-context-menu-item"
        disabled={!modified}
        onClick={() => void handleRestore()}
        role="menuitem"
      >
        <RotateCcw size={14} />
        <span>{modified ? 'Restore Default' : 'Already at Default'}</span>
      </button>

      <button
        type="button"
        className="rp-context-menu-item"
        onClick={() => void handleCopyKey()}
        role="menuitem"
      >
        <Copy size={14} />
        <span>Copy Setting Key</span>
      </button>
    </div>
  );
}
