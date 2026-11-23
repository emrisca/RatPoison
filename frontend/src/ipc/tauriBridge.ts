import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import {
  DEFAULT_CONFIG,
  OS_KEYBINDING_PRESETS,
  detectClientOs,
  getByPath,
  getPlatformLabel,
  setByPath,
} from '../config/defaults';
import {
  TAURI_COMMANDS,
  TAURI_EVENTS,
  type GridCell,
  type LabelBadge,
  type MonitorInfo,
  type OverlayMode,
  type OverlayStatePayload,
  type PermissionStatus,
  type PlatformInfo,
  type RatPoisonConfig,
  type UpdateCheckResult,
} from '../types/ipc';

/** Detect whether we are executing inside a Tauri v2 WebView runtime. */
export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** Determine active view mode (`overlay` vs `settings`) from query param or Tauri window label. */
export function resolveViewKind(): 'overlay' | 'settings' {
  const params = new URLSearchParams(window.location.search);
  const queryView = params.get('view');
  if (queryView === 'overlay') return 'overlay';
  if (queryView === 'settings') return 'settings';

  if (isTauriRuntime()) {
    try {
      const label = getCurrentWebviewWindow().label;
      if (label === 'overlay' || label.startsWith('overlay-')) {
        return 'overlay';
      }
      if (label === 'settings') {
        return 'settings';
      }
    } catch {
      // Fallback below
    }
  }
  return 'settings';
}

/**
 * Ensure the overlay window is strictly click-through when running inside Tauri.
 * Rust sets this during window creation; calling defensively guarantees consistency across reloads.
 */
export async function enforceOverlayClickThrough(): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const win = getCurrentWebviewWindow();
    await win.setIgnoreCursorEvents(true);
  } catch (err) {
    console.warn('[RatPoison] Failed to enforce click-through on overlay window:', err);
  }
}

// ---------------------------------------------------------------------------
// Browser-dev fallback state (used only when running Vite outside Tauri)
// ---------------------------------------------------------------------------
let mockConfig: RatPoisonConfig = structuredClone(DEFAULT_CONFIG);

const MOCK_MONITORS: MonitorInfo[] = [
  {
    id: 'display-1',
    name: 'Built-in Retina Display',
    isPrimary: true,
    bounds: { x: 0, y: 0, width: 1512, height: 982 },
    scaleFactor: 2.0,
  },
  {
    id: 'display-2',
    name: 'Studio Display 5K',
    isPrimary: false,
    bounds: { x: 1512, y: 0, width: 2560, height: 1440 },
    scaleFactor: 2.0,
  },
];

function deepMergeDefaults<T extends object>(defaults: T, incoming: Partial<T> | null | undefined): T {
  if (!incoming || typeof incoming !== 'object') return structuredClone(defaults);
  const result: Record<string, unknown> = { ...(defaults as Record<string, unknown>) };
  for (const [k, v] of Object.entries(incoming as Record<string, unknown>)) {
    const defVal = result[k];
    if (
      v !== null &&
      typeof v === 'object' &&
      !Array.isArray(v) &&
      defVal !== null &&
      typeof defVal === 'object' &&
      !Array.isArray(defVal)
    ) {
      result[k] = deepMergeDefaults(defVal as Record<string, unknown>, v as Record<string, unknown>);
    } else if (v !== undefined) {
      result[k] = v;
    }
  }
  return result as T;
}

// ---------------------------------------------------------------------------
// Typed Tauri Command Wrappers
// ---------------------------------------------------------------------------

export async function ipcGetPlatformInfo(): Promise<PlatformInfo> {
  if (!isTauriRuntime()) {
    const os = detectClientOs();
    return {
      os,
      arch: 'web',
      displayName: `${getPlatformLabel(os)} (Browser Preview)`,
      defaultKeybindings: { ...OS_KEYBINDING_PRESETS[os] },
    };
  }
  return invoke<PlatformInfo>(TAURI_COMMANDS.GET_PLATFORM_INFO);
}

export async function ipcGetConfig(defaults: RatPoisonConfig = DEFAULT_CONFIG): Promise<RatPoisonConfig> {
  if (!isTauriRuntime()) return structuredClone(mockConfig);
  const raw = await invoke<Partial<RatPoisonConfig>>(TAURI_COMMANDS.GET_CONFIG);
  return deepMergeDefaults(defaults, raw);
}

export async function ipcUpdateConfig(
  config: RatPoisonConfig,
  defaults: RatPoisonConfig = DEFAULT_CONFIG,
): Promise<RatPoisonConfig> {
  if (!isTauriRuntime()) {
    mockConfig = structuredClone(config);
    return structuredClone(mockConfig);
  }
  const updated = await invoke<Partial<RatPoisonConfig>>(TAURI_COMMANDS.UPDATE_CONFIG, { config });
  return deepMergeDefaults(defaults, updated ?? config);
}

export async function ipcResetSettingKey(
  key: string,
  defaults: RatPoisonConfig = DEFAULT_CONFIG,
): Promise<RatPoisonConfig> {
  if (!isTauriRuntime()) {
    const defVal = getByPath(defaults, key);
    mockConfig = setByPath(mockConfig, key, defVal);
    return structuredClone(mockConfig);
  }
  const updated = await invoke<Partial<RatPoisonConfig>>(TAURI_COMMANDS.RESET_SETTING_KEY, { key });
  return deepMergeDefaults(defaults, updated);
}

export async function ipcGetMonitors(): Promise<MonitorInfo[]> {
  if (!isTauriRuntime()) return MOCK_MONITORS;
  return invoke<MonitorInfo[]>(TAURI_COMMANDS.GET_MONITORS);
}

export const GITHUB_RELEASES_URL = 'https://github.com/emrisca/RatPoison/releases';

export async function ipcCheckForUpdates(channel: string): Promise<UpdateCheckResult> {
  if (!isTauriRuntime()) {
    return {
      currentVersion: '0.1.1',
      latestVersion: channel === 'beta' ? '0.2.0-beta.1' : '0.1.1',
      updateAvailable: channel === 'beta',
      releaseNotes:
        channel === 'beta'
          ? '• Sub-frame 120Hz grid rendering\n• Wayland layer-shell preview\n• Collision-aware Label Mode leader lines'
          : 'You are running the latest stable build of RatPoison.',
      publishedAt: new Date().toISOString(),
      downloadUrl: GITHUB_RELEASES_URL,
      releaseUrl: GITHUB_RELEASES_URL,
      assetName: channel === 'beta' ? 'RatPoison_0.2.0-beta.1_universal.dmg' : undefined,
    };
  }
  return invoke<UpdateCheckResult>(TAURI_COMMANDS.CHECK_FOR_UPDATES, { channel });
}

export async function ipcOpenExternalUrl(url: string): Promise<void> {
  if (!isTauriRuntime()) {
    window.open(url, '_blank', 'noopener,noreferrer');
    return;
  }
  await invoke<void>(TAURI_COMMANDS.OPEN_EXTERNAL_URL, { url });
}

export async function ipcGetOverlayState(): Promise<OverlayStatePayload | null> {
  if (!isTauriRuntime()) return null;
  return invoke<OverlayStatePayload>(TAURI_COMMANDS.GET_OVERLAY_STATE);
}

export async function ipcGetPermissions(): Promise<PermissionStatus> {
  if (!isTauriRuntime()) {
    return {
      platform: detectClientOs(),
      accessibilityGranted: true,
      inputMonitoringGranted: true,
      secureInputActive: false,
      hookActive: true,
      hookBackend: 'Browser Preview',
    };
  }
  return invoke<PermissionStatus>(TAURI_COMMANDS.GET_PERMISSIONS);
}

export async function ipcRequestPermissions(): Promise<PermissionStatus> {
  if (!isTauriRuntime()) {
    return ipcGetPermissions();
  }
  return invoke<PermissionStatus>(TAURI_COMMANDS.REQUEST_PERMISSIONS);
}

export async function ipcOpenAccessibilitySettings(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke<void>(TAURI_COMMANDS.OPEN_ACCESSIBILITY_SETTINGS);
}

export async function ipcToggleOverlayMode(mode: 'grid' | 'label' | 'free' | 'idle'): Promise<OverlayStatePayload | null> {
  if (!isTauriRuntime()) return null;
  return invoke<OverlayStatePayload>(TAURI_COMMANDS.TOGGLE_OVERLAY_MODE, { mode });
}

// ---------------------------------------------------------------------------
// Event Subscriptions
// ---------------------------------------------------------------------------

export async function subscribeOverlayState(
  handler: (payload: OverlayStatePayload) => void,
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) {
    return () => {};
  }
  return listen<OverlayStatePayload>(TAURI_EVENTS.OVERLAY_STATE, (event) => {
    handler(event.payload);
  });
}

export async function subscribeConfigUpdated(
  handler: (config: RatPoisonConfig) => void,
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) {
    return () => {};
  }
  return listen<Partial<RatPoisonConfig>>(TAURI_EVENTS.CONFIG_UPDATED, (event) => {
    handler(deepMergeDefaults(DEFAULT_CONFIG, event.payload));
  });
}

// ---------------------------------------------------------------------------
// Deterministic Synthetic Overlay Builder (for Browser Dev Preview)
// ---------------------------------------------------------------------------

export function buildDemoOverlayState(
  mode: OverlayMode,
  width: number,
  height: number,
  config: RatPoisonConfig,
  typedPrefix = '',
): OverlayStatePayload {
  const rows = Math.max(4, Math.min(36, config.grid.rows));
  const cols = Math.max(4, Math.min(36, config.grid.cols));
  const alphabet = config.grid.alphabet || 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ';
  const cellW = width / cols;
  const cellH = height / rows;

  const gridCells: GridCell[] = [];
  for (let r = 0; r < rows; r++) {
    const first = alphabet[r % alphabet.length];
    for (let c = 0; c < cols; c++) {
      const second = alphabet[c % alphabet.length];
      const code = `${first}${second}`;
      const compareCode = config.grid.caseSensitive ? code : code.toLowerCase();
      const comparePrefix = config.grid.caseSensitive ? typedPrefix : typedPrefix.toLowerCase();
      gridCells.push({
        code,
        row: r,
        col: c,
        rect: { x: c * cellW, y: r * cellH, width: cellW, height: cellH },
        matchedPrefix: comparePrefix.length === 0 || compareCode.startsWith(comparePrefix),
        exactMatch: comparePrefix.length > 0 && compareCode === comparePrefix,
      });
    }
  }

  // Pick a centered cell for Subgrid demo
  const targetCell = gridCells[Math.floor(rows * 0.45) * cols + Math.floor(cols * 0.5)] ?? gridCells[0];
  const subRows = config.grid.subgridRows;
  const subCols = config.grid.subgridCols;
  const subCells: GridCell[] = [];
  const subCharset = 'qweasdzxc123456789';
  for (let r = 0; r < subRows; r++) {
    for (let c = 0; c < subCols; c++) {
      const idx = r * subCols + c;
      const code = subCharset[idx % subCharset.length];
      subCells.push({
        code,
        row: r,
        col: c,
        rect: {
          x: targetCell.rect.x + (c * targetCell.rect.width) / subCols,
          y: targetCell.rect.y + (r * targetCell.rect.height) / subRows,
          width: targetCell.rect.width / subCols,
          height: targetCell.rect.height / subRows,
        },
        matchedPrefix: true,
        exactMatch: idx === Math.floor((subRows * subCols) / 2),
      });
    }
  }

  const rawLabels: Array<{ code: string; role: string; title: string; x: number; y: number; w: number; h: number }> = [
    { code: 'aa', role: 'BTN', title: 'Back', x: 32, y: 24, w: 68, h: 28 },
    { code: 'as', role: 'BTN', title: 'Forward', x: 108, y: 24, w: 68, h: 28 },
    { code: 'ad', role: 'TXT', title: 'Search bar', x: 220, y: 22, w: 340, h: 32 },
    { code: 'af', role: 'BTN', title: 'Reload', x: 568, y: 24, w: 32, h: 28 },
    { code: 'fa', role: 'LNK', title: 'Issues', x: 180, y: 140, w: 96, h: 30 },
    { code: 'fs', role: 'LNK', title: 'Pull Requests', x: 188, y: 152, w: 120, h: 30 },
    { code: 'fd', role: 'BTN', title: 'New Release', x: Math.max(420, width - 220), y: 140, w: 132, h: 34 },
    { code: 'jk', role: 'BTN', title: 'Apply Settings', x: Math.max(320, width * 0.48), y: Math.max(260, height * 0.56), w: 148, h: 36 },
    { code: 'jl', role: 'CHK', title: 'Enable Subgrid', x: Math.max(320, width * 0.48), y: Math.max(310, height * 0.64), w: 164, h: 28 },
  ];

  const labelBadges: LabelBadge[] = rawLabels.map((item, i) => {
    const matchedPrefix =
      typedPrefix.length === 0 || item.code.toLowerCase().startsWith(typedPrefix.toLowerCase());
    return {
      id: `demo-el-${i}`,
      code: item.code,
      role: item.role,
      title: item.title,
      rect: { x: item.x, y: item.y, width: item.w, height: item.h },
      anchor: { x: item.x, y: item.y },
      matchedPrefix,
      exactMatch: typedPrefix.length > 0 && item.code.toLowerCase() === typedPrefix.toLowerCase(),
    };
  });

  return {
    mode,
    visible: mode !== 'idle',
    activeMonitor: {
      id: 'display-1',
      name: 'Primary Display',
      isPrimary: true,
      bounds: { x: 0, y: 0, width, height },
      scaleFactor: window.devicePixelRatio || 1,
    },
    typedPrefix,
    gridRows: rows,
    gridCols: cols,
    gridCells,
    subgrid:
      mode === 'subgrid'
        ? {
            parentCode: targetCell.code,
            parentRect: targetCell.rect,
            rows: subRows,
            cols: subCols,
            cells: subCells,
            nudgeStepPx: config.grid.nudgeStepPx,
            crosshair: {
              x: targetCell.rect.x + targetCell.rect.width / 2,
              y: targetCell.rect.y + targetCell.rect.height / 2,
            },
          }
        : null,
    labelBadges: mode === 'label' ? labelBadges : [],
    freeMode:
      mode === 'free'
        ? {
            cursor: { x: width * 0.52, y: height * 0.48 },
            velocity: { vx: 420, vy: -180 },
            speedProfile: 'fast',
            scrollMode: false,
            dragActive: false,
            activeDirections: ['up', 'right'],
            heldModifiers: [config.freeMode.modFast],
          }
        : null,
    cursorPreview: { x: width * 0.5, y: height * 0.5 },
    activeModifiers: mode === 'free' ? [config.freeMode.modFast] : [],
    frameTimestampMs: performance.now(),
  };
}
