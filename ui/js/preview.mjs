// UI-PREVIEW: the current page at a chosen zoom, with rotate, filters, crop
// handles and the recognized-text overlay. What is drawn is rendered by a
// worker (CMD-RENDER); the page is never parsed here.

import { h, icon, clear } from "./dom.mjs";
import { t, onLanguage } from "./i18n.mjs";
import { api, renderUrl } from "./ipc.mjs";
import { bus, pages, selection } from "./state.mjs";
import { clamp } from "./format.mjs";
import { displayToPage, dragRect, normalize, wordBox, confClass } from "./geometry.mjs";
import * as act from "./actions.mjs";
import { showError } from "./dialogs.mjs";

const ZOOM_STEPS = [0.25, 0.5, 0.75, 1, 1.5, 2, 3, 4];
const MAX_RENDER_PX = 4096;
const DEFAULT_CROP = { x0: 0.05, y0: 0.05, x1: 0.95, y1: 0.95 };

export function mountPreview(root) {
  let pageId = null;
  let zoom = 1;
  let cropRect = null; // display frame, fractions
  let showText = false;
  let words = null; // {words, imagePx} of the current page
  let doubtful = -1;
  let loadToken = 0;
  let frameCss = { w: 0, h: 0 };
  let shownKey = null;

  const btn = (name, label, onClick, extra = {}) =>
    h("button", { class: "btn small icon-only", type: "button", title: label, "aria-label": label, onClick, ...extra }, icon(name));

  const prev = btn("prev", t("preview.prev"), () => step(-1));
  const next = btn("next", t("preview.next"), () => step(1));
  const pageLabel = h("span", { class: "pg", "aria-live": "polite" });
  const ccw = btn("ccw", t("menu.rotate_ccw"), () => act.rotate(-90, pageId ? [pageId] : undefined));
  const cw = btn("cw", t("menu.rotate_cw"), () => act.rotate(90, pageId ? [pageId] : undefined));
  const zOut = btn("zoomout", t("preview.zoom_out"), () => stepZoom(-1));
  const zLabel = h("span", { class: "pg", style: { minWidth: "48px" } });
  const zIn = btn("zoomin", t("preview.zoom_in"), () => stepZoom(1));
  const zFit = btn("fit", t("preview.fit"), () => setZoom(1));
  const modeButtons = ["Color", "Gray", "BW"].map((mode) =>
    h("button", { type: "button", "data-mode": mode, onClick: () => act.setFilter({ mode }, targets()) }, t(`filter.${mode}`)),
  );
  const modes = h("div", { class: "seg", role: "group", "aria-label": t("preview.filter") }, modeButtons);
  const deskew = h("button", { class: "btn small outline", type: "button", "aria-pressed": "false", onClick: () => toggleFilter("deskew") }, t("filter.deskew"));
  const clean = h("button", { class: "btn small outline", type: "button", "aria-pressed": "false", onClick: () => toggleFilter("despeckle") }, t("filter.despeckle"));
  const cropBtn = h("button", { class: "btn small outline", type: "button", "aria-pressed": "false", onClick: () => (cropRect ? endCrop() : startCrop()) }, icon("crop"), h("span", { text: t("preview.crop") }));
  const textBtn = h("button", { class: "btn small outline", type: "button", "aria-pressed": "false", onClick: toggleText }, icon("text"), h("span", { text: t("preview.text") }));
  const bar = h(
    "div",
    { class: "preview-bar" },
    prev, pageLabel, next, h("span", { class: "sep" }), ccw, cw, h("span", { class: "sep" }), zOut, zLabel, zIn, zFit,
    h("span", { class: "sep" }), modes, deskew, clean, h("span", { class: "grow" }), cropBtn, textBtn,
  );

  const frame = h("div", { class: "page-frame loading", hidden: true });
  const stage = h("div", { class: "preview-stage" }, frame);
  const emptyMsg = h("div", { class: "preview-empty" });
  root.append(bar, stage, emptyMsg);

  let img = null;

  const targets = () => {
    // The filter buttons act on the whole selection, like the tray does.
    const ids = selection.targets();
    return ids.length ? ids : pageId ? [pageId] : [];
  };

  function currentPage() {
    return pageId === null ? undefined : pages.byId(pageId);
  }

  function toggleFilter(which) {
    const page = currentPage();
    if (!page) return;
    if (which === "deskew") act.setFilter({ deskew: page.filter.deskew === "Off" ? "Auto" : "Off" }, targets());
    else act.setFilter({ despeckle: !page.filter.despeckle }, targets());
  }

  async function step(delta) {
    const at = pages.indexOfLoaded(pageId);
    if (at < 0) return;
    const to = clamp(at + delta, 0, pages.total - 1);
    await act.selectIndex(to);
    bus.emit("tray-scroll", to);
  }

  function setZoom(z) {
    zoom = clamp(z, ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.length - 1]);
    stage.classList.toggle("zoomed", zoom > 1);
    layout();
    ensureResolution();
  }

  function stepZoom(dir) {
    const at = ZOOM_STEPS.findIndex((z) => z >= zoom - 1e-6);
    const i = clamp((at < 0 ? ZOOM_STEPS.length - 1 : at) + dir, 0, ZOOM_STEPS.length - 1);
    setZoom(ZOOM_STEPS[i]);
  }

  // ---- layout and loading ---------------------------------------------------

  function available() {
    const box = stage.getBoundingClientRect();
    return { w: Math.max(80, box.width - 32), h: Math.max(80, box.height - 32) };
  }

  /** Size the frame to the picture's aspect at the current zoom. */
  function layout() {
    zLabel.textContent = `${Math.round(zoom * 100)}%`;
    if (!img || !img.naturalWidth) return;
    const aspect = img.naturalWidth / img.naturalHeight;
    const a = available();
    const fitW = aspect >= a.w / a.h ? a.w : a.h * aspect;
    const w = fitW * zoom;
    frameCss = { w, h: w / aspect };
    frame.style.width = `${frameCss.w}px`;
    frame.style.height = `${frameCss.h}px`;
    drawCrop();
  }

  /** Ask for a sharper picture when the frame outgrew the one we have. */
  function ensureResolution() {
    if (!img || !pageId) return;
    const dpr = window.devicePixelRatio || 1;
    const needed = Math.min(MAX_RENDER_PX, Math.ceil(Math.max(frameCss.w, frameCss.h) * dpr));
    const have = Math.max(img.naturalWidth, img.naturalHeight);
    if (needed > have * 1.1) load(pageId, needed);
  }

  async function load(id, px) {
    const token = ++loadToken;
    const dpr = window.devicePixelRatio || 1;
    const a = available();
    const want = px ?? Math.min(MAX_RENDER_PX, Math.ceil(Math.max(a.w, a.h) * dpr * zoom));
    try {
      const key = await api.render(id, clamp(want / 1000, 0.05, 4.096));
      if (token !== loadToken) return;
      if (key === shownKey) return;
      const next = new Image();
      next.alt = t("preview.page_alt");
      next.onload = () => {
        if (token !== loadToken) return;
        shownKey = key;
        img = next;
        clear(frame).append(img);
        frame.classList.remove("loading");
        layout();
        redrawOverlays();
      };
      next.onerror = () => {
        if (token === loadToken) frame.classList.remove("loading");
      };
      next.src = renderUrl(key);
    } catch (e) {
      if (token === loadToken) {
        frame.classList.remove("loading");
        showError(e);
      }
    }
  }

  async function show() {
    const id = selection.currentId();
    const total = pages.total;
    if (id === null || !total) {
      pageId = null;
      shownKey = null;
      img = null;
      frame.hidden = true;
      stage.hidden = true;
      bar.style.visibility = "hidden";
      emptyMsg.hidden = false;
      emptyMsg.textContent = total ? t("preview.none_selected") : t("preview.empty");
      return;
    }
    const changed = id !== pageId;
    pageId = id;
    bar.style.visibility = "visible";
    emptyMsg.hidden = true;
    stage.hidden = false;
    frame.hidden = false;
    if (changed) {
      endCrop();
      words = null;
      doubtful = -1;
      frame.classList.add("loading");
      shownKey = null;
    }
    const at = pages.indexOfLoaded(id);
    pageLabel.textContent = at >= 0 ? t("preview.page_of", { n: at + 1, total }) : "";
    prev.disabled = at <= 0;
    next.disabled = at < 0 || at >= total - 1;
    syncControls();
    await load(id);
    if (showText) loadWords();
  }

  function syncControls() {
    const page = currentPage();
    const f = page?.filter;
    for (const b of modeButtons) b.setAttribute("aria-pressed", String(f?.mode === b.dataset.mode));
    deskew.setAttribute("aria-pressed", String(!!f && f.deskew !== "Off"));
    clean.setAttribute("aria-pressed", String(!!f?.despeckle));
    cropBtn.setAttribute("aria-pressed", String(!!cropRect));
    textBtn.setAttribute("aria-pressed", String(showText));
    const noPage = !page;
    for (const el of [ccw, cw, ...modeButtons, deskew, clean, cropBtn, textBtn]) el.disabled = noPage;
    textBtn.disabled = noPage || page.ocr.state !== "done";
    textBtn.title = textBtn.disabled && !noPage ? t("preview.text_needs_ocr") : t("preview.text_title");
    // A page that is already cropped is edited by resetting first, which keeps one rule for what "crop" means.
    cropBtn.querySelector("span").textContent = page?.crop && !cropRect ? t("preview.crop_reset") : t("preview.crop");
  }

  // ---- text overlay ---------------------------------------------------------

  async function toggleText() {
    showText = !showText;
    textBtn.setAttribute("aria-pressed", String(showText));
    if (showText) await loadWords();
    redrawOverlays();
  }

  async function loadWords() {
    const page = currentPage();
    if (!page || page.ocr.state !== "done") {
      words = null;
      redrawOverlays();
      return;
    }
    const id = page.id;
    try {
      const result = await api.ocrResult(id);
      if (id !== pageId) return;
      words = result;
      bus.emit("ocr-words", { id, result });
    } catch {
      words = null;
    }
    redrawOverlays();
  }

  function redrawOverlays() {
    for (const el of frame.querySelectorAll(".word")) el.remove();
    drawCrop();
    if (!showText || !words) return;
    words.words.forEach((w, i) => {
      const box = wordBox(w.bbox, words.imagePx);
      const cls = confClass(w.conf);
      if (!box || (cls === "ok" && i !== doubtful)) return;
      const el = h("div", { class: `word ${cls}${i === doubtful ? " cur" : ""}`, title: `${w.text} (${Math.round(w.conf)}%)` });
      el.style.left = `${box.left}%`;
      el.style.top = `${box.top}%`;
      el.style.width = `${box.width}%`;
      el.style.height = `${box.height}%`;
      frame.append(el);
    });
  }

  /** Jump to the next word with doubtful confidence (OCR-CONF-UI). */
  bus.on("ocr-doubtful", (index) => {
    doubtful = index;
    if (!showText) toggleText();
    else redrawOverlays();
    const el = frame.querySelector(".word.cur");
    el?.scrollIntoView({ block: "center", inline: "center" });
  });

  // ---- crop -----------------------------------------------------------------

  function startCrop() {
    const page = currentPage();
    if (!page) return;
    if (page.crop) {
      act.attempt(api.apply({ op: "crop", ids: [page.id], rect: null }));
      return;
    }
    cropRect = { ...DEFAULT_CROP };
    syncControls();
    drawCrop();
  }

  function endCrop() {
    cropRect = null;
    frame.querySelector(".crop-layer")?.remove();
    frame.querySelector(".crop-hint")?.remove();
    syncControls();
  }

  async function applyCrop() {
    const page = currentPage();
    if (!page || !cropRect) return;
    const rect = displayToPage(normalize(cropRect), page.rot);
    const ids = selection.ids.size > 1 && selection.has(page.id) ? [...selection.ids] : [page.id];
    endCrop();
    await act.attempt(api.apply({ op: "crop", ids, rect }));
  }

  function drawCrop() {
    frame.querySelector(".crop-layer")?.remove();
    frame.querySelector(".crop-hint")?.remove();
    if (!cropRect) return;
    const rectEl = h("div", { class: "crop-rect", role: "group", "aria-label": t("preview.crop_area") });
    const place = () => {
      rectEl.style.left = `${cropRect.x0 * 100}%`;
      rectEl.style.top = `${cropRect.y0 * 100}%`;
      rectEl.style.width = `${(cropRect.x1 - cropRect.x0) * 100}%`;
      rectEl.style.height = `${(cropRect.y1 - cropRect.y0) * 100}%`;
    };
    const handles = {
      nw: [0, 0, "nwse-resize"], n: [50, 0, "ns-resize"], ne: [100, 0, "nesw-resize"], e: [100, 50, "ew-resize"],
      se: [100, 100, "nwse-resize"], s: [50, 100, "ns-resize"], sw: [0, 100, "nesw-resize"], w: [0, 50, "ew-resize"],
    };
    for (const [name, [x, y, cursor]] of Object.entries(handles)) {
      const el = h("div", { class: "h", dataset: { handle: name } });
      el.style.left = `calc(${x}% - 7px)`;
      el.style.top = `calc(${y}% - 7px)`;
      el.style.cursor = cursor;
      rectEl.append(el);
    }
    place();
    const layer = h("div", { class: "crop-layer" }, rectEl);
    let drag = null;
    layer.addEventListener("pointerdown", (e) => {
      const handle = e.target.dataset?.handle ?? (e.target === rectEl ? "move" : null);
      if (!handle) return;
      drag = { handle, x: e.clientX, y: e.clientY, rect: { ...cropRect } };
      layer.setPointerCapture(e.pointerId);
      e.preventDefault();
    });
    layer.addEventListener("pointermove", (e) => {
      if (!drag) return;
      const box = frame.getBoundingClientRect();
      cropRect = dragRect(drag.rect, drag.handle, (e.clientX - drag.x) / box.width, (e.clientY - drag.y) / box.height);
      place();
    });
    const stop = () => (drag = null);
    layer.addEventListener("pointerup", stop);
    layer.addEventListener("pointercancel", stop);
    // The rectangle can also be moved from the keyboard: arrows move it, Shift+arrows resize it.
    rectEl.tabIndex = 0;
    rectEl.addEventListener("keydown", (e) => {
      const d = 0.01;
      const dir = { ArrowLeft: [-d, 0], ArrowRight: [d, 0], ArrowUp: [0, -d], ArrowDown: [0, d] }[e.key];
      if (!dir) return;
      e.preventDefault();
      cropRect = e.shiftKey ? dragRect(cropRect, "se", dir[0], dir[1]) : dragRect(cropRect, "move", dir[0], dir[1]);
      place();
    });
    const multi = selection.ids.size > 1 && selection.has(pageId);
    const hint = h(
      "div",
      { class: "crop-hint" },
      h("span", { text: multi ? t("preview.crop_apply_n", { n: selection.ids.size }) : t("preview.crop_hint") }),
      h("button", { class: "btn small primary", type: "button", onClick: applyCrop }, t("preview.crop_apply")),
      h("button", { class: "btn small outline", type: "button", onClick: endCrop }, t("common.cancel")),
    );
    frame.append(layer, hint);
  }

  // ---- wiring ---------------------------------------------------------------

  stage.addEventListener(
    "wheel",
    (e) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      stepZoom(e.deltaY < 0 ? 1 : -1);
    },
    { passive: false },
  );
  new ResizeObserver(() => {
    layout();
    ensureResolution();
  }).observe(stage);

  bus.on("selection", show);
  bus.on("pages-reset", show);
  bus.on("pages-loaded", show);
  bus.on("preview-focus", () => stage.scrollTo({ top: 0 }));
  bus.on("preview-toggle-zoom", () => setZoom(zoom === 1 ? 2 : 1));
  bus.on("preview-crop", () => (cropRect ? endCrop() : startCrop()));
  bus.on("page-patched", show);
  onLanguage(() => {
    prev.title = prev.ariaLabel = t("preview.prev");
    next.title = next.ariaLabel = t("preview.next");
    zOut.title = zOut.ariaLabel = t("preview.zoom_out");
    zIn.title = zIn.ariaLabel = t("preview.zoom_in");
    zFit.title = zFit.ariaLabel = t("preview.fit");
    ccw.title = ccw.ariaLabel = t("menu.rotate_ccw");
    cw.title = cw.ariaLabel = t("menu.rotate_cw");
    modeButtons.forEach((b) => (b.textContent = t(`filter.${b.dataset.mode}`)));
    deskew.textContent = t("filter.deskew");
    clean.textContent = t("filter.despeckle");
    textBtn.querySelector("span").textContent = t("preview.text");
    cropBtn.querySelector("span").textContent = t("preview.crop");
    show();
  });

  layout();
  show();
  return { show };
}
