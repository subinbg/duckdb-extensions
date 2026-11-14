// The browser application of the webui extension: plain modules, which the extension serves as they are written.
//
// | Module      | What it draws                                                        |
// |-------------|----------------------------------------------------------------------|
// | `catalog`   | The tree of databases, tables and columns beside the notebook.       |
// | `cell`      | One cell of the notebook: its toolbar, its `editor`, and its result. |
// | `grid`      | The rows of a result, of which only those in view exist in the page. |
// | `menu`      | The menu of a column or of the export button.                        |
// | `inspector` | The panel that shows one whole value, or the summary of one column.  |
//
// `api` sends every request to the extension, `dom` makes elements, `format` spells numbers, and `sql` reads SQL.

import { ask, token } from "./api.js";
import * as catalog from "./catalog.js";
import { Cell } from "./cell.js";

const FIRST_CELL = "-- Run with Ctrl+Enter or ⌘+Enter. Only the last statement's rows are shown.\nSHOW ALL TABLES;";
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

async function start() {
  const notebook = await ask("notebook").catch(() => ({ cells: [] }));
  const texts = notebook.cells.length > 0 ? notebook.cells : [FIRST_CELL];
  texts.forEach((sql) => addCell(sql));

  // Typing in a cell, adding a cell and removing one all change the notebook.
  cells.addEventListener("input", scheduleSave);
  new MutationObserver(scheduleSave).observe(cells, { childList: true });
  document.getElementById("add").onclick = () => addCell("").focus();

  // A table or view of the catalog opens in a new cell, which runs at once.
  catalog.start((sql) => {
    const cell = addCell(sql);
    cell.focus();
    cell.run();
  });
  setInterval(() => document.hidden || catalog.refresh(false), CATALOG_INTERVAL_MS);
}

if (token === null) {
  document.getElementById("app").hidden = true;
  document.getElementById("no-token").hidden = false;
} else {
  start();
}
