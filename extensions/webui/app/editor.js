// The SQL editor of a cell: a text area, and over it a coloured copy of its text.
//
// The browser handles typing, selection and undo in the text area, whose own text is transparent.
// The copy is drawn again after every change, and carries the list of completions for the word at the caret.

import { identifier, names } from "./catalog.js";
import { fill, h } from "./dom.js";
import { KEYWORDS, tokenize } from "./sql.js";

const MAX_COMPLETIONS = 8;
/** Characters of a word typed before completions are offered. */
const MIN_PREFIX_CHARS = 2;

export class Editor {
  /** The open list of completions: where the word being typed starts, the items, and which one is chosen. */
  completion = null;

  /** `run` is called when the keys that run a cell are pressed. */
  constructor(sql, run) {
    this.run = run;
    this.area = h("textarea", { className: "sql", value: sql, rows: 1, placeholder: "SQL", spellcheck: false });
    this.area.oninput = (event) => {
      // Completions are offered for what a person types, not for what `replace` inserts.
      this.completion = event.isTrusted ? this.complete() : null;
      this.draw();
    };
    this.area.onkeydown = (event) => this.onKey(event);
    this.area.onblur = () => this.close();

    this.copy = h("pre", { className: "hl", ariaHidden: "true" });
    this.element = h("div", { className: "editor" }, this.area, this.copy);
    this.draw();
  }

  /** The text selected in the editor; empty when none is. */
  selected() {
    return this.area.value.slice(this.area.selectionStart, this.area.selectionEnd);
  }

  /** Replace characters `start..end` of the text with `inserted`, put the caret after it, and say so as typing does. */
  replace(start, end, inserted) {
    this.area.setRangeText(inserted, start, end, "end");
    this.area.dispatchEvent(new Event("input", { bubbles: true }));
  }

  close() {
    this.completion = null;
    this.draw();
  }

  /** Completions for the word that ends at the caret. */
  complete() {
    const text = this.area.value;
    const caret = this.area.selectionStart;

    // The caret must be at the end of a bare word: not in a string, a comment or a quoted name.
    const word = tokenize(text).find((token) => token.end === caret && ["word", "keyword"].includes(token.kind));
    if (word === undefined || this.area.selectionEnd !== caret || caret - word.start < MIN_PREFIX_CHARS) return null;
    const prefix = text.slice(word.start, caret);
    const matches = (candidate) => candidate.toLowerCase().startsWith(prefix.toLowerCase()) && candidate !== prefix;

    // Names from the catalog come first, then keywords in the case the user is typing in.
    const lowercase = prefix === prefix.toLowerCase();
    const keywords = KEYWORDS.filter(matches).map((keyword) => (lowercase ? keyword.toLowerCase() : keyword));
    const items = [...names().filter(matches).map(identifier), ...keywords].slice(0, MAX_COMPLETIONS);
    return items.length > 0 ? { start: word.start, items, chosen: 0 } : null;
  }

  /** Replace the word being typed with one of the completions. */
  accept(index) {
    const { start, items } = this.completion;
    this.replace(start, this.area.selectionStart, items[index]);
  }

  onKey(event) {
    const { key } = event;
    const open = this.completion !== null;

    if (key === "Enter" && (event.ctrlKey || event.metaKey)) {
      this.close();
      this.run();
    } else if (open && (key === "ArrowDown" || key === "ArrowUp")) {
      const { items, chosen } = this.completion;
      this.completion.chosen = (chosen + (key === "ArrowDown" ? 1 : items.length - 1)) % items.length;
      this.draw();
    } else if (open && (key === "Enter" || key === "Tab")) {
      this.accept(this.completion.chosen);
    } else if (open && key === "Escape") {
      this.close();
    } else if (key === "Tab" && !event.shiftKey) {
      this.replace(this.area.selectionStart, this.area.selectionEnd, "  ");
    } else {
      return;
    }
    event.preventDefault();
  }

  /** Draw the coloured copy: one piece per token, and the completions just before the word they are for. */
  draw() {
    const text = this.area.value;
    const pieces = tokenize(text).map(({ kind, start, end }) => [
      this.completion?.start === start && this.drawCompletion(),
      h("span", { className: kind }, text.slice(start, end)),
    ]);
    // The line after a final line break would take no height in the copy, unlike in the text area,
    // without the zero-width space that ends it.
    fill(this.copy, pieces, "​");
  }

  drawCompletion() {
    const { items, chosen } = this.completion;
    const item = (text, index) => {
      // The pointer is caught before it takes the caret out of the text area.
      const onmousedown = (event) => {
        event.preventDefault();
        this.accept(index);
      };
      return h("div", { className: index === chosen ? "item chosen" : "item", onmousedown }, text);
    };
    return h("span", { className: "caret" }, h("span", { className: "completion" }, items.map(item)));
  }
}
