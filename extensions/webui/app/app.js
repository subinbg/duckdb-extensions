// The browser application of the webui extension: plain modules, which the extension serves as they are written.
//
// | Module      | What it draws                                                        |
// |-------------|----------------------------------------------------------------------|
// | `catalog`   | The tree of databases, tables and columns beside the notebook.       |
// | `cell`      | One cell of the notebook: its toolbar, its `editor`, and its result. |
// | `grid`      | The rows of a result, of which only those in view exist in the page. |
// | `menu`      | The menu of a column or of the export button.                        |
// | `inspector` | The panel that shows one whole value, or the summary of one column.  |
// | `templates` | The SQL that reads a source, made from the fields the user fills in. |
//
// `api` sends every request to the extension, `dom` makes elements, `format` spells numbers, and `sql` reads SQL.

import { ask, token } from "./api.js";
import * as catalog from "./catalog.js";
import { Cell } from "./cell.js";
import * as templates from "./templates.js";

const FIRST_CELL = "-- Run with Ctrl+Enter or ⌘+Enter. Only the last statement's rows are shown.\nSHOW ALL TABLES;";
/** The narrowest a side pane is dragged to, and the share of the window it may take at most. */
const MIN_PANE_PX = 180;
const MAX_PANE_SHARE = 0.5;
/** The notebook is saved this long after the last change to it. */
const SAVE_DELAY_MS = 700;
/** Another client of the same DuckDB process may attach or detach databases, so the catalog is read again. */
const CATALOG_INTERVAL_MS = 5000;

const cells = document.getElementById("cells");

/** Add a cell at the end of the notebook. */
function addCell(sql) {
  const cell = new Cell(sql);
  cells.append(cell.element);
  return cell;
}

/** Save the texts of the cells, shortly after the last change to them. */
let saving;
function scheduleSave() {
  const texts = () => Array.from(cells.querySelectorAll("textarea"), (area) => area.value);
  clearTimeout(saving);
  saving = setTimeout(() => ask("save_notebook", { cells: texts() }).catch(() => {}), SAVE_DELAY_MS);
}

/**
 * Let `handle` drag the width `property` of the page, which `width` reads from where the pointer is.
 * This browser remembers the width: it is the viewer's, not the notebook's.
 */
function resizable(handle, property, width) {
  const app = document.getElementById("app");
  // The browser may refuse storage; the width then lasts as long as the page.
  const store = (act) => {
    try {
      return act(localStorage);
    } catch {
      return null;
    }
  };
  const saved = store((storage) => storage.getItem(property));
  if (saved) app.style.setProperty(property, saved);

  handle.onpointerdown = (event) => {
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    handle.classList.add("dragging");
    handle.onpointermove = ({ clientX }) => {
      const pixels = Math.min(Math.max(width(clientX), MIN_PANE_PX), innerWidth * MAX_PANE_SHARE);
      app.style.setProperty(property, `${Math.round(pixels)}px`);
    };
    handle.onpointerup = () => {
      handle.onpointermove = null;
      handle.classList.remove("dragging");
      store((storage) => storage.setItem(property, app.style.getPropertyValue(property)));
    };
  };
}

async function start() {
  resizable(document.getElementById("sidebar-resizer"), "--sidebar-width", (x) => x);
  resizable(document.getElementById("inspector-resizer"), "--inspector-width", (x) => innerWidth - x);

  const notebook = await ask("notebook").catch(() => ({ cells: [] }));
  const texts = notebook.cells.length > 0 ? notebook.cells : [FIRST_CELL];
  texts.forEach((sql) => addCell(sql));

  // Typing in a cell, adding a cell and removing one all change the notebook.
  cells.addEventListener("input", scheduleSave);
  new MutationObserver(scheduleSave).observe(cells, { childList: true });
  document.getElementById("add").onclick = () => addCell("").focus();

  // A table or view of the catalog opens in a new cell, which runs at once; a template's cell runs if asked to.
  const open = (sql, run = true) => {
    const cell = addCell(sql);
    cell.focus();
    if (run) cell.run();
  };
  catalog.start(open);
  templates.start(document.getElementById("add-source"), open);
  setInterval(() => document.hidden || catalog.refresh(false), CATALOG_INTERVAL_MS);
}

if (token === null) {
  document.getElementById("app").hidden = true;
  document.getElementById("no-token").hidden = false;
} else {
  start();
}
