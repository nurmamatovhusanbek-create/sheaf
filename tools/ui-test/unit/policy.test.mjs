// SEC-NONET / SEC-IPC at the source level: what the UI is allowed to contain.
// The browser-level checks live in e2e/; these run anywhere node does.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const walk = (dir) => fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)]));
const files = walk(path.join(root, "ui")).filter((f) => /\.(html|css|mjs|js|json)$/.test(f));
const read = (f) => fs.readFileSync(f, "utf8");
const rel = (f) => path.relative(root, f);

test("the UI folder holds only embeddable assets", () => {
  const all = walk(path.join(root, "ui")).map(rel);
  const odd = all.filter((f) => !/\.(html|css|mjs|json|md)$/.test(f));
  assert.deepEqual(odd, []);
});

test("no inline script, style or event-handler attribute in the HTML (CSP: no 'unsafe-inline')", () => {
  const html = read(path.join(root, "ui/index.html"));
  assert.ok(!/<script(?![^>]*\bsrc=)[^>]*>/i.test(html), "inline <script>");
  assert.ok(!/<style[\s>]/i.test(html), "inline <style>");
  assert.ok(!/\sstyle\s*=/i.test(html), "style attribute");
  assert.ok(!/\son[a-z]+\s*=/i.test(html), "inline event handler");
});

test("nothing in the UI reaches a remote origin", () => {
  const allowed = [/^http:\/\/sheaf-thumb\.localhost\/$/, /^http:\/\/www\.w3\.org\/2000\/svg$/];
  for (const f of files.filter((f) => !f.endsWith(".json"))) {
    for (const m of read(f).matchAll(/https?:\/\/[^\s"'`)<>]+/g)) {
      assert.ok(allowed.some((re) => re.test(m[0])), `${rel(f)} mentions ${m[0]}`);
    }
  }
  for (const f of files.filter((f) => f.endsWith(".css"))) assert.ok(!/@import|url\(/i.test(read(f)), `${rel(f)} loads something`);
});

test("no dynamic code and no markup injection", () => {
  const banned = [/\beval\s*\(/, /new Function\s*\(/, /\binnerHTML\b/, /\bouterHTML\b/, /insertAdjacentHTML/, /document\.write/, /\bsetTimeout\s*\(\s*["'`]/, /\bsetInterval\s*\(\s*["'`]/, /localStorage|sessionStorage|indexedDB/, /XMLHttpRequest|WebSocket|EventSource|sendBeacon/, /\bimportScripts\b/];
  for (const f of files.filter((f) => f.endsWith(".mjs"))) {
    const src = read(f);
    for (const re of banned) assert.ok(!re.test(src), `${rel(f)} uses ${re}`);
  }
});

test("fetch is used only to read the app's own language files", () => {
  const users = files.filter((f) => f.endsWith(".mjs") && /\bfetch\s*\(/.test(read(f))).map((f) => path.basename(f));
  assert.deepEqual(users, ["i18n.mjs"]);
  assert.match(read(path.join(root, "ui/js/i18n.mjs")), /fetch\(`i18n\/\$\{code\}\.json`\)/);
});

test("the UI calls only commands the shell grants (capabilities/main.json)", () => {
  const cap = JSON.parse(read(path.join(root, "src-tauri/capabilities/main.json")));
  const granted = new Set(cap.permissions.filter((p) => p.startsWith("allow-")).map((p) => p.slice(6).replaceAll("-", "_")));
  const ipc = read(path.join(root, "ui/js/ipc.mjs"));
  const used = [...ipc.matchAll(/call\("([a-z_]+)"/g)].map((m) => m[1]);
  assert.ok(used.length >= 20, `found ${used.length} commands`);
  assert.deepEqual(used.filter((c) => !granted.has(c)), [], "commands the UI calls but the capability does not grant");
  // And nothing else is granted beyond events and these commands.
  assert.deepEqual(
    cap.permissions.filter((p) => !p.startsWith("allow-") && !p.startsWith("core:event:")),
    [],
    "unexpected permission",
  );
});

test("the page may call only plain commands: no dialog, shell, fs or http plugin permission", () => {
  const cap = JSON.parse(read(path.join(root, "src-tauri/capabilities/main.json")));
  for (const word of ["dialog", "shell", "fs:", "http:", "opener", "process", "updater", "window:", "webview:", "path:", "app:"]) {
    assert.ok(!cap.permissions.some((p) => p.includes(word)), `capability grants ${word}`);
  }
  assert.deepEqual(cap.windows, ["main"]);
  const conf = JSON.parse(read(path.join(root, "src-tauri/tauri.conf.json")));
  const csp = conf.app.security.csp;
  assert.equal(csp["default-src"], "'self'");
  assert.ok(!Object.values(csp).some((v) => /unsafe-inline|unsafe-eval|\*|https?:\/\/(?!ipc\.localhost|sheaf-thumb\.localhost)/.test(v)), JSON.stringify(csp));
});
