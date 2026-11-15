// UI-SHORTCUTS. Every action also has a button or menu entry, so the keyboard
// is a convenience, not the only way (accessibility rule of DOC-UI).

import { bus } from "./state.mjs";
import * as act from "./actions.mjs";
import { isModalOpen, toast } from "./dialogs.mjs";
import { t } from "./i18n.mjs";

function typing(target) {
  if (!target || target === document.body) return false;
  const tag = target.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target.isContentEditable;
}

export function installShortcuts({ onExport, search }) {
  document.addEventListener("keydown", (e) => {
    if (isModalOpen() || e.defaultPrevented) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    if (ctrl && key === "f") {
      e.preventDefault();
      search.focus();
      return;
    }
    if (typing(e.target)) return;
    const hit = (fn) => {
      e.preventDefault();
      fn();
    };
    if (ctrl && key === "o") return hit(act.openFiles);
    if (ctrl && e.shiftKey && key === "s") return hit(() => toast(t("toolbar.scan_soon")));
    if (ctrl && key === "e") return hit(() => onExport());
    if (ctrl && key === "z" && !e.shiftKey) return hit(() => act.undo(false));
    if (ctrl && (key === "y" || (key === "z" && e.shiftKey))) return hit(() => act.undo(true));
    if (ctrl && key === "d") return hit(() => act.duplicatePages());
    if (ctrl) return;
    if (key === "r") return hit(() => act.rotate(e.shiftKey ? -90 : 90));
    if (key === "Delete") return hit(() => act.deletePages());
    if (key === "f") return hit(() => (act.runOcr(), bus.emit("show-ocr")));
    if (key === "]") return hit(() => bus.emit("ocr-next-doubtful"));
    if (key === " " && (e.target === document.body || e.target.closest?.("#preview"))) return hit(() => bus.emit("preview-toggle-zoom"));
  });
}
