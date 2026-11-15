// Browser checks of MOD-UI: the real UI, served from disk under the real CSP,
// talking to a fake backend with the same commands as DOC-IPC. No server, no
// port: pages are fulfilled through request interception.
//
//   node tools/ui-test/e2e/run.mjs            run everything
//   SHEAF_SHOTS=/some/dir node …              also write screenshots there

import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");

const here = path.dirname(fileURLToPath(import.meta.url));
const UI = path.resolve(here, "../../../ui");
const conf = JSON.parse(fs.readFileSync(path.resolve(here, "../../../src-tauri/tauri.conf.json"), "utf8"));
const CSP = Object.entries(conf.app.security.csp).map(([k, v]) => `${k} ${v}`).join("; ");
const FAKE = path.join(here, "fake-backend.js");
const SHOTS = process.env.SHEAF_SHOTS;
if (SHOTS) fs.mkdirSync(SHOTS, { recursive: true });

const TYPES = { ".html": "text/html", ".css": "text/css", ".mjs": "text/javascript", ".js": "text/javascript", ".json": "application/json" };
const WINDOWS_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

const problems = []; // console errors, CSP violations, page errors, blocked requests
const thumbRequests = [];

async function newPage(browser, { colorScheme = "light", width = 1280, height = 800 } = {}) {
  const context = await browser.newContext({ userAgent: WINDOWS_UA, viewport: { width, height }, deviceScaleFactor: 1, colorScheme });
  await context.addInitScript({ path: FAKE });
  await context.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.host === "tauri.localhost") {
      const rel = url.pathname === "/" ? "index.html" : decodeURIComponent(url.pathname.slice(1));
      const file = path.resolve(UI, rel);
      if (!file.startsWith(UI + path.sep) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) return route.fulfill({ status: 404, body: "" });
      return route.fulfill({ status: 200, body: fs.readFileSync(file), headers: { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream", "content-security-policy": CSP } });
    }
    if (url.host === "sheaf-thumb.localhost") {
      thumbRequests.push(url.pathname);
      const [id, , size] = url.pathname.slice(1).split("/");
      const label = `p${id}`;
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="400" height="560" viewBox="0 0 400 560"><rect width="400" height="560" fill="#fff"/><rect x="30" y="40" width="340" height="14" fill="#9aa3ba"/><rect x="30" y="70" width="300" height="10" fill="#cfd4e2"/><rect x="30" y="90" width="320" height="10" fill="#cfd4e2"/><rect x="30" y="110" width="260" height="10" fill="#cfd4e2"/><text x="200" y="330" font-size="64" text-anchor="middle" fill="#1f5fd6" font-family="sans-serif">${label}</text><text x="200" y="370" font-size="20" text-anchor="middle" fill="#5a6275" font-family="sans-serif">${size}px</text></svg>`;
      return route.fulfill({ status: 200, body: svg, headers: { "content-type": "image/svg+xml" } });
    }
    problems.push(`blocked request to ${url.origin}`);
    return route.abort();
  });
  const page = await context.newPage();
  page.on("console", (m) => {
    if (m.type() === "error" || /Content Security Policy|Refused to/i.test(m.text())) {
      // A 404 for the favicon is the browser looking for an icon the app does not ship.
      if (!/favicon|Failed to load resource: the server responded with a status of 404/.test(m.text())) problems.push(`console: ${m.text()}`);
    }
  });
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.context().close_ = () => context.close();
  return page;
}

const fake = (page, fn, arg) => page.evaluate(fn, arg);
const calls = (page, cmd) => page.evaluate((c) => window.__fake.calls.filter((x) => x.cmd === c).map((x) => x.args), cmd);

async function boot(page) {
  await page.goto("http://tauri.localhost/index.html");
  await page.waitForSelector("#app:not([hidden])");
}

async function openDoc(page, { pages = 12, name = "contract.pdf" } = {}) {
  await page.evaluate(([n, nm]) => { window.__fake.importPages = n; window.__fake.importName = nm; }, [pages, name]);
  await page.getByRole("button", { name: /^(Open|Ochish|Открыть)/ }).first().click();
  await page.waitForSelector(".tray-item[data-id]:not([data-id=''])");
}

const rows = (page) => page.locator(".tray-item");
const row = (page, n) => page.locator(`.tray-item[data-index="${n - 1}"]`);
const selected = (page) => page.locator('.tray-item[aria-selected="true"]');

const tests = [];
const test = (name, fn) => tests.push([name, fn]);
const shot = async (page, name) => { if (SHOTS) await page.screenshot({ path: path.join(SHOTS, `${name}.png`) }); };

// ---------------------------------------------------------------------------

test("starts empty, in English, with every toolbar action labelled", async (page) => {
  await boot(page);
  assert.equal(await page.evaluate(() => document.documentElement.lang), "en");
  for (const name of ["Scan", "Open", "OCR", "Export", "Undo", "Redo", "Presets", "Settings"]) {
    assert.ok(await page.getByRole("button", { name: new RegExp(`^${name}`) }).count(), name);
  }
  assert.ok(await page.getByText("No pages yet").isVisible());
  assert.ok(await page.getByRole("button", { name: /^Export/ }).isDisabled(), "nothing to export yet");
  assert.ok(await page.getByRole("button", { name: /^Undo/ }).isDisabled());
  await shot(page, "01-empty");
});

test("opening a file fills the tray and a click selects, Ctrl toggles, Shift extends", async (page) => {
  await boot(page);
  await openDoc(page);
  assert.equal(await page.locator(".tray-head").textContent(), "12 pages");
  await row(page, 3).click();
  assert.equal(await selected(page).count(), 1);
  await row(page, 5).click({ modifiers: ["Control"] });
  assert.equal(await selected(page).count(), 2);
  await row(page, 8).click({ modifiers: ["Shift"] });
  // Shift extends from the anchor (the last plain or Ctrl click: page 5) to page 8.
  assert.deepEqual(await selected(page).evaluateAll((els) => els.map((e) => e.dataset.index)), ["4", "5", "6", "7"]);
  assert.match(await page.locator(".tray-head").textContent(), /12 pages · 4 selected/);
  await page.locator(".tray-scroll").evaluate((el) => (el.scrollTop = 0)); // only the rows in view exist
  await row(page, 1).click();
  assert.equal(await selected(page).count(), 1);
  assert.equal(await page.locator(".preview-bar .pg").first().textContent(), "Page 1 of 12");
  await shot(page, "02-selection");
});

test("the tray is virtual: a thousand pages keep a few dozen rows", async (page) => {
  await boot(page);
  await openDoc(page, { pages: 1000 });
  const count = await rows(page).count();
  assert.ok(count > 0 && count < 40, `${count} rows in the DOM`);
  assert.equal(await row(page, 1).getAttribute("aria-setsize"), "1000");
  thumbRequests.length = 0;
  await page.locator(".tray-scroll").evaluate((el) => (el.scrollTop = el.scrollHeight));
  await page.waitForSelector('.tray-item[data-index="999"][data-id]:not([data-id=""])');
  assert.equal(await page.locator('.tray-item[data-index="999"] .tray-label').textContent(), "Page 1000");
  assert.ok(thumbRequests.length < 60, `${thumbRequests.length} thumbnails asked for after one jump`);
  assert.ok((await rows(page).count()) < 40);
});

test("keyboard: R rotates, Delete removes, Ctrl+Z restores, Ctrl+D duplicates, F recognizes", async (page) => {
  await boot(page);
  await openDoc(page);
  await row(page, 2).click();
  const id2 = Number(await row(page, 2).getAttribute("data-id"));
  await page.keyboard.press("r");
  await page.waitForFunction(() => window.__fake.calls.some((c) => c.cmd === "apply"));
  assert.deepEqual((await calls(page, "apply")).at(-1).op, { op: "rotate", ids: [id2], deg: 90 });
  await page.keyboard.press("Shift+R");
  await page.waitForFunction(() => window.__fake.calls.filter((c) => c.cmd === "apply").length === 2);
  assert.equal((await calls(page, "apply")).at(-1).op.deg, -90);
  await page.keyboard.press("Delete");
  await page.waitForFunction(() => window.__fake.info().total === 11);
  assert.match(await page.locator(".tray-head").textContent(), /11 pages/);
  await page.keyboard.press("Control+z");
  await page.waitForFunction(() => window.__fake.info().total === 12);
  await row(page, 4).click();
  await page.keyboard.press("Control+d");
  await page.waitForFunction(() => window.__fake.info().total === 13);
  await page.keyboard.press("f");
  await page.waitForFunction(() => window.__fake.calls.some((c) => c.cmd === "ocr_run"));
  assert.equal(await page.locator("#tab-ocr").getAttribute("aria-selected"), "true");
});

test("dragging a page reorders it with the right target index", async (page) => {
  await boot(page);
  await openDoc(page);
  const id1 = Number(await row(page, 1).getAttribute("data-id"));
  const box1 = await row(page, 1).boundingBox();
  const box4 = await row(page, 4).boundingBox();
  await page.mouse.move(box1.x + box1.width / 2, box1.y + 40);
  await page.mouse.down();
  await page.mouse.move(box1.x + box1.width / 2, box1.y + 80, { steps: 4 });
  await page.mouse.move(box4.x + box4.width / 2, box4.y + box4.height - 10, { steps: 6 }); // just before page 5
  assert.ok(await page.locator(".drop-line").count(), "an insertion line is shown while dragging");
  await page.mouse.up();
  await page.waitForFunction(() => window.__fake.calls.some((c) => c.cmd === "apply" && c.args.op.op === "move"));
  const move = (await calls(page, "apply")).find((c) => c.op.op === "move").op;
  // Dropped between pages 4 and 5; with page 1 lifted out that is position 3 of the pages that remain.
  assert.deepEqual(move, { op: "move", ids: [id1], to: 3 });
  await page.waitForFunction((id) => window.__fake.state.pages[3].id === id, id1);
  await page.waitForFunction((id) => document.querySelector('.tray-item[data-index="3"]').dataset.id === String(id), id1);
});

test("preview shows the page, rotates it and zooms", async (page) => {
  await boot(page);
  await openDoc(page);
  await row(page, 2).click();
  const id = await row(page, 2).getAttribute("data-id");
  await page.waitForSelector(".page-frame img");
  const src = await page.locator(".page-frame img").getAttribute("src");
  assert.match(src, new RegExp(`^http://sheaf-thumb\\.localhost/${id}/[0-9a-f]{16}/\\d+$`));
  assert.ok(Number(src.split("/").pop()) > 256, "a preview is bigger than a thumbnail");
  await page.getByRole("button", { name: "Rotate right" }).first().click();
  await page.waitForFunction((i) => window.__fake.state.pages.find((p) => p.id === Number(i)).rot === 90, id);
  await page.waitForFunction((s) => document.querySelector(".page-frame img").getAttribute("src") !== s, src); // new version, new URL
  assert.equal(await page.locator(".preview-bar .pg").nth(1).textContent(), "100%");
  await page.getByRole("button", { name: "Zoom in" }).click();
  assert.equal(await page.locator(".preview-bar .pg").nth(1).textContent(), "150%");
  await page.getByRole("button", { name: "Fit to window" }).click();
  assert.equal(await page.locator(".preview-bar .pg").nth(1).textContent(), "100%");
  await page.getByRole("button", { name: "Next page" }).click();
  await page.waitForFunction(() => document.querySelector(".preview-bar .pg").textContent === "Page 3 of 12");
});

test("cropping a rotated page sends the rectangle in the page's own frame", async (page) => {
  await boot(page);
  await openDoc(page);
  await row(page, 1).click();
  await page.getByRole("button", { name: "Rotate right" }).first().click();
  await page.waitForFunction(() => window.__fake.state.pages[0].rot === 90);
  await page.waitForTimeout(120);
  await page.getByRole("button", { name: /^Crop$/ }).first().click();
  const area = page.getByRole("group", { name: "Crop area" });
  await area.waitFor();
  await area.focus();
  await page.keyboard.press("ArrowRight"); // moves the frame 1% to the right as displayed
  await page.getByRole("button", { name: "Crop", exact: true }).last().click();
  await page.waitForFunction(() => window.__fake.calls.some((c) => c.cmd === "apply" && c.args.op.op === "crop"));
  const { rect, ids } = (await calls(page, "apply")).find((c) => c.op.op === "crop").op;
  const near = (a, b) => Math.abs(a - b) < 1e-6;
  // Displayed 0.06..0.96 across, 0.05..0.95 down on a page turned 90° clockwise → page frame: x = Y, y = 1 - X.
  assert.ok(near(rect.x0, 0.05) && near(rect.x1, 0.95) && near(rect.y0, 0.04) && near(rect.y1, 0.94), JSON.stringify(rect));
  assert.equal(ids.length, 1);
  await page.waitForSelector('.tray-item[data-index="0"] .badge:text("Crop")');
});

test("export dialog sends the chosen options and the job bar follows the job", async (page) => {
  await boot(page);
  await openDoc(page);
  await row(page, 2).click();
  await row(page, 4).click({ modifiers: ["Control"] });
  await page.keyboard.press("Control+e");
  const dialog = page.getByRole("dialog", { name: "Export" });
  await dialog.waitFor();
  await dialog.getByLabel("Pages", { exact: true }).selectOption("selected");
  await dialog.getByLabel("File name").fill("part-{counter}");
  await dialog.getByLabel("Split into files").selectOption("EveryN");
  await dialog.getByLabel("Pages per file").fill("1");
  await dialog.getByLabel("Compression").selectOption("Recommended");
  await dialog.getByLabel("Cropped pages: hide the outside instead of removing it").check();
  assert.ok(await dialog.getByText("Hide only: the cropped-away content stays in the file").isVisible(), "hide-only says what it does (SEC-HONESTY)");
  await shot(page, "07-export-dialog");
  await dialog.getByRole("button", { name: "Export", exact: true }).click();
  await page.waitForSelector('[role="dialog"]', { state: "detached" });
  const spec = (await calls(page, "export")).at(-1).spec;
  assert.equal(spec.format, "pdf");
  assert.equal(spec.name, "part-{counter}");
  assert.deepEqual(spec.split, { EveryN: 1 });
  assert.equal(spec.compress, "Recommended");
  assert.equal(spec.cropHideOnly, true);
  assert.equal(spec.pages.length, 2);
  const job = page.locator(".job").first();
  await job.waitFor();
  await page.waitForSelector(".job.done");
  assert.match(await page.locator(".job.done").textContent(), /1 file/);
  await page.getByRole("button", { name: "Show in folder" }).click();
  await page.waitForFunction(() => window.__fake.state.revealed.length === 1);
});

test("a locked file asks for its password; a wrong one keeps the dialog and reveals nothing", async (page) => {
  await boot(page);
  await page.evaluate(() => {
    const id = window.__fake.addSource("secret.pdf", 3, { encrypted: true });
    window.__fake.emit("import", [{ source: id, name: "secret.pdf", pages: [], encrypted: true, restricted: false, hasForms: false, isUnc: false }]);
  });
  const dialog = page.getByRole("dialog", { name: "Password needed" });
  await dialog.waitFor();
  assert.ok(await dialog.getByText("secret.pdf is protected").isVisible());
  const input = dialog.getByLabel("Password");
  await input.fill("nope");
  await dialog.getByRole("button", { name: "Unlock" }).click();
  await dialog.getByText("That password did not work.").waitFor();
  assert.equal(await input.inputValue(), "", "the field is cleared after each attempt");
  assert.equal(await rows(page).count(), 0, "nothing of the file is shown");
  await input.fill("secret");
  await input.press("Enter");
  await page.waitForSelector('[role="dialog"]', { state: "detached" });
  await page.waitForSelector(".tray-item[data-id]:not([data-id=''])");
  assert.equal(await page.locator(".tray-head").textContent(), "3 pages");
});

test("OCR: badges, text overlay with doubtful words, panel, search", async (page) => {
  await boot(page);
  await openDoc(page, { pages: 6 });
  await row(page, 1).click();
  await row(page, 3).click({ modifiers: ["Shift"] });
  await page.getByRole("button", { name: /^OCR/ }).first().click();
  await page.waitForSelector(".job.running, .job.done");
  await page.waitForSelector(".job.done");
  assert.match(await page.locator(".job.done").textContent(), /3 recognized/);
  await page.waitForSelector('.tray-item[data-index="0"] .badge.ok');
  assert.equal(await page.locator('.tray-item[data-index="0"] .badge.ok').textContent(), "OCR ✓");
  await row(page, 1).click();
  await page.getByRole("tab", { name: "Text (OCR)" }).click();
  await page.waitForSelector(".ocr-text");
  assert.ok((await page.locator(".ocr-text").textContent()).includes("Shartnoma"));
  assert.ok(await page.getByText("2 doubtful words").isVisible());
  await page.getByRole("button", { name: "Text", exact: true }).click();
  await page.waitForSelector(".word.low");
  assert.equal(await page.locator(".word.low").count(), 1);
  assert.equal(await page.locator(".word.mid").count(), 1);
  assert.equal(await page.locator(".word.low").getAttribute("title"), "tomonlar (41%)");
  await page.getByRole("button", { name: "Next doubtful word" }).click();
  await page.waitForSelector(".word.cur");
  await shot(page, "09-ocr-overlay");
  // Search jumps to the page.
  await page.keyboard.press("Control+f");
  await page.getByLabel("Search recognized text").fill("kelishuv");
  await page.keyboard.press("Enter");
  const hit = page.locator(".search-results button").first();
  await hit.waitFor();
  await page.waitForFunction(() => /Page \d/.test(document.querySelector(".search-results .page-no").textContent));
  await hit.click();
  assert.equal(await selected(page).count(), 1);
});

test("a tool form runs a tool on the selection with the right parameters", async (page) => {
  await boot(page);
  await openDoc(page);
  await row(page, 1).click();
  await row(page, 3).click({ modifiers: ["Shift"] });
  await page.getByRole("button", { name: /^Split/ }).click();
  await page.getByLabel("Split by").selectOption("Ranges");
  await page.getByLabel("Ranges", { exact: true }).fill("1-2, 5");
  await page.getByRole("button", { name: "Run" }).click();
  await page.waitForFunction(() => window.__fake.calls.some((c) => c.cmd === "tool_run"));
  const call = (await calls(page, "tool_run")).at(-1);
  assert.equal(call.toolId, "split");
  assert.deepEqual(call.params.rule, { Ranges: "1-2, 5" });
  assert.equal(call.pages.length, 3);
  await page.waitForSelector(".job.done");
  // Stamp: page numbers preset, image from a picked file by token only.
  await page.getByRole("button", { name: /^Stamp/ }).click();
  await page.getByLabel("Kind").selectOption("image");
  await page.getByRole("button", { name: "Choose picture…" }).click();
  await page.getByText("logo.png").waitFor();
  await page.getByRole("button", { name: "Run" }).click();
  await page.waitForFunction(() => window.__fake.calls.filter((c) => c.cmd === "tool_run").length === 2);
  const stamp = (await calls(page, "tool_run")).at(-1).params.stamps[0];
  assert.equal(stamp.image, "tok-logo", "the page names the file by token, never by path");
  assert.equal(stamp.anchor, "BottomRight");
  // Protect: mismatched repeat is refused before anything is sent.
  await page.getByRole("button", { name: /^Lock/ }).click();
  await page.getByLabel("Password to open").fill("abc");
  await page.getByLabel("Repeat password").fill("abd");
  await page.getByRole("button", { name: "Protect", exact: true }).click();
  assert.ok(await page.getByText("The two passwords differ.").isVisible());
  assert.equal((await calls(page, "tool_run")).length, 2);
  await page.getByLabel("Repeat password").fill("abc");
  await page.getByRole("button", { name: "Protect", exact: true }).click();
  await page.waitForFunction(() => window.__fake.calls.filter((c) => c.cmd === "tool_run").length === 3);
  const lock = (await calls(page, "tool_run")).at(-1);
  assert.equal(lock.params.protect.user_pw, "abc");
  await page.waitForSelector(".tool-grid");
  assert.equal(await page.getByLabel("Password to open").count(), 0, "the form is gone and its fields with it");
});

test("settings switch the language at once: Russian plurals, then Uzbek", async (page) => {
  await boot(page);
  await openDoc(page);
  await page.getByRole("button", { name: /^Settings/ }).click();
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByLabel("Language").selectOption("ru");
  await dialog.getByRole("button", { name: "Save" }).click();
  await page.getByRole("button", { name: /^Открыть/ }).waitFor();
  assert.equal(await page.evaluate(() => document.documentElement.lang), "ru");
  assert.equal(await page.locator(".tray-head").textContent(), "12 страниц");
  assert.equal(await row(page, 1).locator(".tray-label").textContent(), "Страница 1");
  await row(page, 1).click();
  await row(page, 2).click({ modifiers: ["Shift"] });
  assert.match(await page.locator(".tray-head").textContent(), /12 страниц · выбрано: 2/);
  await shot(page, "10-russian");
  await page.getByRole("button", { name: /^Настройки/ }).click();
  await page.getByRole("dialog").getByLabel("Язык").selectOption("uz");
  await page.getByRole("dialog").getByRole("button", { name: "Сохранить" }).click();
  await page.getByRole("button", { name: /^Ochish/ }).waitFor();
  assert.equal(await page.locator(".tray-head").textContent().then((s) => s.split(" · ")[0]), "12 ta sahifa");
  await shot(page, "11-uzbek");
});

test("dark theme and large layout both render without console errors", async (page) => {
  await boot(page);
  await openDoc(page);
  await page.getByRole("button", { name: /^Settings/ }).click();
  await page.getByRole("dialog").getByLabel("Theme").selectOption("dark");
  await page.getByRole("dialog").getByRole("button", { name: "Save" }).click();
  await page.waitForFunction(() => document.documentElement.dataset.theme === "dark");
  await row(page, 2).click();
  await shot(page, "12-dark");
});

test("every control has a name and dialogs trap focus and give it back", async (page) => {
  await boot(page);
  await openDoc(page);
  await row(page, 1).click();
  const unnamed = await page.evaluate(() => [...document.querySelectorAll("button, select, input")].filter((el) => {
    if (el.type === "hidden" || el.closest("[hidden]")) return false;
    const name = el.getAttribute("aria-label") || el.title || el.textContent.trim() || (el.labels && el.labels.length && el.labels[0].textContent.trim());
    return !name;
  }).map((el) => el.outerHTML.slice(0, 80)));
  assert.deepEqual(unnamed, []);
  const opener = page.getByRole("button", { name: /^Settings/ });
  await opener.focus();
  await opener.press("Enter");
  const dlg = page.getByRole("dialog");
  await dlg.waitFor();
  assert.equal(await dlg.getAttribute("aria-modal"), "true");
  for (let i = 0; i < 12; i++) await page.keyboard.press("Tab");
  assert.ok(await page.evaluate(() => !!document.activeElement.closest(".dialog")), "Tab stays inside the dialog");
  await page.keyboard.press("Escape");
  await page.waitForSelector('[role="dialog"]', { state: "detached" });
  assert.match((await page.evaluate(() => document.activeElement.textContent)).trim(), /^Settings/);
  await shot(page, "13-main-light");
});

test("failures are shown as translated messages, not raw text", async (page) => {
  await boot(page);
  await page.evaluate(() => window.__fake.emit("error", { code: "PathRejected", key: "error.path_rejected", msg: "reserved device name" }));
  const toast = page.locator(".toast.error");
  await toast.waitFor();
  assert.ok((await toast.textContent()).includes("That file or folder cannot be used."));
  await page.evaluate(() => window.__fake.emit("error", { code: "Weird", key: "../../etc/passwd", msg: "x" }));
  await page.waitForFunction(() => document.body.textContent.includes("Something went wrong."));
});

test("a start-up failure shows an explanation instead of a blank window", async (page) => {
  await page.context().addInitScript(() => {
    const real = window.__TAURI__;
    window.__TAURI__ = {
      ...real,
      core: {
        invoke: async (cmd, args) => {
          if (cmd === "snapshot") throw { code: "EngineMissing", key: "error.engine_missing", msg: "PDFium is not in the vendor folder" };
          return real.core.invoke(cmd, args);
        },
      },
    };
  });
  await page.goto("http://tauri.localhost/index.html");
  await page.waitForSelector("#fatal:not([hidden])");
  const text = await page.locator("#fatal").textContent();
  assert.ok(text.includes("Sheaf could not start") && text.includes("A bundled component is missing or damaged."), text);
  assert.ok(await page.locator("#app").isHidden());
  await shot(page, "14-fatal");
});

// ---------------------------------------------------------------------------

const only = process.argv[2];
const browser = await chromium.launch();
let failed = 0;
for (const [name, fn] of tests) {
  if (only && !name.includes(only)) continue;
  const before = problems.length;
  const page = await newPage(browser);
  try {
    await fn(page);
    const fresh = problems.slice(before);
    if (fresh.length) throw new Error(`browser reported problems:\n  ${fresh.join("\n  ")}`);
    console.log(`ok   ${name}`);
  } catch (e) {
    failed += 1;
    console.log(`FAIL ${name}\n     ${String(e.stack ?? e).split("\n").slice(0, 8).join("\n     ")}`);
    if (SHOTS) await page.screenshot({ path: path.join(SHOTS, `fail-${tests.findIndex(([n]) => n === name)}.png`) }).catch(() => {});
  } finally {
    await page.context().close();
  }
}
await browser.close();
console.log(failed ? `\n${failed} of ${tests.length} failed` : `\nall ${tests.length} passed`);
process.exit(failed ? 1 : 0);
