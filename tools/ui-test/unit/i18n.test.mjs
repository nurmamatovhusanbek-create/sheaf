// ADR-I18N: the three language files stay in step with each other and with the code.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { setLoader, setLanguage, t, language } from "../../../ui/js/i18n.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const load = (code) => JSON.parse(fs.readFileSync(path.join(root, "ui/i18n", `${code}.json`), "utf8"));
const LANGS = ["en", "uz", "ru"];
const tables = Object.fromEntries(LANGS.map((l) => [l, load(l)]));

const placeholders = (v) => [...new Set([...String(Array.isArray(v) ? v[0] : v).matchAll(/\{(\w+)\}/g)].map((m) => m[1]))].sort();

test("every language has exactly the same keys", () => {
  const keys = (l) => Object.keys(tables[l]).sort();
  assert.deepEqual(keys("uz"), keys("en"), "uz vs en");
  assert.deepEqual(keys("ru"), keys("en"), "ru vs en");
});

test("the same placeholders appear in every translation of a string", () => {
  for (const key of Object.keys(tables.en)) {
    const want = placeholders(tables.en[key]);
    for (const l of ["uz", "ru"]) {
      const forms = [].concat(tables[l][key]);
      for (const form of forms) assert.deepEqual(placeholders(form), want, `${l}:${key}`);
    }
  }
});

test("plural arrays have the right number of forms", () => {
  const ok = { en: [1, 2], uz: [1, 2], ru: [1, 3] };
  for (const l of LANGS) {
    for (const [key, value] of Object.entries(tables[l])) {
      if (Array.isArray(value)) assert.ok(ok[l].includes(value.length), `${l}:${key} has ${value.length} forms`);
    }
  }
  // A count that changes the wording must be plural-aware in Russian.
  for (const key of ["tray.count", "job.files", "scope.selected", "ocr.doubtful"]) {
    assert.equal(tables.ru[key].length, 3, key);
  }
});

test("no value is empty and none carries markup", () => {
  for (const l of LANGS) {
    for (const [key, value] of Object.entries(tables[l])) {
      for (const form of [].concat(value)) {
        if (!key.startsWith("export.split_arg_") || !["None", "BlankSheet"].some((s) => key.endsWith(s))) assert.ok(String(form).length > 0, `${l}:${key}`);
        assert.ok(!/[<>]/.test(form), `${l}:${key} contains markup`);
      }
    }
  }
});

function sourceFiles() {
  const dir = path.join(root, "ui/js");
  return fs.readdirSync(dir).filter((f) => f.endsWith(".mjs")).map((f) => [f, fs.readFileSync(path.join(dir, f), "utf8")]);
}

test("every key the code asks for exists", () => {
  const missing = [];
  for (const [file, src] of sourceFiles()) {
    for (const m of src.matchAll(/\bt\(\s*"([a-z0-9_.]+)"/g)) if (!(m[1] in tables.en)) missing.push(`${file}: ${m[1]}`);
  }
  assert.deepEqual(missing, []);
});

test("keys built at run time all resolve", () => {
  const ids = ["merge", "split", "extract", "remove", "organize", "rotate", "crop", "compress", "img2pdf", "pdf2img", "stamp", "lock", "ocr", "scan"];
  const want = [];
  for (const id of ids) want.push(`tool.${id}.name`, `tool.${id}.hint`);
  for (const a of ["TopLeft", "TopCenter", "TopRight", "Center", "BottomLeft", "BottomCenter", "BottomRight"]) want.push(`anchor.${a}`);
  for (const m of ["Color", "Gray", "BW"]) want.push(`filter.${m}`);
  for (const s of ["None", "EveryN", "BlankSheet", "TextMatch", "Ranges"]) want.push(`export.split_${s}`, `export.split_arg_${s}`);
  for (const m of ["add", "redo", "text"]) want.push(`ocr.mode_${m}`, `ocr.mode_${m}_hint`);
  for (const s of ["none", "queued", "running", "done", "skipped", "failed"]) want.push(`ocr.state_${s}`);
  for (const v of ["system", "light", "dark"]) want.push(`settings.theme_${v}`);
  for (const v of ["Fast", "Accurate"]) want.push(`settings.quality_${v}`);
  for (const v of ["Manual", "Auto", "Off"]) want.push(`settings.ocr_${v}`);
  for (const k of ["import", "export", "ocr"]) want.push(`job.${k}`);
  assert.deepEqual(want.filter((k) => !(k in tables.en)), []);
});

test("every error code of the core has a message", () => {
  const rs = fs.readFileSync(path.join(root, "crates/sheaf-core/src/error.rs"), "utf8");
  const keys = [...rs.matchAll(/=>\s*"(error\.[a-z_]+)"/g)].map((m) => m[1]);
  assert.ok(keys.length >= 13, `found ${keys.length} error keys in sheaf-core`);
  assert.deepEqual(keys.filter((k) => !(k in tables.en)), []);
});

test("t() fills placeholders, picks plural forms and falls back to English", async () => {
  setLoader(async (code) => tables[code]);
  await setLanguage("ru");
  assert.equal(language(), "ru");
  assert.equal(t("tray.count", { n: 1 }), "1 страница");
  assert.equal(t("tray.count", { n: 3 }), "3 страницы");
  assert.equal(t("tray.count", { n: 11 }), "11 страниц");
  assert.equal(t("tray.page", { n: 4 }), "Страница 4");
  assert.equal(t("no.such.key"), "no.such.key");
  await setLanguage("en");
  assert.equal(t("tray.count", { n: 1 }), "1 page");
  assert.equal(t("tray.count", { n: 2 }), "2 pages");
  assert.equal(t("preview.page_of", { n: 2 }), "Page 2 of {total}", "a missing variable is left visible, not hidden");
  await setLanguage("xx"); // unknown codes fall back to English
  assert.equal(language(), "en");
});
