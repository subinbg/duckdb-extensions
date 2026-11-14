//! The browser application of the webui extension.
//!
//! It is a Rust program compiled to WebAssembly, served by the extension itself, and loads nothing from elsewhere.
//! Leptos draws the page: what the page remembers is held in signals,
//! and a part of the page is drawn again when a signal it reads changes.
//!
//! | Module      | What it draws                                                            |
//! |-------------|--------------------------------------------------------------------------|
//! | `catalog`   | The tree of databases, tables and columns beside the notebook.           |
//! | `cell`      | One cell of the notebook: its toolbar, its `editor`, and its result.     |
//! | `grid`      | The rows of a result, of which only those in view exist in the page.     |
//! | `menu`      | The menu of a column or of the export button.                            |
//! | `inspector` | The panel that shows one whole value, or the summary of one column.      |
//!
//! `api` sends every request to the extension, and `format` spells numbers.

mod api;
mod catalog;
mod cell;
mod editor;
mod format;
mod grid;
mod inspector;
mod menu;

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::{ev, logging};
use wasm_bindgen::prelude::wasm_bindgen;
use webui_protocol::{Notebook, Request};

use api::ask;
use catalog::{CatalogState, Sidebar};
use cell::{Cell, CellView};
use inspector::{Inspected, Inspector};
use menu::{Menu, MenuLayer};

const FIRST_CELL: &str =
    "-- Run with Ctrl+Enter or ⌘+Enter. Only the last statement's rows are shown.\nSHOW ALL TABLES;";
/// The notebook is saved this long after the last change to it.
const SAVE_DELAY: Duration = Duration::from_millis(700);
/// Another client of the same DuckDB process may attach or detach databases, so the catalog is read again this often.
const CATALOG_INTERVAL: Duration = Duration::from_secs(5);

/// What the parts of the page share.
#[derive(Clone, Copy)]
pub struct Page {
    pub cells: RwSignal<Vec<Cell>>,
    pub catalog: CatalogState,
    pub inspected: RwSignal<Option<Inspected>>,
    pub menu: RwSignal<Option<Menu>>,
    /// The address of an exported file; the hidden frame that opens it makes the browser download it.
    pub download: RwSignal<Option<String>>,
    /// The owner of the whole page: a cell's signals are made under it, so they outlive the handler that adds the cell.
    owner: StoredValue<Owner>,
}

impl Page {
    /// Add a cell at the end of the notebook.
    pub fn add_cell(self, sql: &str) -> Cell {
        let id = self.cells.with_untracked(|cells| cells.last().map_or(1, |last| last.id + 1));
        let cell = self.owner.with_value(|owner| owner.with(|| Cell::new(id, sql)));
        self.cells.update(|cells| cells.push(cell));
        cell
    }
}

#[component]
fn App() -> impl IntoView {
    let page = Page {
        cells: RwSignal::new(Vec::new()),
        catalog: CatalogState::new(),
        inspected: RwSignal::new(None),
        menu: RwSignal::new(None),
        download: RwSignal::new(None),
        owner: StoredValue::new(Owner::current().expect("the page has an owner")),
    };

    // The texts of the cells as last loaded or saved; nothing is saved before the notebook is loaded.
    let saved = StoredValue::new(None::<Vec<String>>);
    spawn_local(async move {
        let notebook: Notebook = ask(Request::Notebook).await.unwrap_or_default();
        let texts = match notebook.cells.is_empty() {
            true => vec![FIRST_CELL.to_string()],
            false => notebook.cells,
        };
        saved.set_value(Some(texts.clone()));
        for text in &texts {
            page.add_cell(text);
        }
    });

    let pending_save = StoredValue::new(None::<TimeoutHandle>);
    Effect::new(move |_| {
        let cells: Vec<String> = page.cells.get().iter().map(|cell| cell.sql.get()).collect();
        if saved.with_value(|saved| saved.as_ref().is_none_or(|saved| *saved == cells)) {
            return;
        }

        let save = move || {
            saved.set_value(Some(cells.clone()));
            spawn_local(async move { drop(ask::<()>(Request::SaveNotebook(Notebook { cells })).await) });
        };
        pending_save.get_value().inspect(TimeoutHandle::clear);
        pending_save.set_value(set_timeout_with_handle(save, SAVE_DELAY).ok());
    });

    page.catalog.refresh(false);
    set_interval(
        move || {
            if !document().hidden() {
                page.catalog.refresh(false)
            }
        },
        CATALOG_INTERVAL,
    );
    window_event_listener(ev::keydown, move |event| {
        if event.key() == "Escape" {
            page.menu.set(None)
        }
    });

    view! {
        <div id="app" class:with-inspector=move || page.inspected.with(Option::is_some)>
            <Sidebar page/>
            <main id="main">
                <For each=move || page.cells.get() key=|cell| cell.id let(cell)>
                    <CellView cell page/>
                </For>
                <button class="btn add" on:click=move |_| page.add_cell("").focus()>"+ Add cell"</button>
            </main>
            <Inspector page/>
            <MenuLayer page/>
            <iframe hidden src=move || page.download.get()></iframe>
        </div>
    }
}

#[component]
fn NoToken() -> impl IntoView {
    view! {
        <div class="boot">
            <h1>"DuckDB"</h1>
            <p>"This page needs the access token that is part of its address."</p>
            <p>
                "Open the address that "<code>"CALL webui_start()"</code>" returned, or run "
                <code>"CALL webui_status()"</code>" to see it again."
            </p>
        </div>
    }
}

#[wasm_bindgen(start)]
pub fn start() {
    // Without this, a panic shows in the browser's console as "unreachable" and nothing else.
    std::panic::set_hook(Box::new(|panic| logging::error!("{panic}")));

    if let Some(loading) = document().get_element_by_id("loading") {
        loading.remove();
    }
    match api::adopt_token() {
        true => leptos::mount::mount_to_body(App),
        false => leptos::mount::mount_to_body(NoToken),
    }
}
