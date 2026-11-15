// Typed wrappers around the DOC-IPC commands. The only module that touches
// `window.__TAURI__`; tests install a fake transport with `setTransport`.

/** @typedef {{code: string, key: string, msg: string}} IpcError */

let transport = null;

/** {invoke(cmd, args), listen(event, fn) → Promise<unlisten>, thumbBase: string} */
export function setTransport(t) {
  transport = t;
}

function tauri() {
  if (transport) return transport;
  const T = globalThis.__TAURI__;
  if (!T?.core?.invoke) throw new Error("not running inside the app");
  const windows = /Windows/i.test(globalThis.navigator?.userAgent ?? "");
  transport = {
    invoke: (cmd, args) => T.core.invoke(cmd, args),
    listen: (name, fn) => T.event.listen(name, (e) => fn(e.payload)),
    // WebView2 serves custom schemes as http://<scheme>.localhost; elsewhere <scheme>://localhost.
    thumbBase: windows ? "http://sheaf-thumb.localhost/" : "sheaf-thumb://localhost/",
  };
  return transport;
}

/** A failed command, with the code and i18n key the shell sent. */
export class CommandError extends Error {
  constructor(raw) {
    const e = typeof raw === "string" ? { code: "Internal", key: "error.internal", msg: raw } : raw ?? {};
    super(e.msg || e.code || "error");
    this.code = e.code ?? "Internal";
    this.key = e.key ?? "error.internal";
    this.detail = e.msg ?? "";
  }
}

async function call(command, args) {
  try {
    return await tauri().invoke(command, args);
  } catch (e) {
    throw e instanceof CommandError ? e : new CommandError(e);
  }
}

export const api = {
  snapshot: () => call("snapshot"),
  pages: (start, count) => call("pages", { start, count }),
  apply: (op) => call("apply", { op }),
  undo: (redo = false) => call("undo", { redo }),
  render: (pageId, zoom) => call("render", { pageId, zoom }),
  openFiles: () => call("open_files"),
  unlockPw: (source, password) => call("unlock_pw", { source, password }),
  ocrRun: (pages, policy) => call("ocr_run", { pages, policy }),
  ocrResult: (pageId) => call("ocr_result", { pageId }),
  search: (query) => call("search", { query }),
  toolRun: (toolId, params, pages) => call("tool_run", { toolId, params, pages }),
  exportRun: (spec) => call("export", { spec }),
  jobCancel: (jobId) => call("job_cancel", { jobId }),
  jobs: () => call("jobs"),
  reveal: (jobId, index = 0) => call("reveal", { jobId, index }),
  settings: () => call("settings"),
  setSettings: (patch) => call("set_settings", { patch }),
  chooseOutputDir: () => call("choose_output_dir"),
  clearOutputDir: () => call("clear_output_dir"),
  pickImage: () => call("pick_image"),
  scanDevices: () => call("scan_devices"),
  presets: () => call("presets"),
};

/** Subscribe to a core event (`pages`, `page-ready`, `job`, `import`, `error`, `engines`, `drop-hover`). */
export function onEvent(name, fn) {
  return tauri().listen(`sheaf:${name}`, fn);
}

/** URL of a page picture (CMD-THUMB). `ver` is the page's fingerprint, `px` its longest side. */
export function thumbUrl(pageId, ver, px) {
  return `${tauri().thumbBase}${pageId}/${ver}/${px}`;
}

/** URL for a key returned by `render` (`<id>/<ver>/<px>`). */
export function renderUrl(key) {
  return `${tauri().thumbBase}${key}`;
}
