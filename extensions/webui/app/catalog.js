// The catalog tree: attached databases, their schemas, tables and views, and the columns of each;
// and under it, how much memory DuckDB holds.

import { ask } from "./api.js";
import { fill, h } from "./dom.js";
import { compact, size } from "./format.js";
import { confirmMenu } from "./menu.js";
import { isKeyword } from "./sql.js";

const tree = document.getElementById("tree");
const memory = document.getElementById("memory");
/** The share of DuckDB's memory limit beyond which the meter turns red. */
const HIGH_MEMORY = 0.8;

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
  drawMemory(catalog.memory);

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

/** What DuckDB holds in memory, against its limit, and what it spilled to disk when memory ran short. */
function drawMemory({ used, spilled, limit }) {
  const share = limit > 0 ? Math.min(used / limit, 1) : 0;
  const bar = h("div", { style: `width: ${(share * 100).toFixed(1)}%` });
  const onDisk = spilled > 0 && ` · ${size(spilled)} on disk`;
  fill(
    memory,
    h("div", {}, "Memory ", h("b", {}, size(used)), ` of ${size(limit)}`, onDisk),
    h("div", { className: share > HIGH_MEMORY ? "meter high" : "meter" }, bar),
  );
}

/** A button of a node, shown as the pointer is over it, that runs `act` without opening or closing the node. */
function nodeButton(className, title, text, act) {
  const onclick = (event) => {
    event.stopPropagation();
    act(event);
  };
  return h("button", { className, title, onclick }, text);
}
const span = (className, text) => h("span", { className }, text);
const twist = (open) => span(open ? "twist expanded" : "twist");

function drawDatabase({ name, kind, path, read_only: readOnly }) {
  const open = expanded.has(key(name));
  const className = name === catalog.current_database ? "node current" : "node";
  const where = path === null ? "in memory" : `${readOnly ? "read-only · " : ""}${path}`;
  // Neither the database that unqualified names resolve to nor the one DuckDB was opened with is detached.
  const detachable = name !== catalog.current_database && name !== catalog.default_database;
  const detach = (event) =>
    confirmMenu(event, `Detach ${name}?`, "DuckDB forgets the database; its data stays where it is.", "Detach", () =>
      ask("detach", { database: name }).then(() => refresh(true)),
    );
  const node = h(
    "div",
    { className, title: `${kind} · ${where}`, onclick: () => toggle(name) },
    twist(open),
    span("icon database"),
    span("label", name),
    span("dim", kind),
    detachable && nodeButton("drop", "Detach this database", "✕", detach),
  );
  if (!open) return node;

  const listed = relations.get(name);
  if (listed === undefined) return [node, h("div", { className: "node level-1 dim" }, "Loading…")];
  if (listed.length === 0) return [node, h("div", { className: "node level-1 dim" }, "No tables or views")];

  // Relations come sorted by schema, and each schema gets a heading.
  const heading = ({ schema }, index) =>
    schema !== listed[index - 1]?.schema &&
    h("div", { className: "node level-1 schema" }, span("icon schema"), span("label", schema));
  // Only DuckDB's own databases drop tables here: another's would lose them at their source.
  const droppable = kind === "duckdb" && !readOnly;
  const relation = (listed, index) => [heading(listed, index), drawRelation(name, listed, droppable)];
  return [node, listed.map(relation)];
}

function drawRelation(database, { schema, name, is_view: isView, estimated_rows: rows, columns }, droppable) {
  const open = expanded.has(key(database, schema, name));
  const kind = isView ? "view" : "table";

  const preview = () => openRelation(`FROM ${identifier(database)}.${identifier(schema)}.${identifier(name)}`);
  const consequence = isView ? "Only its definition is deleted." : "Its rows are deleted.";
  const dropIt = (event) =>
    confirmMenu(event, `Drop ${kind} ${name}?`, consequence, "Drop", () =>
      ask("drop_relation", { database, schema, name, is_view: isView }).then(() => refresh(true)),
    );
  const node = h(
    "div",
    { className: "node level-2", title: kind, onclick: () => toggle(database, schema, name) },
    twist(open),
    span(`icon ${kind}`),
    span("label", name),
    span("dim", rows === null ? "" : compact(rows)),
    nodeButton("open", "Open in a new cell", "▶", preview),
    droppable && nodeButton("drop", `Drop this ${kind}`, "✕", dropIt),
  );

  const column = ({ name, data_type: dataType }) => {
    const title = `${name} ${dataType}`;
    return h("div", { className: "node level-3 column", title }, span("label", name), span("dim", dataType));
  };
  return [node, open && columns.map(column)];
}
