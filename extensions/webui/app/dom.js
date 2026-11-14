// Making elements. Text given as a child is never read as HTML.

/** Make an element with properties and children. */
export function h(tag, properties = {}, ...children) {
  return fill(Object.assign(document.createElement(tag), properties), ...children);
}

/** Replace the children of an element; lists are flattened, and a child that is null or false is left out. */
export function fill(element, ...children) {
  element.replaceChildren(...children.flat(Infinity).filter((child) => child != null && child !== false));
  return element;
}
