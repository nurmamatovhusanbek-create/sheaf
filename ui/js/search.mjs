// UI-SEARCH: full text over recognized pages (CMD-SEARCH). Results jump to the page.

import { h, icon, clear } from "./dom.mjs";
import { t, onLanguage } from "./i18n.mjs";
import { api } from "./ipc.mjs";
import { bus, pages } from "./state.mjs";
import * as act from "./actions.mjs";

export function createSearch() {
  const input = h("input", { type: "search", "aria-label": t("search.label"), placeholder: t("search.placeholder"), id: "search" });
  const results = h("div", { class: "search-results", role: "listbox", hidden: true });
  const box = h("div", { class: "search" }, icon("search"), input, results);
  let timer = null;
  let token = 0;

  async function go() {
    const q = input.value.trim();
    const mine = ++token;
    if (!q) {
      results.hidden = true;
      return;
    }
    const hits = await act.attempt(api.search(q));
    if (mine !== token || !hits) return;
    clear(results);
    results.hidden = false;
    if (!hits.length) {
      results.append(h("div", { class: "none", text: t("search.none") }));
      return;
    }
    for (const hit of hits.slice(0, 50)) {
      results.append(
        h("button", { type: "button", role: "option", onClick: () => jump(hit.page) },
          h("span", { class: "page-no", text: t("search.page", { n: "…" }) }),
          h("span", { text: hit.snippet })),
      );
    }
    // Page numbers are positions, which only the workspace knows; resolve them lazily.
    const buttons = [...results.querySelectorAll("button")];
    hits.slice(0, 50).forEach(async (hit, i) => {
      const at = await position(hit.page);
      if (at >= 0 && buttons[i]) buttons[i].querySelector(".page-no").textContent = t("search.page", { n: at + 1 });
    });
  }

  /** Index of a page id: loaded pages answer at once, otherwise the list is read through. */
  async function position(id) {
    let at = pages.indexOfLoaded(id);
    for (let from = 0; at < 0 && from < pages.total; from += 120) {
      await pages.ensure(from, from + 120);
      at = pages.indexOfLoaded(id);
    }
    return at;
  }

  async function jump(id) {
    const at = await position(id);
    results.hidden = true;
    if (at < 0) return;
    await act.selectIndex(at);
    bus.emit("tray-scroll", at);
    bus.emit("preview-focus");
  }

  input.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(go, 250);
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      clearTimeout(timer);
      go();
    } else if (e.key === "Escape") {
      input.value = "";
      results.hidden = true;
      input.blur();
    } else if (e.key === "ArrowDown") {
      results.querySelector("button")?.focus();
    }
  });
  document.addEventListener("pointerdown", (e) => {
    if (!box.contains(e.target)) results.hidden = true;
  });
  onLanguage(() => {
    input.setAttribute("aria-label", t("search.label"));
    input.placeholder = t("search.placeholder");
  });
  return { el: box, focus: () => input.focus() };
}
