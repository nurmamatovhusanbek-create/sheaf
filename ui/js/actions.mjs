// What the user can do to pages. Shared by the toolbar, the tray, shortcuts and menus.

import { api } from "./ipc.mjs";
import { bus, pages, selection } from "./state.mjs";
import { showError, toast } from "./dialogs.mjs";
import { t } from "./i18n.mjs";

/** Run a command; show its error instead of throwing. Resolves to undefined on failure. */
export async function attempt(promise) {
  try {
    return await promise;
  } catch (e) {
    showError(e);
    return undefined;
  }
}

export function openFiles() {
  return attempt(api.openFiles());
}

export async function rotate(deg, ids = selection.targets()) {
  if (!ids.length) return;
  await attempt(api.apply({ op: "rotate", ids, deg }));
}

export async function deletePages(ids = selection.targets()) {
  if (!ids.length) return;
  const rev = await attempt(api.apply({ op: "delete", ids }));
  if (rev !== undefined) selection.set([...selection.ids].filter((id) => !ids.includes(id)), -1, -1);
}

export async function duplicatePages(ids = selection.targets()) {
  if (!ids.length) return;
  await attempt(api.apply({ op: "duplicate", ids }));
}

/** Insert a blank page after the last selected page (or at the end). */
export async function insertBlank() {
  let at = pages.total;
  const ids = selection.targets();
  if (ids.length) {
    const indexes = ids.map((id) => pages.indexOfLoaded(id)).filter((i) => i >= 0);
    if (indexes.length) at = Math.max(...indexes) + 1;
  }
  await attempt(api.apply({ op: "insertBlank", at }));
}

export async function undo(redo = false) {
  const rev = await attempt(api.undo(redo));
  // Which pages came or went is not known here; start from a clean selection.
  if (rev !== undefined && rev !== null) selection.clear();
}

/** Move `ids` so they start at `to` in the list that remains without them. */
export async function movePages(ids, to) {
  await attempt(api.apply({ op: "move", ids, to }));
}

/** Recognize text on the selection (or every page if nothing is selected). */
export async function runOcr({ mode = "add", langs = null, all = false } = {}) {
  const ids = all ? null : selection.targets();
  if (ids && !ids.length) {
    toast(t("ocr.nothing_selected"));
    return;
  }
  const job = await attempt(api.ocrRun(ids?.length ? ids : null, { mode, langs }));
  if (job !== undefined) bus.emit("job-started", job);
}

/** Settings the filter controls edit: modes plus the two switches. */
export function filterFor(page) {
  const f = page?.filter ?? {};
  return {
    mode: f.mode ?? "Color",
    threshold: f.threshold ?? "Auto",
    contrast: f.contrast ?? 0,
    deskew: f.deskew ?? "Off",
    despeckle: f.despeckle ?? false,
  };
}

export async function setFilter(patch, ids = selection.targets()) {
  if (!ids.length) return;
  const base = filterFor(pages.byId(ids[0]));
  await attempt(api.apply({ op: "setFilter", ids, filter: { ...base, ...patch } }));
}

/** Select the page at `index` (Shift extends from the anchor, Ctrl toggles). */
export async function selectIndex(index, { shift = false, ctrl = false } = {}) {
  await pages.ensure(index, index + 1);
  const page = pages.get(index);
  if (!page) return;
  if (shift && selection.anchor >= 0) {
    const ids = await pages.idsBetween(selection.anchor, index);
    selection.set(ctrl ? [...selection.ids, ...ids] : ids, selection.anchor, index);
  } else if (ctrl) {
    const next = new Set(selection.ids);
    if (next.has(page.id)) next.delete(page.id);
    else next.add(page.id);
    selection.set(next, index, index);
  } else {
    selection.set([page.id], index, index);
  }
}
