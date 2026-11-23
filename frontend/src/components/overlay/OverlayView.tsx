import { useEffect, useState } from 'react';
import {
  buildDemoOverlayState,
  enforceOverlayClickThrough,
  ipcGetConfig,
  ipcGetOverlayState,
  isTauriRuntime,
  subscribeConfigUpdated,
  subscribeOverlayState,
} from '../../ipc/tauriBridge';
import { useOverlayStore } from '../../store/useOverlayStore';
import type { OverlayMode } from '../../types/ipc';
import { FreeModeHud } from './FreeModeHud';
import { GridCanvas } from './GridCanvas';
import { LabelLayer } from './LabelLayer';

/**
 * Transparent, always-on-top, click-through overlay root view (`?view=overlay`).
 * In production Tauri runtime, listens directly to Rust IPC events (`ratpoison://overlay-state`
 * and `ratpoison://config-updated`). In standalone browser dev mode, mounts a lightweight
 * mode switcher so frontend engineers can inspect Grid, Subgrid, Label, and Free modes live.
 */
export function OverlayView() {
  const setOverlayState = useOverlayStore((s) => s.setOverlayState);
  const setConfig = useOverlayStore((s) => s.setConfig);
  const config = useOverlayStore((s) => s.config);
  const lastRenderMs = useOverlayStore((s) => s.lastRenderMs);

  const inTauri = isTauriRuntime();
  const [demoMode, setDemoMode] = useState<OverlayMode>('grid');
  const [demoPrefix, setDemoPrefix] = useState<string>('');

  useEffect(() => {
    let active = true;
    let unlistenOverlay: (() => void) | null = null;
    let unlistenConfig: (() => void) | null = null;

    async function init() {
      await enforceOverlayClickThrough();

      try {
        const [cfg, initialOverlay] = await Promise.all([
          ipcGetConfig(),
          ipcGetOverlayState(),
        ]);
        if (active) {
          setConfig(cfg);
          if (initialOverlay) setOverlayState(initialOverlay);
        }
      } catch (err) {
        console.warn('[RatPoison] Overlay failed to fetch initial state:', err);
      }

      unlistenOverlay = await subscribeOverlayState((payload) => {
        if (active) setOverlayState(payload);
      });

      unlistenConfig = await subscribeConfigUpdated((nextCfg) => {
        if (active) setConfig(nextCfg);
      });
    }

    void init();

    return () => {
      active = false;
      unlistenOverlay?.();
      unlistenConfig?.();
    };
  }, [setConfig, setOverlayState]);

  // Browser-only demo state synthesizer (strictly disabled inside Tauri runtime)
  useEffect(() => {
    if (inTauri) return;

    const refreshDemo = () => {
      const payload = buildDemoOverlayState(
        demoMode,
        window.innerWidth,
        window.innerHeight,
        config,
        demoPrefix,
      );
      setOverlayState(payload);
    };

    refreshDemo();
    window.addEventListener('resize', refreshDemo);
    return () => window.removeEventListener('resize', refreshDemo);
  }, [inTauri, demoMode, demoPrefix, config, setOverlayState]);

  return (
    <div className="rp-overlay-root" data-reduce-motion={config.appearance.reduceMotion ? 'true' : 'false'}>
      <GridCanvas />
      <LabelLayer />
      <FreeModeHud />

      {/* Browser-dev inspection toolbar (never mounted inside Tauri WebView) */}
      {!inTauri && (
        <div className="rp-overlay-devbar">
          <span className="rp-devbar-title">Overlay Preview</span>
          {(['grid', 'subgrid', 'label', 'free'] as OverlayMode[]).map((mode) => (
            <button
              key={mode}
              type="button"
              className={`rp-devbar-btn${demoMode === mode ? ' is-active' : ''}`}
              onClick={() => {
                setDemoMode(mode);
                setDemoPrefix('');
              }}
            >
              {mode}
            </button>
          ))}
          <input
            type="text"
            className="rp-devbar-input"
            placeholder="Prefix (e.g. a)"
            maxLength={2}
            value={demoPrefix}
            onChange={(e) => setDemoPrefix(e.target.value)}
          />
          <span className="rp-devbar-metric">{lastRenderMs.toFixed(2)}ms</span>
        </div>
      )}
    </div>
  );
}
