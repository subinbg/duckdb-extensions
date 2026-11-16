// The result grid of a cell.
//
// Only the rows in view exist in the page.
// Scrolling draws them from pages of rows fetched on demand,
// and asks the server to load more of the result when the end of what it has loaded comes near.
// The grid therefore scrolls through a result of any size without ever holding much of it.

import { ask, copy } from "./api.js";
import { fill, h } from "./dom.js";
import { inspect } from "./inspector.js";
import { columnItems, openMenu, OPERATORS } from "./menu.js";

const PAGE_ROWS = 200;
/** Pages kept; those farthest from the rows in view are dropped first. */
const MAX_PAGES = 50;
/** Rows drawn above and below the ones in view, so a small scroll shows no gap. */
const OVERSCAN_ROWS = 8;

// These repeat sizes that style.css gives the grid.
const ROW_HEIGHT = 24;
const HEAD_HEIGHT = 40;
const ROW_NUMBER_WIDTH = 64;
/** The tallest the rows of a grid are in view. */
const VIEW_HEIGHT = 620 - HEAD_HEIGHT;
const VIEW_ROWS = Math.ceil(VIEW_HEIGHT / ROW_HEIGHT) + 1;
/** Browsers cap an element's height. Beyond this one the scroll bar maps to rows proportionally. */
const MAX_SCROLL_HEIGHT = 10_000_000;

/** The types whose values are numbers, which the grid aligns to the right. */
const NUMERIC = /^(U?(TINY|SMALL|BIG|HUGE)INT|U?INTEGER|FLOAT|DOUBLE|BIGNUM|DECIMAL\(.*\))$/;
/** How far each key moves the selected value, in rows and columns. */
const STEPS = {
  ArrowUp: [-1, 0],
  ArrowDown: [1, 0],
  ArrowLeft: [0, -1],
  ArrowRight: [0, 1],
  PageUp: [-20, 0],
  PageDown: [20, 0],
};

const clamp = (value, low, high) => Math.min(Math.max(value, low), high);

/** The rows a grid draws and where it draws them, for a number of loaded rows and a scroll position. */
function windowAt(rows, scrollTop) {
  const fullHeight = rows * ROW_HEIGHT;
  const height = Math.min(fullHeight, MAX_SCROLL_HEIGHT);
  const scaled = fullHeight > MAX_SCROLL_HEIGHT;

  const share = clamp(scrollTop / (height - VIEW_HEIGHT), 0, 1);
  const first = Math.floor(scaled ? share * (rows - VIEW_ROWS) : scrollTop / ROW_HEIGHT);
  const start = clamp(first - OVERSCAN_ROWS, 0, rows);
  const end = Math.min(first + VIEW_ROWS + OVERSCAN_ROWS, rows);

  // Where the first drawn row is, from the top of all rows.
  const top = scaled ? Math.max(scrollTop - (first - start) * ROW_HEIGHT, 0) : start * ROW_HEIGHT;
  return { start, end, top, height };
}

/** Give every column a width that suits its name, its type and the values of the first rows. */
function fitWidths(columns, sample) {
  return columns.map(({ name, data_type: dataType }, index) => {
    const longest = Math.max(0, ...sample.slice(0, 60).map((row) => Math.min(row[index]?.length ?? 0, 48)));
    return clamp(Math.max(longest * 7.9 + 22, name.length * 8.6 + 46, dataType.length * 7.6 + 38), 72, 440);
  });
}

export class Grid {
  /** Rows of the result fetched so far, by page number, and the pages being fetched. */
  pages = new Map();
  fetching = new Set();
  /** Whether the widths were fitted to the first rows, or set by the user. */
  fitted = false;
  /** The row and column of the selected value. */
  selected = null;

  /** The grid of `info`, the result that `cell` shows; the cell keeps the state of that result up to date. */
  constructor(cell, info) {
    Object.assign(this, { cell, result: info.id, columns: info.columns, view: info.view });
    this.widths = fitWidths(info.columns, []);

    this.rows = h("div", { className: "grows" });
    this.space = h("div", { className: "gspace" }, this.rows);
    const headers = info.columns.map((column, index) => this.drawHeader(column, index));
    const head = h("div", { className: "ghead" }, h("div", { className: "rn" }), headers);
    this.body = h("div", { className: "gbody", tabIndex: 0 }, head, this.space);
    this.body.onscroll = () => this.draw();
    this.body.onkeydown = (event) => this.onKey(event);

    this.grid = h("div", { className: "grid" }, this.body);
    this.element = h("div", {}, this.drawChips(), this.grid);
    this.setWidths();
    this.draw();
  }

  /** Draw the rows in view, fetch the pages they need, and load more of the result when its end is near. */
  draw() {
    const { loaded, streaming } = this.cell.shown;
    this.window = windowAt(loaded, this.body.scrollTop);
    const { start, end, top, height } = this.window;

    this.space.style.height = `${Math.max(height, ROW_HEIGHT)}px`;
    this.rows.style.top = `${top}px`;
    const rows = Array.from({ length: end - start }, (_, index) => this.drawRow(start + index));
    fill(this.rows, rows, loaded === 0 && h("div", { className: "gr none" }, "No rows"));

    for (let page = Math.floor(start / PAGE_ROWS); page * PAGE_ROWS < end; page += 1) {
      if (!this.pages.has(page) && !this.fetching.has(page)) this.fetch(page);
    }

    // Near the end of the loaded rows, more are loaded; by running the statement again only if that is quick.
    const resume = this.cell.resumesOnScroll();
    const nearEnd = end + PAGE_ROWS >= loaded;
    if (nearEnd && this.cell.canLoadMore() && (streaming || resume) && !this.cell.loading) this.cell.loadMore(resume);
  }

  drawRow(row) {
    const values = this.pages.get(Math.floor(row / PAGE_ROWS))?.[row % PAGE_ROWS];
    const value = (text, column) => {
      const numeric = NUMERIC.test(this.columns[column].data_type) ? " num" : "";
      const selected = this.selected?.[0] === row && this.selected[1] === column ? " sel" : "";
      // Line breaks and tabs are made visible, since a row is one line.
      const shown = text?.replaceAll("\n", "↵").replaceAll("\t", "→") ?? h("span", { className: "null" }, "NULL");
      return h("div", { className: `gc${numeric}${selected}`, onclick: () => this.select(row, column) }, shown);
    };
    const waiting = h("div", { className: "gc wait" }, "…");
    return h("div", { className: "gr" }, h("div", { className: "rn" }, row + 1), values?.map(value) ?? waiting);
  }

  drawHeader({ name, data_type: dataType }, index) {
    const { sort, filters } = this.view;
    const position = sort.findIndex((key) => key.column === index);
    const arrow = sort[position]?.descending ? "▼" : "▲";
    const mark = position >= 0 && h("span", { className: "sort" }, arrow, sort.length > 1 && position + 1);
    const filtered = filters.some((filter) => filter.column === index);

    const menu = (event) => {
      event.stopPropagation();
      openMenu(event, columnItems(this.cell, index));
    };
    return h(
      "div",
      {
        className: filtered ? "hc filtered" : "hc",
        title: `${name} · ${dataType}`,
        onclick: (event) => this.cell.toggleSort(index, event.shiftKey),
      },
      h("div", { className: "hn" }, name, mark),
      h("div", { className: "ht" }, dataType),
      h("button", { className: "hm", title: "Sort, filter, summary", onclick: menu }, "⋮"),
      this.drawHandle(index),
    );
  }

  /** The edge of a header, which resizes its column when dragged. */
  drawHandle(index) {
    // Where the drag started, and the width of the column then.
    let from = null;
    const handle = h("span", { className: "rz" });
    handle.onpointerdown = (event) => {
      handle.setPointerCapture(event.pointerId);
      from = [event.clientX, this.widths[index]];
    };
    handle.onpointermove = (event) => {
      if (from === null) return;
      this.fitted = true;
      this.widths[index] = Math.max(48, from[1] + event.clientX - from[0]);
      this.setWidths();
    };
    handle.onpointerup = () => (from = null);
    // The click that ends a drag is not a click on the header.
    handle.onclick = (event) => event.stopPropagation();
    return handle;
  }

  /** The filters in force, each with a button that removes it. */
  drawChips() {
    const { filters } = this.view;
    const chip = ({ column, op, value }, index) => {
      const rest = { ...this.view, filters: filters.filter((_, other) => other !== index) };
      const remove = h("button", { title: "Remove this filter", onclick: () => this.cell.reshape(rest) }, "✕");
      const name = this.columns[column].name;
      return h("span", { className: "chip" }, `${name} `, h("b", {}, OPERATORS[op]), ` ${value}`, remove);
    };
    const clear = () => this.cell.reshape({ sort: [], filters: [] });
    const clearAll = h("button", { className: "btn ghost", onclick: clear }, "Clear sort and filters");
    return filters.length > 0 && h("div", { className: "chips" }, filters.map(chip), clearAll);
  }

  setWidths() {
    this.grid.style.setProperty("--columns", this.widths.map((width) => `${width}px`).join(" "));
  }

  async fetch(page) {
    this.fetching.add(page);
    const wanted = { result: this.result, offset: page * PAGE_ROWS, limit: PAGE_ROWS };
    const rows = await ask("page", wanted).catch(() => null);
    this.fetching.delete(page);
    // The answer is dropped if it failed, or if the cell shows another result by now.
    if (rows === null || this.cell.grid !== this) return;

    this.pages.set(page, rows);
    if (page === 0 && !this.fitted) {
      this.fitted = true;
      this.widths = fitWidths(this.columns, rows);
      this.setWidths();
    }

    // Keep the pages nearest to the rows in view.
    const distance = (other) => Math.abs(other - Math.floor(this.window.start / PAGE_ROWS));
    const nearest = [...this.pages.keys()].sort((a, b) => distance(a) - distance(b));
    nearest.slice(MAX_PAGES).forEach((far) => this.pages.delete(far));
    this.draw();
  }

  /** Select a value and open it in the inspector. */
  select(row, column) {
    this.selected = [row, column];
    this.draw();
    const { name, data_type: dataType } = this.columns[column];
    inspect({ cell: this.cell, result: this.result, column, name, dataType, row });
  }

  /** The arrow keys move the selected value, and Ctrl+C copies all of it. */
  async onKey(event) {
    if (this.selected === null) return;
    const [row, column] = this.selected;

    if (event.key === "c" && (event.ctrlKey || event.metaKey)) {
      const whole = { result: this.result, row, column, limit: Number.MAX_SAFE_INTEGER };
      return copy((await ask("value", whole)).text ?? "");
    }
    const step = STEPS[event.key];
    if (step === undefined) return;
    event.preventDefault();

    const nextRow = clamp(row + step[0], 0, this.cell.shown.loaded - 1);
    const nextColumn = clamp(column + step[1], 0, this.columns.length - 1);
    this.select(nextRow, nextColumn);

    // Scroll just enough to show the value, clear of the header and the row numbers that stay in place.
    const left = this.widths.slice(0, nextColumn).reduce((sum, width) => sum + width, 0);
    const viewWidth = this.body.clientWidth - ROW_NUMBER_WIDTH;
    this.body.scrollLeft = clamp(this.body.scrollLeft, left + this.widths[nextColumn] - viewWidth, left);

    // Not when the scroll bar maps to rows proportionally.
    if (this.window.height < MAX_SCROLL_HEIGHT) {
      const top = nextRow * ROW_HEIGHT;
      const viewHeight = this.body.clientHeight - HEAD_HEIGHT;
      this.body.scrollTop = clamp(this.body.scrollTop, top + ROW_HEIGHT - viewHeight, top);
    }
  }
}
