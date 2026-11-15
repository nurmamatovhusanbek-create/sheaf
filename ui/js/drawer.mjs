// UI-DRAWER: the tools grid (searchable) and the small form each tool opens.
// UI-OCRPANEL lives in the tab beside it. Every tool ends in one command:
// a page operation (CMD-APPLY), a tool run (CMD-TOOL-RUN) or an export dialog.

import { h, icon, clear, uid } from "./dom.mjs";
import { t, onLanguage } from "./i18n.mjs";
import { api } from "./ipc.mjs";
import { app, bus, selection } from "./state.mjs";
import * as act from "./actions.mjs";
import { toast, showError } from "./dialogs.mjs";
import { mountOcrPanel } from "./ocrpanel.mjs";
import { promptUnlock } from "./passwords.mjs";

/** A labelled field. `control` is the input; the label is wired to it. */
function field(label, control, note) {
  const id = control.id || (control.id = uid());
  return h("div", { class: "field" }, h("label", { for: id }, label), control, note ? h("div", { class: "note" }, note) : null);
}

function check(label, input) {
  const id = input.id || (input.id = uid());
  return h("div", { class: "check" }, input, h("label", { for: id }, label));
}

function select(options, value) {
  const el = h("select", {}, options.map(([v, text]) => h("option", { value: v, text, selected: v === value })));
  return el;
}

function numberInput(value, { min, max, step = 1 } = {}) {
  return h("input", { type: "number", value, min, max, step });
}

/** Which pages a tool applies to, as a sentence. */
function scopeText() {
  const n = selection.targets().length;
  return n ? t("scope.selected", { n }) : t("scope.all", { n: app.total });
}

function scopeNote() {
  return h("div", { class: "note", dataset: { scope: "" }, text: scopeText() });
}

const pagesParam = () => {
  const ids = selection.targets();
  return ids.length ? ids : null;
};

async function run(toolId, params) {
  const job = await act.attempt(api.toolRun(toolId, params, pagesParam()));
  if (job !== undefined) bus.emit("job-started", job);
}

// ---- forms ------------------------------------------------------------------

function splitForm({ back }) {
  const rule = select(
    [["EveryN", t("split.every_n")], ["BlankSheet", t("split.blank")], ["TextMatch", t("split.text")], ["Ranges", t("split.ranges")]],
    "EveryN",
  );
  const n = numberInput(2, { min: 1, max: 10000 });
  const text = h("input", { type: "text", placeholder: "Shartnoma\\s*№\\s*(\\S+)" });
  const ranges = h("input", { type: "text", placeholder: "1-3, 5, 8-" });
  const name = h("input", { type: "text", value: "{source}-{counter}" });
  const nField = field(t("split.n"), n);
  const textField = field(t("split.regex"), text, t("split.regex_hint"));
  const rangeField = field(t("split.ranges_label"), ranges, t("split.ranges_hint"));
  const sync = () => {
    nField.hidden = rule.value !== "EveryN";
    textField.hidden = rule.value !== "TextMatch";
    rangeField.hidden = rule.value !== "Ranges";
  };
  rule.addEventListener("change", sync);
  sync();
  const ruleJson = () => {
    switch (rule.value) {
      case "EveryN": return { EveryN: Math.max(1, Math.trunc(Number(n.value) || 1)) };
      case "TextMatch": return { TextMatch: text.value };
      case "Ranges": return { Ranges: ranges.value };
      default: return "BlankSheet";
    }
  };
  return {
    body: [
      scopeNote(),
      field(t("split.rule"), rule), nField, textField, rangeField,
      field(t("export.name"), name, t("export.name_hint")),
      h("div", { class: "actions" }, h("button", { class: "btn primary", type: "button", onClick: async () => { await run("split", { rule: ruleJson(), name: name.value }); back(); } }, t("common.run"))),
    ],
  };
}

function compressForm({ back }) {
  const levels = [["Low", t("compress.low"), t("compress.low_hint")], ["Recommended", t("compress.recommended"), t("compress.recommended_hint")], ["Extreme", t("compress.extreme"), t("compress.extreme_hint")]];
  const name = `lvl-${uid()}`;
  const radios = levels.map(([value, label, hint], i) => {
    const input = h("input", { type: "radio", name, value, checked: i === 1 });
    return h("div", { class: "check" }, input, h("label", { for: (input.id = uid()) }, h("strong", { text: label }), h("div", { class: "note", text: hint })));
  });
  return {
    body: [
      scopeNote(), ...radios,
      h("div", { class: "note", text: t("compress.report") }),
      h("div", { class: "actions" }, h("button", { class: "btn primary", type: "button", onClick: async () => { const level = radios.map((r) => r.querySelector("input")).find((i) => i.checked)?.value ?? "Recommended"; await run("compress", { level }); back(); } }, t("common.run"))),
    ],
  };
}

function pdf2imgForm({ back }) {
  const mode = select([["render", t("pdf2img.render")], ["extract", t("pdf2img.extract")]], "render");
  const format = select([["png", "PNG"], ["jpg", "JPG"]], "png");
  const dpi = select([[72, "72"], [150, "150"], [200, "200"], [300, "300"], [600, "600"]].map(([v, l]) => [String(v), `${l} dpi`]), "150");
  const dpiField = field(t("pdf2img.dpi"), dpi);
  const formatField = field(t("pdf2img.format"), format);
  mode.addEventListener("change", () => {
    dpiField.hidden = formatField.hidden = mode.value === "extract";
  });
  return {
    body: [
      scopeNote(), field(t("pdf2img.mode"), mode), formatField, dpiField,
      h("div", { class: "note", text: t("pdf2img.note") }),
      h("div", { class: "actions" }, h("button", { class: "btn primary", type: "button", onClick: async () => { await run("pdf2img", { mode: mode.value, format: format.value, dpi: Number(dpi.value) }); back(); } }, t("common.run"))),
    ],
  };
}

const ANCHORS = ["TopLeft", "TopCenter", "TopRight", "Center", "BottomLeft", "BottomCenter", "BottomRight"];

function stampForm({ back }) {
  const kind = select([["watermark", t("stamp.watermark")], ["pagenum", t("stamp.pagenum")], ["image", t("stamp.image")]], "watermark");
  const text = h("input", { type: "text", value: "DRAFT" });
  const size = numberInput(64, { min: 1, max: 2000 });
  const opacity = numberInput(25, { min: 0, max: 100 });
  const rotate = numberInput(45, { min: -180, max: 180 });
  const anchor = select(ANCHORS.map((a) => [a, t(`anchor.${a}`)]), "Center");
  const margin = numberInput(0, { min: 0, max: 2000 });
  const color = h("input", { type: "color", value: "#808080" });
  const from = numberInput(1, { min: 1 });
  const to = h("input", { type: "number", min: 1, placeholder: t("stamp.to_end") });
  const first = numberInput(1, { step: 1 });
  let picked = null;
  const pickName = h("span", { class: "note", text: t("stamp.no_image") });
  const pick = h("button", { class: "btn outline", type: "button", onClick: async () => {
    const r = await act.attempt(api.pickImage());
    if (r) { picked = r; pickName.textContent = r.name; }
  } }, icon("image"), h("span", { text: t("stamp.choose_image") }));
  const textField = field(t("stamp.text"), text, t("stamp.text_hint"));
  const imageField = h("div", { class: "field" }, h("div", { class: "lab", text: t("stamp.image") }), h("div", { class: "row" }, pick, pickName));
  const firstField = field(t("stamp.first_number"), first);
  const presets = {
    watermark: () => { text.value = "DRAFT"; size.value = 64; opacity.value = 25; rotate.value = 45; anchor.value = "Center"; margin.value = 0; color.value = "#808080"; },
    pagenum: () => { text.value = "{n} / {N}"; size.value = 10; opacity.value = 100; rotate.value = 0; anchor.value = "BottomCenter"; margin.value = 24; color.value = "#000000"; },
    image: () => { size.value = 160; opacity.value = 60; rotate.value = 0; anchor.value = "BottomRight"; margin.value = 24; },
  };
  const sync = () => {
    textField.hidden = kind.value === "image";
    imageField.hidden = kind.value !== "image";
    firstField.hidden = kind.value !== "pagenum";
  };
  kind.addEventListener("change", () => { presets[kind.value](); sync(); });
  sync();
  const hex = (c) => [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16));
  const build = () => ({
    text: kind.value === "image" ? "" : text.value,
    image: kind.value === "image" ? picked?.token ?? null : null,
    size_pt: Number(size.value),
    opacity: Number(opacity.value) / 100,
    rotate_deg: Number(rotate.value),
    anchor: anchor.value,
    margin_pt: Number(margin.value),
    color: hex(color.value),
    from_page: Math.max(1, Math.trunc(Number(from.value) || 1)),
    to_page: to.value ? Math.trunc(Number(to.value)) : null,
    first_number: Math.trunc(Number(first.value) || 1),
  });
  return {
    body: [
      scopeNote(), field(t("stamp.kind"), kind), textField, imageField,
      h("div", { class: "field" }, h("div", { class: "row" }, field(t("stamp.size"), size), field(t("stamp.opacity"), opacity), field(t("stamp.rotation"), rotate))),
      h("div", { class: "field" }, h("div", { class: "row" }, field(t("stamp.position"), anchor), field(t("stamp.margin"), margin), field(t("stamp.color"), color))),
      h("div", { class: "field" }, h("div", { class: "row" }, field(t("stamp.from"), from), field(t("stamp.to"), to))),
      firstField,
      h("div", { class: "note", text: t("stamp.page_range_note") }),
      h("div", { class: "actions" }, h("button", { class: "btn primary", type: "button", onClick: async () => {
        if (kind.value === "image" && !picked) { toast(t("stamp.need_image"), { kind: "error" }); return; }
        await run("stamp", { stamps: [build()] });
        back();
      } }, t("common.run"))),
    ],
  };
}

function lockForm({ back }) {
  const user = h("input", { type: "password", autocomplete: "new-password" });
  const again = h("input", { type: "password", autocomplete: "new-password" });
  const owner = h("input", { type: "password", autocomplete: "new-password" });
  const print = h("input", { type: "checkbox", checked: true });
  const copy = h("input", { type: "checkbox", checked: true });
  const edit = h("input", { type: "checkbox", checked: false });
  const error = h("div", { class: "note bad", hidden: true });
  const locked = app.sources.filter((s) => s.encrypted && !s.unlocked);
  const lockedList = locked.length
    ? h("div", { class: "form" }, h("div", { class: "lab", text: t("lock.locked_files") }), locked.map((s) => h("div", { class: "row" }, h("span", { text: s.name }), h("button", { class: "btn small outline", type: "button", onClick: () => promptUnlock(s) }, icon("lock"), t("lock.unlock")))))
    : h("div", { class: "note", text: t("lock.none_locked") });
  const protect = async () => {
    error.hidden = true;
    if (!user.value && !owner.value) { error.textContent = t("lock.need_password"); error.hidden = false; return; }
    if (user.value !== again.value) { error.textContent = t("lock.mismatch"); error.hidden = false; return; }
    await run("protect", { protect: { user_pw: user.value, owner_pw: owner.value, allow_print: print.checked, allow_copy: copy.checked, allow_edit: edit.checked } });
    user.value = again.value = owner.value = "";
    back();
  };
  return {
    body: [
      scopeNote(),
      h("h3", { text: t("lock.protect") }),
      field(t("lock.user_pw"), user, t("lock.user_pw_hint")), field(t("lock.repeat"), again), field(t("lock.owner_pw"), owner, t("lock.owner_pw_hint")),
      h("div", { class: "field" }, h("div", { class: "lab", text: t("lock.allow") }), check(t("lock.print"), print), check(t("lock.copy"), copy), check(t("lock.edit"), edit)),
      h("div", { class: "note", text: t("lock.strength") }), error,
      h("div", { class: "actions" }, h("button", { class: "btn primary", type: "button", onClick: protect }, icon("lock"), t("lock.protect_run"))),
      h("hr"),
      h("h3", { text: t("lock.unlock_title") }), lockedList,
      h("div", { class: "note", text: t("lock.remove_note") }),
      h("div", { class: "actions" }, h("button", { class: "btn outline", type: "button", onClick: async () => { await run("unlock", {}); back(); } }, t("lock.remove_run"))),
    ],
  };
}

// ---- the grid ---------------------------------------------------------------

export function mountSide(root, { onExport, onOpenFiles }) {
  const tabTools = h("button", { role: "tab", id: "tab-tools", "aria-selected": "true", "aria-controls": "panel-tools", onClick: () => open("tools") }, t("side.tools"));
  const tabOcr = h("button", { role: "tab", id: "tab-ocr", "aria-selected": "false", "aria-controls": "panel-ocr", onClick: () => open("ocr") }, t("side.ocr"));
  const tabs = h("div", { class: "tabs", role: "tablist" }, tabTools, tabOcr);
  const toolsBody = h("div", { class: "side-body", role: "tabpanel", id: "panel-tools", "aria-labelledby": "tab-tools" });
  const ocrBody = h("div", { class: "side-body", role: "tabpanel", id: "panel-ocr", "aria-labelledby": "tab-ocr", hidden: true });
  root.append(tabs, toolsBody, ocrBody);
  mountOcrPanel(ocrBody);

  const TOOLS = [
    { id: "merge", icon: "merge", run: onOpenFiles },
    { id: "split", icon: "split", form: splitForm },
    { id: "extract", icon: "extract", run: () => onExport({ selected: true }) },
    { id: "remove", icon: "trash", run: () => act.deletePages() },
    { id: "organize", icon: "blank", run: () => act.insertBlank() },
    { id: "rotate", icon: "cw", run: () => act.rotate(90) },
    { id: "crop", icon: "crop", run: () => bus.emit("preview-crop") },
    { id: "compress", icon: "compress", form: compressForm },
    { id: "img2pdf", icon: "image", run: onOpenFiles },
    { id: "pdf2img", icon: "image", form: pdf2imgForm },
    { id: "stamp", icon: "stamp", form: stampForm },
    { id: "lock", icon: "lock", form: lockForm },
    { id: "ocr", icon: "ocr", run: () => open("ocr") },
    { id: "scan", icon: "scan", disabled: true },
  ];

  let query = "";
  let current = null; // tool id with a form open

  function open(tab) {
    const tools = tab === "tools";
    tabTools.setAttribute("aria-selected", String(tools));
    tabOcr.setAttribute("aria-selected", String(!tools));
    toolsBody.hidden = !tools;
    ocrBody.hidden = tools;
    if (!tools) bus.emit("ocr-tab-shown");
  }

  function drawGrid() {
    current = null;
    const q = query.trim().toLowerCase();
    const search = h("input", { class: "tool-search", type: "search", placeholder: t("side.search_tools"), "aria-label": t("side.search_tools"), value: query });
    search.addEventListener("input", () => {
      query = search.value;
      const caret = search.selectionStart;
      drawGrid();
      const again = toolsBody.querySelector(".tool-search");
      again.focus();
      again.setSelectionRange(caret, caret);
    });
    const cards = TOOLS.filter((tool) => !q || `${t(`tool.${tool.id}.name`)} ${t(`tool.${tool.id}.hint`)}`.toLowerCase().includes(q)).map((tool) =>
      h(
        "button",
        { class: "tool-card", type: "button", disabled: tool.disabled, onClick: () => (tool.form ? drawForm(tool) : tool.run()) },
        icon(tool.icon),
        h("span", { class: "name", text: t(`tool.${tool.id}.name`) }),
        h("span", { class: "hint", text: tool.disabled ? t("tool.soon") : t(`tool.${tool.id}.hint`) }),
      ),
    );
    clear(toolsBody).append(search, h("div", { class: "tool-grid" }, cards.length ? cards : h("div", { class: "note", text: t("side.no_tools") })));
  }

  function drawForm(tool) {
    current = tool.id;
    const back = () => drawGrid();
    const { body } = tool.form({ back });
    clear(toolsBody).append(
      h(
        "div",
        { class: "form" },
        h("button", { class: "btn small outline back", type: "button", onClick: back }, icon("prev"), t("common.back")),
        h("h3", { text: t(`tool.${tool.id}.name`) }),
        body,
      ),
    );
    toolsBody.querySelector("input:not([type=checkbox]):not([type=radio]), select")?.focus();
  }

  bus.on("show-tool", (id) => {
    open("tools");
    const tool = TOOLS.find((x) => x.id === id);
    if (tool?.form) drawForm(tool);
    else drawGrid();
  });
  bus.on("show-ocr", () => open("ocr"));
  // Scope sentences and the locked-file list depend on the selection and sources.
  const redraw = () => {
    if (current) {
      const tool = TOOLS.find((x) => x.id === current);
      // Keep what the person is typing: only the lock form lists sources, and only it is redrawn on source changes.
      if (tool && tool.id === "lock" && lockSig !== signature()) drawForm(tool);
    }
  };
  const signature = () => app.sources.map((s) => `${s.id}${s.unlocked}`).join(",");
  let lockSig = signature();
  bus.on("app", () => {
    redraw();
    lockSig = signature();
  });
  bus.on("selection", () => {
    // Update the scope sentence in place; a redraw would clear what is typed in the form.
    for (const note of toolsBody.querySelectorAll(".note[data-scope]")) note.textContent = scopeText();
  });
  onLanguage(() => {
    tabTools.textContent = t("side.tools");
    tabOcr.textContent = t("side.ocr");
    if (current) {
      const tool = TOOLS.find((x) => x.id === current);
      if (tool) drawForm(tool);
    } else drawGrid();
  });
  drawGrid();
  return { open };
}

export { showError };
