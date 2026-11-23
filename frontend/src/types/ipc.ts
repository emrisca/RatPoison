/**
 * Shared IPC & domain types for RatPoison Frontend <-> Rust/Tauri v2 Core.
 * Strictly aligned with docs/SWARM_CONTRACT.md.
 */

export interface LogicalPoint {
  x: number;
  y: number;
}

export interface LogicalRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PhysicalPoint {
  x: number;
  y: number;
}

export interface MonitorInfo {
  id: string;
  name: string;
  isPrimary: boolean;
  bounds: LogicalRect;
  scaleFactor: number;
}

export type OverlayMode = 'idle' | 'grid' | 'subgrid' | 'label' | 'free';

export interface GridCell {
  /** Case-sensitive 2-character label (e.g. "Aa", "fK"). */
  code: string;
  row: number;
  col: number;
  rect: LogicalRect;
  /** True if cell code starts with the active typed prefix. */
  matchedPrefix: boolean;
  /** True if cell code exactly matches the active typed prefix. */
  exactMatch: boolean;
}

export interface SubgridState {
  parentCode: string;
  parentRect: LogicalRect;
  rows: number;
  cols: number;
  cells: GridCell[];
  nudgeStepPx: number;
  crosshair: LogicalPoint;
}

export interface LabelBadge {
  id: string;
  code: string;
  role: string;
  title?: string;
  /** Accessibility element bounding box in logical screen coordinates. */
  rect: LogicalRect;
  /** Preferred badge anchor point in logical coordinates. */
  anchor: LogicalPoint;
  matchedPrefix: boolean;
  exactMatch: boolean;
  /** Optional collision resolution delta computed by Rust or frontend. */
  collisionOffset?: LogicalPoint;
}

export type SpeedProfile = 'slow' | 'normal' | 'fast';

export interface FreeModeState {
  cursor: LogicalPoint;
  velocity: { vx: number; vy: number };
  speedProfile: SpeedProfile;
  scrollMode: boolean;
  dragActive: boolean;
  activeDirections: Array<'up' | 'down' | 'left' | 'right'>;
  heldModifiers: string[];
}

/**
 * Payload emitted by Rust via `ratpoison://overlay-state`.
 */
export interface OverlayStatePayload {
  mode: OverlayMode;
  visible: boolean;
  activeMonitor: MonitorInfo;
  typedPrefix: string;
  gridRows: number;
  gridCols: number;
  gridCells: GridCell[];
  subgrid: SubgridState | null;
  labelBadges: LabelBadge[];
  freeMode: FreeModeState | null;
  cursorPreview: LogicalPoint | null;
  activeModifiers: string[];
  frameTimestampMs?: number;
}

export type MultiMonitorStrategy = 'active-follow' | 'per-monitor' | 'virtual-span';
export type HudPosition = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right' | 'cursor-follow';
export type BadgePlacement = 'smart' | 'top-left' | 'center' | 'top-right';
export type AppTheme = 'dark' | 'light' | 'system';
export type UpdateChannel = 'stable' | 'beta';

export interface RatPoisonConfig {
  general: {
    launchAtLogin: boolean;
    showTrayIcon: boolean;
    multiMonitorStrategy: MultiMonitorStrategy;
    hideCursorInOverlay: boolean;
    autoExitAfterClick: boolean;
    escapeCancelsMode: boolean;
    confirmBeforeQuit: boolean;
  };
  grid: {
    rows: number;
    cols: number;
    alphabet: string;
    caseSensitive: boolean;
    enableSubgridZoom: boolean;
    subgridRows: number;
    subgridCols: number;
    subgridZoomScale: number;
    nudgeStepPx: number;
    fineNudgeStepPx: number;
    showSubgridCrosshair: boolean;
    dimNonMatchingCells: boolean;
    dimOpacity: number;
  };
  freeMode: {
    baseSpeedPxPerSec: number;
    fastMultiplier: number;
    slowMultiplier: number;
    accelerationCurve: number;
    scrollSpeed: number;
    naturalScroll: boolean;
    showHudBadge: boolean;
    hudPosition: HudPosition;
    keyUp: string;
    keyDown: string;
    keyLeft: string;
    keyRight: string;
    modFast: string;
    modSlow: string;
    modScroll: string;
    modDrag: string;
  };
  labelMode: {
    enabled: boolean;
    charset: string;
    minElementSizePx: number;
    maxBadges: number;
    collisionAvoidance: boolean;
    includeMenubar: boolean;
    includeDock: boolean;
    showElementRoleHint: boolean;
    badgePlacement: BadgePlacement;
  };
  keybindings: {
    toggleGrid: string;
    toggleLabelMode: string;
    toggleFreeMode: string;
    leftClick: string;
    rightClick: string;
    middleClick: string;
    doubleClick: string;
    holdDrag: string;
    cycleMonitor: string;
    openSettings: string;
  };
  appearance: {
    theme: AppTheme;
    gridBgColor: string;
    gridBgOpacity: number;
    gridLineColor: string;
    gridLineWidth: number;
    gridTextColor: string;
    gridAccentColor: string;
    gridFontFamily: string;
    gridFontSize: number;
    gridFontWeight: number;
    subgridAccentColor: string;
    subgridBgColor: string;
    subgridBgOpacity: number;
    crosshairColor: string;
    labelBgColor: string;
    labelBgOpacity: number;
    labelBorderColor: string;
    labelTextColor: string;
    labelMatchPrefixColor: string;
    labelFontFamily: string;
    labelFontSize: number;
    labelBorderRadius: number;
    hudBgColor: string;
    hudTextColor: string;
    hudAccentColor: string;
    hudOpacity: number;
    reduceMotion: boolean;
  };
  updates: {
    channel: UpdateChannel;
    autoCheck: boolean;
    autoDownload: boolean;
  };
}

export type PlatformOs = 'macos' | 'windows' | 'linux';

export interface PlatformInfo {
  os: PlatformOs;
  arch: string;
  displayName: string;
  defaultKeybindings: RatPoisonConfig['keybindings'];
}

export interface PermissionStatus {
  platform: string;
  accessibilityGranted: boolean;
  inputMonitoringGranted: boolean;
  secureInputActive: boolean;
  hookActive: boolean;
  hookBackend: string;
  guidance?: string;
  lastError?: string;
}

export interface UpdateCheckResult {
  currentVersion: string;
  latestVersion: string;
  updateAvailable: boolean;
  releaseNotes: string;
  publishedAt: string;
  downloadUrl?: string;
  releaseUrl?: string;
  assetName?: string;
}

export type UpdateStatusState =
  | 'idle'
  | 'checking'
  | 'available'
  | 'downloading'
  | 'installing'
  | 'ready'
  | 'upToDate'
  | 'error';

export interface UpdateStatus {
  state: UpdateStatusState;
  currentVersion: string;
  latestVersion?: string;
  downloadedBytes: number;
  totalBytes?: number;
  percent?: number;
  releaseNotes?: string;
  publishedAt?: string;
  message?: string;
  autoInstall: boolean;
}

export type SettingsTabId =
  | 'general'
  | 'grid'
  | 'freeMode'
  | 'labelMode'
  | 'keybindings'
  | 'appearance'
  | 'updates';

export const TAURI_EVENTS = {
  OVERLAY_STATE: 'ratpoison://overlay-state',
  CONFIG_UPDATED: 'ratpoison://config-updated',
  UPDATE_STATUS: 'ratpoison://update-status',
} as const;

export const TAURI_COMMANDS = {
  GET_CONFIG: 'get_config',
  UPDATE_CONFIG: 'update_config',
  RESET_SETTING_KEY: 'reset_setting_key',
  GET_MONITORS: 'get_monitors',
  GET_PLATFORM_INFO: 'get_platform_info',
  GET_OVERLAY_STATE: 'get_overlay_state',
  GET_PERMISSIONS: 'get_permissions',
  REQUEST_PERMISSIONS: 'request_permissions',
  OPEN_ACCESSIBILITY_SETTINGS: 'open_accessibility_settings',
  CHECK_FOR_UPDATES: 'check_for_updates',
  GET_UPDATE_STATUS: 'get_update_status',
  INSTALL_UPDATE: 'install_update',
  OPEN_EXTERNAL_URL: 'open_external_url',
  TOGGLE_OVERLAY_MODE: 'toggle_overlay_mode',
} as const;
