import { useState } from 'react';
import { useSettingsStore } from '../../store/useSettingsStore';
import { hexToRgba } from '../overlay/colorUtils';

type PreviewTab = 'grid' | 'label' | 'hud';

/**
 * Interactive real-time Appearance mini-preview rendering miniature Grid Mode
 * (with prefix highlight + Subgrid zoom inset), Label Mode badges, and Free Mode HUD.
 */
export function LiveOverlayPreview() {
  const config = useSettingsStore((s) => s.config);
  const [previewTab, setPreviewTab] = useState<PreviewTab>('grid');
  const [samplePrefix, setSamplePrefix] = useState<string>('f');

  const app = config.appearance;
  const gridCfg = config.grid;

  const miniCols = 5;
  const miniRows = 3;
  const rowChars = ['a', 'f', 'k'];
  const colChars = ['a', 's', 'd', 'f', 'g'];

  return (
    <div className="rp-preview-card">
      <div className="rp-preview-header">
        <div>
          <h3 className="rp-preview-title">Live Overlay Preview</h3>
          <p className="rp-preview-subtitle">Updates instantly as you adjust colors, opacity, or fonts.</p>
        </div>
        <div className="rp-preview-switcher">
          {(['grid', 'label', 'hud'] as PreviewTab[]).map((tab) => (
            <button
              key={tab}
              type="button"
              className={`rp-preview-tab${previewTab === tab ? ' is-active' : ''}`}
              onClick={() => setPreviewTab(tab)}
            >
              {tab === 'grid' ? 'Grid + Subgrid' : tab === 'label' ? 'Label Badges' : 'Free HUD'}
            </button>
          ))}
        </div>
      </div>

      {/* Simulated wallpaper + desktop app chrome */}
      <div className="rp-preview-stage">
        <div className="rp-preview-mock-window">
          <div className="rp-mock-titlebar">
            <span className="rp-mock-dot rp-mock-dot--r" />
            <span className="rp-mock-dot rp-mock-dot--y" />
            <span className="rp-mock-dot rp-mock-dot--g" />
            <span className="rp-mock-url">mouseless-workspace — RatPoison</span>
          </div>
          <div className="rp-mock-body">
            <div className="rp-mock-sidebar">
              <div className="rp-mock-line w-70" />
              <div className="rp-mock-line w-55" />
              <div className="rp-mock-line w-85" />
            </div>
            <div className="rp-mock-main">
              <div className="rp-mock-btn-row">
                <span className="rp-mock-btn">Deploy</span>
                <span className="rp-mock-btn">Inspect</span>
                <span className="rp-mock-btn rp-mock-btn--primary">Apply</span>
              </div>
              <div className="rp-mock-line w-90" />
              <div className="rp-mock-line w-65" />
            </div>
          </div>
        </div>

        {/* Grid + Subgrid Overlay Preview */}
        {previewTab === 'grid' && (
          <div
            className="rp-preview-grid-layer"
            style={{
              backgroundColor: hexToRgba(app.gridBgColor, app.gridBgOpacity),
            }}
          >
            <div
              className="rp-preview-grid-cells"
              style={{
                gridTemplateColumns: `repeat(${miniCols}, 1fr)`,
                gridTemplateRows: `repeat(${miniRows}, 1fr)`,
              }}
            >
              {rowChars.map((rChar) =>
                colChars.map((cChar) => {
                  const code = `${rChar}${cChar}`;
                  const isSubgridDemo = code === 'fd';
                  const matches =
                    !samplePrefix || code.toLowerCase().startsWith(samplePrefix.toLowerCase());
                  const cellOpacity =
                    matches || !gridCfg.dimNonMatchingCells ? 1 : gridCfg.dimOpacity;

                  return (
                    <div
                      key={code}
                      className="rp-preview-grid-cell"
                      style={{
                        borderColor: hexToRgba(app.gridLineColor, 0.42),
                        borderWidth: `${Math.min(2, app.gridLineWidth)}px`,
                        backgroundColor:
                          samplePrefix && matches
                            ? hexToRgba(app.gridAccentColor, 0.14)
                            : 'transparent',
                        opacity: cellOpacity,
                        fontFamily: app.gridFontFamily,
                        fontSize: `${Math.max(10, Math.min(18, app.gridFontSize))}px`,
                        fontWeight: app.gridFontWeight,
                      }}
                    >
                      {isSubgridDemo ? (
                        <div
                          className="rp-preview-subgrid-inset"
                          style={{
                            borderColor: app.subgridAccentColor,
                            backgroundColor: hexToRgba(app.subgridBgColor, app.subgridBgOpacity),
                          }}
                        >
                          {['q', 'w', 'e', 'a', 's', 'd', 'z', 'x', 'c'].map((sc) => (
                            <span
                              key={sc}
                              className="rp-preview-subcell"
                              style={{
                                color: sc === 's' ? app.subgridAccentColor : app.gridTextColor,
                                borderColor: hexToRgba(app.subgridAccentColor, 0.35),
                              }}
                            >
                              {sc}
                            </span>
                          ))}
                          {gridCfg.showSubgridCrosshair && (
                            <span
                              className="rp-preview-crosshair"
                              style={{ borderColor: app.crosshairColor }}
                            />
                          )}
                        </div>
                      ) : samplePrefix && matches ? (
                        <span>
                          <span style={{ color: app.gridAccentColor }}>{code[0]}</span>
                          <span style={{ color: app.gridTextColor }}>{code[1]}</span>
                        </span>
                      ) : (
                        <span style={{ color: app.gridTextColor }}>{code}</span>
                      )}
                    </div>
                  );
                }),
              )}
            </div>
          </div>
        )}

        {/* Label Mode Badges Preview */}
        {previewTab === 'label' && (
          <div className="rp-preview-label-layer">
            {[
              { code: 'fa', role: 'BTN', left: '34%', top: '34%', matched: true },
              { code: 'fs', role: 'BTN', left: '53%', top: '34%', matched: true },
              { code: 'fd', role: 'BTN', left: '74%', top: '34%', matched: true, exact: true },
              { code: 'aa', role: 'LNK', left: '12%', top: '42%', matched: !samplePrefix },
              { code: 'jk', role: 'TXT', left: '42%', top: '66%', matched: !samplePrefix },
            ].map((item) => {
              const showPrefix = Boolean(samplePrefix) && item.code.startsWith(samplePrefix);
              return (
                <div
                  key={item.code}
                  className="rp-preview-label-pill"
                  style={{
                    left: item.left,
                    top: item.top,
                    opacity: item.matched ? 1 : 0.22,
                    backgroundColor: hexToRgba(app.labelBgColor, app.labelBgOpacity),
                    borderColor: item.exact ? app.labelMatchPrefixColor : app.labelBorderColor,
                    color: app.labelTextColor,
                    fontFamily: app.labelFontFamily,
                    fontSize: `${Math.max(10, Math.min(17, app.labelFontSize))}px`,
                    borderRadius: `${app.labelBorderRadius}px`,
                  }}
                >
                  <span style={{ fontWeight: 700 }}>
                    {showPrefix ? (
                      <>
                        <span style={{ color: app.labelMatchPrefixColor }}>{item.code[0]}</span>
                        <span>{item.code.slice(1)}</span>
                      </>
                    ) : (
                      item.code
                    )}
                  </span>
                  {config.labelMode.showElementRoleHint && (
                    <span className="rp-label-role">{item.role}</span>
                  )}
                </div>
              );
            })}
          </div>
        )}

        {/* Free Mode HUD Preview */}
        {previewTab === 'hud' && (
          <div className="rp-preview-hud-layer">
            <div
              className="rp-free-hud"
              style={{
                position: 'relative',
                backgroundColor: hexToRgba(app.hudBgColor, app.hudOpacity),
                color: app.hudTextColor,
                borderColor: hexToRgba(app.hudAccentColor, 0.5),
              }}
            >
              <span
                className="rp-free-hud-mode"
                style={{ backgroundColor: app.hudAccentColor, color: '#090d16' }}
              >
                FREE
              </span>
              <span className="rp-free-hud-speed">FAST</span>
              <span className="rp-free-hud-divider" />
              <div className="rp-free-hud-keys">
                {['J', 'I', 'K', 'L'].map((k) => (
                  <kbd
                    key={k}
                    className="rp-hud-kbd"
                    style={
                      k === 'I'
                        ? { backgroundColor: app.hudAccentColor, color: '#090d16', borderColor: app.hudAccentColor }
                        : undefined
                    }
                  >
                    {k}
                  </kbd>
                ))}
              </div>
              <span className="rp-free-hud-divider" />
              <div className="rp-free-hud-mods">
                {['A', 'S', 'D', 'F'].map((m) => (
                  <span
                    key={m}
                    className={`rp-hud-mod${m === 'F' ? ' rp-hud-mod--active' : ''}`}
                    style={m === 'F' ? { color: app.hudAccentColor, borderColor: app.hudAccentColor } : undefined}
                  >
                    {m}
                  </span>
                ))}
              </div>
            </div>
          </div>
        )}
      </div>

      {previewTab !== 'hud' && (
        <div className="rp-preview-footer">
          <span className="rp-preview-hint">Simulate typed prefix:</span>
          <div className="rp-preview-prefix-pills">
            {['', 'f', 'fd'].map((p) => (
              <button
                key={p || 'none'}
                type="button"
                className={`rp-mini-chip${samplePrefix === p ? ' is-active' : ''}`}
                onClick={() => setSamplePrefix(p)}
              >
                {p === '' ? 'None' : `"${p}"`}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
