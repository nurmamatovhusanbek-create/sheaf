// Application state: one store, a page cache and the selection. Components
// subscribe to the parts they draw.

import { api } from "./ipc.mjs";

/** A minimal event emitter. */
export class Bus {
  #handlers = new Map();
  on(name, fn) {
    if (!this.#handlers.has(name)) this.#handlers.set(name, new Set());
    this.#handlers.get(name).add(fn);
    return () => this.#handlers.get(name)?.delete(fn);
  }
  emit(name, payload) {
    for (const fn of [...(this.#handlers.get(name) ?? [])]) fn(payload);
  }
}

export const bus = new Bus();

/** Everything the window knows that is not a page. */
export const app = {
  rev: 0,
  total: 0,
  canUndo: false,
  canRedo: false,
  sources: [],
  settings: {},
  engines: {},
  /** Per source id: facts from the import report (forms, restrictions, network share). */
  flags: new Map(),
  version: "",
};

const PAGE_CHUNK = 120;

/**
 * Pages by position, fetched on demand in chunks. The workspace is the
 * source of truth (MOD-CORE); this is only a window onto it.
 */
export class PageCache {
  #byIndex = new Map();
  #byId = new Map();
  #inflight = new Map();
  #epoch = 0;
  total = 0;

  get(index) {
    return this.#byIndex.get(index);
  }

  byId(id) {
    return this.#byId.get(id);
  }

  indexOfLoaded(id) {
    const page = this.#byId.get(id);
    if (!page) return -1;
    for (const [index, p] of this.#byIndex) if (p.id === id) return index;
    return -1;
  }

  /** Forget everything: the order or content changed. */
  reset(total) {
    this.#epoch += 1;
    this.#byIndex.clear();
    this.#byId.clear();
    this.#inflight.clear();
    this.total = total;
  }

  /** Replace one page in place (OCR finished, rotation recorded). */
  patch(page) {
    const old = this.#byId.get(page.id);
    if (!old) return false;
    this.#byId.set(page.id, page);
    for (const [index, p] of this.#byIndex) {
      if (p.id === page.id) {
        this.#byIndex.set(index, page);
        break;
      }
    }
    return true;
  }

  /** Make sure `[start, end)` is loaded; resolves when it is. */
  async ensure(start, end) {
    const first = Math.max(0, start);
    const last = Math.min(this.total, end);
    const jobs = [];
    for (let chunk = Math.floor(first / PAGE_CHUNK); chunk * PAGE_CHUNK < last; chunk += 1) {
      const from = chunk * PAGE_CHUNK;
      if (this.#byIndex.has(from) && this.#byIndex.has(Math.min(from + PAGE_CHUNK, this.total) - 1)) continue;
      jobs.push(this.#load(from));
    }
    await Promise.all(jobs);
  }

  #load(from) {
    if (this.#inflight.has(from)) return this.#inflight.get(from);
    const epoch = this.#epoch;
    const promise = api
      .pages(from, PAGE_CHUNK)
      .then((dto) => {
        if (epoch !== this.#epoch) return;
        this.total = dto.total;
        dto.pages.forEach((page, i) => {
          this.#byIndex.set(dto.start + i, page);
          this.#byId.set(page.id, page);
        });
        bus.emit("pages-loaded", { start: dto.start, count: dto.pages.length });
      })
      .finally(() => {
        if (epoch === this.#epoch) this.#inflight.delete(from);
      });
    this.#inflight.set(from, promise);
    return promise;
  }

  /** Ids in `[a, b]` (inclusive, either order), loading what is missing. */
  async idsBetween(a, b) {
    const lo = Math.min(a, b);
    const hi = Math.max(a, b);
    await this.ensure(lo, hi + 1);
    const ids = [];
    for (let i = lo; i <= hi; i += 1) {
      const page = this.get(i);
      if (page) ids.push(page.id);
    }
    return ids;
  }
}

export const pages = new PageCache();

/** The selection: page ids in the order they were added, plus the anchor for Shift-click. */
export const selection = {
  ids: new Set(),
  anchor: -1, // index
  focus: -1, // index of the page that has the keyboard focus
  set(ids, anchor = this.anchor, focus = this.focus) {
    this.ids = new Set(ids);
    this.anchor = anchor;
    this.focus = focus;
    bus.emit("selection", this);
  },
  clear() {
    this.set([], -1, this.focus);
  },
  has(id) {
    return this.ids.has(id);
  },
  /** The page shown in the preview: the focused one if it exists. */
  currentId() {
    const focused = pages.get(this.focus);
    if (focused) return focused.id;
    const [first] = this.ids;
    return first ?? null;
  },
  /** Ids an action applies to: the selection, else the focused page. */
  targets() {
    if (this.ids.size) return [...this.ids];
    const focused = pages.get(this.focus);
    return focused ? [focused.id] : [];
  },
};

/** Re-read the page count and drop the cache, so the next draw fetches fresh pages. */
export async function refresh(snapshot) {
  const snap = snapshot ?? (await api.snapshot());
  Object.assign(app, {
    rev: snap.rev,
    total: snap.total,
    canUndo: snap.canUndo,
    canRedo: snap.canRedo,
    sources: snap.sources ?? app.sources,
    settings: snap.settings ?? app.settings,
    engines: snap.engines ?? app.engines,
    version: snap.version ?? app.version,
  });
  pages.reset(snap.total);
  // Drop selected ids that no longer exist after the caller reloads (done lazily by components).
  bus.emit("pages-reset", app);
  bus.emit("app", app);
}
