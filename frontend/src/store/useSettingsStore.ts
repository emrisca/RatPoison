import { create } from 'zustand';
import { DEFAULT_CONFIG, getByPath, getModifiedPathsForTab, setByPath } from '../config/defaults';
import {
  ipcActivateLicense,
  ipcApplyOfflineResponse,
  ipcCheckForUpdates,
  ipcDeactivateMachine,
  ipcGenerateOfflineChallenge,
  ipcGetConfig,
  ipcGetLicenseStatus,
  ipcGetMonitors,
  ipcResetSettingKey,
  ipcUpdateConfig,
} from '../ipc/tauriBridge';
import type {
  LicenseStatus,
  MonitorInfo,
  OfflineChallenge,
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
  config: RatPoisonConfig;
  monitors: MonitorInfo[];
  licenseStatus: LicenseStatus | null;
  offlineChallenge: OfflineChallenge | null;
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

  openContextMenu: (x: number, y: number, settingPath: string, settingLabel: string) => void;
  closeContextMenu: () => void;

  activateLicense: (licenseKey: string) => Promise<void>;
  deactivateMachine: () => Promise<void>;
  generateOfflineChallenge: () => Promise<void>;
  applyOfflineResponse: (responseBlob: string) => Promise<void>;
  checkForUpdates: () => Promise<void>;
  clearFeedback: () => void;
}

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
  config: structuredClone(DEFAULT_CONFIG),
  monitors: [],
  licenseStatus: null,
  offlineChallenge: null,
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
      const [config, monitors, licenseStatus] = await Promise.all([
        ipcGetConfig(),
        ipcGetMonitors(),
        ipcGetLicenseStatus(),
      ]);
      set({ config, monitors, licenseStatus, loading: false });
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
    const optimistic = setByPath(previous, path, value);
    set({ config: optimistic, errorMessage: null });
    try {
      const synced = await ipcUpdateConfig(optimistic);
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
    const defaultValue = getByPath(DEFAULT_CONFIG, path);
    const optimistic = setByPath(previous, path, defaultValue);
    set({ config: optimistic, errorMessage: null });
    try {
      const synced = await ipcResetSettingKey(path);
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
    const modified = getModifiedPathsForTab(get().config, tab);
    if (modified.length === 0) return;

    let nextConfig = get().config;
    for (const path of modified) {
      nextConfig = setByPath(nextConfig, path, getByPath(DEFAULT_CONFIG, path));
    }
    set({ config: nextConfig, errorMessage: null });
    try {
      const synced = await ipcUpdateConfig(nextConfig);
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
    const fresh = structuredClone(DEFAULT_CONFIG);
    set({ config: fresh, errorMessage: null });
    try {
      const synced = await ipcUpdateConfig(fresh);
      set({ config: synced, toastMessage: 'All settings restored to factory defaults' });
      scheduleToastClear(set);
    } catch (err) {
      set({
        errorMessage: err instanceof Error ? err.message : 'Failed to restore factory defaults',
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

  activateLicense: async (licenseKey) => {
    set({ busyAction: 'activate', errorMessage: null });
    try {
      const licenseStatus = await ipcActivateLicense(licenseKey);
      set({ licenseStatus, busyAction: null, toastMessage: 'License activated successfully' });
      scheduleToastClear(set);
    } catch (err) {
      set({
        busyAction: null,
        errorMessage: err instanceof Error ? err.message : 'License activation failed',
      });
    }
  },

  deactivateMachine: async () => {
    set({ busyAction: 'deactivate', errorMessage: null });
    try {
      const licenseStatus = await ipcDeactivateMachine();
      set({ licenseStatus, busyAction: null, toastMessage: 'Machine seat released' });
      scheduleToastClear(set);
    } catch (err) {
      set({
        busyAction: null,
        errorMessage: err instanceof Error ? err.message : 'Deactivation failed',
      });
    }
  },

  generateOfflineChallenge: async () => {
    set({ busyAction: 'challenge', errorMessage: null });
    try {
      const offlineChallenge = await ipcGenerateOfflineChallenge();
      set({ offlineChallenge, busyAction: null, toastMessage: 'Offline challenge generated' });
      scheduleToastClear(set);
    } catch (err) {
      set({
        busyAction: null,
        errorMessage: err instanceof Error ? err.message : 'Failed to generate offline challenge',
      });
    }
  },

  applyOfflineResponse: async (responseBlob) => {
    set({ busyAction: 'offline-apply', errorMessage: null });
    try {
      const licenseStatus = await ipcApplyOfflineResponse(responseBlob);
      set({
        licenseStatus,
        busyAction: null,
        toastMessage: 'Offline signed license verified and activated',
      });
      scheduleToastClear(set);
    } catch (err) {
      set({
        busyAction: null,
        errorMessage: err instanceof Error ? err.message : 'Offline verification failed',
      });
    }
  },

  checkForUpdates: async () => {
    set({ busyAction: 'updates', errorMessage: null });
    try {
      const updateResult = await ipcCheckForUpdates(get().config.updates.channel);
      set({
        updateResult,
        busyAction: null,
        toastMessage: updateResult.updateAvailable
          ? `Update v${updateResult.latestVersion} available`
          : 'RatPoison is up to date',
      });
      scheduleToastClear(set);
    } catch (err) {
      set({
        busyAction: null,
        errorMessage: err instanceof Error ? err.message : 'Update check failed',
      });
    }
  },

  clearFeedback: () => set({ errorMessage: null, toastMessage: null }),
}));
