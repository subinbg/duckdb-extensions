//! The menu of a column or of the export button, drawn over the page just below the button that opens it.

use leptos::ev::MouseEvent;
use leptos::html;
use leptos::prelude::*;
use leptos::web_sys::Element;
use webui_protocol::{Column, ExportFormat, Filter, FilterOp, SortKey};

use crate::Page;
use crate::api::copy;
use crate::cell::Cell;
use crate::inspector::Inspected;

#[derive(Clone)]
pub enum Kind {
    Column { cell: Cell, index: usize, column: Column },
    Export(Cell),
}

#[derive(Clone)]
pub struct Menu {
    left: f64,
    top: f64,
    kind: Kind,
}

impl Menu {
    /// A menu just below the element that was clicked.
    pub fn below(event: &MouseEvent, kind: Kind) -> Menu {
        let place = event_target::<Element>(event).get_bounding_client_rect();
        let page_width = window().inner_width().ok().and_then(|width| width.as_f64()).unwrap_or(1200.0);
        Menu { left: place.left().min(page_width - 280.0).max(8.0), top: place.bottom() + 4.0, kind }
    }
}

#[component]
pub fn MenuLayer(page: Page) -> impl IntoView {
    move || {
        page.menu.get().map(|menu| {
            let items = match menu.kind {
                Kind::Column { cell, index, column } => column_items(page, cell, index, column).into_any(),
                Kind::Export(cell) => export_items(page, cell).into_any(),
            };
            view! {
                // A click anywhere else closes the menu.
                <div class="backdrop" on:click=move |_| page.menu.set(None)></div>
                <div class="popover" style:left=format!("{}px", menu.left) style:top=format!("{}px", menu.top)>
                    {items}
                </div>
            }
        })
    }
}

fn export_items(page: Page, cell: Cell) -> impl IntoView {
    let formats = [(ExportFormat::Csv, "CSV"), (ExportFormat::Parquet, "Parquet"), (ExportFormat::Json, "JSON")];
    let item = move |(format, label)| view! { <button on:click=move |_| cell.export(page, format)>{label}</button> };
    view! {
        <div class="menu-title">"Export every row"</div>
        {formats.map(item).collect_view()}
    }
}

fn column_items(page: Page, cell: Cell, index: usize, column: Column) -> impl IntoView {
    let view_now =
        move || cell.shown.with_untracked(|shown| shown.as_ref().map(|shown| shown.view.clone())).unwrap_or_default();
    let sort = move |descending: bool| {
        move |_| {
            let mut view = view_now();
            view.sort = vec![SortKey { column: index, descending }];
            cell.reshape(page, view);
        }
    };

    let (name, data_type) = (column.name.clone(), column.data_type.clone());
    let summary = move |_| {
        let Some(result) = cell.shown.with_untracked(|shown| shown.as_ref().map(|shown| shown.id)) else { return };
        let (name, data_type) = (name.clone(), data_type.clone());
        page.inspected.set(Some(Inspected { cell: cell.id, result, column: index, name, data_type, row: None }));
        page.menu.set(None);
    };
    let name = column.name.clone();
    let copy_name = move |_| {
        copy(&name);
        page.menu.set(None);
    };

    let op = RwSignal::new(FilterOp::Contains);
    let value = RwSignal::new(String::new());
    let filter = move || {
        let op = op.get_untracked();
        let value = match op {
            FilterOp::IsNull | FilterOp::IsNotNull => String::new(),
            _ => value.get_untracked(),
        };
        let mut view = view_now();
        view.filters.push(Filter { column: index, op, value });
        cell.reshape(page, view);
    };

    let input = NodeRef::<html::Input>::new();
    Effect::new(move |_| input.get().map(|input| input.focus()));

    view! {
        <div class="menu-title">{column.name}" "<span class="dim">{column.data_type}</span></div>
        <button on:click=sort(false)>"Sort ascending"</button>
        <button on:click=sort(true)>"Sort descending"</button>
        <button on:click=summary>"Summary of all rows"</button>
        <button on:click=copy_name>"Copy name"</button>
        <div class="filter-form">
            <select on:change:target=move |event| op.set(FilterOp::ALL[event.target().selected_index() as usize].0)>
                {FilterOp::ALL.map(|(_, label)| view! { <option>{label}</option> }).collect_view()}
            </select>
            <input
                node_ref=input
                placeholder="value"
                autocomplete="off"
                on:input:target=move |event| value.set(event.target().value())
                on:keydown=move |event| if event.key() == "Enter" { filter() }
            />
            <button class="btn run" on:click=move |_| filter()>"Filter"</button>
        </div>
    }
}
