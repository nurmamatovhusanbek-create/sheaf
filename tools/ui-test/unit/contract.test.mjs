// The fake backend the browser tests run against must send what the real
// shell sends. contract/shapes.json is written by the Rust test
// `wire_shapes_match_the_ui_contract` from real payloads; here the fake's
// payloads are held to it, field for field.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const shapes = JSON.parse(fs.readFileSync(path.join(here, "../contract/shapes.json"), "utf8"));
const source = fs.readFileSync(path.join(here, "../e2e/fake-backend.js"), "utf8");

/** A fresh fake with its event log. */
function makeFake() {
  const win = { setTimeout, clearTimeout };
  vm.runInNewContext(source, { window: win, setTimeout, JSON, Promise, Math, Object, Array, Number, String });
  const events = [];
  const listen = (name) => win.__TAURI__.event.listen(`sheaf:${name}`, (e) => events.push([name, e.payload]));
  return { win, events, listen, call: (cmd, args) => win.__TAURI__.core.invoke(cmd, args), sleep: (ms) => new Promise((r) => setTimeout(r, ms)) };
}

/** Fields that are a number or null in the real payload (`Option<usize>`); the sample happened to hold a number. */
const MAY_BE_NULL = new Set(["settings.workers", "snapshot.settings.workers"]);

/** Does `actual` have the shape `expected` describes? "null" in the contract means "may be anything". */
function conforms(expected, actual, where, problems) {
  if (expected === "null") return;
  if (actual === null && MAY_BE_NULL.has(where)) return;
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual)) return problems.push(`${where}: expected an array, got ${JSON.stringify(actual)?.slice(0, 40)}`);
    if (expected.length && actual.length) conforms(expected[0], actual[0], `${where}[]`, problems);
    return;
  }
  if (expected && typeof expected === "object") {
    if (!actual || typeof actual !== "object" || Array.isArray(actual)) return problems.push(`${where}: expected an object, got ${JSON.stringify(actual)?.slice(0, 40)}`);
    const want = Object.keys(expected).sort();
    const have = Object.keys(actual).sort();
    for (const k of want) if (!have.includes(k)) problems.push(`${where}: the real payload has "${k}", the fake does not`);
    for (const k of have) if (!want.includes(k)) problems.push(`${where}: the fake sends "${k}", the real payload does not`);
    for (const k of want) if (k in actual) conforms(expected[k], actual[k], `${where}.${k}`, problems);
    return;
  }
  const kind = actual === null ? "null" : typeof actual === "boolean" ? "bool" : typeof actual;
  if (kind !== expected) problems.push(`${where}: expected ${expected}, got ${kind}`);
}

test("the fake backend sends the shapes the real backend sends", async () => {
  const f = makeFake();
  for (const e of ["pages", "page-ready", "job", "import", "error", "engines"]) await f.listen(e);
  const got = {};

  got.snapshot = await f.call("snapshot");
  got.import = await f.call("open_files");
  got.pages = await f.call("pages", { start: 0, count: 10 });
  got.settings = await f.call("settings");
  const ids = got.pages.pages.map((p) => p.id);

  const exp = await f.call("export", { spec: { format: "pdf" } });
  await f.sleep(250);
  const ocr = await f.call("ocr_run", { pages: [ids[0]], policy: { mode: "redo", langs: ["eng"] } });
  await f.sleep(250);
  const jobs = await f.call("jobs");
  got["job.export"] = jobs.find((j) => j.id === exp);
  got["job.ocr"] = jobs.find((j) => j.id === ocr);
  got.ocr_result = await f.call("ocr_result", { pageId: ids[0] });
  got.search = await f.call("search", { query: "tomon" });
  got.pick = await f.call("pick_image");
  const lastEvent = (name) => f.events.filter(([n]) => n === name).at(-1)?.[1];
  got["event.pages"] = lastEvent("pages");
  got["event.page-ready"] = lastEvent("page-ready");
  got["event.import"] = lastEvent("import");
  got["event.job"] = lastEvent("job");
  f.win.__fake.emit("error", { code: "NotFound", key: "error.not_found", msg: "page" });
  got.error = lastEvent("error");
  got.engines = got.snapshot.engines;

  const problems = [];
  for (const [name, shape] of Object.entries(shapes)) {
    assert.ok(name in got, `the contract has "${name}" and this test has no sample for it`);
    conforms(shape, got[name], name, problems);
  }
  for (const name of Object.keys(got)) assert.ok(name in shapes, `the fake produced "${name}", which is not in the contract`);
  assert.deepEqual(problems, []);
});

test("every command the UI calls exists in the fake", async () => {
  const ipc = fs.readFileSync(path.join(here, "../../../ui/js/ipc.mjs"), "utf8");
  const used = [...ipc.matchAll(/call\("([a-z_]+)"/g)].map((m) => m[1]);
  const f = makeFake();
  const missing = [];
  for (const cmd of used) {
    try {
      await f.call(cmd, { start: 0, count: 1, pageId: 1, jobId: 1, index: 0, query: "", op: { op: "none" }, spec: {}, policy: {}, patch: {}, params: {}, toolId: "x", source: 1, password: "", redo: false, zoom: 1 });
    } catch (e) {
      // A refusal is an answer; only "no such command" is a gap.
      if (e?.msg === "fake InvalidArg" && !["apply"].includes(cmd) && !["scan_devices"].includes(cmd)) missing.push(cmd);
    }
  }
  assert.deepEqual(missing, []);
});
