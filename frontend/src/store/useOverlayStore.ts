import { create } from 'zustand';
import { DEFAULT_CONFIG } from '../config/defaults';
import type { OverlayStatePayload, RatPoisonConfig } from '../types/ipc';

export interface OverlayStoreState {
  overlay: OverlayStatePayload | null;
  config: RatPoisonConfig;
  /** Monotonic revision incremented on every IPC state frame for rAF canvas dirty checks. */
  frameRevision: number;
  lastRenderMs: number;
  setOverlayState: (payload: OverlayStatePayload) => void;
  setConfig: (config: RatPoisonConfig) => void;
  setLastRenderMs: (ms: number) => void;
  clearOverlay: () => void;
}

export const useOverlayStore = create<OverlayStoreState>((set) => ({
  overlay: null,
  config: structuredClone(DEFAULT_CONFIG),
  frameRevision: 0,
  lastRenderMs: 0,
  setOverlayState: (payload) =>
    set((state) => ({
      overlay: payload,
      frameRevision: state.frameRevision + 1,
    })),
  setConfig: (config) =>
    set((state) => ({
      config,
      frameRevision: state.frameRevision + 1,
    })),
  setLastRenderMs: (ms) => set({ lastRenderMs: ms }),
  clearOverlay: () =>
    set((state) => ({
      overlay: null,
      frameRevision: state.frameRevision + 1,
    })),
}));
