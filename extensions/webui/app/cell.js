// One cell of the notebook: its toolbar, its editor, and the result of its last run.

import { ask, interrupt } from "./api.js";
import { refresh as refreshCatalog } from "./catalog.js";
import { fill, h } from "./dom.js";
import { Editor } from "./editor.js";
import { duration, grouped } from "./format.js";
import { Grid } from "./grid.js";
import { closeFor as closeInspector } from "./inspector.js";
import { closeMenu, exportItems, openMenu } from "./menu.js";

/** A statement that returned its first rows within this time is run again without asking. */
const QUICK_MS = 1000;
const PROGRESS_INTERVAL_MS = 300;
/** Rows the server is asked to load beyond what it has, when the grid nears their end. */
const LOAD_AHEAD_ROWS = 5000;
const LOAD_MORE_HINT = "The statement is no longer open; this runs it again and skips the rows already loaded";

const sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

/** Tell the server to stop holding results the page no longer shows. */
const forget = (results) => results.forEach((result) => ask("forget", { result }).catch(() => {}));

export class Cell {
  /** When the cell started running, in milliseconds since the epoch; null when it is not running. */
  running = null;
  /** How far DuckDB says the run is, in percent, when it says. */
  progress = null;
  /** How long the cell has been running, or how long its last run took, in milliseconds. */
  elapsed = null;
  /** The message under the editor: `{ text, error }`. */
  note = null;
  /** The result of the last run. */
  base = null;
  /** The result in the grid: the one of the last run, or its rows sorted and filtered. */
  shown = null;
  grid = null;
  /** Whether the server is loading more rows of `shown`. */
  loading = false;

  constructor(sql) {
    const button = (className, label, title, onclick) => h("button", { className, title, onclick }, label);
    this.toolbar = {
      run: button("btn run", "▶ Run", "Run (Ctrl+Enter or ⌘+Enter)", () => this.run()),
      stop: button("btn stop", "■ Stop", "Cancel the query", interrupt),
      status: h("span", { className: "status" }),
      more: button("btn", "Load more rows", LOAD_MORE_HINT, () => this.loadMore(true)),
      loading: h("span", { className: "dim" }, "loading more rows…"),
      spacer: h("span", { className: "spacer" }),
      download: button("btn ghost", "Export ▾", "Download every row", (event) => openMenu(event, exportItems(this))),
      remove: button("btn ghost", "✕", "Delete this cell", () => this.remove()),
      progress: h("div", { className: "progress" }, h("div")),
    };
    this.editor = new Editor(sql, () => this.run());
    this.noteBox = h("div");
    this.gridBox = h("div");

    const bar = h("div", { className: "cell-bar" }, Object.values(this.toolbar));
    this.element = h("section", { className: "cell" }, bar, this.editor.element, this.noteBox, this.gridBox);
    this.draw();
  }

  /** Bring the toolbar and the note up to date with the state of the cell. */
  draw() {
    const { run, stop, status, more, loading, download, progress } = this.toolbar;
    const running = this.running !== null;

    run.hidden = running;
    stop.hidden = !running;
    status.textContent = this.status();
    more.hidden = !this.waitsToLoad();
    loading.hidden = !this.loading || running;
    download.hidden = this.shown === null || running;
    progress.hidden = !running;
    progress.classList.toggle("busy", this.progress === null);
    progress.firstChild.style.width = this.progress === null ? "" : `${this.progress.toFixed(1)}%`;

    const { text, error } = this.note ?? {};
    fill(this.noteBox, text && h(error ? "pre" : "div", { className: error ? "note error" : "note" }, text));
  }

  /** What the toolbar says about the run or its result. */
  status() {
    if (this.running !== null) {
      const percent = this.progress === null ? "" : ` · ${this.progress.toFixed(0)}%`;
      return `Running · ${duration(this.elapsed)}${percent}`;
    }
    if (this.shown === null) return "";

    const { loaded, complete, view } = this.shown;
    const rows = complete ? `${grouped(loaded)} rows` : `first ${grouped(loaded)} rows`;

    const reshaped = view.sort.length + view.filters.length > 0 ? " · sorted or filtered by DuckDB" : "";
    return `${rows} · ${duration(this.elapsed)}${reshaped}`;
  }

  /** Whether the server can load more rows of the result: from the open statement, or by running it again. */
  canLoadMore() {
    const { complete, streaming, repeatable } = this.shown;
    return !complete && (streaming || repeatable);
  }

  /**
   * Whether scrolling to the end of the loaded rows may run the statement again to get more.
   * A statement that took long is run again only when asked to.
   */
  resumesOnScroll() {
    return this.elapsed <= QUICK_MS;
  }

  /** Whether the grid waits for the user to ask for more rows: the statement is closed, and it took long. */
  waitsToLoad() {
    const closed = this.shown !== null && this.canLoadMore() && !this.shown.streaming;
    return closed && !this.resumesOnScroll() && !this.loading && this.running === null;
  }

  /** The results the server holds for this cell: the one it ran, and its sorted or filtered form if any. */
  held() {
    return [...new Set([this.base, this.shown?.id])].filter((result) => result != null);
  }

  /** Put the caret in the cell's editor. */
  focus() {
    this.editor.area.scrollIntoView({ block: "nearest" });
    this.editor.area.focus();
  }

  remove() {
    forget(this.held());
    closeInspector(this);
    this.element.remove();
  }

  /** Mark the cell as running, clear what its last run left on the page, and follow DuckDB's progress. */
  begin() {
    const started = Date.now();
    Object.assign(this, { running: started, progress: null, elapsed: 0, note: null });
    closeMenu();
    closeInspector(this);
    this.draw();
    this.watchProgress(started);
    return started;
  }

  async watchProgress(started) {
    while (true) {
      await sleep(PROGRESS_INTERVAL_MS);
      const progress = await ask("progress").catch(() => null);
      if (this.running !== started) return;

      this.elapsed = Date.now() - started;
      this.progress = progress?.percentage > 0 ? progress.percentage : null;
      this.draw();
    }
  }

  finish(started) {
    this.running = null;
    this.elapsed = Date.now() - started;
    this.draw();
  }

  fail(error) {
    this.note = { text: error.message, error: true };
    this.draw();
  }

  /**
   * Put a result in the grid, or nothing.
   * Its rows are not counted until all are loaded: counting an unfinished result would read its whole source.
   */
  show(info) {
    Object.assign(this, { shown: info, loading: false });
    this.grid = info && new Grid(this, info);
    fill(this.gridBox, this.grid?.element);
    this.draw();
  }

  /** Run the cell: its selected text if any is selected, otherwise all of it. */
  async run() {
    const sql = this.editor.selected() || this.editor.area.value;
    if (this.running !== null || sql.trim() === "") return;

    forget(this.held());
    this.base = null;
    this.show(null);
    const started = this.begin();
    const failed = (error) => ({ outcome: { kind: "error", message: error.message } });
    const { outcome } = await ask("run", { sql }).catch(failed);
    this.finish(started);

    const took = duration(this.elapsed);
    if (outcome.kind === "rows") {
      this.base = outcome.id;
      this.show(outcome);
    } else if (outcome.kind === "error") {
      this.fail(outcome);
    } else {
      const changed = outcome.kind === "changed" ? `${grouped(outcome.rows)} rows changed` : "Done";
      this.note = { text: `${changed} · ${took}` };
      this.draw();
    }
    // The statement may have created, dropped or attached something.
    refreshCatalog(true);
  }

  /** Show the result sorted and filtered by `view`, computed by DuckDB over every row. */
  async reshape(view) {
    if (this.running !== null) return;
    const replaced = this.shown.id;
    const started = this.begin();
    const reshaped = await ask("reshape", { result: this.base, view }).catch((error) => this.fail(error));
    this.finish(started);
    if (reshaped === undefined) return;

    if (replaced !== this.base && replaced !== reshaped.id) forget([replaced]);
    this.show(reshaped);
  }

  /**
   * A click on a column header cycles its sort: ascending, descending, none.
   * With `keepOthers`, the column is added to the columns already sorted by.
   */
  toggleSort(column, keepOthers) {
    const { sort, filters } = this.shown.view;
    const was = sort.find((key) => key.column === column)?.descending;

    let next = sort.filter((key) => keepOthers || key.column === column);
    if (was === undefined) next.push({ column, descending: false });
    else if (was) next = next.filter((key) => key.column !== column);
    else next = next.map((key) => (key.column === column ? { column, descending: true } : key));
    this.reshape({ sort: next, filters });
  }

  /**
   * Ask the server to load more rows of the result in the grid.
   * With `resume`, it runs the statement again if it is no longer open.
   */
  async loadMore(resume) {
    const { id: result, loaded } = this.shown;
    this.loading = true;
    this.draw();
    const load = ask("load", { result, want: loaded + LOAD_AHEAD_ROWS, resume });
    const info = await load.catch((error) => error);
    if (this.shown?.id !== result) return;

    this.loading = false;
    if (info instanceof Error) {
      // The statement was closed; the rows loaded so far stay, and more can be asked for by hand.
      this.shown.streaming = false;
      this.fail(new Error(`Loading more rows failed.\n${info.message}`));
    } else {
      this.shown = info;
      this.draw();
      this.grid.draw();
    }
  }

  /** Have DuckDB write every row of the result to a file, then let the browser download it. */
  async export(format) {
    closeMenu();
    this.note = { text: `Exporting every row as ${format}…` };
    this.draw();

    const link = await ask("export", { result: this.shown.id, format }).catch((error) => this.fail(error));
    if (link === undefined) return;
    // A hidden frame that opens the link makes the browser download the file.
    document.getElementById("download").src = link;
    this.note = null;
    this.draw();
  }
}
