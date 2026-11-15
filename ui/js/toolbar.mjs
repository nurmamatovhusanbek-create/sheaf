// UI-TOOLBAR: Scan, Open, OCR, Export, Undo/Redo, Presets, Search, Settings.

import { h, icon, clear } from "./dom.mjs";
import { t, onLanguage } from "./i18n.mjs";
import { app, bus } from "./state.mjs";
import * as act from "./actions.mjs";
import { toast } from "./dialogs.mjs";

export function mountToolbar(root, { search, onExport, onSettings }) {
  function button(name, label, onClick, { disabled = false, title, primary = false, key } = {}) {
    return h(
      "button",
      { class: `btn${primary ? " primary" : ""}`, type: "button", disabled, onClick, title: key ? `${title ?? label} (${key})` : (title ?? label) },
      icon(name),
      h("span", { text: label }),
    );
  }

  function render() {
    const noPages = app.total === 0;
    clear(root).append(
      button("scan", t("toolbar.scan"), () => toast(t("toolbar.scan_soon")), { title: t("toolbar.scan_soon"), key: "Ctrl+Shift+S" }),
      button("open", t("toolbar.open"), act.openFiles, { key: "Ctrl+O" }),
      button("ocr", t("toolbar.ocr"), () => (act.runOcr(), bus.emit("show-ocr")), { disabled: noPages || app.settings.ocrMode === "Off", key: "F" }),
      button("export", t("toolbar.export"), () => onExport(), { disabled: noPages, primary: true, key: "Ctrl+E" }),
      h("span", { class: "sep" }),
      button("undo", t("toolbar.undo"), () => act.undo(false), { disabled: !app.canUndo, key: "Ctrl+Z" }),
      button("redo", t("toolbar.redo"), () => act.undo(true), { disabled: !app.canRedo, key: "Ctrl+Y" }),
      h("span", { class: "sep" }),
      button("tools", t("toolbar.presets"), () => toast(t("toolbar.presets_soon")), { title: t("toolbar.presets_soon") }),
      h("span", { class: "grow" }),
      search.el,
      button("settings", t("toolbar.settings"), onSettings),
    );
  }

  bus.on("app", render);
  onLanguage(render);
  render();
}
