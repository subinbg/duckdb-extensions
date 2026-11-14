//! The panel beside the notebook: one whole value, or the summary of one column.

use leptos::prelude::*;
use serde_json::Value;
use webui_protocol::{CellValue, Request, Summary};

use crate::Page;
use crate::api::{ask, copy, interrupt};
use crate::format::{grouped, percent};

/// Characters of a value fetched when it is opened; the rest is fetched on request.
const FIRST_CHARS: u64 = 100_000;
/// Nodes of a nested value drawn as a tree; a larger value is better read as text.
const MAX_TREE_NODES: usize = 3000;
/// Levels of a nested value that start opened.
const OPEN_LEVELS: usize = 2;

const TREE_CUT: &str = "The value has more parts than are drawn here; Text shows all of it.";
const PARTIAL: &str = "Computed over the rows loaded so far, not the whole result.";
const TOO_MANY: &str = "Too many distinct values to list the most frequent ones.";

/// What the panel shows: a value of a result, or the summary of one of its columns.
#[derive(Clone, PartialEq)]
pub struct Inspected {
    pub cell: u32,
    pub result: u64,
    pub column: usize,
    pub name: String,
    pub data_type: String,
    /// The row of the value; `None` shows the summary of the column.
    pub row: Option<u64>,
}

#[component]
pub fn Inspector(page: Page) -> impl IntoView {
    move || {
        page.inspected.get().map(|Inspected { result, column, name, data_type, row, .. }| {
            let (what, panel) = match row {
                Some(row) => {
                    (format!("row {}", grouped(row + 1)), view! { <ValuePanel result row column/> }.into_any())
                }
                None => ("summary".to_string(), view! { <SummaryPanel result column/> }.into_any()),
            };
            view! {
                <aside id="inspector">
                    <header>
                        <div class="title">
                            <b>{name}</b>
                            <span class="dim">{data_type}</span>
                            <span class="dim">{what}</span>
                        </div>
                        <button class="btn ghost" title="Close" on:click=move |_| page.inspected.set(None)>
                            "✕"
                        </button>
                    </header>
                    {panel}
                </aside>
            }
        })
    }
}

#[component]
fn ValuePanel(result: u64, row: u64, column: usize) -> impl IntoView {
    let limit = RwSignal::new(FIRST_CHARS);
    let value =
        LocalResource::new(move || ask::<CellValue>(Request::Value { result, row, column, limit: limit.get() }));
    let as_tree = RwSignal::new(true);

    move || match value.get() {
        None => view! { <div class="pad dim">"Loading…"</div> }.into_any(),
        Some(Err(error)) => view! { <pre class="error">{error}</pre> }.into_any(),
        Some(Ok(CellValue { text: None, .. })) => {
            view! { <div class="pad"><span class="null">"NULL"</span></div> }.into_any()
        }
        Some(Ok(CellValue { text: Some(text), length, json })) => {
            // A nested value is offered as a tree when its JSON arrived whole.
            let tree = json.and_then(|json| serde_json::from_str::<Value>(&json).ok());
            let modes = tree.is_some().then(|| {
                view! {
                    <button class="btn seg" class:on=move || as_tree.get() on:click=move |_| as_tree.set(true)>
                        "Tree"
                    </button>
                    <button class="btn seg" class:on=move || !as_tree.get() on:click=move |_| as_tree.set(false)>
                        "Text"
                    </button>
                }
            });
            let shown = text.chars().count() as u64;
            let rest = (shown < length).then(|| {
                view! {
                    <div class="pad notice">
                        "Showing the first "{grouped(shown)}" characters. "
                        <button class="btn" on:click=move |_| limit.set(u64::MAX)>"Load all"</button>
                    </div>
                }
            });

            let copied = text.clone();
            let body = move || match (&tree, as_tree.get()) {
                (Some(tree), true) => {
                    let mut budget = MAX_TREE_NODES;
                    let drawn = tree_node(None, tree, 0, &mut budget);
                    let cut = (budget == 0).then(|| view! { <div class="notice">{TREE_CUT}</div> });
                    view! { <div class="tree-value">{drawn}{cut}</div> }.into_any()
                }
                _ => view! { <pre class="value-text">{text.clone()}</pre> }.into_any(),
            };
            view! {
                <div class="tools">
                    {modes}
                    <button class="btn" on:click=move |_| copy(&copied)>"Copy"</button>
                    <span class="dim">{grouped(length)}" characters"</span>
                </div>
                {rest}
                {body}
            }
            .into_any()
        }
    }
}

/// Draw one JSON value as a node of a tree; objects and arrays fold, and `budget` bounds the nodes drawn.
fn tree_node(key: Option<String>, value: &Value, depth: usize, budget: &mut usize) -> AnyView {
    if *budget == 0 {
        return ().into_any();
    }
    *budget -= 1;
    let label = key.map(|key| view! { <span class="k">{key}</span> });
    let counted =
        |count: usize, noun: &str| format!("{} {noun}{}", grouped(count as u64), if count == 1 { "" } else { "s" });

    let (summary, children): (String, Vec<AnyView>) = match value {
        Value::Array(items) => {
            let item =
                |(index, item): (usize, &Value)| tree_node(Some((index + 1).to_string()), item, depth + 1, budget);
            (format!("[ ] {}", counted(items.len(), "item")), items.iter().enumerate().map(item).collect())
        }
        Value::Object(fields) => {
            let field = |(name, field): (&String, &Value)| tree_node(Some(name.clone()), field, depth + 1, budget);
            (format!("{{ }} {}", counted(fields.len(), "field")), fields.iter().map(field).collect())
        }
        leaf => {
            let (class, text) = match leaf {
                Value::Null => ("v null", "NULL".to_string()),
                Value::Bool(flag) => ("v bool", flag.to_string()),
                Value::String(text) => ("v str", text.clone()),
                number => ("v num", number.to_string()),
            };
            return view! { <div class="leaf">{label}<span class=class>{text}</span></div> }.into_any();
        }
    };
    view! {
        <details open=depth < OPEN_LEVELS>
            <summary>{label}<span class="dim">{summary}</span></summary>
            {children}
        </details>
    }
    .into_any()
}

#[component]
fn SummaryPanel(result: u64, column: usize) -> impl IntoView {
    let summary = LocalResource::new(move || ask::<Summary>(Request::Summarize { result, column }));

    move || match summary.get() {
        None => view! {
            <div class="pad dim">
                "Computing over all rows… "<button class="btn" on:click=|_| interrupt()>"Cancel"</button>
            </div>
        }
        .into_any(),
        Some(Err(error)) => view! { <pre class="error">{error}</pre> }.into_any(),
        Some(Ok(Summary { all_rows, rows, facts, top })) => {
            let partial = (!all_rows).then(|| view! { <div class="notice">{PARTIAL}</div> });
            let fact = |(label, fact): (String, String)| view! { <tr><th>{label}</th><td>{fact}</td></tr> };

            let most = top.iter().map(|(_, count)| *count).max().unwrap_or(0).max(1);
            let frequent = move |(value, count): (Option<String>, u64)| {
                let value = match value {
                    Some(value) => value.into_any(),
                    None => view! { <span class="null">"NULL"</span> }.into_any(),
                };
                view! {
                    <div class="top-row">
                        <div class="top-bar" style:width=percent(count, most)></div>
                        <span class="top-value">{value}</span>
                        <span class="dim">{grouped(count)}" · "{percent(count, rows)}</span>
                    </div>
                }
            };
            let top = match top.is_empty() {
                true => view! { <div class="notice">{TOO_MANY}</div> }.into_any(),
                false => top.into_iter().map(frequent).collect_view().into_any(),
            };

            view! {
                <div class="pad">
                    {partial}
                    <table class="facts">
                        <tr><th>"Rows"</th><td>{grouped(rows)}</td></tr>
                        {facts.into_iter().map(fact).collect_view()}
                    </table>
                    <h4>"Most frequent values"</h4>
                    {top}
                </div>
            }
            .into_any()
        }
    }
}
