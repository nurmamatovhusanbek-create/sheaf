// UI-EXPORTDLG: FLOW-EXPORT. Format, page scope, split rule, name pattern,
// compression, protection and the output folder. The folder is chosen in a
// native dialog run by the shell; this window never handles a path (SEC-PATHS).

import { h, uid } from "./dom.mjs";
import { t } from "./i18n.mjs";
import { api } from "./ipc.mjs";
import { app, bus, selection } from "./state.mjs";
import { modal } from "./dialogs.mjs";
import { attempt } from "./actions.mjs";

const SPLITS = ["None", "EveryN", "BlankSheet", "TextMatch", "Ranges"];

function field(label, control, note) {
  const id = control.id || (control.id = uid());
  return h("div", { class: "field" }, h("label", { for: id }, label), control, note ? h("div", { class: "note", text: note }) : null);
}

export function openExportDialog({ selected = false } = {}) {
  const chosen = selection.targets();
  const scope = h("select", {}, h("option", { value: "all", text: t("export.scope_all", { n: app.total }) }), h("option", { value: "selected", text: t("export.scope_selected", { n: chosen.length }), disabled: !chosen.length, selected: selected && chosen.length > 0 }));
  const format = h("select", {}, [["pdf", t("export.format_pdf")], ["images", t("export.format_images")], ["text", t("export.format_text")]].map(([v, l]) => h("option", { value: v, text: l })));
  const name = h("input", { type: "text", placeholder: "{source}.edited" });
  const split = h("select", {}, SPLITS.map((s) => h("option", { value: s, text: t(`export.split_${s}`) })));
  const splitArg = h("input", { type: "text" });
  const compress = h("select", {}, [["", t("export.compress_none")], ["Low", t("compress.low")], ["Recommended", t("compress.recommended")], ["Extreme", t("compress.extreme")]].map(([v, l]) => h("option", { value: v, text: l })));
  const protect = h("input", { type: "checkbox" });
  const userPw = h("input", { type: "password", autocomplete: "new-password" });
  const ownerPw = h("input", { type: "password", autocomplete: "new-password" });
  const flatten = h("input", { type: "checkbox" });
  const hideOnly = h("input", { type: "checkbox" });
  const imgFormat = h("select", {}, [["png", "PNG"], ["jpg", "JPG"]].map(([v, l]) => h("option", { value: v, text: l })));
  const dpi = h("select", {}, ["72", "150", "200", "300", "600"].map((v) => h("option", { value: v, text: `${v} dpi`, selected: v === "150" })));
  const folder = h("span", { class: "note" });
  const error = h("div", { class: "note bad", hidden: true, role: "alert" });

  const paintFolder = () => {
    folder.textContent = app.settings.outputDir ? app.settings.outputDir : t("export.beside_source");
  };
  paintFolder();

  const row = (label, control, note) => field(label, control, note);
  const splitField = row(t("export.split"), split);
  const splitArgField = row(t("export.split_arg"), splitArg);
  const nameField = row(t("export.name"), name, t("export.name_hint"));
  const compressField = row(t("export.compress"), compress);
  const protectBox = h("div", { class: "form" },
    h("div", { class: "check" }, protect, h("label", { for: (protect.id = uid()), text: t("export.protect") })),
    field(t("lock.user_pw"), userPw), field(t("lock.owner_pw"), ownerPw, t("lock.owner_pw_hint")), h("div", { class: "note", text: t("lock.strength") }));
  const flattenBox = h("div", { class: "check" }, flatten, h("label", { for: (flatten.id = uid()), text: t("export.flatten") }));
  const hideBox = h("div", { class: "form" }, h("div", { class: "check" }, hideOnly, h("label", { for: (hideOnly.id = uid()), text: t("export.hide_only") })), h("div", { class: "note warn", text: t("export.hide_only_warn"), hidden: true }));
  const imgFields = h("div", { class: "field" }, h("div", { class: "row" }, row(t("pdf2img.format"), imgFormat), row(t("pdf2img.dpi"), dpi)));
  const pdfOnly = [splitField, splitArgField, compressField, protectBox, flattenBox, hideBox];

  const sync = () => {
    const f = format.value;
    for (const el of pdfOnly) el.hidden = f !== "pdf";
    imgFields.hidden = f !== "images";
    nameField.hidden = f === "text" ? false : false;
    splitArgField.hidden = f !== "pdf" || !["EveryN", "TextMatch", "Ranges"].includes(split.value);
    splitArgField.querySelector("label").textContent = t(`export.split_arg_${split.value}`);
    splitArg.placeholder = split.value === "EveryN" ? "2" : split.value === "Ranges" ? "1-3, 5, 8-" : "Shartnoma\\s*№\\s*(\\S+)";
    for (const el of [userPw, ownerPw]) el.disabled = !protect.checked;
    hideBox.querySelector(".note.warn").hidden = !hideOnly.checked;
    name.placeholder = f === "images" ? "{source}-images" : f === "text" ? "{source}" : "{source}.edited";
  };
  for (const el of [format, split, protect, hideOnly]) el.addEventListener("change", sync);
  sync();

  const body = h("div", { class: "form" },
    row(t("export.scope"), scope), row(t("export.format"), format), nameField, splitField, splitArgField, imgFields, compressField, protectBox, flattenBox, hideBox,
    h("div", { class: "field" }, h("div", { class: "lab", text: t("export.folder") }), h("div", { class: "row" }, folder,
      h("button", { class: "btn small outline", type: "button", onClick: async () => { const s = await attempt(api.chooseOutputDir()); if (s) { app.settings = s; paintFolder(); bus.emit("app", app); } } }, t("export.choose_folder")),
      h("button", { class: "btn small outline", type: "button", onClick: async () => { const s = await attempt(api.clearOutputDir()); if (s) { app.settings = s; paintFolder(); bus.emit("app", app); } } }, t("export.reset_folder")))),
    error);

  const start = async () => {
    error.hidden = true;
    const spec = { format: format.value, name: name.value, pages: scope.value === "selected" ? chosen : null };
    if (format.value === "pdf") {
      const arg = splitArg.value;
      spec.split = split.value === "None" ? "None" : split.value === "BlankSheet" ? "BlankSheet" : split.value === "EveryN" ? { EveryN: Math.max(1, Math.trunc(Number(arg) || 0)) } : { [split.value]: arg };
      spec.compress = compress.value || null;
      spec.flattenForms = flatten.checked;
      spec.cropHideOnly = hideOnly.checked;
      if (protect.checked) {
        if (!userPw.value && !ownerPw.value) {
          error.textContent = t("lock.need_password");
          error.hidden = false;
          return false;
        }
        spec.protect = { user_pw: userPw.value, owner_pw: ownerPw.value, allow_print: true, allow_copy: true, allow_edit: false };
      }
    } else if (format.value === "images") {
      spec.imageFormat = imgFormat.value;
      spec.dpi = Number(dpi.value);
    }
    const job = await attempt(api.exportRun(spec));
    userPw.value = ownerPw.value = "";
    if (job === undefined) return false;
    bus.emit("job-started", job);
    return true;
  };

  modal({
    title: t("export.title"),
    body,
    wide: true,
    actions: [
      { label: t("common.cancel") },
      { label: t("export.start"), primary: true, onClick: async () => ((await start()) ? undefined : false) },
    ],
  });
}
