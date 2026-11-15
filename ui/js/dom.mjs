// Tiny DOM helpers. The UI never builds markup from strings: text is always set as text.

const SVG = "http://www.w3.org/2000/svg";

/** Element builder: `h("button", {class: "btn", onClick: fn}, "Label", child)`. */
export function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") el.className = value;
    else if (key === "text") el.textContent = value;
    else if (key === "dataset") Object.assign(el.dataset, value);
    else if (key === "style") Object.assign(el.style, value); // CSSOM, allowed by the CSP
    else if (key.startsWith("on") && typeof value === "function") el.addEventListener(key.slice(2).toLowerCase(), value);
    else if (key === "for") el.htmlFor = value;
    else if (["disabled", "checked", "value", "hidden", "type", "min", "max", "step", "placeholder", "title", "id", "name", "src", "alt", "rows", "tabIndex", "selected", "readOnly", "open"].includes(key)) el[key] = value;
    else el.setAttribute(key, value === true ? "" : String(value));
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

export function clear(el) {
  while (el.firstChild) el.removeChild(el.firstChild);
  return el;
}

const PATHS = {
  open: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
  scan: "M3 14h18v5H3z M6 14V6h12v8",
  ocr: "M4 7V4h3 M20 7V4h-3 M4 17v3h3 M20 17v3h-3 M8 9h8 M8 12h8 M8 15h5",
  export: "M12 3v12 M7 10l5 5 5-5 M4 20h16",
  undo: "M9 14L4 9l5-5 M4 9h10a6 6 0 0 1 0 12h-3",
  redo: "M15 14l5-5-5-5 M20 9H10a6 6 0 0 0 0 12h3",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z M21 21l-5-5",
  settings: "M4 6h10 M18 6h2 M4 12h4 M12 12h8 M4 18h12 M20 18h0 M14 4v4 M8 10v4 M16 16v4",
  tools: "M4 4h7v7H4z M13 4h7v7h-7z M4 13h7v7H4z M13 13h7v7h-7z",
  cw: "M20 11a8 8 0 1 0-2.3 5.7 M20 4v7h-7",
  ccw: "M4 11a8 8 0 1 1 2.3 5.7 M4 4v7h7",
  trash: "M4 7h16 M9 7V4h6v3 M6 7l1 13h10l1-13 M10 11v6 M14 11v6",
  copy: "M9 9h11v11H9z M5 15V4h11",
  blank: "M6 3h8l4 4v14H6z M14 3v4h4 M12 11v6 M9 14h6",
  crop: "M6 2v16h16 M2 6h16v16",
  zoomin: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z M21 21l-5-5 M11 8v6 M8 11h6",
  zoomout: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z M21 21l-5-5 M8 11h6",
  fit: "M4 9V4h5 M20 9V4h-5 M4 15v5h5 M20 15v5h-5",
  text: "M5 5h14 M12 5v14 M9 19h6",
  prev: "M15 5l-7 7 7 7",
  next: "M9 5l7 7-7 7",
  close: "M6 6l12 12 M18 6L6 18",
  check: "M5 12l5 5 10-10",
  lock: "M6 11h12v9H6z M8 11V8a4 4 0 0 1 8 0v3",
  stamp: "M4 20h16 M6 16h12v-3a3 3 0 0 0-3-3H9a3 3 0 0 0-3 3z M12 10V6",
  compress: "M4 4l6 6 M10 5v5H5 M20 4l-6 6 M14 5v5h5 M4 20l6-6 M10 19v-5H5 M20 20l-6-6 M14 19v-5h5",
  split: "M12 3v18 M7 8L3 12l4 4 M17 8l4 4-4 4",
  merge: "M6 4v6a6 6 0 0 0 6 6 M18 4v6a6 6 0 0 1-6 6 M12 16v5",
  image: "M4 5h16v14H4z M4 16l5-5 4 4 3-3 4 4 M9 9h.01",
  extract: "M5 3h10l4 4v14H5z M9 14h6 M12 11v6",
  reveal: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z M12 11v5 M10 14l2 2 2-2",
  filter: "M4 5h16l-6 8v6l-4-2v-4z",
};

/** A 24x24 stroke icon. Decorative: the button carries the accessible name. */
export function icon(name) {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(SVG, "path");
  path.setAttribute("d", PATHS[name] ?? PATHS.tools);
  svg.append(path);
  return svg;
}

let nextId = 0;
/** A unique element id for label/for wiring. */
export function uid(prefix = "f") {
  nextId += 1;
  return `${prefix}${nextId}`;
}
