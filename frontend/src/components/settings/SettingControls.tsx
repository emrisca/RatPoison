import { useState, type KeyboardEvent } from 'react';
import { AlertTriangle, Keyboard } from 'lucide-react';
import { FONT_FAMILY_PRESETS, getByPath } from '../../config/defaults';
import type { PlatformOs } from '../../types/ipc';
import { useSettingsStore } from '../../store/useSettingsStore';
import { SettingRow } from './SettingRow';

interface ToggleProps {
  path: string;
}

function ToggleControl({ path }: ToggleProps) {
  const value = useSettingsStore((s) => Boolean(getByPath(s.config, path)));
  const updateSetting = useSettingsStore((s) => s.updateSetting);

  return (
    <button
      type="button"
      role="switch"
      aria-checked={value}
      className={`rp-toggle${value ? ' is-on' : ''}`}
      onClick={() => void updateSetting(path, !value)}
    >
      <span className="rp-toggle-thumb" />
    </button>
  );
}

interface SliderProps {
  path: string;
  min: number;
  max: number;
  step?: number;
  unit?: string;
  format?: (v: number) => string;
}

function SliderControl({ path, min, max, step = 1, unit = '', format }: SliderProps) {
  const value = useSettingsStore((s) => Number(getByPath(s.config, path) ?? min));
  const updateSetting = useSettingsStore((s) => s.updateSetting);

  const formatted = format ? format(value) : `${value}${unit}`;

  return (
    <div className="rp-slider-wrap">
      <input
        type="range"
        className="rp-slider"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => void updateSetting(path, Number(e.target.value))}
      />
      <span className="rp-slider-value">{formatted}</span>
    </div>
  );
}

interface SelectOption {
  value: string;
  label: string;
}

function SelectControl({ path, options }: { path: string; options: SelectOption[] }) {
  const value = useSettingsStore((s) => String(getByPath(s.config, path) ?? ''));
  const updateSetting = useSettingsStore((s) => s.updateSetting);

  return (
    <select
      className="rp-select"
      value={value}
      onChange={(e) => void updateSetting(path, e.target.value)}
    >
      {options.map((opt) => (
        <option key={opt.value} value={opt.value}>
          {opt.label}
        </option>
      ))}
    </select>
  );
}

function TextControl({
  path,
  maxLength,
  mono = false,
  compact = false,
}: {
  path: string;
  maxLength?: number;
  mono?: boolean;
  compact?: boolean;
}) {
  const value = useSettingsStore((s) => String(getByPath(s.config, path) ?? ''));
  const updateSetting = useSettingsStore((s) => s.updateSetting);

  return (
    <input
      type="text"
      className={`rp-input${mono ? ' rp-input--mono' : ''}${compact ? ' rp-input--compact' : ''}`}
      maxLength={maxLength}
      value={value}
      onChange={(e) => void updateSetting(path, e.target.value)}
    />
  );
}

function ColorPickerControl({ path }: { path: string }) {
  const value = useSettingsStore((s) => String(getByPath(s.config, path) ?? '#000000'));
  const updateSetting = useSettingsStore((s) => s.updateSetting);

  const validHex = /^#[0-9a-fA-F]{6}$/.test(value) ? value : '#0A84FF';

  return (
    <div className="rp-color-control">
      <label className="rp-color-swatch" style={{ backgroundColor: validHex }}>
        <input
          type="color"
          className="rp-color-native"
          value={validHex}
          onChange={(e) => void updateSetting(path, e.target.value)}
          aria-label={`Color picker for ${path}`}
        />
      </label>
      <input
        type="text"
        className="rp-input rp-input--mono rp-input--hex"
        value={value}
        maxLength={7}
        onChange={(e) => void updateSetting(path, e.target.value)}
      />
    </div>
  );
}

function FontFamilyControl({ path }: { path: string }) {
  const value = useSettingsStore((s) => String(getByPath(s.config, path) ?? ''));
  const updateSetting = useSettingsStore((s) => s.updateSetting);

  const matchesPreset = FONT_FAMILY_PRESETS.some((p) => p.value === value);

  return (
    <div className="rp-font-control">
      <select
        className="rp-select"
        value={matchesPreset ? value : '__custom__'}
        onChange={(e) => {
          if (e.target.value !== '__custom__') {
            void updateSetting(path, e.target.value);
          }
        }}
      >
        {FONT_FAMILY_PRESETS.map((preset) => (
          <option key={preset.label} value={preset.value}>
            {preset.label}
          </option>
        ))}
        {!matchesPreset && <option value="__custom__">Custom Font Stack</option>}
      </select>
      <input
        type="text"
        className="rp-input rp-input--mono"
        value={value}
        placeholder="CSS font-family stack"
        onChange={(e) => void updateSetting(path, e.target.value)}
      />
    </div>
  );
}

function formatKeyEvent(e: KeyboardEvent<HTMLButtonElement>, os: PlatformOs): string | null {
  const ignoreOnly = new Set(['Meta', 'Control', 'Alt', 'Shift']);
  if (ignoreOnly.has(e.key)) return null;

  const parts: string[] = [];
  if (os === 'macos') {
    if (e.ctrlKey) parts.push('Ctrl');
    if (e.altKey) parts.push('Option');
    if (e.shiftKey) parts.push('Shift');
    if (e.metaKey) parts.push('Cmd');
  } else if (os === 'windows') {
    if (e.ctrlKey) parts.push('Ctrl');
    if (e.altKey) parts.push('Alt');
    if (e.shiftKey) parts.push('Shift');
    if (e.metaKey) parts.push('Win');
  } else {
    if (e.metaKey) parts.push('Super');
    if (e.ctrlKey) parts.push('Ctrl');
    if (e.altKey) parts.push('Alt');
    if (e.shiftKey) parts.push('Shift');
  }

  let keyName = e.key;
  if (keyName === ' ') keyName = 'Space';
  else if (keyName.length === 1) keyName = keyName.toUpperCase();

  parts.push(keyName);
  return parts.join('+');
}

function HotkeyRecorderControl({ path }: { path: string }) {
  const value = useSettingsStore((s) => String(getByPath(s.config, path) ?? ''));
  const keybindings = useSettingsStore((s) => s.config.keybindings);
  const platformOs = useSettingsStore((s) => s.platform.os);
  const updateSetting = useSettingsStore((s) => s.updateSetting);
  const [recording, setRecording] = useState(false);

  // Detect duplicate chord across keybindings
  const currentKeyName = path.replace('keybindings.', '');
  const conflictWith = Object.entries(keybindings).find(
    ([k, v]) => k !== currentKeyName && v.toLowerCase() === value.toLowerCase() && value.trim() !== '',
  )?.[0];

  const handleKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (!recording) return;
    e.preventDefault();
    e.stopPropagation();

    if (e.key === 'Escape') {
      setRecording(false);
      return;
    }

    const chord = formatKeyEvent(e, platformOs);
    if (chord) {
      void updateSetting(path, chord);
      setRecording(false);
    }
  };

  return (
    <div className="rp-hotkey-wrap">
      {conflictWith && (
        <span
          className="rp-hotkey-conflict"
          title={`Conflicts with keybindings.${conflictWith}`}
        >
          <AlertTriangle size={13} />
          Conflict
        </span>
      )}
      <button
        type="button"
        className={`rp-hotkey-btn${recording ? ' is-recording' : ''}`}
        onClick={() => setRecording((r) => !r)}
        onBlur={() => setRecording(false)}
        onKeyDown={handleKeyDown}
      >
        <Keyboard size={13} />
        <span>{recording ? 'Press shortcut… (Esc to cancel)' : value || 'Unbound'}</span>
      </button>
    </div>
  );
}

/**
 * Renders the appropriate interactive control wrapped in SettingRow for any registry path.
 * Shared across Tab Views and Global Live Search Results so both stay 100% consistent.
 */
export function SettingControlByPath({
  path,
  highlightQuery,
  badgeText,
}: {
  path: string;
  highlightQuery?: string;
  badgeText?: string;
}) {
  const renderControl = () => {
    switch (path) {
      // General
      case 'general.launchAtLogin':
      case 'general.showTrayIcon':
      case 'general.hideCursorInOverlay':
      case 'general.autoExitAfterClick':
      case 'general.escapeCancelsMode':
      case 'general.confirmBeforeQuit':
        return <ToggleControl path={path} />;
      case 'general.multiMonitorStrategy':
        return (
          <SelectControl
            path={path}
            options={[
              { value: 'active-follow', label: 'Follow Active Monitor (Recommended)' },
              { value: 'per-monitor', label: 'Dedicated Window per Monitor' },
              { value: 'virtual-span', label: 'Single Virtual Desktop Span' },
            ]}
          />
        );

      // Grid & Subgrid
      case 'grid.rows':
        return <SliderControl path={path} min={6} max={36} step={1} />;
      case 'grid.cols':
        return <SliderControl path={path} min={8} max={40} step={1} />;
      case 'grid.alphabet':
        return <TextControl path={path} mono />;
      case 'grid.caseSensitive':
      case 'grid.dimNonMatchingCells':
      case 'grid.enableSubgridZoom':
      case 'grid.showSubgridCrosshair':
        return <ToggleControl path={path} />;
      case 'grid.dimOpacity':
        return <SliderControl path={path} min={0} max={0.85} step={0.05} format={(v) => `${Math.round(v * 100)}%`} />;
      case 'grid.subgridRows':
      case 'grid.subgridCols':
        return <SliderControl path={path} min={2} max={6} step={1} />;
      case 'grid.subgridZoomScale':
        return <SliderControl path={path} min={1.0} max={3.0} step={0.05} format={(v) => `${v.toFixed(2)}×`} />;
      case 'grid.nudgeStepPx':
        return <SliderControl path={path} min={2} max={32} step={1} unit="px" />;
      case 'grid.fineNudgeStepPx':
        return <SliderControl path={path} min={1} max={8} step={1} unit="px" />;

      // Free Mode
      case 'freeMode.baseSpeedPxPerSec':
        return <SliderControl path={path} min={150} max={1600} step={20} unit=" px/s" />;
      case 'freeMode.fastMultiplier':
        return <SliderControl path={path} min={1.2} max={4.5} step={0.1} format={(v) => `${v.toFixed(1)}×`} />;
      case 'freeMode.slowMultiplier':
        return <SliderControl path={path} min={0.1} max={0.8} step={0.05} format={(v) => `${v.toFixed(2)}×`} />;
      case 'freeMode.accelerationCurve':
        return <SliderControl path={path} min={1.0} max={2.5} step={0.05} format={(v) => v.toFixed(2)} />;
      case 'freeMode.scrollSpeed':
        return <SliderControl path={path} min={10} max={120} step={2} />;
      case 'freeMode.naturalScroll':
      case 'freeMode.showHudBadge':
        return <ToggleControl path={path} />;
      case 'freeMode.hudPosition':
        return (
          <SelectControl
            path={path}
            options={[
              { value: 'bottom-right', label: 'Bottom Right' },
              { value: 'bottom-left', label: 'Bottom Left' },
              { value: 'top-right', label: 'Top Right' },
              { value: 'top-left', label: 'Top Left' },
              { value: 'cursor-follow', label: 'Follow Cursor' },
            ]}
          />
        );
      case 'freeMode.keyUp':
      case 'freeMode.keyDown':
      case 'freeMode.keyLeft':
      case 'freeMode.keyRight':
      case 'freeMode.modFast':
      case 'freeMode.modSlow':
      case 'freeMode.modScroll':
      case 'freeMode.modDrag':
        return <TextControl path={path} maxLength={1} mono compact />;

      // Label Mode
      case 'labelMode.enabled':
      case 'labelMode.collisionAvoidance':
      case 'labelMode.includeMenubar':
      case 'labelMode.includeDock':
      case 'labelMode.showElementRoleHint':
      case 'labelMode.startInSearchMode':
        return <ToggleControl path={path} />;
      case 'labelMode.charset':
        return <TextControl path={path} mono />;
      case 'labelMode.minElementSizePx':
        return <SliderControl path={path} min={4} max={48} step={2} unit="px" />;
      case 'labelMode.maxBadges':
        return <SliderControl path={path} min={30} max={400} step={10} />;
      case 'labelMode.badgePlacement':
        return (
          <SelectControl
            path={path}
            options={[
              { value: 'smart', label: 'Smart Collision-Aware' },
              { value: 'top-left', label: 'Top-Left Corner' },
              { value: 'center', label: 'Element Center' },
              { value: 'top-right', label: 'Top-Right Corner' },
            ]}
          />
        );

      // Keybindings
      case 'keybindings.toggleGrid':
      case 'keybindings.toggleLabelMode':
      case 'keybindings.toggleFreeMode':
      case 'keybindings.leftClick':
      case 'keybindings.rightClick':
      case 'keybindings.middleClick':
      case 'keybindings.doubleClick':
      case 'keybindings.holdDrag':
      case 'keybindings.cycleMonitor':
      case 'keybindings.openSettings':
        return <HotkeyRecorderControl path={path} />;

      // Appearance
      case 'appearance.theme':
        return (
          <SelectControl
            path={path}
            options={[
              { value: 'system', label: 'Follow System' },
              { value: 'dark', label: 'Dark' },
              { value: 'light', label: 'Light' },
            ]}
          />
        );
      case 'appearance.reduceMotion':
        return <ToggleControl path={path} />;
      case 'appearance.gridBgColor':
      case 'appearance.gridLineColor':
      case 'appearance.gridTextColor':
      case 'appearance.gridAccentColor':
      case 'appearance.subgridAccentColor':
      case 'appearance.subgridBgColor':
      case 'appearance.crosshairColor':
      case 'appearance.labelBgColor':
      case 'appearance.labelBorderColor':
      case 'appearance.labelTextColor':
      case 'appearance.labelMatchPrefixColor':
      case 'appearance.hudBgColor':
      case 'appearance.hudTextColor':
      case 'appearance.hudAccentColor':
        return <ColorPickerControl path={path} />;
      case 'appearance.gridBgOpacity':
      case 'appearance.subgridBgOpacity':
      case 'appearance.labelBgOpacity':
      case 'appearance.hudOpacity':
        return <SliderControl path={path} min={0} max={1} step={0.02} format={(v) => `${Math.round(v * 100)}%`} />;
      case 'appearance.gridLineWidth':
        return <SliderControl path={path} min={0.5} max={3} step={0.5} unit="px" />;
      case 'appearance.gridFontFamily':
      case 'appearance.labelFontFamily':
        return <FontFamilyControl path={path} />;
      case 'appearance.gridFontSize':
        return <SliderControl path={path} min={9} max={28} step={1} unit="px" />;
      case 'appearance.gridFontWeight':
        return <SliderControl path={path} min={400} max={800} step={100} />;
      case 'appearance.labelFontSize':
        return <SliderControl path={path} min={9} max={24} step={1} unit="px" />;
      case 'appearance.labelBorderRadius':
        return <SliderControl path={path} min={0} max={14} step={1} unit="px" />;

      // Updates
      case 'updates.channel':
        return (
          <SelectControl
            path={path}
            options={[
              { value: 'stable', label: 'Stable' },
              { value: 'beta', label: 'Beta (Early Access)' },
            ]}
          />
        );
      case 'updates.autoCheck':
      case 'updates.autoDownload':
        return <ToggleControl path={path} />;

      default:
        return <TextControl path={path} />;
    }
  };

  return (
    <SettingRow path={path} highlightQuery={highlightQuery} badgeText={badgeText}>
      {renderControl()}
    </SettingRow>
  );
}
