import { create } from 'zustand';
import {
  DEFAULT_CONFIG,
  OS_KEYBINDING_PRESETS,
  detectClientOs,
  getByPath,
  getDefaultConfigForOs,
  getModifiedPathsForTab,
  getPlatformLabel,
  setByPath,
} from '../config/defaults';
import {
  ipcCheckForUpdates,
  ipcGetConfig,
  ipcGetMonitors,
  ipcGetPermissions,
  ipcGetPlatformInfo,
  ipcOpenAccessibilitySettings,
  ipcOpenExternalUrl,
  ipcRequestPermissions,
  ipcResetSettingKey,
  ipcUpdateConfig,
} from '../ipc/tauriBridge';
import type {
  MonitorInfo,
  PermissionStatus,
  PlatformInfo,
  PlatformOs,
  RatPoisonConfig,
  SettingsTabId,
  UpdateCheckResult,
} from '../types/ipc';

export interface ContextMenuState {
  open: boolean;
  x: number;
  y: number;
  settingPath: string;
  settingLabel: string;
}

export interface SettingsStoreState {
  platform: PlatformInfo;
  permissions: PermissionStatus | null;
  defaultConfig: RatPoisonConfig;
  config: RatPoisonConfig;
  monitors: MonitorInfo[];
  updateResult: UpdateCheckResult | null;
  activeTab: SettingsTabId;
  searchQuery: string;
  loading: boolean;
  busyAction: string | null;
  errorMessage: string | null;
  toastMessage: string | null;
  contextMenu: ContextMenuState;

  loadInitialData: () => Promise<void>;
  applyExternalConfig: (config: RatPoisonConfig) => void;
  setActiveTab: (tab: SettingsTabId) => void;
  setSearchQuery: (query: string) => void;
  updateSetting: (path: string, value: unknown) => Promise<void>;
  resetSettingKey: (path: string) => Promise<void>;
  resetTabToDefaults: (tab: SettingsTabId) => Promise<void>;
  resetAllDefaults: () => Promise<void>;
  applyOsKeybindPreset: (os?: PlatformOs) => Promise<void>;

  openContextMenu: (x: number, y: number, settingPath: string, settingLabel: string) => void;
  closeContextMenu: () => void;

  refreshPermissions: () => Promise<void>;
  requestPermissions: () => Promise<void>;
  openAccessibilitySettings: () => Promise<void>;
  checkForUpdates: (options?: { silent?: boolean }) => Promise<void>;
  openExternalUrl: (url: string) => Promise<void>;
  clearFeedback: () => void;
}

const initialOs = detectClientOs();
const initialPlatform: PlatformInfo = {
  os: initialOs,
  arch: '',
  displayName: getPlatformLabel(initialOs),
  defaultKeybindings: { ...OS_KEYBINDING_PRESETS[initialOs] },
};

let toastTimer: number | null = null;

function scheduleToastClear(set: (partial: Partial<SettingsStoreState>) => void) {
  if (toastTimer !== null) {
    window.clearTimeout(toastTimer);
  }
  toastTimer = window.setTimeout(() => {
    set({ toastMessage: null });
    toastTimer = null;
  }, 2600);
}

export const useSettingsStore = create<SettingsStoreState>((set, get) => ({
  platform: initialPlatform,
  permissions: null,
  defaultConfig: structuredClone(DEFAULT_CONFIG),
  config: structuredClone(DEFAULT_CONFIG),
  monitors: [],
  updateResult: null,
  activeTab: 'general',
  searchQuery: '',
  loading: true,
  busyAction: null,
  errorMessage: null,
  toastMessage: null,
  contextMenu: {
    open: false,
    x: 0,
    y: 0,
    settingPath: '',
    settingLabel: '',
  },

  loadInitialData: async () => {
    set({ loading: true, errorMessage: null });
    try {
      const platform = await ipcGetPlatformInfo();
      const defaultConfig: RatPoisonConfig = {
        ...getDefaultConfigForOs(platform.os),
        keybindings: { ...platform.defaultKeybindings },
      };
      const [config, monitors, permissions] = await Promise.all([
        ipcGetConfig(defaultConfig),
        ipcGetMonitors(),
        ipcGetPermissions(),
      ]);
      set({ platform, permissions, defaultConfig, config, monitors, loading: false });
    } catch (err) {
      set({
        loading: false,
        errorMessage: err instanceof Error ? err.message : 'Failed to load settings from Rust backend.',
      });
    }
  },

  applyExternalConfig: (config) => set({ config }),

  setActiveTab: (tab) => set({ activeTab: tab }),

  setSearchQuery: (query) => set({ searchQuery: query }),

  updateSetting: async (path, value) => {
    const previous = get().config;
    const defaults = get().defaultConfig;
    const optimistic = setByPath(previous, path, value);
    set({ config: optimistic, errorMessage: null });
    try {
      const synced = await ipcUpdateConfig(optimistic, defaults);
      set({ config: synced });
    } catch (err) {
      set({
        config: previous,
        errorMessage: err instanceof Error ? err.message : `Failed to update ${path}`,
      });
    }
  },

  resetSettingKey: async (path) => {
    const previous = get().config;
    const defaults = get().defaultConfig;
    const defaultValue = getByPath(defaults, path);
    const optimistic = setByPath(previous, path, defaultValue);
    set({ config: optimistic, errorMessage: null });
    try {
      const synced = await ipcResetSettingKey(path, defaults);
      set({ config: synced, toastMessage: `Restored default for ${path}` });
      scheduleToastClear(set);
    } catch (err) {
      set({
        config: previous,
        errorMessage: err instanceof Error ? err.message : `Failed to reset ${path}`,
      });
    }
  },

  resetTabToDefaults: async (tab) => {
    const defaults = get().defaultConfig;
    const modified = getModifiedPathsForTab(get().config, tab, defaults);
    if (modified.length === 0) return;

    let nextConfig = get().config;
    for (const path of modified) {
      nextConfig = setByPath(nextConfig, path, getByPath(defaults, path));
    }
    set({ config: nextConfig, errorMessage: null });
    try {
      const synced = await ipcUpdateConfig(nextConfig, defaults);
      set({
        config: synced,
        toastMessage: `Restored ${modified.length} setting${modified.length === 1 ? '' : 's'} to default`,
      });
      scheduleToastClear(set);
    } catch (err) {
      set({
        errorMessage: err instanceof Error ? err.message : 'Failed to reset tab defaults',
      });
    }
  },

  resetAllDefaults: async () => {
    const defaults = get().defaultConfig;
    const fresh = structuredClone(defaults);
    set({ config: fresh, errorMessage: null });
    try {
      const synced = await ipcUpdateConfig(fresh, defaults);
      set({ config: synced, toastMessage: 'All settings restored to native defaults' });
      scheduleToastClear(set);
    } catch (err) {
      set({
        errorMessage: err instanceof Error ? err.message : 'Failed to restore defaults',
      });
    }
  },

  applyOsKeybindPreset: async (os) => {
    const { platform, config, defaultConfig } = get();
    const targetOs = os ?? platform.os;
    const preset =
      targetOs === platform.os
        ? { ...platform.defaultKeybindings }
        : { ...OS_KEYBINDING_PRESETS[targetOs] };
    const nextConfig: RatPoisonConfig = {
      ...structuredClone(config),
      keybindings: preset,
    };
    set({ config: nextConfig, errorMessage: null });
    try {
      const synced = await ipcUpdateConfig(nextConfig, defaultConfig);
      set({
        config: synced,
        toastMessage: `Applied ${getPlatformLabel(targetOs)} keybindings`,
      });
      scheduleToastClear(set);
    } catch (err) {
      set({
        config,
        errorMessage: err instanceof Error ? err.message : 'Failed to apply OS keybinding preset',
      });
    }
  },

  openContextMenu: (x, y, settingPath, settingLabel) =>
    set({
      contextMenu: { open: true, x, y, settingPath, settingLabel },
    }),

  closeContextMenu: () =>
    set((state) => ({
      contextMenu: { ...state.contextMenu, open: false },
    })),

  checkForUpdates: async (options) => {
    const silent = options?.silent ?? false;
    if (!silent) {
      set({ busyAction: 'updates', errorMessage: null });
    }
    try {
      const updateResult = await ipcCheckForUpdates(get().config.updates.channel);
      if (silent) {
        set({
          updateResult,
          ...(updateResult.updateAvailable
            ? { toastMessage: `Update v${updateResult.latestVersion} available on GitHub` }
            : {}),
        });
        if (updateResult.updateAvailable) {
          scheduleToastClear(set);
        }
      } else {
        set({
          updateResult,
          busyAction: null,
          toastMessage: updateResult.updateAvailable
            ? `Update v${updateResult.latestVersion} available`
            : 'RatPoison is up to date',
        });
        scheduleToastClear(set);
      }
    } catch (err) {
      if (!silent) {
        set({
          busyAction: null,
          errorMessage: err instanceof Error ? err.message : 'Update check failed',
        });
      }
    }
  },

  openExternalUrl: async (url) => {
    try {
      await ipcOpenExternalUrl(url);
    } catch (err) {
      set({
        errorMessage: err instanceof Error ? err.message : 'Failed to open external link',
      });
    }
  },

  refreshPermissions: async () => {
    try {
      const permissions = await ipcGetPermissions();
      set({ permissions });
    } catch {
      // Ignore background permission poll failures
    }
  },

  requestPermissions: async () => {
    try {
      const permissions = await ipcRequestPermissions();
      set({
        permissions,
        toastMessage: permissions.hookActive
          ? 'Global hotkey hook is active'
          : 'Enable RatPoison in Privacy & Security → Accessibility',
      });
      scheduleToastClear(set);
    } catch (err) {
      set({
        errorMessage: err instanceof Error ? err.message : 'Failed to request permissions',
      });
    }
  },

  openAccessibilitySettings: async () => {
    try {
      await ipcOpenAccessibilitySettings();
    } catch (err) {
      set({
        errorMessage: err instanceof Error ? err.message : 'Failed to open Accessibility settings',
      });
    }
  },

  clearFeedback: () => set({ errorMessage: null, toastMessage: null }),
}));
