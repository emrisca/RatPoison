import type { PlatformOs, RatPoisonConfig, SettingsTabId } from '../types/ipc';

export const OS_KEYBINDING_PRESETS: Record<PlatformOs, RatPoisonConfig['keybindings']> = {
  macos: {
    toggleGrid: 'Option+Space',
    toggleLabelMode: 'Option+Shift+Space',
    toggleFreeMode: 'Option+F',
    leftClick: 'Space',
    rightClick: 'Shift+Space',
    middleClick: 'Option+M',
    doubleClick: 'Enter',
    holdDrag: 'Shift',
    cycleMonitor: 'Tab',
    openSettings: 'Cmd+,',
  },
  windows: {
    toggleGrid: 'Ctrl+Alt+Space',
    toggleLabelMode: 'Ctrl+Alt+Shift+Space',
    toggleFreeMode: 'Ctrl+Alt+F',
    leftClick: 'Space',
    rightClick: 'Shift+Space',
    middleClick: 'Alt+M',
    doubleClick: 'Enter',
    holdDrag: 'Shift',
    cycleMonitor: 'Tab',
    openSettings: 'Ctrl+,',
  },
  linux: {
    toggleGrid: 'Super+Alt+Space',
    toggleLabelMode: 'Super+Alt+Shift+Space',
    toggleFreeMode: 'Super+Alt+F',
    leftClick: 'Space',
    rightClick: 'Shift+Space',
    middleClick: 'Alt+M',
    doubleClick: 'Enter',
    holdDrag: 'Shift',
    cycleMonitor: 'Tab',
    openSettings: 'Ctrl+,',
  },
};

export function detectClientOs(): PlatformOs {
  if (typeof navigator === 'undefined') return 'linux';
  const ua = `${navigator.userAgent} ${navigator.platform ?? ''}`.toLowerCase();
  if (ua.includes('mac')) return 'macos';
  if (ua.includes('win')) return 'windows';
  return 'linux';
}

export function getPlatformLabel(os: PlatformOs): string {
  switch (os) {
    case 'macos':
      return 'macOS';
    case 'windows':
      return 'Windows';
    default:
      return 'Linux';
  }
}

const BASE_DEFAULT_CONFIG: Omit<RatPoisonConfig, 'keybindings'> = {
  general: {
    launchAtLogin: true,
    showTrayIcon: true,
    multiMonitorStrategy: 'active-follow',
    hideCursorInOverlay: false,
    autoExitAfterClick: true,
    escapeCancelsMode: true,
    confirmBeforeQuit: false,
  },
  grid: {
    rows: 18,
    cols: 26,
    alphabet: 'asdfghjklqwertyuiopzxcvbnmASDFGHJKLQWERTYUIOPZXCVBNM',
    caseSensitive: true,
    enableSubgridZoom: true,
    subgridRows: 3,
    subgridCols: 3,
    subgridZoomScale: 1.85,
    nudgeStepPx: 8,
    fineNudgeStepPx: 2,
    showSubgridCrosshair: true,
    dimNonMatchingCells: true,
    dimOpacity: 0.2,
  },
  freeMode: {
    baseSpeedPxPerSec: 640,
    fastMultiplier: 2.5,
    slowMultiplier: 0.25,
    accelerationCurve: 1.35,
    scrollSpeed: 42,
    naturalScroll: true,
    showHudBadge: true,
    hudPosition: 'bottom-right',
    keyUp: 'I',
    keyDown: 'K',
    keyLeft: 'J',
    keyRight: 'L',
    modFast: 'F',
    modSlow: 'S',
    modScroll: 'D',
    modDrag: 'A',
  },
  labelMode: {
    enabled: true,
    charset: 'asdfghjklqweruiopzxcvbnm',
    minElementSizePx: 14,
    maxBadges: 160,
    collisionAvoidance: true,
    includeMenubar: true,
    includeDock: true,
    showElementRoleHint: true,
    badgePlacement: 'smart',
  },
  appearance: {
    theme: 'dark',
    gridBgColor: '#0b0e14',
    gridBgOpacity: 0.42,
    gridLineColor: '#94a3b8',
    gridLineWidth: 1,
    gridTextColor: '#f8fafc',
    gridAccentColor: '#f59e0b',
    gridFontFamily: '"JetBrains Mono", "SF Mono", Menlo, Consolas, monospace',
    gridFontSize: 13,
    gridFontWeight: 600,
    subgridAccentColor: '#38bdf8',
    subgridBgColor: '#0f172a',
    subgridBgOpacity: 0.86,
    crosshairColor: '#fb7185',
    labelBgColor: '#fef08a',
    labelBgOpacity: 0.96,
    labelBorderColor: '#a16207',
    labelTextColor: '#111827',
    labelMatchPrefixColor: '#dc2626',
    labelFontFamily: '"SF Pro Text", "Inter", system-ui, -apple-system, sans-serif',
    labelFontSize: 12,
    labelBorderRadius: 5,
    hudBgColor: '#0f172a',
    hudTextColor: '#f8fafc',
    hudAccentColor: '#38bdf8',
    hudOpacity: 0.88,
    reduceMotion: false,
  },
  updates: {
    channel: 'stable',
    autoCheck: true,
    autoDownload: false,
  },
};

export function getDefaultConfigForOs(os: PlatformOs): RatPoisonConfig {
  return {
    ...structuredClone(BASE_DEFAULT_CONFIG),
    keybindings: { ...OS_KEYBINDING_PRESETS[os] },
  };
}

export const DEFAULT_CONFIG: RatPoisonConfig = getDefaultConfigForOs(detectClientOs());

export interface SettingDescriptor {
  /** Dotted key path matching RatPoisonConfig and Rust `reset_setting_key`. */
  path: string;
  tab: SettingsTabId;
  section: string;
  label: string;
  description: string;
  keywords: string[];
}

export const SETTINGS_TABS: Array<{
  id: SettingsTabId;
  label: string;
  shortDescription: string;
}> = [
  {
    id: 'general',
    label: 'General',
    shortDescription: 'Startup, tray icon, multi-monitor overlay behavior, and exit defaults.',
  },
  {
    id: 'grid',
    label: 'Grid & Subgrid',
    shortDescription: 'NxM grid dimensions, case-sensitive labels, subgrid zoom, and pixel nudge.',
  },
  {
    id: 'freeMode',
    label: 'Free Mode',
    shortDescription: 'Continuous keyboard pointer movement, scrolling, speed profiles, and HUD.',
  },
  {
    id: 'labelMode',
    label: 'Label Mode',
    shortDescription: 'Accessibility element scanning, hint badges, and collision avoidance.',
  },
  {
    id: 'keybindings',
    label: 'Keybindings',
    shortDescription: 'Global hotkeys, mouse button chords, drag toggle, and monitor switching.',
  },
  {
    id: 'appearance',
    label: 'Appearance',
    shortDescription: 'Live colors, opacity, typography, and real-time overlay preview.',
  },
  {
    id: 'updates',
    label: 'Updates',
    shortDescription: 'Release channel, automatic update checks, and release notes.',
  },
];

export const SETTING_REGISTRY: SettingDescriptor[] = [
  // General
  { path: 'general.launchAtLogin', tab: 'general', section: 'Startup', label: 'Launch at login', description: 'Start RatPoison silently in the background when you sign in.', keywords: ['autostart', 'boot', 'startup', 'login'] },
  { path: 'general.showTrayIcon', tab: 'general', section: 'Startup', label: 'Show menu bar / tray icon', description: 'Display quick status and Settings access in the system tray.', keywords: ['menubar', 'tray', 'status', 'icon'] },
  { path: 'general.multiMonitorStrategy', tab: 'general', section: 'Displays', label: 'Multi-monitor overlay strategy', description: 'Follow the active cursor display, spawn one overlay per display, or span the virtual desktop.', keywords: ['monitor', 'screen', 'display', 'span', 'dpi'] },
  { path: 'general.hideCursorInOverlay', tab: 'general', section: 'Behavior', label: 'Hide OS cursor while overlay is active', description: 'Reduce visual clutter while picking grid or label coordinates.', keywords: ['cursor', 'pointer', 'hide'] },
  { path: 'general.autoExitAfterClick', tab: 'general', section: 'Behavior', label: 'Auto-dismiss overlay after click', description: 'Immediately return keyboard focus after triggering a click action.', keywords: ['dismiss', 'close', 'exit', 'click'] },
  { path: 'general.escapeCancelsMode', tab: 'general', section: 'Behavior', label: 'Escape cancels active mode', description: 'Press Esc at any point to dismiss the overlay without clicking.', keywords: ['esc', 'cancel', 'abort'] },
  { path: 'general.confirmBeforeQuit', tab: 'general', section: 'Behavior', label: 'Confirm before quitting', description: 'Show a confirmation prompt when quitting from the system tray.', keywords: ['quit', 'exit', 'confirm'] },

  // Grid & Subgrid
  { path: 'grid.rows', tab: 'grid', section: 'Grid Geometry', label: 'Grid rows (N)', description: 'Number of horizontal rows in primary Grid Mode.', keywords: ['nxm', 'height', 'density', 'cells'] },
  { path: 'grid.cols', tab: 'grid', section: 'Grid Geometry', label: 'Grid columns (M)', description: 'Number of vertical columns in primary Grid Mode.', keywords: ['nxm', 'width', 'density', 'cells'] },
  { path: 'grid.alphabet', tab: 'grid', section: 'Grid Geometry', label: 'Label character set', description: 'Characters used to synthesize 2-character grid coordinates.', keywords: ['charset', 'letters', 'keys', 'codes'] },
  { path: 'grid.caseSensitive', tab: 'grid', section: 'Grid Geometry', label: 'Case-sensitive 2-char labels', description: 'Allow uppercase and lowercase letters for dense high-DPI grids.', keywords: ['uppercase', 'lowercase', 'shift', 'case'] },
  { path: 'grid.dimNonMatchingCells', tab: 'grid', section: 'Prefix Filtering', label: 'Dim non-matching cells', description: 'Fade cells that do not match the typed first character.', keywords: ['filter', 'prefix', 'dim', 'highlight'] },
  { path: 'grid.dimOpacity', tab: 'grid', section: 'Prefix Filtering', label: 'Dimmed cell opacity', description: 'Opacity of unmatched cells while filtering a prefix.', keywords: ['alpha', 'transparency', 'fade'] },
  { path: 'grid.enableSubgridZoom', tab: 'grid', section: 'Subgrid & Nudge', label: 'Enable Subgrid Zoom', description: 'Zoom into the selected primary cell for pixel-accurate targeting.', keywords: ['zoom', 'subgrid', 'precision', 'inset'] },
  { path: 'grid.subgridRows', tab: 'grid', section: 'Subgrid & Nudge', label: 'Subgrid rows', description: 'Subdivision rows inside a zoomed cell.', keywords: ['subgrid', 'rows'] },
  { path: 'grid.subgridCols', tab: 'grid', section: 'Subgrid & Nudge', label: 'Subgrid columns', description: 'Subdivision columns inside a zoomed cell.', keywords: ['subgrid', 'cols'] },
  { path: 'grid.subgridZoomScale', tab: 'grid', section: 'Subgrid & Nudge', label: 'Subgrid magnification scale', description: 'Visual magnification factor for the zoomed subgrid inset.', keywords: ['magnify', 'scale', 'zoom'] },
  { path: 'grid.nudgeStepPx', tab: 'grid', section: 'Subgrid & Nudge', label: 'Nudge step (logical px)', description: 'Distance moved per arrow / nudge key press in Subgrid mode.', keywords: ['nudge', 'micro', 'step', 'pixels'] },
  { path: 'grid.fineNudgeStepPx', tab: 'grid', section: 'Subgrid & Nudge', label: 'Fine nudge step (logical px)', description: 'Micro-adjustment distance when holding the Slow modifier.', keywords: ['fine', 'precision', '1px', '2px'] },
  { path: 'grid.showSubgridCrosshair', tab: 'grid', section: 'Subgrid & Nudge', label: 'Show precision crosshair', description: 'Display target reticle and coordinates during Subgrid / Nudge.', keywords: ['reticle', 'crosshair', 'target'] },

  // Free Mode
  { path: 'freeMode.baseSpeedPxPerSec', tab: 'freeMode', section: 'Motion Physics', label: 'Base pointer speed (px/s)', description: 'Nominal cursor velocity when holding I/J/K/L directional keys.', keywords: ['velocity', 'speed', 'cursor', 'movement'] },
  { path: 'freeMode.fastMultiplier', tab: 'freeMode', section: 'Motion Physics', label: 'Fast modifier multiplier', description: 'Speed boost applied while holding the Fast modifier key.', keywords: ['turbo', 'boost', 'fast', 'multiplier'] },
  { path: 'freeMode.slowMultiplier', tab: 'freeMode', section: 'Motion Physics', label: 'Slow modifier multiplier', description: 'Precision dampening applied while holding the Slow modifier.', keywords: ['slow', 'precision', 'creep'] },
  { path: 'freeMode.accelerationCurve', tab: 'freeMode', section: 'Motion Physics', label: 'Acceleration exponent', description: 'Ramp-up curve exponent for smooth continuous pointer motion.', keywords: ['accel', 'ramp', 'curve', 'smooth'] },
  { path: 'freeMode.scrollSpeed', tab: 'freeMode', section: 'Scrolling', label: 'Scroll speed', description: 'Scroll delta per tick when Scroll modifier is active.', keywords: ['wheel', 'scroll', 'vertical', 'horizontal'] },
  { path: 'freeMode.naturalScroll', tab: 'freeMode', section: 'Scrolling', label: 'Natural scroll direction', description: 'Invert scroll direction to match macOS trackpad inertia.', keywords: ['invert', 'trackpad', 'natural'] },
  { path: 'freeMode.showHudBadge', tab: 'freeMode', section: 'HUD Indicator', label: 'Show minimal Free Mode HUD', description: 'Display a lightweight non-focusable badge showing active speed and modifiers.', keywords: ['hud', 'overlay', 'badge', 'status'] },
  { path: 'freeMode.hudPosition', tab: 'freeMode', section: 'HUD Indicator', label: 'HUD screen placement', description: 'Corner anchor or cursor-follow placement for the Free Mode HUD.', keywords: ['corner', 'position', 'anchor'] },
  { path: 'freeMode.keyUp', tab: 'freeMode', section: 'Free Mode Keys', label: 'Move Up key', description: 'Directional key for upward pointer movement (default I).', keywords: ['ijkl', 'up', 'direction'] },
  { path: 'freeMode.keyDown', tab: 'freeMode', section: 'Free Mode Keys', label: 'Move Down key', description: 'Directional key for downward pointer movement (default K).', keywords: ['ijkl', 'down', 'direction'] },
  { path: 'freeMode.keyLeft', tab: 'freeMode', section: 'Free Mode Keys', label: 'Move Left key', description: 'Directional key for leftward pointer movement (default J).', keywords: ['ijkl', 'left', 'direction'] },
  { path: 'freeMode.keyRight', tab: 'freeMode', section: 'Free Mode Keys', label: 'Move Right key', description: 'Directional key for rightward pointer movement (default L).', keywords: ['ijkl', 'right', 'direction'] },
  { path: 'freeMode.modFast', tab: 'freeMode', section: 'Free Mode Keys', label: 'Fast modifier key', description: 'Modifier held with directional keys for rapid traversal (default F).', keywords: ['asdf', 'fast', 'modifier'] },
  { path: 'freeMode.modSlow', tab: 'freeMode', section: 'Free Mode Keys', label: 'Slow modifier key', description: 'Modifier held for fine cursor control (default S).', keywords: ['asdf', 'slow', 'modifier'] },
  { path: 'freeMode.modScroll', tab: 'freeMode', section: 'Free Mode Keys', label: 'Scroll modifier key', description: 'Switches directional keys into 2D wheel scrolling (default D).', keywords: ['asdf', 'scroll', 'modifier'] },
  { path: 'freeMode.modDrag', tab: 'freeMode', section: 'Free Mode Keys', label: 'Drag modifier key', description: 'Holds primary mouse button while moving cursor (default A).', keywords: ['asdf', 'drag', 'modifier'] },

  // Label Mode
  { path: 'labelMode.enabled', tab: 'labelMode', section: 'Accessibility Discovery', label: 'Enable Label Mode', description: 'Query OS accessibility trees to place instant hint badges on UI controls.', keywords: ['a11y', 'accessibility', 'hints', 'vimium', 'elements'] },
  { path: 'labelMode.charset', tab: 'labelMode', section: 'Accessibility Discovery', label: 'Badge hint characters', description: 'Home-row characters prioritized for short Label Mode badges.', keywords: ['alphabet', 'home row', 'hints'] },
  { path: 'labelMode.minElementSizePx', tab: 'labelMode', section: 'Accessibility Discovery', label: 'Minimum target size (px)', description: 'Ignore tiny invisible or decorative elements smaller than this threshold.', keywords: ['filter', 'size', 'threshold'] },
  { path: 'labelMode.maxBadges', tab: 'labelMode', section: 'Accessibility Discovery', label: 'Maximum visible badges', description: 'Upper cap on rendered badges to guarantee sub-frame responsiveness.', keywords: ['limit', 'performance', 'cap'] },
  { path: 'labelMode.collisionAvoidance', tab: 'labelMode', section: 'Badge Layout', label: 'Collision-aware badge placement', description: 'Nudge overlapping badges apart with leader lines so every code stays readable.', keywords: ['overlap', 'collision', 'layout', 'pills'] },
  { path: 'labelMode.badgePlacement', tab: 'labelMode', section: 'Badge Layout', label: 'Badge anchor preference', description: 'Preferred anchor corner relative to each interactive element bounding box.', keywords: ['anchor', 'position', 'corner'] },
  { path: 'labelMode.includeMenubar', tab: 'labelMode', section: 'Scope', label: 'Include system menu bar', description: 'Scan top menu bar items and status extras.', keywords: ['menubar', 'top bar', 'macos'] },
  { path: 'labelMode.includeDock', tab: 'labelMode', section: 'Scope', label: 'Include Dock / Taskbar', description: 'Scan pinned and active Dock or Taskbar icons.', keywords: ['dock', 'taskbar', 'panel'] },
  { path: 'labelMode.showElementRoleHint', tab: 'labelMode', section: 'Badge Layout', label: 'Show compact role tag', description: 'Append subtle role micro-tag (BTN, LNK, TXT) next to matched badges.', keywords: ['role', 'button', 'link', 'hint'] },

  // Keybindings
  { path: 'keybindings.toggleGrid', tab: 'keybindings', section: 'Mode Activators', label: 'Toggle Grid Mode', description: 'Global shortcut to activate or dismiss the NxM Grid overlay.', keywords: ['hotkey', 'shortcut', 'grid', 'trigger'] },
  { path: 'keybindings.toggleLabelMode', tab: 'keybindings', section: 'Mode Activators', label: 'Toggle Label Mode', description: 'Global shortcut to scan interactive UI elements and show badges.', keywords: ['hotkey', 'shortcut', 'labels', 'hints'] },
  { path: 'keybindings.toggleFreeMode', tab: 'keybindings', section: 'Mode Activators', label: 'Toggle Free Mode', description: 'Global shortcut to enter continuous keyboard cursor control.', keywords: ['hotkey', 'shortcut', 'free', 'ijkl'] },
  { path: 'keybindings.leftClick', tab: 'keybindings', section: 'Mouse Actions', label: 'Left click', description: 'Trigger a primary click at the targeted coordinate.', keywords: ['click', 'primary', 'space'] },
  { path: 'keybindings.rightClick', tab: 'keybindings', section: 'Mouse Actions', label: 'Right click', description: 'Trigger a secondary context-menu click.', keywords: ['context', 'secondary', 'right'] },
  { path: 'keybindings.middleClick', tab: 'keybindings', section: 'Mouse Actions', label: 'Middle click', description: 'Trigger a middle button click (e.g. open link in new tab).', keywords: ['middle', 'tab', 'wheel'] },
  { path: 'keybindings.doubleClick', tab: 'keybindings', section: 'Mouse Actions', label: 'Double click', description: 'Trigger a rapid double primary click.', keywords: ['double', 'open', 'enter'] },
  { path: 'keybindings.holdDrag', tab: 'keybindings', section: 'Mouse Actions', label: 'Toggle hold / drag', description: 'Latch left mouse button down to drag selections or windows.', keywords: ['drag', 'drop', 'hold'] },
  { path: 'keybindings.cycleMonitor', tab: 'keybindings', section: 'Navigation', label: 'Cycle active monitor', description: 'Move the active overlay focus to the next connected display.', keywords: ['monitor', 'display', 'screen', 'tab'] },
  { path: 'keybindings.openSettings', tab: 'keybindings', section: 'Navigation', label: 'Open Settings window', description: 'Show and focus the RatPoison Settings window.', keywords: ['preferences', 'settings', 'config'] },

  // Appearance
  { path: 'appearance.theme', tab: 'appearance', section: 'Window Theme', label: 'Settings UI theme', description: 'Choose Dark, Light, or follow the OS appearance.', keywords: ['dark', 'light', 'theme', 'mode'] },
  { path: 'appearance.reduceMotion', tab: 'appearance', section: 'Window Theme', label: 'Reduce motion', description: 'Disable subtle overlay transition animations for instant snapping.', keywords: ['animation', 'motion', 'instant'] },
  { path: 'appearance.gridBgColor', tab: 'appearance', section: 'Grid Overlay', label: 'Grid backdrop color', description: 'Base tint drawn behind the NxM grid lines.', keywords: ['color', 'background', 'grid', 'backdrop'] },
  { path: 'appearance.gridBgOpacity', tab: 'appearance', section: 'Grid Overlay', label: 'Grid backdrop opacity', description: 'Transparency level of the grid backdrop (0% = fully clear).', keywords: ['opacity', 'alpha', 'transparent'] },
  { path: 'appearance.gridLineColor', tab: 'appearance', section: 'Grid Overlay', label: 'Grid line color', description: 'Stroke color for horizontal and vertical grid divisions.', keywords: ['lines', 'border', 'stroke'] },
  { path: 'appearance.gridLineWidth', tab: 'appearance', section: 'Grid Overlay', label: 'Grid line thickness (px)', description: 'Stroke width of primary grid hairlines.', keywords: ['thickness', 'width', 'stroke'] },
  { path: 'appearance.gridTextColor', tab: 'appearance', section: 'Grid Overlay', label: 'Grid label text color', description: 'Color of 2-character grid coordinate codes.', keywords: ['text', 'font', 'color'] },
  { path: 'appearance.gridAccentColor', tab: 'appearance', section: 'Grid Overlay', label: 'Active prefix highlight color', description: 'Accent color highlighting the typed character and exact matches.', keywords: ['accent', 'highlight', 'prefix', 'active'] },
  { path: 'appearance.gridFontFamily', tab: 'appearance', section: 'Grid Overlay', label: 'Grid font family', description: 'Typeface used for Grid and Subgrid coordinate labels.', keywords: ['font', 'typeface', 'mono'] },
  { path: 'appearance.gridFontSize', tab: 'appearance', section: 'Grid Overlay', label: 'Grid font size (px)', description: 'Base font size for grid cell coordinate labels.', keywords: ['font', 'size', 'px'] },
  { path: 'appearance.gridFontWeight', tab: 'appearance', section: 'Grid Overlay', label: 'Grid font weight', description: 'Weight of grid cell label text (400–800).', keywords: ['bold', 'weight', 'font'] },
  { path: 'appearance.subgridAccentColor', tab: 'appearance', section: 'Subgrid & Crosshair', label: 'Subgrid border accent', description: 'Highlight frame color around the zoomed subgrid inset.', keywords: ['subgrid', 'border', 'accent'] },
  { path: 'appearance.subgridBgColor', tab: 'appearance', section: 'Subgrid & Crosshair', label: 'Subgrid fill color', description: 'Fill backdrop inside the zoomed subgrid inset.', keywords: ['subgrid', 'background'] },
  { path: 'appearance.subgridBgOpacity', tab: 'appearance', section: 'Subgrid & Crosshair', label: 'Subgrid fill opacity', description: 'Opacity of the zoomed subgrid panel.', keywords: ['subgrid', 'opacity'] },
  { path: 'appearance.crosshairColor', tab: 'appearance', section: 'Subgrid & Crosshair', label: 'Nudge crosshair color', description: 'Color of the precision nudge reticle lines.', keywords: ['crosshair', 'reticle', 'nudge'] },
  { path: 'appearance.labelBgColor', tab: 'appearance', section: 'Label Mode Badges', label: 'Badge background color', description: 'Pill fill color for Label Mode accessibility hints.', keywords: ['label', 'badge', 'pill', 'yellow', 'color'] },
  { path: 'appearance.labelBgOpacity', tab: 'appearance', section: 'Label Mode Badges', label: 'Badge opacity', description: 'Opacity of Label Mode hint badges.', keywords: ['label', 'opacity', 'alpha'] },
  { path: 'appearance.labelBorderColor', tab: 'appearance', section: 'Label Mode Badges', label: 'Badge border color', description: 'Outline border color around each Label Mode pill.', keywords: ['label', 'border', 'outline'] },
  { path: 'appearance.labelTextColor', tab: 'appearance', section: 'Label Mode Badges', label: 'Badge text color', description: 'Foreground text color for unmatched hint characters.', keywords: ['label', 'text', 'contrast'] },
  { path: 'appearance.labelMatchPrefixColor', tab: 'appearance', section: 'Label Mode Badges', label: 'Matched prefix text color', description: 'Color of characters already typed inside matching badges.', keywords: ['prefix', 'typed', 'match', 'label'] },
  { path: 'appearance.labelFontFamily', tab: 'appearance', section: 'Label Mode Badges', label: 'Badge font family', description: 'Typeface used inside Label Mode hint badges.', keywords: ['font', 'label', 'typeface'] },
  { path: 'appearance.labelFontSize', tab: 'appearance', section: 'Label Mode Badges', label: 'Badge font size (px)', description: 'Text size for Label Mode pills.', keywords: ['font', 'size', 'label'] },
  { path: 'appearance.labelBorderRadius', tab: 'appearance', section: 'Label Mode Badges', label: 'Badge corner radius (px)', description: 'Border radius of Label Mode pills.', keywords: ['radius', 'rounded', 'pill'] },
  { path: 'appearance.hudBgColor', tab: 'appearance', section: 'Free Mode HUD', label: 'HUD background color', description: 'Surface color of the floating Free Mode HUD indicator.', keywords: ['hud', 'background', 'color'] },
  { path: 'appearance.hudTextColor', tab: 'appearance', section: 'Free Mode HUD', label: 'HUD text color', description: 'Text color inside the Free Mode HUD.', keywords: ['hud', 'text'] },
  { path: 'appearance.hudAccentColor', tab: 'appearance', section: 'Free Mode HUD', label: 'HUD active modifier accent', description: 'Highlight color for active directional/speed modifiers in HUD.', keywords: ['hud', 'accent', 'modifier'] },
  { path: 'appearance.hudOpacity', tab: 'appearance', section: 'Free Mode HUD', label: 'HUD opacity', description: 'Overall opacity of the Free Mode HUD pill.', keywords: ['hud', 'opacity'] },

  // Updates
  { path: 'updates.channel', tab: 'updates', section: 'Release Channel', label: 'Update channel', description: 'Track Stable releases or opt into Beta builds.', keywords: ['stable', 'beta', 'release', 'channel'] },
  { path: 'updates.autoCheck', tab: 'updates', section: 'Automation', label: 'Check for updates automatically', description: 'Periodically check GitHub Releases on startup.', keywords: ['auto', 'check', 'updates'] },
  { path: 'updates.autoDownload', tab: 'updates', section: 'Automation', label: 'Download updates in background', description: 'Pre-fetch release packages ready for restart.', keywords: ['download', 'background', 'updates'] },
];

export const SETTING_MAP: Record<string, SettingDescriptor> = Object.fromEntries(
  SETTING_REGISTRY.map((entry) => [entry.path, entry]),
);

export function getByPath<T = unknown>(obj: unknown, path: string): T {
  const parts = path.split('.');
  let current: unknown = obj;
  for (const part of parts) {
    if (current == null || typeof current !== 'object') {
      return undefined as T;
    }
    current = (current as Record<string, unknown>)[part];
  }
  return current as T;
}

export function setByPath<T extends object>(obj: T, path: string, value: unknown): T {
  const parts = path.split('.');
  const clone = structuredClone(obj);
  let cursor: Record<string, unknown> = clone as Record<string, unknown>;
  for (let i = 0; i < parts.length - 1; i++) {
    const key = parts[i];
    const next = cursor[key];
    cursor[key] = next && typeof next === 'object' ? { ...(next as Record<string, unknown>) } : {};
    cursor = cursor[key] as Record<string, unknown>;
  }
  cursor[parts[parts.length - 1]] = value;
  return clone;
}

export function isModifiedPath(
  config: RatPoisonConfig,
  path: string,
  defaults: RatPoisonConfig = DEFAULT_CONFIG,
): boolean {
  const current = getByPath(config, path);
  const def = getByPath(defaults, path);
  return JSON.stringify(current) !== JSON.stringify(def);
}

export function getModifiedPathsForTab(
  config: RatPoisonConfig,
  tab: SettingsTabId,
  defaults: RatPoisonConfig = DEFAULT_CONFIG,
): string[] {
  return SETTING_REGISTRY.filter((s) => s.tab === tab && isModifiedPath(config, s.path, defaults)).map(
    (s) => s.path,
  );
}

export function formatSettingValue(value: unknown): string {
  if (typeof value === 'boolean') return value ? 'Enabled' : 'Disabled';
  if (typeof value === 'number') return String(value);
  if (typeof value === 'string') return value.length > 28 ? `${value.slice(0, 26)}…` : value;
  return JSON.stringify(value);
}

/**
 * Lightweight fuzzy + substring scorer for Settings live search.
 * Returns null if no match, or a positive relevance score (higher = better).
 */
export function scoreSettingMatch(descriptor: SettingDescriptor, rawQuery: string): number | null {
  const query = rawQuery.trim().toLowerCase();
  if (!query) return 1;

  const haystackLabel = descriptor.label.toLowerCase();
  const haystackPath = descriptor.path.toLowerCase();
  const haystackDesc = descriptor.description.toLowerCase();
  const haystackSection = descriptor.section.toLowerCase();
  const haystackKeywords = descriptor.keywords.join(' ').toLowerCase();

  // Exact / prefix / substring priority
  if (haystackLabel === query) return 1000;
  if (haystackLabel.startsWith(query)) return 850;
  if (haystackLabel.includes(query)) return 700;
  if (haystackKeywords.includes(query)) return 620;
  if (haystackPath.includes(query)) return 560;
  if (haystackSection.includes(query)) return 500;
  if (haystackDesc.includes(query)) return 420;

  // Multi-token AND substring matching
  const tokens = query.split(/\s+/).filter(Boolean);
  const combined = `${haystackLabel} ${haystackSection} ${haystackKeywords} ${haystackDesc} ${haystackPath}`;
  if (tokens.length > 1 && tokens.every((t) => combined.includes(t))) {
    return 380;
  }

  // Ordered subsequence fuzzy match against label + keywords
  const fuzzyTarget = `${haystackLabel} ${haystackKeywords}`;
  let qi = 0;
  let consecutiveBonus = 0;
  let lastMatchIdx = -2;
  for (let i = 0; i < fuzzyTarget.length && qi < query.length; i++) {
    if (fuzzyTarget[i] === query[qi]) {
      if (i === lastMatchIdx + 1) consecutiveBonus += 12;
      lastMatchIdx = i;
      qi++;
    }
  }
  if (qi === query.length) {
    return 180 + consecutiveBonus;
  }

  return null;
}

export const FONT_FAMILY_PRESETS = [
  { label: 'JetBrains Mono / SF Mono', value: '"JetBrains Mono", "SF Mono", Menlo, Consolas, monospace' },
  { label: 'SF Pro / Inter (System UI)', value: '"SF Pro Text", "Inter", system-ui, -apple-system, sans-serif' },
  { label: 'Fira Code / Cascadia Code', value: '"Fira Code", "Cascadia Code", "SF Mono", monospace' },
  { label: 'IBM Plex Mono', value: '"IBM Plex Mono", Menlo, monospace' },
  { label: 'System Monospace', value: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace' },
];
