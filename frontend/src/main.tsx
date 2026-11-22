import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { OverlayView } from './components/overlay/OverlayView';
import { SettingsView } from './components/settings/SettingsView';
import { resolveViewKind } from './ipc/tauriBridge';

import './styles/tokens.css';
import './styles/global.css';
import './styles/overlay.css';
import './styles/settings.css';

const viewKind = resolveViewKind();
document.documentElement.dataset.view = viewKind;

const rootEl = document.getElementById('root');
if (!rootEl) {
  throw new Error('RatPoison mount point #root not found');
}

createRoot(rootEl).render(
  <StrictMode>{viewKind === 'overlay' ? <OverlayView /> : <SettingsView />}</StrictMode>,
);
