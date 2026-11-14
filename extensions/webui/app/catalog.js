// The catalog tree: attached databases, their schemas, tables and views, and the columns of each.

import { ask } from "./api.js";
import { fill, h } from "./dom.js";
import { compact } from "./format.js";
import { isKeyword } from "./sql.js";

const tree = document.getElementById("tree");

let catalog = { databases: [] };
/** The tables and views of each database that was opened in the tree. */
const relations = new Map();
/** The opened nodes, each by its path: a database, or a database, a schema and a table or view. */
const expanded = new Set();
const key = (...path) => JSON.stringify(path);
/** Opens a table or view in a new cell; given to `start`. */
let openRelation;

/** Show the catalog and keep it fresh; `open` is called with a query when a table or view is to be opened. */
export function start(open) {
  openRelation = open;
  document.getElementById("reload").onclick = () => {
    relations.clear();
    refresh(true);
  };
  refresh(false);
}

/**
 * Fetch the list of databases again.
 * With `tables`, also fetch the tables of every database that is open in the tree, as after a cell ran;
 * without, only of those whose tables are not known yet.
 */
export async function refresh(tables) {
  const fetched = await ask("catalog").catch(() => null);
  if (fetched === null) return;

  // The database the notebook starts in is opened the first time.
  if (catalog.databases.length === 0) expanded.add(key(fetched.current_database));
  catalog = fetched;
  document.getElementById("version").textContent = catalog.duckdb_version;

  const names = catalog.databases.map((database) => database.name);
  [...relations.keys()].filter((database) => !names.includes(database)).forEach((gone) => relations.delete(gone));
  draw();
  names.filter((name) => expanded.has(key(name)) && (tables || !relations.has(name))).forEach(loadRelations);
}

async function loadRelations(database) {
  // A database that cannot be listed shows as empty, and can be retried with the reload button.
  relations.set(database, await ask("relations", { database }).catch(() => []));
  draw();
}

/** Open or close a node; a database's tables are fetched the first time it opens. */
function toggle(...path) {
  if (!expanded.delete(key(...path))) {
    expanded.add(key(...path));
    if (path.length === 1 && !relations.has(path[0])) loadRelations(path[0]);
  }
  draw();
}

/** Names the editor can complete: databases, schemas, tables, views and columns seen so far. */
export function names() {
  const seen = catalog.databases.map((database) => database.name);
  for (const { schema, name, columns } of [...relations.values()].flat()) {
    seen.push(schema, name, ...columns.map((column) => column.name));
  }
  return [...new Set(seen)].sort();
}

/** Spell a name as SQL, quoted only when it has to be. */
export function identifier(name) {
  const plain = /^[a-z_][a-z0-9_]*$/.test(name) && !isKeyword(name);
  return plain ? name : `"${name.replaceAll('"', '""')}"`;
}

const draw = () => fill(tree, catalog.databases.map(drawDatabase));
const span = (className, text) => h("span", { className }, text);
const twist = (open) => span("twist", open ? "▾" : "▸");

function drawDatabase({ name, kind, path, read_only: readOnly }) {
  const open = expanded.has(key(name));
  const className = name === catalog.current_database ? "node current" : "node";
  const where = path === null ? "in memory" : `${readOnly ? "read-only · " : ""}${path}`;
  const node = h(
    "div",
    { className, title: `${kind} · ${where}`, onclick: () => toggle(name) },
    twist(open),
    span("icon database"),
    span("label", name),
    span("dim", kind),
  );
  if (!open) return node;

  const listed = relations.get(name);
  if (listed === undefined) return [node, h("div", { className: "node level-1 dim" }, "Loading…")];
  if (listed.length === 0) return [node, h("div", { className: "node level-1 dim" }, "No tables or views")];

  // Relations come sorted by schema, and each schema gets a heading.
  const heading = ({ schema }, index) =>
    schema !== listed[index - 1]?.schema &&
    h("div", { className: "node level-1 schema" }, span("icon schema"), span("label", schema));
  return [node, listed.map((relation, index) => [heading(relation, index), drawRelation(name, relation)])];
}

function drawRelation(database, { schema, name, is_view: isView, estimated_rows: rows, columns }) {
  const open = expanded.has(key(database, schema, name));
  const kind = isView ? "view" : "table";

  // The button opens the relation in a new cell, without opening its line of the tree.
  const preview = (event) => {
    event.stopPropagation();
    openRelation(`FROM ${identifier(database)}.${identifier(schema)}.${identifier(name)}`);
  };
  const node = h(
    "div",
    { className: "node level-2", title: kind, onclick: () => toggle(database, schema, name) },
    twist(open),
    span(`icon ${kind}`),
    span("label", name),
    span("dim", rows === null ? "" : compact(rows)),
    h("button", { className: "open", title: "Open in a new cell", onclick: preview }, "▶"),
  );

  const column = ({ name, data_type: dataType }) =>
    h("div", { className: "node level-3 column", title: dataType }, span("label", name), span("dim", dataType));
  return [node, open && columns.map(column)];
}
