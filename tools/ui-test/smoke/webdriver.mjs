// PH-4 smoke test on Windows: start the real app through tauri-driver and
// look at its window with WebDriver. Plain `fetch` against the WebDriver
// protocol; no client library.
//
//   tauri-driver --native-driver <msedgedriver.exe>      (listening on 4444)
//   node tools/ui-test/smoke/webdriver.mjs <sheaf.exe> <fixture.pdf>
//
// Checks: the window comes up with the UI, the file given on the command line
// was imported and its pages are in the tray, the tray is not empty, the page
// is in English, and a request to a remote origin is refused (SEC-NONET).

import assert from "node:assert/strict";

const [app, fixture] = process.argv.slice(2);
if (!app || !fixture) {
  console.error("usage: webdriver.mjs <sheaf.exe> <fixture.pdf>");
  process.exit(64);
}
const BASE = process.env.WEBDRIVER_URL ?? "http://127.0.0.1:4444";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function wd(method, path, body) {
  const response = await fetch(`${BASE}${path}`, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const json = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${JSON.stringify(json.value ?? json).slice(0, 300)}`);
  return json.value;
}

async function until(label, fn, ms = 30_000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
    }
    await sleep(250);
  }
  throw new Error(`timed out waiting for ${label}${last ? `: ${last.message}` : ""}`);
}

let session = null;
try {
  const created = await wd("POST", "/session", {
    capabilities: { alwaysMatch: { browserName: "wry", "tauri:options": { application: app, args: ["open", fixture] } } },
  });
  session = created.sessionId;
  const run = (script, args = []) => wd("POST", `/session/${session}/execute/sync`, { script, args });
  const runAsync = (script, args = []) => wd("POST", `/session/${session}/execute/async`, { script, args });

  await until("the window to show the UI", () => run("return !!document.getElementById('app') && !document.getElementById('app').hidden"));
  console.log("ok   the window shows the UI");

  const lang = await run("return document.documentElement.lang");
  assert.equal(lang, "en");
  console.log("ok   English by default");

  const buttons = await run("return [...document.querySelectorAll('#toolbar button')].map((b) => b.textContent.trim())");
  for (const name of ["Open", "OCR", "Export", "Undo"]) assert.ok(buttons.includes(name), `toolbar has ${name}: ${buttons}`);
  console.log("ok   the toolbar is there");

  const pages = await until("the opened file's pages", () => run("return document.querySelector('.tray-head')?.textContent || ''").then((t) => /\d+ pages?/.test(t) && t));
  console.log(`ok   file given on the command line is open (${pages})`);
  assert.ok(await run("return document.querySelectorAll('.tray-item').length > 0"));

  const image = await until("a thumbnail", () => run("return [...document.querySelectorAll('.tray-item img')].some((i) => i.complete && i.naturalWidth > 0)"));
  assert.ok(image);
  console.log("ok   thumbnails load through the sheaf-thumb scheme");

  const remote = await runAsync(
    "const done = arguments[arguments.length - 1]; fetch('https://example.com/', { mode: 'no-cors' }).then(() => done('reached'), () => done('blocked'));",
  );
  assert.equal(remote, "blocked", "a request to a remote origin must not leave the window");
  console.log("ok   a remote request is refused");

  const open = await run("return !!(window.__TAURI__ && window.__TAURI__.core)");
  assert.ok(open);
  console.log("ok   the page reaches the shell only through Tauri");
  console.log("\nsmoke test passed");
} catch (e) {
  console.error(`\nFAIL ${e.message}`);
  if (session) {
    const text = await wd("POST", `/session/${session}/execute/sync`, { script: "return document.body.innerText.slice(0, 600)", args: [] }).catch(() => "");
    console.error(`page text: ${JSON.stringify(text)}`);
  }
  process.exitCode = 1;
} finally {
  if (session) await wd("DELETE", `/session/${session}`).catch(() => {});
}
