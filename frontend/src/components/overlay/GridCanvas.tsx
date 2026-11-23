import { useEffect, useRef } from 'react';
import { useOverlayStore } from '../../store/useOverlayStore';
import type { GridCell, OverlayStatePayload, RatPoisonConfig, SubgridState } from '../../types/ipc';
import { hexToRgba } from './colorUtils';

function drawRoundedRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
  const radius = Math.max(0, Math.min(r, w / 2, h / 2));
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.arcTo(x + w, y, x + w, y + h, radius);
  ctx.arcTo(x + w, y + h, x, y + h, radius);
  ctx.arcTo(x, y + h, x, y, radius);
  ctx.arcTo(x, y, x + w, y, radius);
  ctx.closePath();
}

function collectBoundarySplits(
  cells: GridCell[],
  rows: number,
  cols: number,
  originX: number,
  originY: number,
  totalW: number,
  totalH: number,
): { xSplits: number[]; ySplits: number[] } {
  const xMap = new Map<number, number>();
  const yMap = new Map<number, number>();

  for (let i = 0; i < cells.length; i++) {
    const cell = cells[i];
    if (cell.col > 0 && !xMap.has(cell.col)) {
      xMap.set(cell.col, cell.rect.x);
    }
    if (cell.row > 0 && !yMap.has(cell.row)) {
      yMap.set(cell.row, cell.rect.y);
    }
  }

  const xSplits: number[] = [];
  if (xMap.size > 0) {
    for (let c = 1; c < cols; c++) {
      const x = xMap.get(c);
      if (x !== undefined) xSplits.push(x);
    }
  } else {
    const colW = totalW / Math.max(1, cols);
    for (let c = 1; c < cols; c++) {
      xSplits.push(originX + c * colW);
    }
  }

  const ySplits: number[] = [];
  if (yMap.size > 0) {
    for (let r = 1; r < rows; r++) {
      const y = yMap.get(r);
      if (y !== undefined) ySplits.push(y);
    }
  } else {
    const rowH = totalH / Math.max(1, rows);
    for (let r = 1; r < rows; r++) {
      ySplits.push(originY + r * rowH);
    }
  }

  return { xSplits, ySplits };
}

function drawGridMode(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  overlay: OverlayStatePayload,
  config: RatPoisonConfig,
): void {
  const app = config.appearance;
  const gridCfg = config.grid;
  const cells = overlay.gridCells;
  if (!cells.length) return;

  // 1. Subtle neutral tinted backdrop over active monitor
  ctx.fillStyle = hexToRgba(app.gridBgColor, app.gridBgOpacity);
  ctx.fillRect(0, 0, width, height);

  const prefixLen = overlay.typedPrefix.length;
  const hasPrefix = prefixLen > 0;
  const lineW = Math.max(0.5, app.gridLineWidth);

  const cols = Math.max(1, overlay.gridCols);
  const rows = Math.max(1, overlay.gridRows);
  const colW = width / cols;
  const rowH = height / rows;

  // 2. Crisp device-aligned grid lines derived from actual cell.rect boundaries
  const { xSplits, ySplits } = collectBoundarySplits(cells, rows, cols, 0, 0, width, height);

  ctx.save();
  ctx.strokeStyle = hexToRgba(app.gridLineColor, hasPrefix ? 0.14 : 0.22);
  ctx.lineWidth = lineW;
  ctx.beginPath();

  for (let i = 0; i < xSplits.length; i++) {
    const x = Math.floor(xSplits[i]) + 0.5;
    ctx.moveTo(x, 0);
    ctx.lineTo(x, height);
  }
  for (let i = 0; i < ySplits.length; i++) {
    const y = Math.floor(ySplits[i]) + 0.5;
    ctx.moveTo(0, y);
    ctx.lineTo(width, y);
  }
  ctx.stroke();
  ctx.restore();

  // 3. Render 2-character case-sensitive labels with prefix highlighting & dimming
  const adaptiveFontSize = Math.max(
    9,
    Math.min(app.gridFontSize, Math.floor(Math.min(colW * 0.44, rowH * 0.62))),
  );
  ctx.font = `${app.gridFontWeight} ${adaptiveFontSize}px ${app.gridFontFamily}`;
  ctx.textBaseline = 'middle';

  const dimAlpha = gridCfg.dimNonMatchingCells ? gridCfg.dimOpacity : 1;

  for (let i = 0; i < cells.length; i++) {
    const cell = cells[i];
    const { x, y, width: cw, height: ch } = cell.rect;
    const cx = x + cw * 0.5;
    const cy = y + ch * 0.5;

    if (cell.exactMatch) {
      ctx.fillStyle = hexToRgba(app.gridAccentColor, 0.24);
      ctx.fillRect(x + 1, y + 1, Math.max(0, cw - 2), Math.max(0, ch - 2));
      ctx.strokeStyle = app.gridAccentColor;
      ctx.lineWidth = 1.5;
      ctx.strokeRect(Math.floor(x) + 0.5, Math.floor(y) + 0.5, Math.max(0, cw - 1), Math.max(0, ch - 1));
    } else if (hasPrefix && cell.matchedPrefix) {
      ctx.fillStyle = hexToRgba(app.gridAccentColor, 0.12);
      ctx.fillRect(x + 0.5, y + 0.5, Math.max(0, cw - 1), Math.max(0, ch - 1));
    }

    const alpha = !hasPrefix || cell.matchedPrefix ? 1 : dimAlpha;
    if (alpha <= 0.02) continue;

    ctx.save();
    ctx.globalAlpha = alpha;

    // Subtle dark halo for legibility on both light and dark wallpapers
    ctx.shadowColor = 'rgba(0, 0, 0, 0.68)';
    ctx.shadowBlur = 2.5;
    ctx.shadowOffsetX = 0;
    ctx.shadowOffsetY = 1;

    if (hasPrefix && cell.matchedPrefix && prefixLen < cell.code.length) {
      const matchedPart = cell.code.slice(0, prefixLen);
      const restPart = cell.code.slice(prefixLen);
      const matchedW = ctx.measureText(matchedPart).width;
      const restW = ctx.measureText(restPart).width;
      const startX = cx - (matchedW + restW) / 2;

      ctx.textAlign = 'left';
      ctx.fillStyle = app.gridAccentColor;
      ctx.fillText(matchedPart, startX, cy);
      ctx.fillStyle = app.gridTextColor;
      ctx.fillText(restPart, startX + matchedW, cy);
    } else {
      ctx.textAlign = 'center';
      ctx.fillStyle = cell.exactMatch ? app.gridAccentColor : app.gridTextColor;
      ctx.fillText(cell.code, cx, cy);
    }

    ctx.restore();
  }
}

function drawSubgridTagPill(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  anchorX: number,
  anchorY: number,
  anchorH: number,
  subgrid: SubgridState,
  config: RatPoisonConfig,
): void {
  const app = config.appearance;
  const tagText = `${subgrid.parentCode} · IJKL ${subgrid.nudgeStepPx}px`;

  ctx.save();
  ctx.font = `600 11px ${app.gridFontFamily}`;
  const textW = ctx.measureText(tagText).width;
  const pillH = 22;
  const pillW = textW + 24;
  const pillX = Math.max(8, Math.min(width - pillW - 8, anchorX));
  const pillY =
    anchorY >= pillH + 10
      ? anchorY - pillH - 6
      : Math.min(height - pillH - 8, anchorY + anchorH + 6);

  drawRoundedRect(ctx, pillX, pillY, pillW, pillH, 6);
  ctx.fillStyle = 'rgba(28, 28, 30, 0.90)';
  ctx.fill();
  ctx.strokeStyle = 'rgba(255, 255, 255, 0.14)';
  ctx.lineWidth = 1;
  ctx.stroke();

  // Accent dot
  ctx.beginPath();
  ctx.arc(pillX + 10, pillY + pillH / 2, 3, 0, Math.PI * 2);
  ctx.fillStyle = app.subgridAccentColor;
  ctx.fill();

  ctx.fillStyle = '#F5F5F7';
  ctx.textAlign = 'left';
  ctx.textBaseline = 'middle';
  ctx.fillText(tagText, pillX + 18, pillY + pillH / 2 + 0.5);
  ctx.restore();
}

function drawCrosshairOverlay(
  ctx: CanvasRenderingContext2D,
  viewportW: number,
  viewportH: number,
  boxX: number,
  boxY: number,
  boxW: number,
  boxH: number,
  chX: number,
  chY: number,
  coordXVal: number,
  coordYVal: number,
  config: RatPoisonConfig,
): void {
  const app = config.appearance;
  const alignedX = Math.floor(chX) + 0.5;
  const alignedY = Math.floor(chY) + 0.5;

  ctx.save();
  ctx.strokeStyle = hexToRgba(app.crosshairColor, 0.78);
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(boxX, alignedY);
  ctx.lineTo(boxX + boxW, alignedY);
  ctx.moveTo(alignedX, boxY);
  ctx.lineTo(alignedX, boxY + boxH);
  ctx.stroke();

  // Crisp center reticle ring
  ctx.beginPath();
  ctx.arc(alignedX, alignedY, 4.5, 0, Math.PI * 2);
  ctx.strokeStyle = app.crosshairColor;
  ctx.lineWidth = 1.5;
  ctx.stroke();

  // Compact translucent coordinate pill
  const coordText = `${Math.round(coordXVal)}, ${Math.round(coordYVal)}`;
  ctx.font = `600 10px ${app.gridFontFamily}`;
  const coordW = ctx.measureText(coordText).width + 12;
  const coordH = 18;
  const pillX =
    alignedX + 10 + coordW <= viewportW - 6
      ? alignedX + 10
      : Math.max(6, alignedX - coordW - 10);
  const pillY =
    alignedY + 10 + coordH <= viewportH - 6
      ? alignedY + 10
      : Math.max(6, alignedY - coordH - 10);

  drawRoundedRect(ctx, pillX, pillY, coordW, coordH, 5);
  ctx.fillStyle = 'rgba(28, 28, 30, 0.88)';
  ctx.fill();
  ctx.strokeStyle = 'rgba(255, 255, 255, 0.14)';
  ctx.lineWidth = 1;
  ctx.stroke();

  ctx.fillStyle = app.crosshairColor;
  ctx.textAlign = 'left';
  ctx.textBaseline = 'middle';
  ctx.fillText(coordText, pillX + 6, pillY + coordH / 2 + 0.5);
  ctx.restore();
}

function drawSubgridMode(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  subgrid: SubgridState,
  config: RatPoisonConfig,
): void {
  const app = config.appearance;
  const gridCfg = config.grid;
  const p = subgrid.parentRect;
  const subCols = Math.max(1, subgrid.cols);
  const subRows = Math.max(1, subgrid.rows);

  if (!gridCfg.enableSubgridZoom) {
    // -------------------------------------------------------------------------
    // DEFAULT 1:1 MODE: Direct alignment to exact `cell.rect` over real pixels
    // -------------------------------------------------------------------------

    // Dim everything OUTSIDE `parentRect` cleanly using even-odd cutout so target pixels stay clear
    ctx.save();
    ctx.beginPath();
    ctx.rect(0, 0, width, height);
    ctx.rect(p.x, p.y, p.width, p.height);
    ctx.fillStyle = hexToRgba(app.gridBgColor, Math.min(0.42, app.gridBgOpacity + 0.14));
    ctx.fill('evenodd');
    ctx.restore();

    // Translucent glass fill inside `parentRect` so underlying UI remains legible
    ctx.save();
    ctx.fillStyle = hexToRgba(app.subgridBgColor, Math.min(app.subgridBgOpacity, 0.42));
    ctx.fillRect(p.x, p.y, p.width, p.height);

    // Crisp 1px subdivision lines directly aligned to `subgrid.cells` boundaries
    const { xSplits, ySplits } = collectBoundarySplits(
      subgrid.cells,
      subRows,
      subCols,
      p.x,
      p.y,
      p.width,
      p.height,
    );

    ctx.strokeStyle = hexToRgba(app.gridLineColor, 0.34);
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let i = 0; i < xSplits.length; i++) {
      const x = Math.floor(xSplits[i]) + 0.5;
      ctx.moveTo(x, p.y);
      ctx.lineTo(x, p.y + p.height);
    }
    for (let i = 0; i < ySplits.length; i++) {
      const y = Math.floor(ySplits[i]) + 0.5;
      ctx.moveTo(p.x, y);
      ctx.lineTo(p.x + p.width, y);
    }
    ctx.stroke();

    // Exact-aligned subgrid cell keycaps positioned at each `cell.rect` center
    const sampleCellW = subgrid.cells[0]?.rect.width ?? p.width / subCols;
    const sampleCellH = subgrid.cells[0]?.rect.height ?? p.height / subRows;
    const subFontSize = Math.max(
      9,
      Math.min(app.gridFontSize, Math.floor(Math.min(sampleCellW, sampleCellH) * 0.62)),
    );
    ctx.font = `700 ${subFontSize}px ${app.gridFontFamily}`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';

    for (let i = 0; i < subgrid.cells.length; i++) {
      const cell = subgrid.cells[i];
      const { x, y, width: cw, height: ch } = cell.rect;
      const cx = x + cw * 0.5;
      const cy = y + ch * 0.5;

      if (cell.exactMatch) {
        ctx.fillStyle = hexToRgba(app.subgridAccentColor, 0.26);
        ctx.fillRect(x, y, cw, ch);
      }

      ctx.save();
      ctx.shadowColor = 'rgba(0, 0, 0, 0.85)';
      ctx.shadowBlur = 3;
      ctx.shadowOffsetX = 0;
      ctx.shadowOffsetY = 1;
      ctx.fillStyle = cell.exactMatch ? app.subgridAccentColor : app.gridTextColor;
      ctx.fillText(cell.code, cx, cy);
      ctx.restore();
    }

    // Crisp accent border around true `parentRect`
    ctx.strokeStyle = app.subgridAccentColor;
    ctx.lineWidth = 1.5;
    ctx.strokeRect(
      Math.floor(p.x) + 0.5,
      Math.floor(p.y) + 0.5,
      Math.max(1, Math.round(p.width) - 1),
      Math.max(1, Math.round(p.height) - 1),
    );
    ctx.restore();

    drawSubgridTagPill(ctx, width, height, p.x, p.y, p.height, subgrid, config);

    if (gridCfg.showSubgridCrosshair) {
      const chX = Math.max(p.x, Math.min(p.x + p.width, subgrid.crosshair.x));
      const chY = Math.max(p.y, Math.min(p.y + p.height, subgrid.crosshair.y));
      drawCrosshairOverlay(
        ctx,
        width,
        height,
        p.x,
        p.y,
        p.width,
        p.height,
        chX,
        chY,
        subgrid.crosshair.x,
        subgrid.crosshair.y,
        config,
      );
    }
    return;
  }

  // -------------------------------------------------------------------------
  // OPT-IN MAGNIFIED INSET MODE (`enableSubgridZoom: true`)
  // -------------------------------------------------------------------------
  ctx.fillStyle = hexToRgba(app.gridBgColor, Math.min(0.56, app.gridBgOpacity + 0.2));
  ctx.fillRect(0, 0, width, height);

  // Always draw a crisp 1px outline around the true `parentRect` jump region
  ctx.save();
  ctx.strokeStyle = hexToRgba(app.subgridAccentColor, 0.85);
  ctx.lineWidth = 1;
  ctx.strokeRect(
    Math.floor(p.x) + 0.5,
    Math.floor(p.y) + 0.5,
    Math.max(1, Math.round(p.width) - 1),
    Math.max(1, Math.round(p.height) - 1),
  );
  ctx.restore();

  const scale = Math.max(1, gridCfg.subgridZoomScale);
  const insetW = Math.min(width - 24, Math.max(110, p.width * scale));
  const insetH = Math.min(height - 24, Math.max(90, p.height * scale));
  const centerX = p.x + p.width / 2;
  const centerY = p.y + p.height / 2;
  const insetX = Math.max(12, Math.min(width - insetW - 12, centerX - insetW / 2));
  const insetY = Math.max(12, Math.min(height - insetH - 12, centerY - insetH / 2));

  // Zoomed glass inset card
  ctx.save();
  drawRoundedRect(ctx, insetX, insetY, insetW, insetH, 10);
  ctx.fillStyle = hexToRgba(app.subgridBgColor, Math.max(app.subgridBgOpacity, 0.8));
  ctx.fill();
  ctx.strokeStyle = app.subgridAccentColor;
  ctx.lineWidth = 1.5;
  ctx.stroke();
  ctx.clip();

  const cellW = insetW / subCols;
  const cellH = insetH / subRows;

  ctx.strokeStyle = hexToRgba(app.gridLineColor, 0.24);
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (let c = 1; c < subCols; c++) {
    const x = Math.floor(insetX + c * cellW) + 0.5;
    ctx.moveTo(x, insetY);
    ctx.lineTo(x, insetY + insetH);
  }
  for (let r = 1; r < subRows; r++) {
    const y = Math.floor(insetY + r * cellH) + 0.5;
    ctx.moveTo(insetX, y);
    ctx.lineTo(insetX + insetW, y);
  }
  ctx.stroke();

  const subFontSize = Math.max(
    11,
    Math.min(app.gridFontSize + 3, Math.floor(Math.min(cellW, cellH) * 0.45)),
  );
  ctx.font = `700 ${subFontSize}px ${app.gridFontFamily}`;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';

  for (let i = 0; i < subgrid.cells.length; i++) {
    const cell = subgrid.cells[i];
    const cx = insetX + (cell.col + 0.5) * cellW;
    const cy = insetY + (cell.row + 0.5) * cellH;
    if (cell.exactMatch) {
      ctx.fillStyle = hexToRgba(app.subgridAccentColor, 0.24);
      ctx.fillRect(insetX + cell.col * cellW, insetY + cell.row * cellH, cellW, cellH);
    }
    ctx.fillStyle = cell.exactMatch ? app.subgridAccentColor : app.gridTextColor;
    ctx.fillText(cell.code, cx, cy);
  }
  ctx.restore();

  drawSubgridTagPill(ctx, width, height, insetX, insetY, insetH, subgrid, config);

  if (gridCfg.showSubgridCrosshair) {
    const relX = p.width > 0 ? (subgrid.crosshair.x - p.x) / p.width : 0.5;
    const relY = p.height > 0 ? (subgrid.crosshair.y - p.y) / p.height : 0.5;
    const chX = insetX + Math.max(0, Math.min(1, relX)) * insetW;
    const chY = insetY + Math.max(0, Math.min(1, relY)) * insetH;
    drawCrosshairOverlay(
      ctx,
      width,
      height,
      insetX,
      insetY,
      insetW,
      insetH,
      chX,
      chY,
      subgrid.crosshair.x,
      subgrid.crosshair.y,
      config,
    );
  }
}

/**
 * Hardware-accelerated HiDPI Canvas 2D renderer for Grid Mode & Subgrid Mode.
 * Uses Zustand transient subscription + rAF batching to guarantee <16ms frame latency.
 */
export function GridCanvas() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d', { alpha: true, desynchronized: true });
    if (!ctx) return;

    let rafId = 0;

    const renderFrame = () => {
      rafId = 0;
      const t0 = performance.now();
      const { overlay, config } = useOverlayStore.getState();

      const dpr = window.devicePixelRatio || 1;
      const width = window.innerWidth;
      const height = window.innerHeight;
      const targetW = Math.round(width * dpr);
      const targetH = Math.round(height * dpr);

      if (canvas.width !== targetW || canvas.height !== targetH) {
        canvas.width = targetW;
        canvas.height = targetH;
      }

      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, width, height);

      if (!overlay || !overlay.visible) {
        return;
      }

      if (overlay.mode === 'grid') {
        drawGridMode(ctx, width, height, overlay, config);
      } else if (overlay.mode === 'subgrid' && overlay.subgrid) {
        drawSubgridMode(ctx, width, height, overlay.subgrid, config);
      }

      const elapsed = performance.now() - t0;
      useOverlayStore.getState().setLastRenderMs(elapsed);
    };

    const scheduleRender = () => {
      if (rafId === 0) {
        rafId = window.requestAnimationFrame(renderFrame);
      }
    };

    scheduleRender();

    const unsubscribe = useOverlayStore.subscribe(scheduleRender);
    window.addEventListener('resize', scheduleRender);

    return () => {
      unsubscribe();
      window.removeEventListener('resize', scheduleRender);
      if (rafId !== 0) {
        window.cancelAnimationFrame(rafId);
      }
    };
  }, []);

  return <canvas ref={canvasRef} className="rp-overlay-canvas" aria-hidden="true" />;
}
