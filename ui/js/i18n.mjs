// ADR-I18N: key-based strings in English, Uzbek (Latin) and Russian.

export const LANGUAGES = [
  { code: "en", name: "English" },
  { code: "uz", name: "Oʻzbekcha" },
  { code: "ru", name: "Русский" },
];

let table = {};
let fallback = {};
let current = "en";
const listeners = new Set();

/** Where the JSON files are fetched from; tests swap it. */
let loader = async (code) => {
  const response = await fetch(`i18n/${code}.json`);
  if (!response.ok) throw new Error(`language file ${code}: ${response.status}`);
  return response.json();
};

export function setLoader(fn) {
  loader = fn;
}

export function language() {
  return current;
}

/** Load a language. English is always loaded too, as the fallback. */
export async function setLanguage(code) {
  const wanted = LANGUAGES.some((l) => l.code === code) ? code : "en";
  if (!Object.keys(fallback).length) fallback = await loader("en");
  table = wanted === "en" ? fallback : await loader(wanted);
  current = wanted;
  if (typeof document !== "undefined") document.documentElement.lang = wanted;
  for (const fn of listeners) fn(wanted);
}

export function onLanguage(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Plural form index for a count. Russian has three forms, the others two. */
export function pluralIndex(code, n) {
  const k = Math.abs(Math.trunc(n));
  if (code === "ru") {
    const m10 = k % 10;
    const m100 = k % 100;
    if (m10 === 1 && m100 !== 11) return 0;
    if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return 1;
    return 2;
  }
  return k === 1 ? 0 : 1;
}

function lookup(key) {
  return table[key] ?? fallback[key];
}

/**
 * Translate `key`, filling `{name}` placeholders from `vars`. A value that is
 * an array is picked by `vars.n` (the plural forms of that language), so
 * `"pages": ["{n} page", "{n} pages"]` works in each language.
 */
export function t(key, vars = {}) {
  let value = lookup(key);
  if (value === undefined) return key;
  if (Array.isArray(value)) value = value[Math.min(pluralIndex(current, vars.n ?? 0), value.length - 1)];
  return String(value).replace(/\{(\w+)\}/g, (whole, name) => (name in vars ? String(vars[name]) : whole));
}

/** Apply `data-i18n`, `data-i18n-title` and `data-i18n-label` inside `root`. */
export function translateTree(root) {
  for (const el of root.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
  for (const el of root.querySelectorAll("[data-i18n-title]")) el.title = t(el.dataset.i18nTitle);
  for (const el of root.querySelectorAll("[data-i18n-label]")) el.setAttribute("aria-label", t(el.dataset.i18nLabel));
}
