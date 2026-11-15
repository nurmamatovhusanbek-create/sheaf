// UI-OCRPANEL: run OCR on the selection or all pages, and read what it found
// on the current page: text, language, and the words the engine doubted
// (OCR-CONF-UI; correcting the stored text is not built yet).

import { h, clear, uid } from "./dom.mjs";
import { t, onLanguage } from "./i18n.mjs";
import { api } from "./ipc.mjs";
import { app, bus, pages, selection } from "./state.mjs";
import * as act from "./actions.mjs";
import { confClass } from "./geometry.mjs";

const LANGS = [["eng", "English"], ["rus", "Русский"], ["uzb", "Oʻzbekcha (Latn)"], ["uzb_cyrl", "Ўзбекча (Cyrl)"]];
const DOUBTFUL_BELOW = 70;
const MAX_CHIPS = 60;

export function mountOcrPanel(root) {
  const scopeSel = h("select", {}, h("option", { value: "selected" }), h("option", { value: "all" }));
  const mode = h("select", {}, ["add", "redo", "text"].map((m) => h("option", { value: m })));
  const auto = h("input", { type: "checkbox", checked: true });
  const langBoxes = LANGS.map(([code, name]) => ({ code, name, input: h("input", { type: "checkbox", disabled: true, value: code }) }));
  auto.addEventListener("change", () => langBoxes.forEach((l) => (l.input.disabled = auto.checked)));
  const hint = h("div", { class: "note" });
  const run = h("button", { class: "btn primary", type: "button", onClick: onRun });
  const current = h("div", { class: "form" });
  const info = h("div", { class: "form" });

  const langField = h(
    "div",
    { class: "field" },
    h("div", { class: "lab", id: "ocr-lang-lab" }),
    h("div", { class: "check" }, auto, h("label", { for: (auto.id = uid()) })),
    ...langBoxes.map((l) => h("div", { class: "check" }, l.input, h("label", { for: (l.input.id = uid()), text: l.name }))),
  );
  const fieldOf = (control, labelEl) => h("div", { class: "field" }, labelEl, control);
  const scopeLab = h("label", { for: (scopeSel.id = uid()) });
  const modeLab = h("label", { for: (mode.id = uid()) });
  const form = h("div", { class: "form" }, h("h3"), fieldOf(scopeSel, scopeLab), fieldOf(mode, modeLab), langField, hint, h("div", { class: "actions" }, run));
  root.append(form, h("hr"), info, current);

  function labels() {
    form.querySelector("h3").textContent = t("ocr.title");
    scopeLab.textContent = t("ocr.scope");
    scopeSel.options[0].text = t("ocr.scope_selected", { n: selection.targets().length });
    scopeSel.options[1].text = t("ocr.scope_all", { n: app.total });
    modeLab.textContent = t("ocr.mode");
    for (const o of mode.options) o.text = t(`ocr.mode_${o.value}`);
    langField.querySelector("#ocr-lang-lab").textContent = t("ocr.langs");
    langField.querySelector("label").textContent = t("ocr.langs_auto");
    run.textContent = t("ocr.run");
    hint.textContent = app.engines?.tesseract === false ? t("ocr.no_engine") : t(`ocr.mode_${mode.value}_hint`);
    hint.className = app.engines?.tesseract === false ? "note bad" : "note";
    run.disabled = app.engines?.tesseract === false || app.settings?.ocrMode === "Off";
  }

  async function onRun() {
    const langs = auto.checked ? null : langBoxes.filter((l) => l.input.checked).map((l) => l.code);
    if (langs && !langs.length) {
      hint.textContent = t("ocr.pick_lang");
      return;
    }
    await act.runOcr({ mode: mode.value, langs, all: scopeSel.value === "all" });
  }
  mode.addEventListener("change", labels);

  let token = 0;
  let doubtfulIndexes = [];
  let at = -1;

  async function showCurrent() {
    const id = selection.currentId();
    const page = id === null ? undefined : pages.byId(id);
    const mine = ++token;
    clear(info);
    clear(current);
    scopeSel.options[0].text = t("ocr.scope_selected", { n: selection.targets().length });
    if (!page) return;
    info.append(h("h3", { text: t("ocr.current") }), h("div", { class: "note", text: t(`ocr.state_${page.ocr.state}`, { reason: page.ocr.reason ?? "" }) }));
    if (page.ocr.state !== "done") return;
    let result;
    try {
      result = await api.ocrResult(page.id);
    } catch {
      return;
    }
    if (mine !== token) return;
    doubtfulIndexes = result.words.map((w, i) => (w.conf < DOUBTFUL_BELOW ? i : -1)).filter((i) => i >= 0);
    at = -1;
    const chips = doubtfulIndexes.slice(0, MAX_CHIPS).map((i) => {
      const w = result.words[i];
      return h("button", { class: `chip ${confClass(w.conf)}`, type: "button", title: `${Math.round(w.conf)}%`, onClick: () => { at = doubtfulIndexes.indexOf(i); bus.emit("ocr-doubtful", i); } }, `${w.text} · ${Math.round(w.conf)}%`);
    });
    current.append(
      h("div", { class: "stat" }, h("span", { text: t("ocr.confidence") }), h("strong", { text: `${Math.round(result.meanConf)}%` })),
      h("div", { class: "stat" }, h("span", { text: t("ocr.language") }), h("strong", { text: result.langs.join(" + ") })),
      h("div", { class: "stat" }, h("span", { text: t("ocr.words") }), h("strong", { text: String(result.words.length) })),
      doubtfulIndexes.length
        ? h("div", { class: "form" },
            h("div", { class: "lab", text: t("ocr.doubtful", { n: doubtfulIndexes.length }) }),
            h("button", { class: "btn small outline", type: "button", onClick: nextDoubtful }, t("ocr.next_doubtful")),
            h("div", { class: "chips" }, chips))
        : h("div", { class: "note", text: t("ocr.none_doubtful") }),
      h("div", { class: "lab", text: t("ocr.text") }),
      h("div", { class: "ocr-text", tabIndex: 0, role: "textbox", "aria-readonly": "true", "aria-label": t("ocr.text"), text: result.text }),
    );
  }

  function nextDoubtful() {
    if (!doubtfulIndexes.length) return;
    at = (at + 1) % doubtfulIndexes.length;
    bus.emit("ocr-doubtful", doubtfulIndexes[at]);
  }

  bus.on("selection", showCurrent);
  bus.on("page-patched", showCurrent);
  bus.on("pages-reset", showCurrent);
  bus.on("pages-loaded", showCurrent);
  bus.on("app", labels);
  bus.on("ocr-next-doubtful", nextDoubtful);
  onLanguage(() => {
    labels();
    showCurrent();
  });
  labels();
  showCurrent();
}
