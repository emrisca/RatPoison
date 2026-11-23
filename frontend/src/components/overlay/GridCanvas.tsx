import { useEffect, useRef } from 'react';
import { useOverlayStore } from '../../store/useOverlayStore';
import type { OverlayStatePayload, RatPoisonConfig, SubgridState } from '../../types/ipc';
import { hexToRgba } from './colorUtils';

function drawRoundedRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
  const radius = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.arcTo(x + w, y, x + w, y + h, radius);
  ctx.arcTo(x + w, y + h, x, y + h, radius);
  ctx.arcTo(x, y + h, x, y, radius);
  ctx.arcTo(x, y, x + w, y, radius);
  ctx.closePath();
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

  // 1. Semi-transparent backdrop over active monitor
  ctx.fillStyle = hexToRgba(app.gridBgColor, app.gridBgOpacity);
  ctx.fillRect(0, 0, width, height);

  const prefixLen = overlay.typedPrefix.length;
  const hasPrefix = prefixLen > 0;
  const lineW = Math.max(0.5, app.gridLineWidth);

  // 2. Crisp device-aligned grid lines (batched into a single Canvas path for <1ms draw)
  ctx.save();
  ctx.strokeStyle = hexToRgba(app.gridLineColor, hasPrefix ? 0.24 : 0.42);
  ctx.lineWidth = lineW;
  ctx.beginPath();

  const cols = Math.max(1, overlay.gridCols);
  const rows = Math.max(1, overlay.gridRows);
  const colW = width / cols;
  const rowH = height / rows;

  for (let c = 1; c < cols; c++) {
    const x = Math.floor(c * colW) + 0.5;
    ctx.moveTo(x, 0);
    ctx.lineTo(x, height);
  }
  for (let r = 1; r < rows; r++) {
    const y = Math.floor(r * rowH) + 0.5;
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

    // Highlight active matching cell or exact match
    if (cell.exactMatch) {
      ctx.fillStyle = hexToRgba(app.gridAccentColor, 0.28);
      ctx.fillRect(x + 1, y + 1, cw - 2, ch - 2);
      ctx.strokeStyle = app.gridAccentColor;
      ctx.lineWidth = 2;
      ctx.strokeRect(x + 1, y + 1, cw - 2, ch - 2);
    } else if (hasPrefix && cell.matchedPrefix) {
      ctx.fillStyle = hexToRgba(app.gridAccentColor, 0.11);
      ctx.fillRect(x + 0.5, y + 0.5, cw - 1, ch - 1);
    }

    const alpha = !hasPrefix || cell.matchedPrefix ? 1 : dimAlpha;
    if (alpha <= 0.02) continue;

    ctx.save();
    ctx.globalAlpha = alpha;

    // Subtle dark halo behind text for guaranteed legibility on bright wallpapers
    ctx.shadowColor = 'rgba(0, 0, 0, 0.72)';
    ctx.shadowBlur = 3;
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

function drawSubgridZoom(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  subgrid: SubgridState,
  config: RatPoisonConfig,
): void {
  const app = config.appearance;
  const gridCfg = config.grid;
  const p = subgrid.parentRect;

  // Dim outside area softly so focus snaps to selected primary cell
  ctx.fillStyle = hexToRgba(app.gridBgColor, Math.min(0.68, app.gridBgOpacity + 0.16));
  ctx.fillRect(0, 0, width, height);

  // Compute zoomed inset bounds centered on parent cell, clamped to monitor viewport
  const scale = gridCfg.enableSubgridZoom ? Math.max(1, gridCfg.subgridZoomScale) : 1;
  const insetW = Math.min(width - 24, Math.max(110, p.width * scale));
  const insetH = Math.min(height - 24, Math.max(90, p.height * scale));
  const centerX = p.x + p.width / 2;
  const centerY = p.y + p.height / 2;
  const insetX = Math.max(12, Math.min(width - insetW - 12, centerX - insetW / 2));
  const insetY = Math.max(12, Math.min(height - insetH - 12, centerY - insetH / 2));

  // Outline true physical target cell footprint
  ctx.save();
  ctx.strokeStyle = hexToRgba(app.subgridAccentColor, 0.5);
  ctx.setLineDash([4, 3]);
  ctx.lineWidth = 1;
  ctx.strokeRect(Math.floor(p.x) + 0.5, Math.floor(p.y) + 0.5, p.width, p.height);
  ctx.restore();

  // Zoomed inset card
  ctx.save();
  drawRoundedRect(ctx, insetX, insetY, insetW, insetH, 8);
  ctx.fillStyle = hexToRgba(app.subgridBgColor, app.subgridBgOpacity);
  ctx.fill();
  ctx.strokeStyle = app.subgridAccentColor;
  ctx.lineWidth = 2;
  ctx.stroke();
  ctx.clip();

  // Subgrid internal lines
  const subCols = Math.max(1, subgrid.cols);
  const subRows = Math.max(1, subgrid.rows);
  const cellW = insetW / subCols;
  const cellH = insetH / subRows;

  ctx.strokeStyle = hexToRgba(app.subgridAccentColor, 0.4);
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

  // Subgrid cell labels
  const subFontSize = Math.max(11, Math.min(app.gridFontSize + 3, Math.floor(Math.min(cellW, cellH) * 0.45)));
  ctx.font = `700 ${subFontSize}px ${app.gridFontFamily}`;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';

  for (const cell of subgrid.cells) {
    const cx = insetX + (cell.col + 0.5) * cellW;
    const cy = insetY + (cell.row + 0.5) * cellH;
    if (cell.exactMatch) {
      ctx.fillStyle = hexToRgba(app.subgridAccentColor, 0.25);
      ctx.fillRect(insetX + cell.col * cellW, insetY + cell.row * cellH, cellW, cellH);
    }
    ctx.fillStyle = cell.exactMatch ? app.subgridAccentColor : app.gridTextColor;
    ctx.fillText(cell.code, cx, cy);
  }

  ctx.restore();

  // Header badge showing parent cell code & nudge step
  const tagText = `${subgrid.parentCode} · nudge ${subgrid.nudgeStepPx}px`;
  ctx.save();
  ctx.font = `600 11px ${app.gridFontFamily}`;
  const tagW = ctx.measureText(tagText).width + 16;
  const tagX = insetX;
  const tagY = Math.max(6, insetY - 24);
  drawRoundedRect(ctx, tagX, tagY, tagW, 20, 4);
  ctx.fillStyle = app.subgridAccentColor;
  ctx.fill();
  ctx.fillStyle = '#0a0a0a';
  ctx.textAlign = 'left';
  ctx.textBaseline = 'middle';
  ctx.fillText(tagText, tagX + 8, tagY + 10);
  ctx.restore();

  // Precision Nudge Crosshair
  if (gridCfg.showSubgridCrosshair) {
    const relX = p.width > 0 ? (subgrid.crosshair.x - p.x) / p.width : 0.5;
    const relY = p.height > 0 ? (subgrid.crosshair.y - p.y) / p.height : 0.5;
    const chX = Math.floor(insetX + Math.max(0, Math.min(1, relX)) * insetW) + 0.5;
    const chY = Math.floor(insetY + Math.max(0, Math.min(1, relY)) * insetH) + 0.5;

    ctx.save();
    ctx.strokeStyle = hexToRgba(app.crosshairColor, 0.9);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(insetX, chY);
    ctx.lineTo(insetX + insetW, chY);
    ctx.moveTo(chX, insetY);
    ctx.lineTo(chX, insetY + insetH);
    ctx.stroke();

    // Center reticle ring
    ctx.beginPath();
    ctx.arc(chX, chY, 5, 0, Math.PI * 2);
    ctx.strokeStyle = app.crosshairColor;
    ctx.lineWidth = 1.75;
    ctx.stroke();

    // Coordinate pill
    const coordText = `${Math.round(subgrid.crosshair.x)}, ${Math.round(subgrid.crosshair.y)}`;
    ctx.font = `600 10px ${app.gridFontFamily}`;
    const coordW = ctx.measureText(coordText).width + 10;
    const coordX = Math.min(width - coordW - 6, chX + 10);
    const coordY = Math.min(height - 20, chY + 10);
    drawRoundedRect(ctx, coordX, coordY, coordW, 17, 3);
    ctx.fillStyle = hexToRgba('#0a0a0a', 0.85);
    ctx.fill();
    ctx.fillStyle = app.crosshairColor;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'middle';
    ctx.fillText(coordText, coordX + 5, coordY + 8.5);
    ctx.restore();
  }
}

/**
 * Hardware-accelerated HiDPI Canvas 2D renderer for Grid Mode & Subgrid Zoom.
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
        drawSubgridZoom(ctx, width, height, overlay.subgrid, config);
      }

      const elapsed = performance.now() - t0;
      useOverlayStore.getState().setLastRenderMs(elapsed);
    };

    const scheduleRender = () => {
      if (rafId === 0) {
        rafId = window.requestAnimationFrame(renderFrame);
      }
    };

    // Initial draw
    scheduleRender();

    // Transient store subscription avoids React re-render overhead on rapid keystrokes
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
