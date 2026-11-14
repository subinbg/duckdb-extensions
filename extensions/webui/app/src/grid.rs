//! The result grid of a cell.
//!
//! Only the rows in view exist in the page.
//! Scrolling draws them from pages of rows fetched on demand,
//! and asks the server to load more of the result when the end of what it has loaded comes near.
//! The grid therefore scrolls through a result of any size without ever holding much of it.

use std::collections::{HashMap, HashSet};

use leptos::ev::{KeyboardEvent, MouseEvent, PointerEvent};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::web_sys::Element;
use webui_protocol::{CELL_PREVIEW_CHARS, Column, Request, ResultInfo, Rows, View};

use crate::Page;
use crate::api::{ask, copy};
use crate::cell::Cell;
use crate::inspector::Inspected;
use crate::menu::{Kind, Menu};

/// Rows the server is asked to load beyond what it has, when the grid nears their end.
pub const LOAD_AHEAD_ROWS: u64 = 5000;
const PAGE_ROWS: u64 = 200;
/// Pages kept; those farthest from the rows in view are dropped first.
const MAX_PAGES: usize = 50;
/// Rows drawn above and below the ones in view, so a small scroll shows no gap.
const OVERSCAN_ROWS: u64 = 8;

// These repeat sizes that style.css gives the grid.
const ROW_HEIGHT: f64 = 24.0;
const HEAD_HEIGHT: f64 = 40.0;
const ROW_NUMBER_WIDTH: f64 = 64.0;
/// The tallest the rows of a grid are in view.
const VIEW_HEIGHT: f64 = 620.0 - HEAD_HEIGHT;
const VIEW_ROWS: u64 = (VIEW_HEIGHT / ROW_HEIGHT) as u64 + 2;
/// Browsers cap an element's height. Beyond this one the scroll bar maps to rows proportionally.
const MAX_SCROLL_HEIGHT: f64 = 10_000_000.0;

const NUMERIC_TYPES: [&str; 14] = [
    "TINYINT",
    "SMALLINT",
    "INTEGER",
    "BIGINT",
    "HUGEINT",
    "UTINYINT",
    "USMALLINT",
    "UINTEGER",
    "UBIGINT",
    "UHUGEINT",
    "FLOAT",
    "DOUBLE",
    "BIGNUM",
    "DECIMAL",
];

/// The rows a grid draws and where it draws them, for a number of loaded rows and a scroll position.
#[derive(Clone, Copy, PartialEq)]
struct Window {
    start: u64,
    end: u64,
    /// Where the first drawn row is, from the top of all rows.
    top: f64,
    /// The height all rows take.
    height: f64,
}

impl Window {
    fn at(rows: u64, scroll_top: f64) -> Window {
        let full_height = rows as f64 * ROW_HEIGHT;
        let height = full_height.min(MAX_SCROLL_HEIGHT);
        let scaled = full_height > MAX_SCROLL_HEIGHT;

        let first = match scaled {
            false => (scroll_top / ROW_HEIGHT) as u64,
            true => {
                ((scroll_top / (height - VIEW_HEIGHT)).clamp(0.0, 1.0) * rows.saturating_sub(VIEW_ROWS) as f64) as u64
            }
        };
        let start = first.saturating_sub(OVERSCAN_ROWS).min(rows);
        let top = match scaled {
            false => start as f64 * ROW_HEIGHT,
            true => (scroll_top - (first - start) as f64 * ROW_HEIGHT).max(0.0),
        };
        Window { start, end: (first + VIEW_ROWS + OVERSCAN_ROWS).min(rows), top, height }
    }
}

/// Give every column a width that suits its name and the values of the first rows.
fn fit_widths(columns: &[Column], sample: &[Vec<Option<String>>]) -> Vec<f64> {
    let width = |(index, column): (usize, &Column)| {
        let values = sample.iter().take(60).filter_map(|row| row.get(index)?.as_ref());
        let longest = values.map(|text| text.chars().count().min(48)).max().unwrap_or(0);
        let header = column.name.chars().count() as f64 * 7.8 + 46.0;
        (longest as f64 * 7.4 + 22.0).max(header).clamp(72.0, 420.0)
    };
    columns.iter().enumerate().map(width).collect()
}

/// One value as the grid shows it: cut to the preview length, with line breaks and tabs made visible.
fn preview(value: Option<String>) -> AnyView {
    let Some(text) = value else {
        return view! { <span class="null">"NULL"</span> }.into_any();
    };
    let cut = text.chars().count() > CELL_PREVIEW_CHARS;
    let shown: String = text.chars().take(CELL_PREVIEW_CHARS).chain(cut.then_some('…')).collect();
    shown.replace('\n', "↵").replace('\t', "→").into_any()
}

#[component]
pub fn Grid(cell: Cell, page: Page, info: ResultInfo) -> impl IntoView {
    let result = info.id;
    let ResultInfo { columns, view, .. } = info;
    let numeric: Vec<bool> =
        columns.iter().map(|column| NUMERIC_TYPES.iter().any(|name| column.data_type.starts_with(name))).collect();
    let (columns, numeric) = (StoredValue::new(columns), StoredValue::new(numeric));

    let body = NodeRef::<html::Div>::new();
    let scroll_top = RwSignal::new(0.0);
    let pages = RwSignal::new(HashMap::<u64, Rows>::new());
    let fetching = StoredValue::new(HashSet::<u64>::new());
    let widths = RwSignal::new(columns.with_value(|columns| fit_widths(columns, &[])));
    // Whether the widths were fitted to the first rows, or set by the user.
    let fitted = StoredValue::new(false);
    let selected = RwSignal::new(None::<(u64, usize)>);

    let loaded = Memo::new(move |_| cell.read(result, |shown| shown.loaded).unwrap_or(0));
    let window = Memo::new(move |_| Window::at(loaded.get(), scroll_top.get()));

    let fetch = move |number: u64| {
        spawn_local(async move {
            let fetched = ask::<Rows>(Request::Page { result, offset: number * PAGE_ROWS, limit: PAGE_ROWS }).await;
            fetching.try_update_value(|fetching| fetching.remove(&number));
            let Ok(rows) = fetched else { return };

            if number == 0 && fitted.try_get_value() == Some(false) {
                fitted.set_value(true);
                widths.set(columns.with_value(|columns| fit_widths(columns, &rows)));
            }
            pages.try_update(|pages| {
                pages.insert(number, rows);

                // Keep the pages nearest to the rows in view.
                let current = window.get_untracked().start / PAGE_ROWS;
                let mut numbers: Vec<u64> = pages.keys().copied().collect();
                numbers.sort_by_key(|number| number.abs_diff(current));
                numbers.into_iter().skip(MAX_PAGES).for_each(|far| drop(pages.remove(&far)));
            });
        });
    };

    // Fetch the pages of the rows in view.
    Effect::new(move |_| {
        let window = window.get();
        let numbers = window.start / PAGE_ROWS..window.end.div_ceil(PAGE_ROWS);
        let missing = |number: &u64| pages.with_untracked(|pages| !pages.contains_key(number));
        numbers
            .filter(missing)
            .filter(|number| fetching.try_update_value(|fetching| fetching.insert(*number)) == Some(true))
            .for_each(fetch);
    });

    // Near the end of the loaded rows, more are loaded; by running the statement again only if that is quick.
    Effect::new(move |_| {
        let resume = cell.resumes_on_scroll();
        let can_load = cell.read(result, |shown| shown.can_load_more() && (shown.streaming || resume)) == Some(true);
        if can_load && !cell.loading.get() && window.get().end + PAGE_ROWS >= loaded.get() {
            cell.load_more(loaded.get_untracked() + LOAD_AHEAD_ROWS, resume);
        }
    });

    // Select a value and open it in the inspector.
    let select = move |row: u64, column: usize| {
        selected.set(Some((row, column)));
        let Column { name, data_type } = columns.with_value(|columns| columns[column].clone());
        page.inspected.set(Some(Inspected { cell: cell.id, result, column, name, data_type, row: Some(row) }));
    };

    // The arrow keys move the selected value, and Ctrl+C copies all of it.
    let on_keydown = move |event: KeyboardEvent| {
        let (Some((row, column)), Some(body)) = (selected.get_untracked(), body.get_untracked()) else { return };
        let (rows, columns) = match event.key().as_str() {
            "ArrowUp" => (-1, 0),
            "ArrowDown" => (1, 0),
            "ArrowLeft" => (0, -1),
            "ArrowRight" => (0, 1),
            "PageUp" => (-20, 0),
            "PageDown" => (20, 0),
            "c" if event.ctrl_key() || event.meta_key() => {
                let whole = Request::Value { result, row, column, limit: u64::MAX };
                return spawn_local(async move {
                    drop(ask(whole).await.map(|value: webui_protocol::CellValue| copy(&value.text.unwrap_or_default())))
                });
            }
            _ => return,
        };
        event.prevent_default();
        let row = row.saturating_add_signed(rows).min(loaded.get_untracked().saturating_sub(1));
        let column = column.saturating_add_signed(columns).min(widths.with_untracked(Vec::len) - 1);
        select(row, column);

        // Scroll just enough to show the value, clear of the header and the row numbers that stay in place.
        let (left, width) = widths.with_untracked(|widths| (widths[..column].iter().sum::<f64>(), widths[column]));
        let view_width = body.client_width() as f64 - ROW_NUMBER_WIDTH;
        let scroll_left = (body.scroll_left() as f64).clamp(left + width - view_width, left);
        body.set_scroll_left(scroll_left as i32);

        // Not when the scroll bar maps to rows proportionally.
        if window.get_untracked().height < MAX_SCROLL_HEIGHT {
            let top = row as f64 * ROW_HEIGHT;
            let view_height = body.client_height() as f64 - HEAD_HEIGHT;
            body.set_scroll_top((body.scroll_top() as f64).clamp(top + ROW_HEIGHT - view_height, top) as i32);
        }
    };

    // A column is resized by dragging the edge of its header: the column, where the drag started, and its width then.
    let drag = StoredValue::new(None::<(usize, f64, f64)>);
    let on_drag = move |event: PointerEvent| {
        if let Some((column, from, width)) = drag.get_value() {
            fitted.set_value(true);
            widths.update(|widths| widths[column] = (width + event.client_x() as f64 - from).max(48.0));
        }
    };

    let chips = view! { <Chips cell page view=view.clone() columns=columns.get_value()/> };
    let header = move |(index, column): (usize, Column)| {
        let title = format!("{} · {}", column.name, column.data_type);
        let position = view.sort.iter().position(|key| key.column == index);
        let mark = position.map(|position| {
            let arrow = if view.sort[position].descending { "▼" } else { "▲" };
            let rank = (view.sort.len() > 1).then(|| position + 1);
            view! { <span class="sort">{arrow}{rank}</span> }
        });
        let filtered = view.filters.iter().any(|filter| filter.column == index);

        let menu_column = column.clone();
        let open_menu = move |event: MouseEvent| {
            event.stop_propagation();
            page.menu.set(Some(Menu::below(&event, Kind::Column { cell, index, column: menu_column.clone() })));
        };
        let start_drag = move |event: PointerEvent| {
            let _ = event_target::<Element>(&event).set_pointer_capture(event.pointer_id());
            drag.set_value(Some((index, event.client_x() as f64, widths.with_untracked(|widths| widths[index]))));
        };

        view! {
            <div
                class="hc"
                class:filtered=filtered
                title=title
                on:click=move |event| cell.toggle_sort(page, index, event.shift_key())
            >
                <div class="hn">{column.name}{mark}</div>
                <div class="ht">{column.data_type}</div>
                <button class="hm" title="Sort, filter, summary" on:click=open_menu>"⋮"</button>
                <span
                    class="rz"
                    on:pointerdown=start_drag
                    on:pointermove=on_drag
                    on:pointerup=move |_| drag.set_value(None)
                    on:click=|event| event.stop_propagation()
                ></span>
            </div>
        }
    };

    let row = move |row: u64| {
        let values =
            move || pages.with(|pages| pages.get(&(row / PAGE_ROWS))?.get((row % PAGE_ROWS) as usize).cloned());
        let value = move |(column, value): (usize, Option<String>)| {
            view! {
                <div
                    class="gc"
                    class:num=numeric.with_value(|numeric| numeric[column])
                    class:sel=move || selected.get() == Some((row, column))
                    on:click=move |_| select(row, column)
                >
                    {preview(value)}
                </div>
            }
        };
        view! {
            <div class="gr">
                <div class="rn">{row + 1}</div>
                {move || match values() {
                    Some(values) => values.into_iter().enumerate().map(value).collect_view().into_any(),
                    None => view! { <div class="gc wait">"…"</div> }.into_any(),
                }}
            </div>
        }
    };

    let column_widths =
        move || widths.with(|widths| widths.iter().map(|width| format!("{width}px ")).collect::<String>());
    view! {
        {chips}
        <div class="grid" style=("--columns", column_widths)>
            <div
                class="gbody"
                tabindex="0"
                node_ref=body
                on:keydown=on_keydown
                on:scroll:target=move |event| scroll_top.set(event.target().scroll_top() as f64)
            >
                <div class="ghead">
                    <div class="rn"></div>
                    {columns.get_value().into_iter().enumerate().map(header).collect_view()}
                </div>
                <div class="gspace" style:height=move || format!("{}px", window.get().height.max(ROW_HEIGHT))>
                    <div class="grows" style:top=move || format!("{}px", window.get().top)>
                        <For each=move || window.get().start..window.get().end key=|row| *row children=row/>
                        <Show when=move || loaded.get() == 0>
                            <div class="gr none">"No rows"</div>
                        </Show>
                    </div>
                </div>
            </div>
        </div>
    }
}

/// The filters in force, each with a button that removes it.
#[component]
fn Chips(cell: Cell, page: Page, view: View, columns: Vec<Column>) -> impl IntoView {
    let without = |view: &View, index: usize| {
        let mut view = view.clone();
        view.filters.remove(index);
        view
    };
    let chip = |(index, filter): (usize, &webui_protocol::Filter)| {
        let rest = without(&view, index);
        view! {
            <span class="chip">
                {columns[filter.column].name.clone()}" "<b>{filter.op.label()}</b>" "{filter.value.clone()}
                <button title="Remove this filter" on:click=move |_| cell.reshape(page, rest.clone())>"✕"</button>
            </span>
        }
    };

    (!view.filters.is_empty()).then(|| {
        view! {
            <div class="chips">
                {view.filters.iter().enumerate().map(chip).collect_view()}
                <button class="btn ghost" on:click=move |_| cell.reshape(page, View::default())>
                    "Clear sort and filters"
                </button>
            </div>
        }
    })
}
