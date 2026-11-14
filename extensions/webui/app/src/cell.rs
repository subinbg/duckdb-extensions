//! One cell of the notebook: its toolbar, its editor, and the result of its last run.

use std::time::Duration;

use leptos::ev::MouseEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::{html, web_sys::js_sys::Date};
use webui_protocol::{ExportFormat, Outcome, Progress, Request, ResultInfo, RunResponse, SortKey, View};

use crate::Page;
use crate::api::{ask, interrupt};
use crate::editor::{Editor, selected_text};
use crate::format::{duration, grouped};
use crate::grid::{Grid, LOAD_AHEAD_ROWS};
use crate::menu::{Kind, Menu};

/// A statement that returned its first rows within this time is run again without asking.
const QUICK_MS: u64 = 1000;
const RUN_HINT: &str = "Run (Ctrl+Enter or ⌘+Enter)";
const LOAD_MORE_HINT: &str = "The statement is no longer open; this runs it again and skips the rows already loaded";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(300);

/// The message under a cell's editor: what a statement without rows did, or what went wrong.
#[derive(Clone, PartialEq)]
pub enum Note {
    Info(String),
    Error(String),
}

/// The state of one cell, in signals; copies of a cell share them.
#[derive(Clone, Copy)]
pub struct Cell {
    pub id: u32,
    pub sql: RwSignal<String>,
    pub area: NodeRef<html::Textarea>,
    /// When the cell started running, in milliseconds since the epoch.
    running: RwSignal<Option<f64>>,
    progress: RwSignal<Option<f64>>,
    /// How long the cell has been running, or how long its last run took.
    elapsed_ms: RwSignal<Option<u64>>,
    pub note: RwSignal<Option<Note>>,
    /// The result of the last run.
    base: RwSignal<Option<u64>>,
    /// The result in the grid: the one of the last run, or its rows sorted and filtered.
    pub shown: RwSignal<Option<ResultInfo>>,
    /// The number of rows of `shown`, once DuckDB has counted them.
    total: RwSignal<Option<u64>>,
    /// Whether the server is loading more rows of `shown`.
    pub loading: RwSignal<bool>,
}

fn forget(results: Vec<u64>) {
    spawn_local(async move {
        for result in results {
            let _ = ask::<()>(Request::Forget { result }).await;
        }
    });
}

impl Cell {
    pub fn new(id: u32, sql: &str) -> Cell {
        Cell {
            id,
            sql: RwSignal::new(sql.to_string()),
            area: NodeRef::new(),
            running: RwSignal::new(None),
            progress: RwSignal::new(None),
            elapsed_ms: RwSignal::new(None),
            note: RwSignal::new(None),
            base: RwSignal::new(None),
            shown: RwSignal::new(None),
            total: RwSignal::new(None),
            loading: RwSignal::new(false),
        }
    }

    /// Put the caret in the cell's editor, once the page has drawn it.
    pub fn focus(self) {
        request_animation_frame(move || {
            if let Some(area) = self.area.get_untracked() {
                area.scroll_into_view();
                let _ = area.focus();
            }
        });
    }

    /// The results the server holds for this cell: the one it ran, and its sorted or filtered form if any.
    fn held(self) -> Vec<u64> {
        let shown = self.shown.with_untracked(|shown| shown.as_ref().map(|shown| shown.id));
        let mut held: Vec<u64> = self.base.get_untracked().into_iter().chain(shown).collect();
        held.dedup();
        held
    }

    /// Whether scrolling to the end of the loaded rows may run the statement again to get more.
    /// A statement that took long is run again only when asked to.
    pub fn resumes_on_scroll(self) -> bool {
        self.elapsed_ms.get().is_some_and(|elapsed| elapsed <= QUICK_MS)
    }

    /// Mark the cell as running, clear what its last run left on the page, and follow DuckDB's progress.
    fn begin(self, page: Page) -> f64 {
        let started = Date::now();
        self.running.set(Some(started));
        self.progress.set(None);
        self.elapsed_ms.set(None);
        self.note.set(None);

        page.menu.set(None);
        page.inspected.update(|inspected| *inspected = inspected.take().filter(|inspected| inspected.cell != self.id));
        self.watch_progress(started);
        started
    }

    fn watch_progress(self, started: f64) {
        let watch = move || {
            spawn_local(async move {
                let progress = ask::<Progress>(Request::Progress).await.ok();
                if self.running.get_untracked() != Some(started) {
                    return;
                }
                self.elapsed_ms.set(Some((Date::now() - started) as u64));
                self.progress
                    .set(progress.and_then(|progress| progress.percentage).filter(|percentage| *percentage > 0.0));
                self.watch_progress(started);
            })
        };
        set_timeout(watch, PROGRESS_INTERVAL);
    }

    fn finish(self, started: f64) {
        self.running.set(None);
        self.elapsed_ms.set(Some((Date::now() - started) as u64));
    }

    /// Put a result in the grid, and have DuckDB count its rows when the server has not loaded them all.
    fn show(self, info: ResultInfo) {
        let (result, countable) = (info.id, !info.complete && info.repeatable);
        self.total.set(None);
        self.loading.set(false);
        self.shown.set(Some(info));

        if countable {
            spawn_local(async move {
                // A count that fails, as for a temporary table of the notebook's connection, is simply not shown.
                let total = ask::<u64>(Request::Count { result }).await.ok();
                if self.shows(result) {
                    self.total.set(total);
                }
            });
        }
    }

    /// Read something of `result` while the grid shows it; a result that was replaced has nothing to read.
    pub fn read<T>(self, result: u64, read: impl Fn(&ResultInfo) -> T) -> Option<T> {
        self.shown.with(|shown| shown.as_ref().filter(|shown| shown.id == result).map(read))
    }

    /// Whether the grid shows `result`; an answer about a result that was replaced is dropped.
    fn shows(self, result: u64) -> bool {
        untrack(|| self.read(result, |_| ()).is_some())
    }

    /// Run the cell: its selected text if any is selected, otherwise all of it.
    pub fn run(self, page: Page) {
        let sql = selected_text(self).unwrap_or_else(|| self.sql.get_untracked());
        if self.running.get_untracked().is_some() || sql.trim().is_empty() {
            return;
        }

        let stale = self.held();
        let started = self.begin(page);
        self.base.set(None);
        self.shown.set(None);

        spawn_local(async move {
            forget(stale);
            let response = ask::<RunResponse>(Request::Run { sql }).await;
            self.finish(started);

            let took = duration(self.elapsed_ms.get_untracked().unwrap_or(0));
            match response.map(|response| response.outcome) {
                Ok(Outcome::Rows(info)) => {
                    self.base.set(Some(info.id));
                    self.show(info);
                }
                Ok(Outcome::Changed { rows }) => {
                    self.note.set(Some(Note::Info(format!("{} rows changed · {took}", grouped(rows)))))
                }
                Ok(Outcome::Done) => self.note.set(Some(Note::Info(format!("Done · {took}")))),
                Ok(Outcome::Error { message }) | Err(message) => self.note.set(Some(Note::Error(message))),
            }

            // The statement may have created, dropped or attached something.
            page.catalog.refresh(true);
        });
    }

    /// Show the result sorted and filtered by `view`, computed by DuckDB over every row.
    pub fn reshape(self, page: Page, view: View) {
        let (Some(base), Some(shown)) = (self.base.get_untracked(), self.shown.get_untracked()) else {
            return;
        };
        if self.running.get_untracked().is_some() {
            return;
        }

        let started = self.begin(page);
        spawn_local(async move {
            let reshaped = ask::<ResultInfo>(Request::Reshape { result: base, view }).await;
            self.finish(started);

            match reshaped {
                Ok(info) => {
                    if shown.id != base && shown.id != info.id {
                        forget(vec![shown.id]);
                    }
                    self.show(info);
                }
                Err(error) => self.note.set(Some(Note::Error(error))),
            }
        });
    }

    /// A click on a column header cycles its sort: ascending, descending, none.
    /// With `keep_others`, the column is added to the columns already sorted by.
    pub fn toggle_sort(self, page: Page, column: usize, keep_others: bool) {
        let Some(mut view) = self.shown.with_untracked(|shown| shown.as_ref().map(|shown| shown.view.clone())) else {
            return;
        };
        let was = view.sort.iter().find(|key| key.column == column).map(|key| key.descending);

        if !keep_others {
            view.sort.retain(|key| key.column == column);
        }
        match was {
            None => view.sort.push(SortKey { column, descending: false }),
            Some(false) => {
                view.sort.iter_mut().filter(|key| key.column == column).for_each(|key| key.descending = true)
            }
            Some(true) => view.sort.retain(|key| key.column != column),
        }
        self.reshape(page, view);
    }

    /// Ask the server to load more rows of the result in the grid.
    /// With `resume`, it runs the statement again if it is no longer open.
    pub fn load_more(self, want: u64, resume: bool) {
        let Some(result) = self.shown.with_untracked(|shown| shown.as_ref().map(|shown| shown.id)) else {
            return;
        };
        self.loading.set(true);

        spawn_local(async move {
            let loaded = ask::<ResultInfo>(Request::Load { result, want, resume }).await;
            if !self.shows(result) {
                return;
            }
            self.loading.set(false);

            match loaded {
                Ok(info) => self.shown.set(Some(info)),
                Err(error) => {
                    // The statement was closed; the rows loaded so far stay, and more can be asked for by hand.
                    self.shown.update(|shown| shown.iter_mut().for_each(|shown| shown.streaming = false));
                    self.note.set(Some(Note::Error(format!("Loading more rows failed.\n{error}"))));
                }
            }
        });
    }

    /// Have DuckDB write every row of the result to a file, then let the browser download it.
    pub fn export(self, page: Page, format: ExportFormat) {
        let Some(result) = self.shown.with_untracked(|shown| shown.as_ref().map(|shown| shown.id)) else {
            return;
        };
        page.menu.set(None);
        self.note.set(Some(Note::Info(format!("Exporting every row as {}…", format.extension()))));

        spawn_local(async move {
            match ask::<String>(Request::Export { result, format }).await {
                Ok(link) => {
                    self.note.set(None);
                    page.download.set(Some(link));
                }
                Err(error) => self.note.set(Some(Note::Error(error))),
            }
        });
    }

    fn remove(self, page: Page) {
        forget(self.held());
        page.inspected.update(|inspected| *inspected = inspected.take().filter(|inspected| inspected.cell != self.id));
        page.cells.update(|cells| cells.retain(|cell| cell.id != self.id));
    }

    /// What the toolbar says about the run or its result.
    fn status(self) -> String {
        if self.running.get().is_some() {
            let elapsed = duration(self.elapsed_ms.get().unwrap_or(0));
            return match self.progress.get() {
                Some(percentage) => format!("Running · {elapsed} · {percentage:.0}%"),
                None => format!("Running · {elapsed}"),
            };
        }
        let Some(shown) = self.shown.get() else {
            return String::new();
        };

        let rows = match (shown.complete, self.total.get()) {
            (true, _) => format!("{} rows", grouped(shown.loaded)),
            (false, Some(total)) => format!("{} of {} rows loaded", grouped(shown.loaded), grouped(total)),
            (false, None) => format!("first {} rows", grouped(shown.loaded)),
        };
        let took = self.elapsed_ms.get().map(|elapsed| format!(" · {}", duration(elapsed))).unwrap_or_default();
        let reshaped = if shown.view.is_empty() { "" } else { " · sorted or filtered by DuckDB" };
        format!("{rows}{took}{reshaped}")
    }
}

#[component]
pub fn CellView(cell: Cell, page: Page) -> impl IntoView {
    let running = move || cell.running.get().is_some();

    // The statement is closed and took long: more rows are loaded only when asked for.
    let waits_to_load = move || {
        let closed =
            cell.shown.with(|shown| shown.as_ref().is_some_and(|shown| shown.can_load_more() && !shown.streaming));
        closed && !cell.resumes_on_scroll() && !cell.loading.get() && !running()
    };
    let load_more = move |_| {
        let loaded = cell.shown.with_untracked(|shown| shown.as_ref().map_or(0, |shown| shown.loaded));
        cell.load_more(loaded + LOAD_AHEAD_ROWS, true);
    };
    let export_menu = move |event: MouseEvent| page.menu.set(Some(Menu::below(&event, Kind::Export(cell))));
    let has_result = move || cell.shown.with(Option::is_some);
    let run =
        move || view! { <button class="btn run" title=RUN_HINT on:click=move |_| cell.run(page)>"▶ Run"</button> };

    view! {
        <section class="cell">
            <div class="cell-bar">
                <Show when=running fallback=run>
                    <button class="btn stop" title="Cancel the query" on:click=|_| interrupt()>"■ Stop"</button>
                </Show>
                <span class="status">{move || cell.status()}</span>
                <Show when=waits_to_load>
                    <button class="btn" title=LOAD_MORE_HINT on:click=load_more>"Load more rows"</button>
                </Show>
                <Show when=move || cell.loading.get() && !running()>
                    <span class="dim">"loading more rows…"</span>
                </Show>
                <span class="spacer"></span>
                <Show when=move || has_result() && !running()>
                    <button class="btn ghost" title="Download every row" on:click=export_menu>"Export ▾"</button>
                </Show>
                <button class="btn ghost" title="Delete this cell" on:click=move |_| cell.remove(page)>"✕"</button>
                <Show when=running>
                    <div class="progress" class:busy=move || cell.progress.get().is_none()>
                        <div style:width=move || cell.progress.get().map(|percent| format!("{percent:.1}%"))></div>
                    </div>
                </Show>
            </div>
            <Editor cell page/>
            {move || cell.note.get().map(|note| match note {
                Note::Error(message) => view! { <pre class="note error">{message}</pre> }.into_any(),
                Note::Info(message) => view! { <div class="note">{message}</div> }.into_any(),
            })}
            // Keyed by the result: each result gets a grid of its own, which stays while it loads more rows.
            <For each=move || cell.shown.get() key=|info| info.id let(info)>
                <Grid cell page info/>
            </For>
        </section>
    }
}
