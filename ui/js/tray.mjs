// UI-TRAY: a virtual list of page thumbnails. Only the rows in view (plus a
// few) exist in the DOM, so a 1000-page file scrolls like a 10-page one
// (PERF-LAZY). Selection by click, Shift, Ctrl and rubber band; reorder by
// dragging with the pointer (not HTML5 drag, which the window toolkit's file
// drop would swallow on Windows).

import { h, icon, clear } from "./dom.mjs";
import { t, onLanguage } from "./i18n.mjs";
import { thumbUrl } from "./ipc.mjs";
import { bus, pages, selection } from "./state.mjs";
import { clamp } from "./format.mjs";
import { contextMenu } from "./dialogs.mjs";
import * as act from "./actions.mjs";

export const ITEM_H = 168;
const BUFFER = 4;
const THUMB_PX = 256;
const DRAG_THRESHOLD = 5;

export function mountTray(root, { onOpenFiles, onExport }) {
  const head = h("div", { class: "tray-head" });
  const scroll = h("div", { class: "tray-scroll", tabIndex: 0, role: "listbox", "aria-multiselectable": "true", "aria-label": t("tray.title") });
  const space = h("div", { class: "tray-space" });
  const empty = h(
    "div",
    { class: "empty" },
    h("strong", { text: t("tray.empty_title") }),
    h("div", { text: t("tray.empty_hint") }),
    h("button", { class: "btn primary", onClick: onOpenFiles }, icon("open"), h("span", { text: t("toolbar.open") })),
  );
  scroll.append(space);
  root.append(head, scroll, empty);

  /** index → element */
  const rows = new Map();
  let dropLine = null;
  let rubber = null;

  function label(index) {
    return t("tray.page", { n: index + 1 });
  }

  function badgesFor(page) {
    const out = [];
    const badge = (cls, text, title) => out.push(h("span", { class: `badge ${cls}`, title: title ?? text }, text));
    switch (page.ocr.state) {
      case "queued": badge("", t("badge.ocr_queued")); break;
      case "running": badge("", t("badge.ocr_running")); break;
      case "done": badge("ok", t("badge.ocr_done"), `${t("badge.ocr_done")} (${page.ocr.langs.join("+")})`); break;
      case "failed": badge("bad", t("badge.ocr_failed"), page.ocr.reason ?? ""); break;
      default:
    }
    if (page.hasText === true && page.ocr.state !== "done") badge("", t("badge.text"), t("badge.text_title"));
    if (page.crop) badge("warn", t("badge.crop"));
    const f = page.filter;
    if (f && (f.mode !== "Color" || f.deskew !== "Off" || f.despeckle || f.contrast)) badge("", t("badge.filter"));
    return out;
  }

  function fillRow(el, index, page) {
    el.dataset.index = String(index);
    el.style.transform = `translateY(${index * ITEM_H}px)`;
    el.setAttribute("aria-posinset", String(index + 1));
    el.setAttribute("aria-setsize", String(pages.total));
    el.querySelector(".tray-label").textContent = label(index);
    const media = el.querySelector(".media");
    const badges = el.querySelector(".badges");
    if (!page) {
      el.dataset.id = "";
      el.setAttribute("aria-selected", "false");
      if (el._key !== null) {
        el._key = null;
        clear(media).append(h("div", { class: "ph" }));
        clear(badges);
      }
      return;
    }
    el.dataset.id = String(page.id);
    el.setAttribute("aria-label", `${label(index)}${page.ocr.state === "done" ? `, ${t("badge.ocr_done")}` : ""}`);
    el.setAttribute("aria-selected", String(selection.has(page.id)));
    el.classList.toggle("focus-ring", selection.focus === index && document.activeElement === scroll);
    const key = `${page.id}/${page.ver}`;
    if (el._key !== key) {
      el._key = key;
      const img = h("img", { alt: "", decoding: "async" });
      img.addEventListener("error", () => img.replaceWith(h("div", { class: "ph", title: t("tray.thumb_failed") })));
      img.src = thumbUrl(page.id, page.ver, THUMB_PX);
      clear(media).append(img);
    }
    // OCR state changes without the picture changing, so badges are redrawn every time.
    clear(badges).append(...badgesFor(page));
  }

  function makeRow() {
    const el = h(
      "div",
      { class: "tray-item", role: "option" },
      h("div", { class: "thumb" }, h("div", { class: "media" }), h("div", { class: "badges" })),
      h("div", { class: "tray-label" }),
    );
    el._key = undefined;
    return el;
  }

  function render() {
    const total = pages.total;
    space.style.height = `${total * ITEM_H}px`;
    empty.hidden = total > 0;
    scroll.hidden = total === 0;
    head.textContent = total ? t("tray.count", { n: total }) + (selection.ids.size > 1 ? ` · ${t("tray.selected", { n: selection.ids.size })}` : "") : "";
    const first = Math.max(0, Math.floor(scroll.scrollTop / ITEM_H) - BUFFER);
    const last = Math.min(total, Math.ceil((scroll.scrollTop + scroll.clientHeight) / ITEM_H) + BUFFER);
    for (const [index, el] of rows) {
      if (index < first || index >= last) {
        el.remove();
        rows.delete(index);
      }
    }
    for (let index = first; index < last; index += 1) {
      let el = rows.get(index);
      if (!el) {
        el = makeRow();
        rows.set(index, el);
        space.append(el);
      }
      fillRow(el, index, pages.get(index));
    }
    pages.ensure(first, last).catch(() => {});
  }

  let scheduled = false;
  function schedule() {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      render();
    });
  }

  function restyleSelection() {
    for (const [index, el] of rows) {
      const page = pages.get(index);
      if (page) {
        el.setAttribute("aria-selected", String(selection.has(page.id)));
        el.classList.toggle("focus-ring", selection.focus === index && document.activeElement === scroll);
      }
    }
    head.textContent = pages.total ? t("tray.count", { n: pages.total }) + (selection.ids.size > 1 ? ` · ${t("tray.selected", { n: selection.ids.size })}` : "") : "";
  }

  function scrollTo(index) {
    const top = index * ITEM_H;
    if (top < scroll.scrollTop) scroll.scrollTop = top;
    else if (top + ITEM_H > scroll.scrollTop + scroll.clientHeight) scroll.scrollTop = top + ITEM_H - scroll.clientHeight;
  }

  function indexAt(clientY) {
    const box = space.getBoundingClientRect();
    return Math.floor((clientY - box.top) / ITEM_H);
  }

  const select = act.selectIndex;

  // ---- pointer: click, drag-reorder, rubber band -------------------------

  scroll.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    scroll.focus({ preventScroll: true });
    const row = e.target.closest?.(".tray-item");
    const downY = e.clientY;
    const downX = e.clientX;
    if (!row) {
      startRubber(e);
      return;
    }
    const index = Number(row.dataset.index);
    const page = pages.get(index);
    if (!page) return;
    const modifier = e.shiftKey || e.ctrlKey || e.metaKey;
    const wasSelected = selection.has(page.id);
    if (!modifier && !wasSelected) select(index);
    let dragging = false;
    let ids = [];
    scroll.setPointerCapture(e.pointerId);

    const move = (ev) => {
      if (!dragging && Math.hypot(ev.clientX - downX, ev.clientY - downY) > DRAG_THRESHOLD) {
        dragging = true;
        ids = selection.has(page.id) ? orderedSelection() : [page.id];
        for (const [i, el] of rows) {
          const p = pages.get(i);
          if (p && ids.includes(p.id)) el.classList.add("dragging");
        }
      }
      if (dragging) {
        const slot = clamp(Math.round((ev.clientY - space.getBoundingClientRect().top) / ITEM_H), 0, pages.total);
        showDropLine(slot);
        autoScroll(ev.clientY);
        move.slot = slot;
      }
    };
    const up = async (ev) => {
      scroll.releasePointerCapture(e.pointerId);
      scroll.removeEventListener("pointermove", move);
      scroll.removeEventListener("pointerup", up);
      scroll.removeEventListener("pointercancel", up);
      stopAutoScroll();
      hideDropLine();
      for (const el of rows.values()) el.classList.remove("dragging");
      if (dragging && ev.type === "pointerup" && move.slot !== undefined) {
        await dropAt(ids, move.slot);
      } else if (!dragging && ev.type === "pointerup" && (modifier || wasSelected)) {
        select(index, { shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey });
      }
    };
    scroll.addEventListener("pointermove", move);
    scroll.addEventListener("pointerup", up);
    scroll.addEventListener("pointercancel", up);
  });

  /** Selected ids in list order (loaded pages only; the selection is what is visible and nearby). */
  function orderedSelection() {
    const out = [];
    for (let i = 0; i < pages.total; i += 1) {
      const p = pages.get(i);
      if (p && selection.has(p.id)) out.push(p.id);
    }
    return out.length ? out : [...selection.ids];
  }

  async function dropAt(ids, slot) {
    await pages.ensure(0, slot);
    const moved = new Set(ids);
    let to = 0;
    for (let i = 0; i < slot; i += 1) {
      const p = pages.get(i);
      if (p && !moved.has(p.id)) to += 1;
    }
    await act.movePages(ids, to);
  }

  function showDropLine(slot) {
    if (!dropLine) {
      dropLine = h("div", { class: "drop-line" });
      space.append(dropLine);
    }
    dropLine.style.top = `${slot * ITEM_H - 2}px`;
  }
  function hideDropLine() {
    dropLine?.remove();
    dropLine = null;
  }

  let scrollTimer = null;
  function autoScroll(clientY) {
    stopAutoScroll();
    const box = scroll.getBoundingClientRect();
    const speed = clientY < box.top + 36 ? -14 : clientY > box.bottom - 36 ? 14 : 0;
    if (speed) scrollTimer = setInterval(() => (scroll.scrollTop += speed), 16);
  }
  function stopAutoScroll() {
    clearInterval(scrollTimer);
    scrollTimer = null;
  }

  function startRubber(e) {
    const startY = e.clientY - space.getBoundingClientRect().top;
    const additive = e.ctrlKey || e.metaKey;
    const base = additive ? new Set(selection.ids) : new Set();
    scroll.setPointerCapture(e.pointerId);
    rubber = h("div", { class: "rubber" });
    rubber.style.left = "4px";
    rubber.style.right = "4px";
    space.append(rubber);
    const move = async (ev) => {
      const y = ev.clientY - space.getBoundingClientRect().top;
      const top = Math.min(startY, y);
      const bottom = Math.max(startY, y);
      rubber.style.top = `${top}px`;
      rubber.style.height = `${bottom - top}px`;
      rubber.style.right = "4px";
      const a = clamp(Math.floor(top / ITEM_H), 0, pages.total - 1);
      const b = clamp(Math.floor(bottom / ITEM_H), 0, pages.total - 1);
      const ids = await pages.idsBetween(a, b);
      selection.set([...base, ...ids], a, b);
    };
    const up = () => {
      scroll.releasePointerCapture(e.pointerId);
      scroll.removeEventListener("pointermove", move);
      scroll.removeEventListener("pointerup", up);
      scroll.removeEventListener("pointercancel", up);
      rubber?.remove();
      rubber = null;
    };
    if (!additive) selection.clear();
    scroll.addEventListener("pointermove", move);
    scroll.addEventListener("pointerup", up);
    scroll.addEventListener("pointercancel", up);
  }

  scroll.addEventListener("dblclick", (e) => {
    if (e.target.closest?.(".tray-item")) bus.emit("preview-focus");
  });

  scroll.addEventListener("contextmenu", async (e) => {
    e.preventDefault();
    const row = e.target.closest?.(".tray-item");
    if (!row) return;
    const index = Number(row.dataset.index);
    const page = pages.get(index);
    if (page && !selection.has(page.id)) await select(index);
    const n = selection.targets().length;
    contextMenu(
      e.clientX,
      e.clientY,
      [
        { label: t("menu.rotate_cw"), icon: "cw", key: "R", onClick: () => act.rotate(90) },
        { label: t("menu.rotate_ccw"), icon: "ccw", key: "Shift+R", onClick: () => act.rotate(-90) },
        { label: t("menu.duplicate"), icon: "copy", key: "Ctrl+D", onClick: () => act.duplicatePages() },
        { label: t("menu.insert_blank"), icon: "blank", onClick: () => act.insertBlank() },
        "sep",
        { label: t("menu.ocr", { n }), icon: "ocr", key: "F", onClick: () => act.runOcr() },
        { label: t("menu.extract", { n }), icon: "extract", onClick: () => onExport?.({ selected: true }) },
        "sep",
        { label: t("menu.delete", { n }), icon: "trash", key: "Del", onClick: () => act.deletePages() },
      ],
      scroll,
    );
  });

  // ---- keyboard -----------------------------------------------------------

  scroll.addEventListener("keydown", async (e) => {
    const total = pages.total;
    if (!total) return;
    const page = Math.max(1, Math.floor(scroll.clientHeight / ITEM_H));
    const from = selection.focus < 0 ? 0 : selection.focus;
    let to = null;
    switch (e.key) {
      case "ArrowDown": to = selection.focus < 0 ? 0 : from + 1; break;
      case "ArrowUp": to = selection.focus < 0 ? 0 : from - 1; break;
      case "PageDown": to = from + page; break;
      case "PageUp": to = from - page; break;
      case "Home": to = 0; break;
      case "End": to = total - 1; break;
      case "a":
      case "A":
        if (e.ctrlKey || e.metaKey) {
          e.preventDefault();
          const ids = await pages.idsBetween(0, total - 1);
          selection.set(ids, 0, selection.focus < 0 ? 0 : selection.focus);
        }
        return;
      case " ":
      case "Enter":
        e.preventDefault();
        if (selection.focus >= 0) {
          if (e.ctrlKey) await select(selection.focus, { ctrl: true });
          else bus.emit("preview-focus");
        }
        return;
      case "Escape":
        selection.clear();
        return;
      default:
        return;
    }
    e.preventDefault();
    to = clamp(to, 0, total - 1);
    scrollTo(to);
    await select(to, { shift: e.shiftKey, ctrl: false });
    if (e.ctrlKey && !e.shiftKey) selection.set(selection.ids, selection.anchor, to);
  });

  scroll.addEventListener("focus", restyleSelection);
  scroll.addEventListener("blur", restyleSelection);
  scroll.addEventListener("scroll", schedule, { passive: true });
  new ResizeObserver(schedule).observe(scroll);

  bus.on("pages-reset", schedule);
  bus.on("pages-loaded", schedule);
  bus.on("page-patched", schedule);
  bus.on("selection", restyleSelection);
  onLanguage(() => {
    scroll.setAttribute("aria-label", t("tray.title"));
    clear(empty).append(
      h("strong", { text: t("tray.empty_title") }),
      h("div", { text: t("tray.empty_hint") }),
      h("button", { class: "btn primary", onClick: onOpenFiles }, icon("open"), h("span", { text: t("toolbar.open") })),
    );
    for (const el of rows.values()) el.remove();
    rows.clear();
    schedule();
  });

  render();
  return {
    scrollTo(index) {
      scrollTo(index);
      schedule();
    },
    focus: () => scroll.focus(),
    rerender: schedule,
  };
}

