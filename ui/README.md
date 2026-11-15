# ui (MOD-UI)

Vanilla ES modules, plain HTML and CSS, no framework and no bundler (ADR-VANILLA-UI). Embedded in the
binary by the shell; nothing is fetched at run time and the page may reach no other origin (SEC-NONET).

| Module | Role |
|---|---|
| `js/ipc.mjs` | the only place that touches `window.__TAURI__`; typed wrappers of the DOC-IPC commands |
| `js/state.mjs` | event bus, page cache (a window onto the workspace), selection |
| `js/tray.mjs` | UI-TRAY: virtual list, selection, drag-reorder with the pointer |
| `js/preview.mjs` | UI-PREVIEW: zoom, rotate, filters, crop handles, text overlay |
| `js/drawer.mjs`, `js/ocrpanel.mjs` | UI-DRAWER tools grid and forms, UI-OCRPANEL |
| `js/exportdlg.mjs`, `js/settings.mjs`, `js/search.mjs`, `js/jobs.mjs`, `js/toolbar.mjs`, `js/shortcuts.mjs`, `js/passwords.mjs` | the other UI components |
| `js/i18n.mjs`, `i18n/{en,uz,ru}.json` | ADR-I18N: key-based strings, Russian plural forms |
| `js/geometry.mjs`, `js/format.mjs` | pure functions (crop frames, OCR boxes, sizes) |

Rules that tests enforce (`tools/ui-test/unit/policy.test.mjs`): no inline script or style, no markup built
from strings, no storage, no network except reading the app's own language files, and only commands the
shell grants. Tests: `cd tools/ui-test && node --test "unit/*.test.mjs"` (logic, i18n, contract) and
`node tools/ui-test/e2e/run.mjs` (the real UI in Chromium against a fake backend, under the real CSP).
