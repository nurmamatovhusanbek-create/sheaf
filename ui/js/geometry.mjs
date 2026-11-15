// Pure geometry for crop and OCR overlays (unit-tested without a browser).
// A rectangle is {x0, y0, x1, y1} in fractions of the page, origin top-left.

import { clamp } from "./format.mjs";

/** Crop rectangles are stored in the page's frame *before* its rotation (DATA-PAGE). */
export function displayToPage(rect, rot) {
  const { x0, y0, x1, y1 } = rect;
  const corner = (x, y) => {
    switch (((rot % 360) + 360) % 360) {
      case 90: return [y, 1 - x];
      case 180: return [1 - x, 1 - y];
      case 270: return [1 - y, x];
      default: return [x, y];
    }
  };
  const [ax, ay] = corner(x0, y0);
  const [bx, by] = corner(x1, y1);
  return { x0: Math.min(ax, bx), y0: Math.min(ay, by), x1: Math.max(ax, bx), y1: Math.max(ay, by) };
}

/** The inverse: where a page-frame rectangle appears once the page is rotated. */
export function pageToDisplay(rect, rot) {
  const { x0, y0, x1, y1 } = rect;
  const corner = (x, y) => {
    switch (((rot % 360) + 360) % 360) {
      case 90: return [1 - y, x];
      case 180: return [1 - x, 1 - y];
      case 270: return [y, 1 - x];
      default: return [x, y];
    }
  };
  const [ax, ay] = corner(x0, y0);
  const [bx, by] = corner(x1, y1);
  return { x0: Math.min(ax, bx), y0: Math.min(ay, by), x1: Math.max(ax, bx), y1: Math.max(ay, by) };
}

export const MIN_SIDE = 0.01; // matches NormRect::MIN_SIDE in sheaf-core

/** Keep a rectangle inside the page and at least MIN_SIDE wide and high. */
export function normalize(rect) {
  let { x0, y0, x1, y1 } = rect;
  [x0, x1] = [Math.min(x0, x1), Math.max(x0, x1)];
  [y0, y1] = [Math.min(y0, y1), Math.max(y0, y1)];
  x0 = clamp(x0, 0, 1 - MIN_SIDE);
  y0 = clamp(y0, 0, 1 - MIN_SIDE);
  x1 = clamp(x1, x0 + MIN_SIDE, 1);
  y1 = clamp(y1, y0 + MIN_SIDE, 1);
  return { x0, y0, x1, y1 };
}

/** Move one handle (`n`, `s`, `e`, `w`, `ne`, …, or `move`) by a fractional delta. */
export function dragRect(rect, handle, dx, dy) {
  let { x0, y0, x1, y1 } = rect;
  if (handle === "move") {
    const w = x1 - x0;
    const h = y1 - y0;
    x0 = clamp(x0 + dx, 0, 1 - w);
    y0 = clamp(y0 + dy, 0, 1 - h);
    return { x0, y0, x1: x0 + w, y1: y0 + h };
  }
  if (handle.includes("w")) x0 += dx;
  if (handle.includes("e")) x1 += dx;
  if (handle.includes("n")) y0 += dy;
  if (handle.includes("s")) y1 += dy;
  return normalize({ x0, y0, x1, y1 });
}

/** Position of an OCR word box inside its image, as percentages for CSS. */
export function wordBox(bbox, imagePx) {
  const [left, top, width, height] = bbox;
  const [w, h] = imagePx;
  if (!w || !h) return null;
  return {
    left: (left / w) * 100,
    top: (top / h) * 100,
    width: (width / w) * 100,
    height: (height / h) * 100,
  };
}

/** Confidence class: `low` under 60, `mid` under 80, otherwise `ok`. */
export function confClass(conf) {
  if (conf < 60) return "low";
  if (conf < 80) return "mid";
  return "ok";
}
