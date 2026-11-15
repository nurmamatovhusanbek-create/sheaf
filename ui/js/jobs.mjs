// UI-JOBBAR: one row per job with progress, cancel and "Reveal in Explorer"
// (CMD-JOB-CANCEL, CMD-REVEAL, EVT-JOB). Sizes are shown before and after for
// compression, as the spec asks.

import { h, icon, clear } from "./dom.mjs";
import { t, onLanguage } from "./i18n.mjs";
import { api } from "./ipc.mjs";
import { bus } from "./state.mjs";
import { bytes, percentChange } from "./format.mjs";
import { attempt } from "./actions.mjs";

const KEEP_FINISHED = 4;

/** Words for a job's kind (`tool:compress` → "Compress"). */
export function kindLabel(kind) {
  if (kind.startsWith("tool:")) return t(`tool.${kind.slice(5)}.name`);
  return t(`job.${kind}`);
}

/** The one-line result of a finished job. */
export function summary(job) {
  const d = job.detail ?? {};
  if (job.state === "failed") return t(job.error?.key ?? "error.internal");
  if (job.state === "cancelled") return t("job.cancelled");
  if (job.state !== "done") return job.state === "queued" ? t("job.queued") : `${Math.round(job.progress * 100)}%`;
  if (job.kind === "ocr") {
    return t("job.ocr_result", { done: d.done ?? 0, skipped: d.skipped ?? 0, failed: d.failed ?? 0 });
  }
  if (d.files) {
    const out = d.outputBytes ?? 0;
    const before = d.inputBytes ?? 0;
    const sizes = out && before ? ` · ${bytes(before)} → ${bytes(out)} (${percentChange(before, out)}%)` : out ? ` · ${bytes(out)}` : "";
    const enc = d.files.find((f) => f.encryption)?.encryption;
    return `${t("job.files", { n: d.files.length })}${sizes}${enc ? ` · ${enc}` : ""}`;
  }
  if (typeof d.count === "number") {
    return `${t("job.files", { n: d.count })}${d.skipped ? ` · ${t("job.skipped", { n: d.skipped })}` : ""}`;
  }
  return t("job.done");
}

export function mountJobBar(root) {
  const jobs = new Map();
  const dismissed = new Set();

  function render() {
    const list = [...jobs.values()].filter((j) => !dismissed.has(j.id) && !(j.kind === "import" && j.state === "done"));
    const finished = list.filter((j) => ["done", "failed", "cancelled"].includes(j.state));
    const shown = list.filter((j) => !["done", "failed", "cancelled"].includes(j.state) || finished.slice(-KEEP_FINISHED).includes(j));
    clear(root);
    if (!shown.length) {
      root.append(h("div", { class: "job-empty", text: t("job.none") }));
      return;
    }
    for (const job of shown) {
      const running = job.state === "running" || job.state === "queued";
      root.append(
        h("div", { class: `job ${job.state}`, role: "group", "aria-label": kindLabel(job.kind) },
          h("span", { class: "kind", text: kindLabel(job.kind) }),
          running
            ? h("progress", { max: 1, value: job.state === "queued" ? undefined : job.progress, "aria-label": kindLabel(job.kind) })
            : h("span", { class: "state" }, h("span", { text: summary(job), title: summary(job) })),
          running ? h("span", { class: "state", text: summary(job) }) : job.outputs.length
            ? h("button", { class: "btn small outline", type: "button", onClick: () => attempt(api.reveal(job.id, 0)) }, icon("reveal"), t("job.reveal"))
            : h("span"),
          running
            ? h("button", { class: "btn small outline", type: "button", onClick: () => attempt(api.jobCancel(job.id)) }, t("common.cancel"))
            : h("button", { class: "btn small icon-only", type: "button", title: t("job.dismiss"), "aria-label": t("job.dismiss"), onClick: () => { dismissed.add(job.id); render(); } }, icon("close")),
        ),
      );
    }
  }

  bus.on("job", (view) => {
    jobs.set(view.id, view);
    render();
  });
  onLanguage(render);
  api.jobs().then((list) => { for (const j of list) jobs.set(j.id, j); render(); }).catch(() => render());
  render();
}
