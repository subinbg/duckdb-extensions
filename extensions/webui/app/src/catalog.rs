//! The catalog tree: attached databases, their schemas, tables and views, and the columns of each.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use webui_protocol::{Catalog, Database, Relation, Request, sql};

use crate::Page;
use crate::api::ask;
use crate::format::compact;

/// A node of the tree: a database, or a database, a schema and a table or view.
type Path = Vec<String>;

#[derive(Clone, Copy)]
pub struct CatalogState {
    catalog: RwSignal<Catalog>,
    /// The tables and views of each database that was opened in the tree.
    relations: RwSignal<HashMap<String, Vec<Relation>>>,
    /// The opened nodes.
    expanded: RwSignal<HashSet<Path>>,
}

impl CatalogState {
    pub fn new() -> CatalogState {
        CatalogState {
            catalog: RwSignal::new(Catalog::default()),
            relations: RwSignal::new(HashMap::new()),
            expanded: RwSignal::new(HashSet::new()),
        }
    }

    /// Fetch the list of databases again.
    ///
    /// With `tables`, also fetch the tables of every database that is open in the tree, as after a cell ran;
    /// without, only of those whose tables are not known yet.
    pub fn refresh(self, tables: bool) {
        spawn_local(async move {
            let Ok(catalog) = ask::<Catalog>(Request::Catalog).await else {
                return;
            };

            // The database the notebook starts in is opened the first time.
            if self.catalog.with_untracked(|known| known.databases.is_empty()) {
                self.expanded.update(|expanded| _ = expanded.insert(vec![catalog.current_database.clone()]));
            }
            let names: Vec<String> = catalog.databases.iter().map(|database| database.name.clone()).collect();
            self.relations.update(|relations| relations.retain(|database, _| names.contains(database)));
            self.catalog.set(catalog);

            let open = |database: &String| {
                self.expanded.with_untracked(|expanded| expanded.contains(std::slice::from_ref(database)))
            };
            let unknown =
                |database: &String| self.relations.with_untracked(|relations| !relations.contains_key(database));
            names
                .into_iter()
                .filter(open)
                .filter(|database| tables || unknown(database))
                .for_each(|database| self.load_relations(database));
        });
    }

    fn load_relations(self, database: String) {
        spawn_local(async move {
            // A database that cannot be listed shows as empty, and can be retried with the reload button.
            let listed: Vec<Relation> =
                ask(Request::Relations { database: database.clone() }).await.unwrap_or_default();
            self.relations.update(|relations| _ = relations.insert(database, listed));
        });
    }

    /// Open or close a node; a database's tables are fetched the first time it opens.
    fn toggle(self, path: Path) {
        let was_open = self.expanded.with_untracked(|expanded| expanded.contains(&path));
        let unknown = path.len() == 1 && self.relations.with_untracked(|relations| !relations.contains_key(&path[0]));
        if !was_open && unknown {
            self.load_relations(path[0].clone());
        }
        self.expanded.update(|expanded| _ = if was_open { expanded.remove(&path) } else { expanded.insert(path) });
    }

    /// Names the editor can complete: databases, schemas, tables, views and columns seen so far.
    pub fn names(self) -> Vec<String> {
        let mut names: Vec<String> = self
            .catalog
            .with_untracked(|catalog| catalog.databases.iter().map(|database| database.name.clone()).collect());
        self.relations.with_untracked(|relations| {
            for relation in relations.values().flatten() {
                names.extend([relation.schema.clone(), relation.name.clone()]);
                names.extend(relation.columns.iter().map(|column| column.name.clone()));
            }
        });

        names.sort();
        names.dedup();
        names
    }
}

/// Spell a name as SQL, quoted only when it has to be.
pub fn identifier(name: &str) -> String {
    let plain = name.starts_with(|first: char| first.is_ascii_lowercase() || first == '_')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !sql::is_keyword(name);
    match plain {
        true => name.to_string(),
        false => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

fn twist(open: bool) -> &'static str {
    if open { "▾" } else { "▸" }
}

#[component]
pub fn Sidebar(page: Page) -> impl IntoView {
    let state = page.catalog;
    let reload = move |_| {
        state.relations.set(HashMap::new());
        state.refresh(true);
    };

    view! {
        <aside id="sidebar">
            <header>
                <span class="logo"></span>
                <b>"DuckDB"</b>
                <span class="dim">{move || state.catalog.with(|catalog| catalog.duckdb_version.clone())}</span>
                <button class="btn ghost" title="Reload the catalog" on:click=reload>"⟳"</button>
            </header>
            <div id="tree">
                {move || {
                    let databases = state.catalog.get().databases;
                    databases.into_iter().map(|database| database_view(page, database)).collect_view()
                }}
            </div>
        </aside>
    }
}

fn database_view(page: Page, database: Database) -> impl IntoView {
    let state = page.catalog;
    let path = vec![database.name.clone()];
    let open = state.expanded.with(|expanded| expanded.contains(&path));
    let current = state.catalog.with(|catalog| catalog.current_database == database.name);
    let detail = match (&database.path, database.read_only) {
        (Some(path), true) => format!("{} · read-only · {path}", database.kind),
        (Some(path), false) => format!("{} · {path}", database.kind),
        (None, _) => format!("{} · in memory", database.kind),
    };

    let relations = open.then(|| state.relations.with(|relations| relations.get(&database.name).cloned()));
    let children = match relations {
        None => ().into_any(),
        Some(None) => view! { <div class="node level-1 dim">"Loading…"</div> }.into_any(),
        Some(Some(relations)) if relations.is_empty() => {
            view! { <div class="node level-1 dim">"No tables or views"</div> }.into_any()
        }
        Some(Some(relations)) => {
            let mut schema = None;
            let mut nodes = Vec::new();
            for relation in relations {
                // Relations come sorted by schema, and each schema gets a heading.
                if schema.as_ref() != Some(&relation.schema) {
                    schema = Some(relation.schema.clone());
                    let heading = view! {
                        <div class="node level-1 schema">
                            <span class="icon schema"></span>
                            <span class="label">{relation.schema.clone()}</span>
                        </div>
                    };
                    nodes.push(heading.into_any());
                }
                nodes.push(relation_view(page, &database.name, relation).into_any());
            }
            nodes.into_any()
        }
    };

    view! {
        <div class="node" class:current=current title=detail on:click=move |_| state.toggle(path.clone())>
            <span class="twist">{twist(open)}</span>
            <span class="icon database"></span>
            <span class="label">{database.name.clone()}</span>
            <span class="dim">{database.kind.clone()}</span>
        </div>
        {children}
    }
}

fn relation_view(page: Page, database: &str, relation: Relation) -> impl IntoView {
    let state = page.catalog;
    let path = vec![database.to_string(), relation.schema.clone(), relation.name.clone()];
    let open = state.expanded.with(|expanded| expanded.contains(&path));
    let kind = if relation.is_view { "view" } else { "table" };
    let query =
        format!("FROM {}.{}.{}", identifier(database), identifier(&relation.schema), identifier(&relation.name));

    // The button opens the relation in a new cell and runs it.
    let preview = move |event: leptos::ev::MouseEvent| {
        event.stop_propagation();
        let cell = page.add_cell(&query);
        cell.run(page);
        cell.focus();
    };
    let column = |column: webui_protocol::Column| {
        let title = column.data_type.clone();
        view! {
            <div class="node level-3 column" title=title>
                <span class="label">{column.name}</span>
                <span class="dim">{column.data_type}</span>
            </div>
        }
    };
    let columns = open.then(|| relation.columns.into_iter().map(column).collect_view());

    view! {
        <div class="node level-2" title=kind on:click=move |_| state.toggle(path.clone())>
            <span class="twist">{twist(open)}</span>
            <span class=format!("icon {kind}")></span>
            <span class="label">{relation.name}</span>
            <span class="dim">{relation.estimated_rows.map(compact)}</span>
            <button class="open" title="Open in a new cell" on:click=preview>"▶"</button>
        </div>
        {columns}
    }
}
