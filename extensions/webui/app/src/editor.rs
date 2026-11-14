//! The SQL editor of a cell: a text area, and over it a coloured copy of its text.
//!
//! The browser handles typing, selection and undo in the text area, whose own text is transparent.
//! The copy is drawn again after every change, and carries the list of completions for the word at the caret.

use leptos::ev::KeyboardEvent;
use leptos::prelude::*;
use leptos::web_sys::HtmlTextAreaElement;
use webui_protocol::sql::{self, TokenKind};

use crate::Page;
use crate::catalog::identifier;
use crate::cell::Cell;

const MAX_COMPLETIONS: usize = 8;
/// Characters of a word typed before completions are offered.
const MIN_PREFIX_CHARS: usize = 2;

/// The open list of completions for the word being typed.
#[derive(Clone, PartialEq)]
struct Completion {
    /// Where the word starts in the editor's text, in bytes.
    start: usize,
    items: Vec<String>,
    chosen: usize,
}

/// The selection of a text area as byte offsets into `text`; the browser counts in UTF-16 units.
fn selection(area: &HtmlTextAreaElement, text: &str) -> (usize, usize) {
    let bytes = |units: Option<u32>| {
        let before: Vec<u16> = text.encode_utf16().take(units.unwrap_or(0) as usize).collect();
        String::from_utf16_lossy(&before).len()
    };
    (bytes(area.selection_start().ok().flatten()), bytes(area.selection_end().ok().flatten()))
}

/// The text selected in a cell's editor, if any is.
pub fn selected_text(cell: Cell) -> Option<String> {
    let text = cell.sql.get_untracked();
    let (start, end) = selection(&cell.area.get_untracked()?, &text);
    (start < end).then(|| text[start..end].to_string())
}

/// Replace bytes `start..end` of a cell's text with `inserted` and put the caret after it.
fn replace(cell: Cell, start: usize, end: usize, inserted: &str) {
    let Some(area) = cell.area.get_untracked() else {
        return;
    };
    let text = cell.sql.get_untracked();
    let units = |bytes: usize| text[..bytes].encode_utf16().count() as u32;
    let caret = units(start) + inserted.encode_utf16().count() as u32;

    let _ = area.set_range_text_with_start_and_end(inserted, units(start), units(end));
    let _ = area.set_selection_range(caret, caret);
    cell.sql.set(area.value());
}

/// Completions for the word that ends at the caret.
fn complete(cell: Cell, page: Page) -> Option<Completion> {
    let text = cell.sql.get_untracked();
    let (caret, end) = selection(&cell.area.get_untracked()?, &text);

    // The caret must be at the end of a bare word: not in a string, a comment or a quoted name.
    let token = sql::tokenize(&text).into_iter().find(|token| token.end == caret)?;
    let prefix = &text[token.start..caret];
    let is_word = matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword);
    if caret != end || !is_word || prefix.chars().count() < MIN_PREFIX_CHARS {
        return None;
    }
    let lower = prefix.to_lowercase();
    let matches = |candidate: &str| candidate.to_lowercase().starts_with(&lower) && candidate != prefix;

    // Names from the catalog come first, then keywords in the case the user is typing in.
    let typing_lowercase = !prefix.chars().any(char::is_uppercase);
    let names = page.catalog.names().into_iter().filter(|name| matches(name)).map(|name| identifier(&name));
    let keywords = sql::KEYWORDS.iter().filter(|keyword| matches(keyword)).map(|keyword| match typing_lowercase {
        true => keyword.to_lowercase(),
        false => keyword.to_string(),
    });

    let items: Vec<String> = names.chain(keywords).take(MAX_COMPLETIONS).collect();
    (!items.is_empty()).then_some(Completion { start: token.start, items, chosen: 0 })
}

#[component]
pub fn Editor(cell: Cell, page: Page) -> impl IntoView {
    let completion = RwSignal::new(None::<Completion>);

    // Replace the word being typed with the completion at `index`, or with the chosen one.
    let accept = move |index: Option<usize>| {
        let (Some(open), Some(area)) = (completion.get_untracked(), cell.area.get_untracked()) else {
            return;
        };
        let caret = selection(&area, &cell.sql.get_untracked()).0;
        replace(cell, open.start, caret, &open.items[index.unwrap_or(open.chosen)]);
        completion.set(None);
    };
    let choose = move |step: isize| {
        completion.update(|open| {
            if let Some(open) = open {
                open.chosen = (open.chosen as isize + step).rem_euclid(open.items.len() as isize) as usize;
            }
        })
    };

    let on_keydown = move |event: KeyboardEvent| {
        let open = completion.with_untracked(Option::is_some);
        let command = event.ctrl_key() || event.meta_key();

        match event.key().as_str() {
            "Enter" if command => {
                completion.set(None);
                cell.run(page);
            }
            "ArrowDown" if open => choose(1),
            "ArrowUp" if open => choose(-1),
            "Enter" | "Tab" if open => accept(None),
            "Escape" if open => completion.set(None),
            "Tab" if !event.shift_key() => {
                let Some(area) = cell.area.get_untracked() else { return };
                let (start, end) = selection(&area, &cell.sql.get_untracked());
                replace(cell, start, end, "  ");
            }
            _ => return,
        }
        event.prevent_default();
    };

    // The coloured copy: one piece per token, and the completions just before the word they are for.
    let coloured = move || {
        let text = cell.sql.get();
        let open = completion.get();
        let mut pieces = Vec::new();

        for token in sql::tokenize(&text) {
            if let Some(open) = open.as_ref().filter(|open| open.start == token.start) {
                let item = |(index, item): (usize, &String)| {
                    // The pointer is caught before it takes the caret out of the text area.
                    let pick = move |event: leptos::ev::MouseEvent| {
                        event.prevent_default();
                        accept(Some(index));
                    };
                    view! { <div class="item" class:chosen=index == open.chosen on:mousedown=pick>{item.clone()}</div> }
                };
                let items = open.items.iter().enumerate().map(item).collect_view();
                pieces.push(view! { <span class="caret"><span class="completion">{items}</span></span> }.into_any());
            }

            let class = match token.kind {
                TokenKind::Keyword => "kw",
                TokenKind::String | TokenKind::QuotedIdentifier => "str",
                TokenKind::Number => "num",
                TokenKind::Comment => "com",
                TokenKind::Operator | TokenKind::Semicolon => "op",
                TokenKind::Identifier | TokenKind::Whitespace => "",
            };
            pieces.push(view! { <span class=class>{text[token.start..token.end].to_string()}</span> }.into_any());
        }
        pieces
    };

    view! {
        <div class="editor">
            <textarea
                class="sql"
                node_ref=cell.area
                rows="1"
                spellcheck="false"
                autocapitalize="off"
                autocomplete="off"
                placeholder="SQL"
                prop:value=move || cell.sql.get()
                on:input:target=move |event| {
                    cell.sql.set(event.target().value());
                    completion.set(complete(cell, page));
                }
                on:keydown=on_keydown
                on:blur=move |_| completion.set(None)
            ></textarea>
            // The line after a final line break would take no height in the copy, unlike in the text area,
            // without the zero-width space that ends it.
            <pre class="hl" aria-hidden="true">{coloured}"\u{200b}"</pre>
        </div>
    }
}
