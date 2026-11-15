// Modals (never nested deeper than one level, DOC-UI), context menus and toasts.

import { h, icon, clear } from "./dom.mjs";
import { t } from "./i18n.mjs";

const layer = () => document.getElementById("layer");
let openModal = null;

/** Show an error from a command or event. Never shows file contents (FLOW-ERROR). */
export function showError(error) {
  const key = error?.key && error.key.startsWith("error.") ? error.key : "error.internal";
  toast(t(key), { kind: "error", detail: error?.detail || error?.msg || error?.message || "" });
}

export function toast(message, { kind = "info", detail = "", timeout = 7000 } = {}) {
  const host = document.getElementById("toasts");
  const el = h(
    "div",
    { class: `toast ${kind}` },
    h("div", { class: "msg" }, h("div", {}, message), detail ? h("div", { class: "detail" }, detail) : null),
    h("button", { class: "btn small icon-only", title: t("common.close"), "aria-label": t("common.close"), onClick: () => el.remove() }, icon("close")),
  );
  host.append(el);
  while (host.children.length > 4) host.firstChild.remove();
  if (timeout) setTimeout(() => el.remove(), timeout);
  return el;
}

/** The focusable elements inside `root`, in tab order. */
function focusables(root) {
  return [...root.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])')].filter(
    (el) => !el.disabled && !el.hidden && el.offsetParent !== null,
  );
}

/**
 * A modal dialog. `actions` are `{label, primary, onClick}`; a click handler
 * that returns `false` keeps the dialog open. Returns `{close, el}`.
 */
export function modal({ title, body, actions = [], onClose, wide = false }) {
  openModal?.close();
  const previous = document.activeElement;
  const titleId = `dlg-${Math.random().toString(36).slice(2, 8)}`;
  const buttons = actions.map((a) =>
    h(
      "button",
      {
        class: `btn ${a.primary ? "primary" : "outline"}${a.danger ? " danger" : ""}`,
        type: "button",
        disabled: a.disabled,
        onClick: async (e) => {
          const keep = (await a.onClick?.(e)) === false;
          if (!keep) api.close();
        },
      },
      a.label,
    ),
  );
  const dialog = h(
    "div",
    { class: "dialog", role: "dialog", "aria-modal": "true", "aria-labelledby": titleId, style: wide ? { width: "min(760px, calc(100vw - 32px))" } : {} },
    h("h2", { id: titleId }, title),
    body,
    buttons.length ? h("div", { class: "actions" }, buttons) : null,
  );
  const scrim = h("div", { class: "scrim" }, dialog);
  const onKey = (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      api.close();
    } else if (e.key === "Tab") {
      const items = focusables(dialog);
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };
  scrim.addEventListener("keydown", onKey);
  scrim.addEventListener("mousedown", (e) => {
    if (e.target === scrim) api.close();
  });
  const api = {
    el: dialog,
    close() {
      if (!scrim.isConnected) return;
      scrim.remove();
      if (openModal === api) openModal = null;
      previous?.focus?.();
      onClose?.();
    },
  };
  layer().append(scrim);
  openModal = api;
  (dialog.querySelector("input:not([type=checkbox]), select, textarea") ?? focusables(dialog)[0])?.focus();
  return api;
}

export function isModalOpen() {
  return openModal !== null;
}

/** Close any open modal (used when the language changes under it). */
export function closeModal() {
  openModal?.close();
}

let menuEl = null;

export function closeMenu() {
  menuEl?.remove();
  menuEl = null;
}

/** items: `{label, icon?, key?, onClick, disabled?}` or `"sep"`. */
export function contextMenu(x, y, items, returnFocus) {
  closeMenu();
  const el = h("div", { class: "menu", role: "menu" });
  for (const item of items) {
    if (item === "sep") {
      el.append(h("hr"));
      continue;
    }
    el.append(
      h(
        "button",
        {
          role: "menuitem",
          disabled: item.disabled,
          onClick: () => {
            closeMenu();
            returnFocus?.focus?.();
            item.onClick?.();
          },
        },
        item.icon ? icon(item.icon) : null,
        h("span", {}, item.label),
        item.key ? h("span", { class: "kbd" }, item.key) : null,
      ),
    );
  }
  layer().append(el);
  const box = el.getBoundingClientRect();
  el.style.left = `${Math.max(4, Math.min(x, window.innerWidth - box.width - 4))}px`;
  el.style.top = `${Math.max(4, Math.min(y, window.innerHeight - box.height - 4))}px`;
  menuEl = el;
  el.querySelector("button:not(:disabled)")?.focus();
  const onKey = (e) => {
    if (e.key === "Escape") {
      closeMenu();
      returnFocus?.focus?.();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = [...el.querySelectorAll("button:not(:disabled)")];
      const at = items.indexOf(document.activeElement);
      items[(at + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
    }
  };
  el.addEventListener("keydown", onKey);
  setTimeout(() => {
    const away = (e) => {
      if (!el.contains(e.target)) {
        closeMenu();
        document.removeEventListener("pointerdown", away, true);
      }
    };
    document.addEventListener("pointerdown", away, true);
  });
  return el;
}

/** Replace the children of `el` with `nodes`. */
export function fill(el, ...nodes) {
  clear(el).append(...nodes.flat().filter(Boolean));
  return el;
}
