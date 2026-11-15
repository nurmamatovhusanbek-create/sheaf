// Start-up and wiring. Everything the window shows comes from commands and
// events of the shell (DOC-IPC); nothing is read from disk or network here.

import { api, onEvent, CommandError } from "./ipc.mjs";
import { app, bus, pages, refresh, selection } from "./state.mjs";
import { setLanguage, onLanguage, t, translateTree } from "./i18n.mjs";
import { h, clear } from "./dom.mjs";
import { mountTray } from "./tray.mjs";
import { mountPreview } from "./preview.mjs";
import { mountSide } from "./drawer.mjs";
import { mountToolbar } from "./toolbar.mjs";
import { mountJobBar } from "./jobs.mjs";
import { createSearch } from "./search.mjs";
import { installShortcuts } from "./shortcuts.mjs";
import { openExportDialog } from "./exportdlg.mjs";
import { openSettings } from "./settings.mjs";
import { promptUnlock } from "./passwords.mjs";
import { showError, closeModal, toast } from "./dialogs.mjs";
import * as act from "./actions.mjs";

function applyTheme() {
  const theme = app.settings?.theme;
  if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
  else delete document.documentElement.dataset.theme;
}

function fatal(error) {
  const box = document.getElementById("fatal");
  const e = error instanceof CommandError ? error : new CommandError(error);
  clear(box).append(
    h("div", { class: "card" },
      h("h1", { text: t("fatal.title") }),
      h("div", { text: t(e.key.startsWith("error.") ? e.key : "error.internal") }),
      h("div", { class: "note", text: e.detail }),
      h("div", { class: "note", text: t("fatal.hint") })),
  );
  box.hidden = false;
}

/** Coalesce a burst of events into one refresh. */
let refreshing = null;
function scheduleRefresh() {
  if (refreshing) return refreshing;
  refreshing = Promise.resolve()
    .then(() => refresh())
    .catch(showError)
    .finally(() => {
      refreshing = null;
    });
  return refreshing;
}

async function listen() {
  await Promise.all([
    onEvent("pages", () => scheduleRefresh()),
    onEvent("page-ready", (e) => {
      app.canUndo = e.canUndo;
      app.canRedo = e.canRedo;
      app.rev = e.rev;
      if (pages.patch(e.page)) bus.emit("page-patched", e.page);
      bus.emit("app", app);
    }),
    onEvent("job", (view) => bus.emit("job", view)),
    onEvent("import", (reports) => {
      for (const r of reports) {
        app.flags.set(r.source, r);
        if (r.encrypted) promptUnlock({ id: r.source, name: r.name });
        else if (r.hasForms) bus.emit("notice", { key: "import.forms", name: r.name });
      }
    }),
    onEvent("error", (e) => showError(e)),
    onEvent("engines", (e) => {
      app.engines = e;
      bus.emit("app", app);
    }),
    onEvent("drop-hover", (on) => {
      document.getElementById("drop").hidden = !on;
    }),
  ]);
}

function dragSplitter(el, varName, { min, max, from }) {
  const root = document.documentElement;
  let start = null;
  el.addEventListener("pointerdown", (e) => {
    start = { x: e.clientX, w: parseFloat(getComputedStyle(root).getPropertyValue(varName)) };
    el.setPointerCapture(e.pointerId);
  });
  el.addEventListener("pointermove", (e) => {
    if (!start) return;
    const dx = (e.clientX - start.x) * (from === "right" ? -1 : 1);
    root.style.setProperty(varName, `${Math.min(max, Math.max(min, start.w + dx))}px`);
  });
  el.addEventListener("pointerup", () => (start = null));
  el.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    const now = parseFloat(getComputedStyle(root).getPropertyValue(varName));
    const dir = (e.key === "ArrowRight" ? 1 : -1) * (from === "right" ? -1 : 1);
    root.style.setProperty(varName, `${Math.min(max, Math.max(min, now + dir * 16))}px`);
  });
}

async function boot() {
  let snapshot;
  try {
    await listen();
    snapshot = await api.snapshot();
  } catch (e) {
    await setLanguage("en").catch(() => {});
    fatal(e);
    return;
  }
  Object.assign(app, { settings: snapshot.settings });
  await setLanguage(snapshot.settings.language);
  translateTree(document);
  applyTheme();

  const search = createSearch();
  const onExport = (opts) => openExportDialog(opts);
  mountToolbar(document.getElementById("toolbar"), { search, onExport, onSettings: openSettings });
  const tray = mountTray(document.getElementById("tray"), { onOpenFiles: act.openFiles, onExport });
  mountPreview(document.getElementById("preview"));
  mountSide(document.getElementById("side"), { onExport, onOpenFiles: act.openFiles });
  mountJobBar(document.getElementById("jobbar"));
  installShortcuts({ onExport, search });
  dragSplitter(document.getElementById("split-left"), "--tray-w", { min: 150, max: 420, from: "left" });
  dragSplitter(document.getElementById("split-right"), "--side-w", { min: 260, max: 520, from: "right" });
  bus.on("tray-scroll", (index) => tray.scrollTo(index));
  bus.on("settings", async (s) => {
    applyTheme();
    if (s.language !== document.documentElement.lang) {
      closeModal();
      await setLanguage(s.language);
      translateTree(document);
    }
  });
  bus.on("notice", ({ key, name }) => toast(t(key, { name })));
  onLanguage(() => {
    document.getElementById("drop-text").textContent = t("drop.text");
    document.title = t("app.title");
  });
  document.getElementById("drop-text").textContent = t("drop.text");
  document.title = t("app.title");

  await refresh(snapshot);
  document.getElementById("app").hidden = false;
  // Files given on the command line may already be in, and locked.
  for (const s of snapshot.sources ?? []) if (s.encrypted && !s.unlocked) promptUnlock({ id: s.id, name: s.name });
  // Make the keyboard land somewhere useful.
  tray.focus();
  // Context menu of the page itself is not wanted anywhere else.
  document.addEventListener("contextmenu", (e) => {
    if (!e.target.closest?.(".tray-item") && !["INPUT", "TEXTAREA"].includes(e.target.tagName)) e.preventDefault();
  });
  selection.clear();
}

boot();
