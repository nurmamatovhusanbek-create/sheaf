// Pure logic of MOD-UI: formatting, geometry, plural rules.

import test from "node:test";
import assert from "node:assert/strict";
import { bytes, percentChange, ranges, clamp } from "../../../ui/js/format.mjs";
import { displayToPage, pageToDisplay, normalize, dragRect, wordBox, confClass, MIN_SIDE } from "../../../ui/js/geometry.mjs";
import { pluralIndex } from "../../../ui/js/i18n.mjs";

test("bytes", () => {
  assert.equal(bytes(0), "0 B");
  assert.equal(bytes(1536), "1.5 KB");
  assert.equal(bytes(10 * 1024 * 1024), "10 MB");
  assert.equal(bytes(-1), "");
  assert.equal(bytes(Number.NaN), "");
});

test("percentChange is negative when smaller and safe on zero", () => {
  assert.equal(percentChange(1000, 400), -60);
  assert.equal(percentChange(0, 5), 0);
});

test("ranges collapse sorted page numbers", () => {
  assert.equal(ranges([5, 1, 2, 3, 8, 9]), "1-3, 5, 8-9");
  assert.equal(ranges([]), "");
  assert.equal(ranges([2, 2]), "2");
});

test("clamp", () => assert.deepEqual([clamp(5, 0, 3), clamp(-1, 0, 3), clamp(2, 0, 3)], [3, 0, 2]));

const near = (a, b) => Math.abs(a - b) < 1e-9;
const sameRect = (a, b) => ["x0", "y0", "x1", "y1"].every((k) => near(a[k], b[k]));

test("crop rectangles round-trip through every rotation", () => {
  const rect = { x0: 0.1, y0: 0.2, x1: 0.6, y1: 0.9 };
  for (const rot of [0, 90, 180, 270, -90, 360]) {
    assert.ok(sameRect(displayToPage(pageToDisplay(rect, rot), rot), rect), `rot ${rot}`);
  }
});

test("a rotated page maps display corners to the page's own frame", () => {
  // Turned 90° clockwise, the page's top-left corner is shown at the top-right.
  const shown = pageToDisplay({ x0: 0, y0: 0, x1: 0.25, y1: 0.5 }, 90);
  assert.ok(sameRect(shown, { x0: 0.5, y0: 0, x1: 1, y1: 0.25 }), JSON.stringify(shown));
  const page = displayToPage({ x0: 0.5, y0: 0, x1: 1, y1: 0.25 }, 90);
  assert.ok(sameRect(page, { x0: 0, y0: 0, x1: 0.25, y1: 0.5 }), JSON.stringify(page));
});

test("normalize keeps rectangles inside the page and not thinner than the core allows", () => {
  const r = normalize({ x0: 0.9, y0: -1, x1: 0.2, y1: 5 });
  assert.ok(r.x0 >= 0 && r.y0 >= 0 && r.x1 <= 1 && r.y1 <= 1);
  assert.ok(r.x1 - r.x0 >= MIN_SIDE - 1e-12 && r.y1 - r.y0 >= MIN_SIDE - 1e-12);
  const thin = normalize({ x0: 0.5, y0: 0.5, x1: 0.5, y1: 0.5 });
  assert.ok(thin.x1 - thin.x0 >= MIN_SIDE - 1e-12);
});

test("dragging moves, resizes and never leaves the page", () => {
  const rect = { x0: 0.2, y0: 0.2, x1: 0.6, y1: 0.6 };
  const moved = dragRect(rect, "move", 0.9, -0.9);
  assert.ok(near(moved.x1 - moved.x0, 0.4) && near(moved.y1 - moved.y0, 0.4));
  assert.ok(moved.x1 <= 1 && moved.y0 >= 0);
  const grown = dragRect(rect, "se", 0.1, 0.1);
  assert.ok(near(grown.x1, 0.7) && near(grown.y1, 0.7) && near(grown.x0, 0.2));
  const flipped = dragRect(rect, "w", 2, 0); // dragged past the other edge
  assert.ok(flipped.x1 - flipped.x0 >= MIN_SIDE - 1e-12 && flipped.x1 <= 1);
});

test("OCR word boxes become percentages of their image", () => {
  assert.deepEqual(wordBox([100, 50, 200, 25], [1000, 500]), { left: 10, top: 10, width: 20, height: 5 });
  assert.equal(wordBox([1, 2, 3, 4], [0, 0]), null);
  assert.deepEqual([confClass(30), confClass(70), confClass(95)], ["low", "mid", "ok"]);
});

test("plural forms follow each language", () => {
  assert.deepEqual([1, 2, 5].map((n) => pluralIndex("en", n)), [0, 1, 1]);
  assert.deepEqual([1, 2, 5, 11, 21, 22, 25, 111, 112].map((n) => pluralIndex("ru", n)), [0, 1, 2, 2, 0, 1, 2, 2, 2]);
  assert.equal(pluralIndex("uz", 1), 0);
});
