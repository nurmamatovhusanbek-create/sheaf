// CMD-UNLOCK-PW: ask for a locked file's password. A wrong password reveals
// nothing; the dialog shakes and stays open. The password lives in this input
// until it is sent, then the field is cleared (SEC-SECRETS).

import { h } from "./dom.mjs";
import { t } from "./i18n.mjs";
import { api } from "./ipc.mjs";
import { modal, toast, showError } from "./dialogs.mjs";

const queue = [];
let showing = false;

/** Ask for the password of `source` ({id, name}); resolves to true if it was unlocked. */
export function promptUnlock(source) {
  return new Promise((resolve) => {
    queue.push({ source, resolve });
    next();
  });
}

function next() {
  if (showing || !queue.length) return;
  showing = true;
  const { source, resolve } = queue.shift();
  const input = h("input", { type: "password", autocomplete: "off", "aria-label": t("password.label"), id: "unlock-pw" });
  const message = h("div", { class: "note bad", hidden: true, role: "alert" });
  let done = false;
  const finish = (ok) => {
    if (done) return;
    done = true;
    input.value = "";
    showing = false;
    resolve(ok);
    next();
  };
  let dlg;
  const submit = async () => {
    const pw = input.value;
    input.value = "";
    message.hidden = true;
    try {
      const r = await api.unlockPw(source.id, pw);
      if (r.ok) {
        toast(t("password.unlocked", { name: source.name }));
        finish(true);
        dlg.close();
      } else {
        message.textContent = t("password.wrong");
        message.hidden = false;
        dlg.el.classList.remove("shake");
        void dlg.el.offsetWidth;
        dlg.el.classList.add("shake");
        input.focus();
      }
    } catch (e) {
      showError(e);
      input.focus();
    }
  };
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    }
  });
  dlg = modal({
    title: t("password.title"),
    body: h("div", { class: "form" }, h("div", { text: t("password.prompt", { name: source.name }) }), h("label", { class: "note", for: "unlock-pw" }, t("password.label")), input, message),
    actions: [
      { label: t("password.skip"), onClick: () => finish(false) },
      { label: t("password.unlock"), primary: true, onClick: () => { submit(); return false; } },
    ],
    onClose: () => finish(false),
  });
}
