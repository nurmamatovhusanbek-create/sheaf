// UI-SETTINGS: language, theme, workers, OCR defaults, output folder. Nothing
// here is a document or a secret (DATA-SETTINGS).

import { h, uid } from "./dom.mjs";
import { t, LANGUAGES } from "./i18n.mjs";
import { api } from "./ipc.mjs";
import { app, bus } from "./state.mjs";
import { modal } from "./dialogs.mjs";
import { attempt } from "./actions.mjs";

function row(label, control, note) {
  const id = control.id || (control.id = uid());
  return h("div", { class: "field" }, h("label", { for: id }, label), control, note ? h("div", { class: "note", text: note }) : null);
}

export function openSettings() {
  const s = app.settings;
  const lang = h("select", {}, LANGUAGES.map((l) => h("option", { value: l.code, text: l.name, selected: l.code === s.language })));
  const theme = h("select", {}, ["system", "light", "dark"].map((v) => h("option", { value: v, text: t(`settings.theme_${v}`), selected: v === s.theme })));
  const workers = h("input", { type: "number", min: 0, max: 32, value: s.workers ?? 0 });
  const quality = h("select", {}, ["Fast", "Accurate"].map((v) => h("option", { value: v, text: t(`settings.quality_${v}`), selected: v === s.ocrQuality })));
  const ocrMode = h("select", {}, ["Manual", "Auto", "Off"].map((v) => h("option", { value: v, text: t(`settings.ocr_${v}`), selected: v === s.ocrMode })));
  const e = app.engines ?? {};
  const status = (ok, yes, no) => h("div", { class: ok ? "badge ok" : "badge bad", text: ok ? yes : no });
  const body = h("div", { class: "form" },
    row(t("settings.language"), lang), row(t("settings.theme"), theme),
    row(t("settings.ocr_mode"), ocrMode, t("settings.ocr_mode_hint")),
    row(t("settings.ocr_quality"), quality),
    row(t("settings.workers"), workers, t("settings.workers_hint")),
    h("div", { class: "field" }, h("div", { class: "lab", text: t("settings.engines") }), h("div", { class: "chips" },
      status(e.verified === true, t("settings.verified"), e.verified === false ? t("settings.verify_failed") : t("settings.verifying")),
      status(e.tesseract !== false, t("settings.tesseract_ok"), t("settings.tesseract_missing")),
      status(e.font !== false, t("settings.font_ok"), t("settings.font_missing")))),
    h("div", { class: "note", text: t("settings.about", { version: app.version }) }));
  modal({
    title: t("settings.title"),
    body,
    actions: [
      { label: t("common.cancel") },
      { label: t("common.save"), primary: true, onClick: async () => {
        const next = await attempt(api.setSettings({
          language: lang.value, theme: theme.value, ocrMode: ocrMode.value, ocrQuality: quality.value,
          workers: Math.max(0, Math.trunc(Number(workers.value) || 0)),
        }));
        if (!next) return false;
        app.settings = next;
        bus.emit("settings", next);
        bus.emit("app", app);
        return undefined;
      } },
    ],
  });
}
