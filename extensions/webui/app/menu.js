// The menu of a column or of the export button, drawn over the page just below the button that opens it.

import { copy } from "./api.js";
import { fill, h } from "./dom.js";
import { inspect } from "./inspector.js";

/** The ways to filter a column, and the words the page shows for each. */
export const OPERATORS = {
  contains: "contains",
  not_contains: "does not contain",
  starts_with: "starts with",
  equals: "=",
  not_equals: "≠",
  greater: ">",
  greater_or_equal: "≥",
  less: "<",
  less_or_equal: "≤",
  is_null: "is null",
  is_not_null: "is not null",
};

const layer = document.getElementById("menu");

/** Open a menu with `items` just below the element that was clicked. */
export function openMenu(event, items) {
  const place = event.target.getBoundingClientRect();
  const left = Math.max(8, Math.min(place.left, innerWidth - 280));
  const popover = h("div", { className: "popover", style: `left: ${left}px; top: ${place.bottom + 4}px` }, items);

  // A click anywhere else lands on the backdrop, which closes the menu.
  fill(layer, h("div", { className: "backdrop", onclick: closeMenu }), popover);
  popover.querySelector("input")?.focus();
}

export const closeMenu = () => fill(layer);

document.addEventListener("keydown", (event) => event.key === "Escape" && closeMenu());

/** The menu of a cell's export button. */
export function exportItems(cell) {
  const item = (format) => h("button", { onclick: () => cell.export(format.toLowerCase()) }, format);
  return [h("div", { className: "menu-title" }, "Export every row"), ["CSV", "Parquet", "JSON"].map(item)];
}

/** The menu of column `index` of the result a cell shows: sort, summary, and a form that adds a filter. */
export function columnItems(cell, index) {
  const { id: result, view, columns } = cell.shown;
  const { name, data_type: dataType } = columns[index];

  const sort = (descending) => () => cell.reshape({ ...view, sort: [{ column: index, descending }] });
  const summary = () => {
    closeMenu();
    inspect({ cell, result, column: index, name, dataType });
  };
  const copyName = () => {
    closeMenu();
    copy(name);
  };

  const operator = h("select", {}, Object.entries(OPERATORS).map(([op, label]) => h("option", { value: op }, label)));
  const filter = () => {
    // `is null` and `is not null` take no value.
    const value = operator.value.startsWith("is_") ? "" : input.value;
    cell.reshape({ ...view, filters: [...view.filters, { column: index, op: operator.value, value }] });
  };
  const onkeydown = (event) => event.key === "Enter" && filter();
  const input = h("input", { placeholder: "value", autocomplete: "off", onkeydown });

  const apply = h("button", { className: "btn run", onclick: filter }, "Filter");

  return [
    h("div", { className: "menu-title" }, `${name} `, h("span", { className: "dim" }, dataType)),
    h("button", { onclick: sort(false) }, "Sort ascending"),
    h("button", { onclick: sort(true) }, "Sort descending"),
    h("button", { onclick: summary }, "Summary of all rows"),
    h("button", { onclick: copyName }, "Copy name"),
    h("div", { className: "filter-form" }, operator, input, apply),
  ];
}
