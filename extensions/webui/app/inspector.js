// The panel beside the notebook: one whole value, or the summary of one column.

import { ask, copy, interrupt } from "./api.js";
import { fill, h } from "./dom.js";
import { grouped, percent } from "./format.js";

/** Characters of a value fetched when it is opened; the rest is fetched on request. */
const FIRST_CHARS = 100_000;
const ALL_CHARS = Number.MAX_SAFE_INTEGER;
/** Nodes of a nested value drawn as a tree; a larger value is better read as text. */
const MAX_TREE_NODES = 3000;
/** Levels of a nested value that start opened. */
const OPEN_LEVELS = 2;

const panel = document.getElementById("inspector");
/** What the panel shows; an answer that arrives for something else is dropped. */
let inspected = null;

/**
 * Show `{ cell, result, column, name, dataType, row }`: the value at `row`, or without `row` the summary of the column.
 * `inspect(null)` closes the panel.
 */
export function inspect(next) {
  inspected = next;
  document.getElementById("app").classList.toggle("with-inspector", next !== null);
  if (next === null) return fill(panel);

  const what = next.row === undefined ? "summary" : `row ${grouped(next.row + 1)}`;
  const dim = (text) => h("span", { className: "dim" }, text);
  const title = h("div", { className: "title" }, h("b", {}, next.name), dim(next.dataType), dim(what));
  const close = h("button", { className: "btn ghost", title: "Close", onclick: () => inspect(null) }, "✕");
  const body = h("div");
  fill(panel, h("header", {}, title, close), body);

  if (next.row === undefined) showSummary(next, body);
  else showValue(next, body, FIRST_CHARS);
}

/** Close the panel if it shows something of `cell`, whose result is going away. */
export const closeFor = (cell) => inspected?.cell === cell && inspect(null);

/** Show `waiting` in `body`, then what `draw` makes of the answer, unless the panel has moved on to something else. */
async function load(target, body, waiting, answer, draw) {
  fill(body, waiting);
  const drawn = await answer.then(draw, (error) => h("pre", { className: "error" }, error.message));
  if (inspected === target) fill(body, drawn);
}

/** A JSON number as its digits, which a float cannot hold for a HUGEINT or a DECIMAL. */
class Digits {
  constructor(source) {
    this.source = source;
  }
  toString() {
    return this.source;
  }
}

function showValue(target, body, limit) {
  const { result, row, column } = target;
  const waiting = h("div", { className: "pad dim" }, "Loading…");

  load(target, body, waiting, ask("value", { result, row, column, limit }), ({ text, length, json }) => {
    if (text === null) return h("div", { className: "pad" }, h("span", { className: "null" }, "NULL"));

    // A nested value is offered as a tree when its JSON arrived whole.
    const keepDigits = (_key, value, context) => (typeof value === "number" ? new Digits(context.source) : value);
    const tree = json === null ? undefined : JSON.parse(json, keepDigits);
    const shown = [...text].length;

    const draw = (asTree) => {
      const mode = (label, tree) => {
        const redraw = () => fill(body, draw(tree));
        return h("button", { className: tree === asTree ? "btn seg on" : "btn seg", onclick: redraw }, label);
      };
      const loadAll = h("button", { className: "btn", onclick: () => showValue(target, body, ALL_CHARS) }, "Load all");
      const rest = h("div", { className: "pad notice" }, `Showing the first ${grouped(shown)} characters. `, loadAll);
      return [
        h(
          "div",
          { className: "tools" },
          tree !== undefined && [mode("Tree", true), mode("Text", false)],
          h("button", { className: "btn", onclick: () => copy(text) }, "Copy"),
          h("span", { className: "dim" }, `${grouped(length)} characters`),
        ),
        shown < length && rest,
        asTree && tree !== undefined ? drawTree(tree) : h("pre", { className: "value-text" }, text),
      ];
    };
    return draw(true);
  });
}

/** Draw a JSON value as a tree whose objects and arrays fold; at most MAX_TREE_NODES nodes are drawn. */
function drawTree(tree) {
  let budget = MAX_TREE_NODES;
  const counted = (count, noun) => `${grouped(count)} ${noun}${count === 1 ? "" : "s"}`;

  const node = (key, value, depth) => {
    if (budget === 0) return null;
    budget -= 1;
    const label = key !== undefined && h("span", { className: "k" }, key);

    if (value === null || typeof value !== "object" || value instanceof Digits) {
      const kind = value === null ? "null" : value instanceof Digits ? "number" : typeof value;
      return h("div", { className: "leaf" }, label, h("span", { className: `v ${kind}` }, `${value ?? "NULL"}`));
    }

    const isList = Array.isArray(value);
    const parts = isList ? value.map((item, index) => [`${index + 1}`, item]) : Object.entries(value);
    const summary = isList ? `[ ] ${counted(parts.length, "item")}` : `{ } ${counted(parts.length, "field")}`;
    return h(
      "details",
      { open: depth < OPEN_LEVELS },
      h("summary", {}, label, h("span", { className: "dim" }, summary)),
      parts.map(([name, part]) => node(name, part, depth + 1)),
    );
  };

  const drawn = node(undefined, tree, 0);
  const cut = "The value has more parts than are drawn here; Text shows all of it.";
  return h("div", { className: "tree-value" }, drawn, budget === 0 && h("div", { className: "notice" }, cut));
}

function showSummary(target, body) {
  const { result, column } = target;
  const cancel = h("button", { className: "btn", onclick: interrupt }, "Cancel");
  const waiting = h("div", { className: "pad dim" }, "Computing over all rows… ", cancel);

  load(target, body, waiting, ask("summarize", { result, column }), ({ all_rows, rows, facts, top }) => {
    const most = Math.max(1, ...top.map(([, count]) => count));
    const frequent = ([value, count]) =>
      h(
        "div",
        { className: "top-row" },
        h("div", { className: "top-bar", style: `width: ${percent(count, most)}` }),
        h("span", { className: "top-value" }, value ?? h("span", { className: "null" }, "NULL")),
        h("span", { className: "dim" }, `${grouped(count)} · ${percent(count, rows)}`),
      );
    const fact = ([label, fact]) => h("tr", {}, h("th", {}, label), h("td", {}, fact));

    const partial = "Computed over the rows loaded so far, not the whole result.";
    const tooMany = "Too many distinct values to list the most frequent ones.";
    return h(
      "div",
      { className: "pad" },
      !all_rows && h("div", { className: "notice" }, partial),
      h("table", { className: "facts" }, [["Rows", grouped(rows)], ...facts].map(fact)),
      h("h4", {}, "Most frequent values"),
      top.length > 0 ? top.map(frequent) : h("div", { className: "notice" }, tooMany),
    );
  });
}
