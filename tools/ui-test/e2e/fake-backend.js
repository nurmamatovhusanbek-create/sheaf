// A stand-in for the shell, injected before the page loads: the same command
// names and shapes as DOC-IPC, backed by a small in-memory workspace. It lets
// the real UI run in a browser with no Tauri. Only used by tests.
(() => {
  const listeners = new Map();
  const fake = {
    calls: [],
    importPages: 12,
    ocrDelay: 30,
    state: {
      rev: 1, pages: [], sources: [], undo: [], redo: [], nextPage: 1, nextSource: 1, nextJob: 1, jobs: [], revealed: [],
      settings: { language: "en", outputDir: null, defaultPreset: null, workers: null, ocrQuality: "Fast", ocrMode: "Manual", theme: "system" },
      engines: { verified: true, tesseract: true, font: true, error: null },
    },
  };
  const st = fake.state;

  const fail = (code, key) => Promise.reject({ code, key, msg: "fake " + code });
  const hash = (o) => { const s = JSON.stringify(o); let h = 5381; for (let i = 0; i < s.length; i++) h = ((h * 33) ^ s.charCodeAt(i)) >>> 0; return h.toString(16).padStart(16, "0"); };
  const dto = (p) => ({ id: p.id, kind: p.kind, source: p.source, srcIndex: p.srcIndex, rot: p.rot, crop: p.crop, filter: p.filter,
    ocr: p.ocr, hasText: p.hasText, ver: hash([p.rot, p.crop, p.filter, p.kind, p.source, p.srcIndex]) });
  const defaultFilter = () => ({ mode: "Color", threshold: "Auto", contrast: 0, deskew: "Off", despeckle: false });
  const emit = (name, payload) => { for (const fn of listeners.get("sheaf:" + name) ?? []) fn({ payload }); };
  const info = () => ({ rev: st.rev, total: st.pages.length, canUndo: st.undo.length > 0, canRedo: st.redo.length > 0 });
  const clone = (x) => JSON.parse(JSON.stringify(x));
  const push = () => { st.undo.push(clone(st.pages)); st.redo = []; st.rev++; };
  const announce = () => emit("pages", info());

  function addSource(name, count, { encrypted = false } = {}) {
    const id = st.nextSource++;
    st.sources.push({ id, name, kind: "pdf", pages: encrypted ? 0 : count, encrypted, unlocked: !encrypted });
    return id;
  }
  function addPages(source, count) {
    push();
    const ids = [];
    for (let i = 0; i < count; i++) {
      const id = st.nextPage++;
      ids.push(id);
      st.pages.push({ id, kind: "source", source, srcIndex: i, rot: 0, crop: null, filter: defaultFilter(), ocr: { state: "none", langs: [], reason: null }, hasText: i % 5 === 4 });
    }
    return ids;
  }

  function words(page) {
    const text = ["Shartnoma", "№", "17", "tomonlar", "kelishuvga", "binoan", "to‘lov", "amalga", "oshiriladi", "Договор", "поставки"];
    return text.map((w, i) => ({ text: w, bbox: [100 + (i % 4) * 280, 200 + Math.floor(i / 4) * 90, 240, 50], conf: i === 3 ? 41 : i === 6 ? 66 : 94, line: Math.floor(i / 4) }));
  }

  function startJob(kind, runner) {
    const job = { id: st.nextJob++, kind, state: "queued", progress: 0, outputs: [], error: null, detail: null };
    st.jobs.push(job);
    const view = () => clone(job);
    emit("job", view());
    setTimeout(async () => {
      job.state = "running"; emit("job", view());
      try {
        await runner(job, (p) => { job.progress = p; emit("job", view()); });
        if (job.state !== "cancelled") { job.state = "done"; job.progress = 1; }
      } catch (e) { job.state = "failed"; job.error = { code: "Internal", key: "error.internal", msg: String(e) }; }
      emit("job", view());
    }, 20);
    return job.id;
  }
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  const commands = {
    snapshot: () => ({ ...info(), sources: clone(st.sources), settings: clone(st.settings), engines: clone(st.engines), version: "0.0.0" }),
    pages: ({ start, count }) => ({ pages: st.pages.slice(start, start + count).map(dto), start, ...info() }),
    apply({ op }) {
      const ids = op.ids ?? [];
      const found = (id) => st.pages.some((p) => p.id === id);
      if (ids.some((id) => !found(id))) return fail("NotFound", "error.not_found");
      push();
      const byId = (id) => st.pages.find((p) => p.id === id);
      switch (op.op) {
        case "delete": st.pages = st.pages.filter((p) => !ids.includes(p.id)); break;
        case "rotate": ids.forEach((id) => { const p = byId(id); p.rot = (((p.rot + op.deg) % 360) + 360) % 360; }); break;
        case "crop": ids.forEach((id) => { byId(id).crop = op.rect; byId(id).ocr = { state: "none", langs: [], reason: null }; }); break;
        case "setFilter": ids.forEach((id) => { byId(id).filter = op.filter; }); break;
        case "duplicate": { const out = []; for (const p of st.pages) { out.push(p); if (ids.includes(p.id)) out.push({ ...clone(p), id: st.nextPage++ }); } st.pages = out; break; }
        case "insertBlank": st.pages.splice(op.at, 0, { id: st.nextPage++, kind: "blank", source: null, srcIndex: 0, rot: 0, crop: null, filter: defaultFilter(), ocr: { state: "none", langs: [], reason: null }, hasText: false }); break;
        case "move": { const moved = st.pages.filter((p) => ids.includes(p.id)); const rest = st.pages.filter((p) => !ids.includes(p.id)); rest.splice(op.to, 0, ...moved); st.pages = rest; break; }
        default: st.undo.pop(); return fail("InvalidArg", "error.invalid_arg");
      }
      announce();
      return st.rev;
    },
    undo({ redo }) {
      const from = redo ? st.redo : st.undo;
      if (!from.length) return null;
      (redo ? st.undo : st.redo).push(clone(st.pages));
      st.pages = from.pop(); st.rev++; announce();
      return st.rev;
    },
    render({ pageId, zoom }) {
      const p = st.pages.find((x) => x.id === pageId);
      if (!p) return fail("NotFound", "error.not_found");
      return `${pageId}/${dto(p).ver}/${Math.max(257, Math.round(zoom * 1000))}`;
    },
    open_files() {
      const name = fake.importName ?? "contract.pdf";
      const source = addSource(name, fake.importPages);
      const pages = st.pages.length;
      const ids = addPages(source, fake.importPages);
      const report = { source, name, pages: ids, encrypted: false, restricted: false, hasForms: false, isUnc: false };
      announce(); emit("import", [report]);
      return [report];
    },
    unlock_pw({ source, password }) {
      if (password !== "secret") return { ok: false, pages: [] };
      const s = st.sources.find((x) => x.id === source);
      s.unlocked = true; s.pages = 3;
      const ids = addPages(source, 3);
      announce();
      return { ok: true, pages: ids };
    },
    ocr_run({ pages, policy }) {
      const ids = pages ?? st.pages.map((p) => p.id);
      ids.forEach((id) => { st.pages.find((p) => p.id === id).ocr = { state: "queued", langs: [], reason: null }; });
      announce();
      return startJob("ocr", async (job, progress) => {
        let done = 0;
        for (const id of ids) {
          await sleep(fake.ocrDelay);
          if (job.state === "cancelled") break;
          const p = st.pages.find((x) => x.id === id);
          if (!p) continue;
          p.ocr = { state: "done", langs: policy.langs ?? ["uzb", "eng"], reason: null };
          emit("page-ready", { page: dto(p), rev: ++st.rev, canUndo: st.undo.length > 0, canRedo: st.redo.length > 0 });
          done++; progress(done / ids.length);
        }
        job.detail = { done, skipped: 0, failed: 0 };
      });
    },
    ocr_result({ pageId }) {
      const p = st.pages.find((x) => x.id === pageId);
      if (!p || p.ocr.state !== "done") return fail("NotFound", "error.not_found");
      const w = words(p);
      return { words: w, text: w.map((x) => x.text).join(" "), meanConf: 88.2, langs: p.ocr.langs, imagePx: [1240, 1754] };
    },
    search({ query }) {
      const q = query.toLowerCase();
      return st.pages.filter((p) => p.ocr.state === "done").flatMap((p) => words(p).map((w, i) => ({ page: p.id, word: i, snippet: w.text }))).filter((h) => h.snippet.toLowerCase().includes(q)).slice(0, 20);
    },
    tool_run({ toolId, params, pages }) {
      return startJob("tool:" + toolId, async (job, progress) => {
        for (const p of [0.3, 0.7]) { await sleep(30); progress(p); }
        job.outputs = ["C:\\Docs\\contract.compressed.pdf"];
        job.detail = { files: [{ name: "contract.compressed.pdf", pages: pages?.length ?? st.pages.length, copied: 0, rasterized: 0, textLayers: 0, stamps: 0, flattened: 0, encryption: toolId === "protect" ? "AES-256" : null }], inputBytes: 4_200_000, outputBytes: 1_300_000 };
      });
    },
    export({ spec }) {
      return startJob("export", async (job, progress) => {
        for (const p of [0.5, 0.9]) { await sleep(40); progress(p); }
        job.outputs = ["C:\\Docs\\contract.edited.pdf"];
        job.detail = { files: [{ name: "contract.edited.pdf", pages: spec.pages?.length ?? st.pages.length, copied: spec.pages?.length ?? st.pages.length, rasterized: 0, textLayers: 0, stamps: 0, flattened: 0, encryption: null }], inputBytes: 0, outputBytes: 900_000 };
      });
    },
    job_cancel({ jobId }) { const j = st.jobs.find((x) => x.id === jobId); if (j && j.state === "running") { j.state = "cancelled"; emit("job", clone(j)); } return null; },
    jobs: () => clone(st.jobs),
    reveal({ jobId, index }) { st.revealed.push([jobId, index]); return null; },
    settings: () => clone(st.settings),
    set_settings({ patch }) { Object.assign(st.settings, patch); return clone(st.settings); },
    choose_output_dir() { st.settings.outputDir = "C:\\Scans\\Out"; return clone(st.settings); },
    clear_output_dir() { st.settings.outputDir = null; return clone(st.settings); },
    pick_image: () => ({ token: "tok-logo", name: "logo.png" }),
    scan_devices: () => fail("Unsupported", "error.unsupported"),
    presets: () => [],
  };

  window.__fake = Object.assign(fake, { emit, addSource, addPages, announce, info });
  window.__TAURI__ = {
    core: {
      async invoke(cmd, args) {
        fake.calls.push({ cmd, args: clone(args ?? {}) });
        if (!(cmd in commands)) return fail("InvalidArg", "error.invalid_arg");
        await sleep(2);
        return commands[cmd](args ?? {});
      },
    },
    event: {
      async listen(name, fn) {
        if (!listeners.has(name)) listeners.set(name, new Set());
        listeners.get(name).add(fn);
        return () => listeners.get(name).delete(fn);
      },
    },
  };
})();
