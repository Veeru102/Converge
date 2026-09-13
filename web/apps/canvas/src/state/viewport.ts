export interface Viewport { x: number; y: number; scale: number }

export const MIN_SCALE = 0.1;
export const MAX_SCALE = 8;

export const toWorld = (v: Viewport, sx: number, sy: number) => ({ x: (sx - v.x) / v.scale, y: (sy - v.y) / v.scale });
export const toScreen = (v: Viewport, wx: number, wy: number) => ({ x: wx * v.scale + v.x, y: wy * v.scale + v.y });

/** Zoom by `factor` keeping the screen point (sx, sy) fixed. */
export function zoomAt(v: Viewport, factor: number, sx: number, sy: number): Viewport {
  const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, v.scale * factor));
  const k = scale / v.scale;
  return { scale, x: sx - (sx - v.x) * k, y: sy - (sy - v.y) * k };
}

export function fitTo(box: { x: number; y: number; w: number; h: number } | null, width: number, height: number): Viewport {
  if (!box || box.w === 0 || box.h === 0) return { x: 0, y: 0, scale: 1 };
  const pad = 80;
  const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, Math.min((width - pad * 2) / box.w, (height - pad * 2) / box.h, 2)));
  return { scale, x: width / 2 - (box.x + box.w / 2) * scale, y: height / 2 - (box.y + box.h / 2) * scale };
}
