// Templates of the SQL that reads a source: a database to attach, a BigQuery table or query, or files.
//
// Each asks for the fields its SQL needs, and adds a cell with that SQL: a database is attached, and files and
// BigQuery become views, tables of the catalog whose rows are read only when they are queried.
// Nothing is kept, neither connection nor secret: a PostgreSQL password, for one, comes from the DuckDB process's
// ~/.pgpass or PGPASSWORD.

import { identifier } from "./catalog.js";
import { fill, h } from "./dom.js";
import { closeMenu, openMenu } from "./menu.js";
import { tokenize } from "./sql.js";

/** Text as a SQL string. */
const literal = (text) => `'${text.replaceAll("'", "''")}'`;

/** A value of a PostgreSQL connection string, quoted when it has to be. */
const conninfo = (value) => (/^[^\s'\\]+$/.test(value) ? value : `'${value.replace(/['\\]/g, "\\$&")}'`);

const NAME = (example) => ({ key: "name", label: "Attach as", value: example, required: true });
const READ_ONLY = { key: "readOnly", label: "Read only", kind: "flag", value: true };
const FILES = (example, hint) => ({
  key: "path",
  label: "Path or URL",
  example,
  required: true,
  hint: hint ?? "A path, a pattern such as data/*.parquet, or an https:// or s3:// or gs:// URL",
});
const VIEW = (example) => ({ key: "view", label: "View name", example });

/** `ATTACH` the database at `target` as the field `name`, of `type` unless it is DuckDB's. */
function attach({ name, readOnly }, target, type) {
  const options = [type && `TYPE ${type}`, readOnly && "READ_ONLY"].filter(Boolean);
  return `ATTACH ${literal(target)} AS ${identifier(name)}${options.length > 0 ? ` (${options.join(", ")})` : ""};`;
}

/** A view named `name` that reads `from`. Creating it reads only what DuckDB needs to know its columns. */
const createView = (name, from) => `CREATE OR REPLACE VIEW ${identifier(name)} AS ${from};`;

/** A name for a view of the files at `path`: their file's or folder's, or an Iceberg table's folder's. */
function nameOf(path) {
  const parts = path.replace(/\/metadata\/[^/]*$/, "").split("/").filter(Boolean);
  // A pattern, such as data/*.parquet, is named after its folder.
  const last = /[*?[]/.test(parts.at(-1) ?? "") ? parts.at(-2) : parts.at(-1);
  return (last ?? "").split(".")[0].replace(/\W+/g, "_").replace(/^_+|_+$/g, "") || "files";
}

/** A view of the files of the field `path`, read with `reader`, named by the field `view` or after the files. */
const read =
  (reader) =>
  ({ path, view }) => {
    // The aws extension signs S3 requests with the AWS credential chain: aws configure, SSO or the environment.
    const signed = path.startsWith("s3://") ? "LOAD aws;\n" : "";
    return signed + createView(view || nameOf(path), `FROM ${reader}(${literal(path)})`);
  };

/** Custom extensions are not loaded on their own. */
const bigquery = (statement) => `LOAD bigquery;\n${statement}`;

/** Groups of sources, each with its badge, the fields it asks for, and the SQL made of them. */
const SOURCES = [
  {
    group: "Databases",
    sources: [
      {
        id: "duckdb",
        badge: "DD",
        label: "DuckDB file",
        about: "Attach a .duckdb database",
        fields: [{ key: "path", label: "File", example: "/data/sales.duckdb", required: true }, NAME("db"), READ_ONLY],
        sql: (fields) => attach(fields, fields.path),
      },
      {
        id: "sqlite",
        badge: "SQ",
        label: "SQLite file",
        about: "Attach a SQLite database",
        fields: [{ key: "path", label: "File", example: "/data/app.sqlite", required: true }, NAME("lite"), READ_ONLY],
        sql: (fields) => attach(fields, fields.path, "sqlite"),
      },
      {
        id: "postgres",
        badge: "PG",
        label: "PostgreSQL",
        about: "Attach a PostgreSQL database",
        fields: [
          { key: "host", label: "Host", value: "localhost", required: true },
          { key: "port", label: "Port", value: "5432", required: true },
          { key: "dbname", label: "Database", example: "shop", required: true },
          { key: "user", label: "User", example: "postgres", required: true },
          NAME("pg"),
          READ_ONLY,
        ],
        hint: "The password comes from ~/.pgpass or PGPASSWORD of the DuckDB process, never from the notebook.",
        sql: (fields) => {
          const keys = ["host", "port", "dbname", "user"];
          return attach(fields, keys.map((key) => `${key}=${conninfo(fields[key])}`).join(" "), "postgres");
        },
      },
    ],
  },
  {
    group: "BigQuery",
    sources: [
      {
        id: "bigquery",
        badge: "BQ",
        label: "BigQuery table",
        about: "A view of a table, read through the Storage Read API",
        fields: [
          { key: "table", label: "Table", example: "project.dataset.table", required: true },
          { key: "billing", label: "Billing project", example: "optional: pays for the read" },
          { key: "filter", label: "Filter", example: "optional: GoogleSQL, such as year > 2020" },
          VIEW("optional: named after the table"),
        ],
        hint: "Creating the view reads the table's columns only; its rows are read when the view is queried.",
        sql: ({ table, billing, filter, view }) => {
          const options = [
            billing && `billing_project := ${literal(billing)}`,
            filter && `filter := ${literal(filter)}`,
          ];
          const call = [literal(table), ...options].filter(Boolean).join(", ");
          return bigquery(createView(view || table.split(".").pop(), `FROM bigquery_scan(${call})`));
        },
      },
      {
        id: "bigquery",
        badge: "BQ",
        label: "BigQuery query",
        about: "A view of GoogleSQL that BigQuery runs, over its views as well",
        fields: [
          { key: "project", label: "Project", example: "runs the query and pays for it", required: true },
          { key: "query", label: "Query", example: "SELECT * FROM dataset.view", required: true, kind: "area" },
          { key: "view", label: "View name", value: "query_result", required: true },
        ],
        hint: "Creating the view runs the query once, for its columns, and each query of the view runs it again.",
        sql: ({ project, query, view }) =>
          bigquery(createView(view, `FROM bigquery_query(${literal(project)}, ${literal(query)})`)),
      },
      {
        id: "bigquery",
        badge: "BQ",
        label: "BigQuery catalog",
        about: "List a project's datasets, or a dataset's tables",
        fields: [
          { key: "project", label: "Project", example: "bigquery-public-data", required: true },
          { key: "dataset", label: "Dataset", example: "optional: lists its tables" },
        ],
        sql: ({ project, dataset }) =>
          bigquery(
            dataset
              ? `FROM bigquery_tables(${literal(`${project}.${dataset}`)});`
              : `FROM bigquery_datasets(${literal(project)});`,
          ),
      },
    ],
  },
  {
    group: "Files",
    sources: [
      { id: "parquet", badge: "PQ", label: "Parquet", reader: "read_parquet", example: "data/*.parquet" },
      { id: "csv", badge: "CSV", label: "CSV", reader: "read_csv", example: "data/orders.csv" },
      { id: "json", badge: "JS", label: "JSON", reader: "read_json", example: "data/events.json" },
      { id: "avro", badge: "AV", label: "Avro", reader: "read_avro", example: "data/*.avro" },
      {
        id: "iceberg",
        badge: "IC",
        label: "Iceberg table",
        reader: "iceberg_scan",
        example: "warehouse/orders/metadata/v2.metadata.json",
        pathHint: "The table's metadata .json file, or its folder when that holds a version-hint.text",
      },
    ].map((file) => ({
      ...file,
      about: `A view that reads with ${file.reader}`,
      fields: [FILES(file.example, file.pathHint), VIEW("optional: named after the files")],
      hint: "Creating the view reads only what its columns need, such as a Parquet footer or a CSV file's first rows.",
      sql: read(file.reader),
    })),
  },
];

const badge = ({ id, badge }) => h("span", { className: `badge ${id}` }, badge);

/** Have `button` open the list of sources; `add(sql, run)` adds a cell with the SQL of the one filled in. */
export function start(button, add) {
  button.onclick = (event) => {
    const popover = openMenu(event, [], "popover sources");
    const choose = (source) => () => fill(popover, form(source, showList, add));
    const item = (source) =>
      h("button", { className: "source", onclick: choose(source) }, badge(source), [
        h("b", {}, source.label),
        h("small", {}, source.about),
      ]);
    const showList = () =>
      fill(
        popover,
        SOURCES.map(({ group, sources }) => [h("div", { className: "menu-title" }, group), sources.map(item)]),
      );
    showList();
  };
}

/** The fields of `source`, the SQL they make, and the buttons that add it as a cell. */
function form(source, back, add) {
  const initial = ({ kind, value }) => value ?? (kind === "flag" ? false : "");
  const values = Object.fromEntries(source.fields.map((field) => [field.key, initial(field)]));
  const empty = ({ key, required }) => required && values[key].trim() === "";

  /** The SQL of the fields, in which a required field that is empty shows as its label. */
  const sql = () => {
    const given = (field) => {
      const value = values[field.key];
      if (typeof value === "boolean") return value;
      return empty(field) ? `‹${field.label}›` : value.trim();
    };
    return source.sql(Object.fromEntries(source.fields.map((field) => [field.key, given(field)])));
  };

  const preview = h("pre", { className: "preview hl" });
  const submit = (run) => () => {
    if (source.fields.some(empty)) return;
    closeMenu();
    add(sql(), run);
  };
  const buttons = [
    h("button", { className: "btn", onclick: submit(false) }, "Add cell"),
    h("button", { className: "btn run", onclick: submit(true) }, "▶ Add and run"),
  ];
  const update = () => {
    const text = sql();
    fill(preview, tokenize(text).map(({ kind, start, end }) => h("span", { className: kind }, text.slice(start, end))));
    buttons.forEach((button) => (button.disabled = source.fields.some(empty)));
  };

  const field = ({ key, label, example, required, kind, hint }) => {
    const set = (event) => {
      values[key] = kind === "flag" ? event.target.checked : event.target.value;
      update();
    };
    // Enter adds the cell; in the query's text area, Ctrl+Enter or ⌘+Enter adds and runs it.
    const onkeydown = (event) => {
      if (event.key !== "Enter" || (kind === "area" && !event.ctrlKey && !event.metaKey)) return;
      event.preventDefault();
      submit(kind === "area")();
    };
    if (kind === "flag") {
      const box = h("input", { type: "checkbox", checked: values[key], onchange: set });
      return h("label", { className: "flag" }, box, label);
    }
    const properties = { value: values[key], placeholder: example ?? "", oninput: set, onkeydown, autocomplete: "off" };
    return h(
      "label",
      { className: "field" },
      h("span", {}, label, required && h("b", { className: "required" }, " *")),
      kind === "area" ? h("textarea", { ...properties, rows: 4, spellcheck: false }) : h("input", properties),
      hint && h("small", {}, hint),
    );
  };

  const header = h(
    "div",
    { className: "source-head" },
    h("button", { className: "btn ghost", title: "All sources", onclick: back }, "‹"),
    badge(source),
    h("b", {}, source.label),
  );
  const drawn = [
    header,
    source.fields.map(field),
    source.hint && h("div", { className: "hint" }, source.hint),
    h("div", { className: "menu-title" }, "SQL of the new cell"),
    preview,
    h("div", { className: "source-buttons" }, buttons),
  ];
  update();
  queueMicrotask(() => drawn[1][0]?.querySelector("input, textarea")?.focus());
  return drawn;
}
