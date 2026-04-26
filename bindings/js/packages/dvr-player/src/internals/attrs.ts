// Tiny attribute helpers for vanilla custom elements.
//
// Pattern matches what production media-player web components
// converge on (template-literal HTML strings + attribute reflection
// + attributeChangedCallback dispatch). Kept local so the
// dvr-player package has zero web-component framework deps.

export function getBooleanAttr(el: HTMLElement, name: string): boolean {
  return el.hasAttribute(name);
}

export function setBooleanAttr(el: HTMLElement, name: string, value: boolean): void {
  if (value) {
    if (!el.hasAttribute(name)) el.setAttribute(name, '');
  } else if (el.hasAttribute(name)) {
    el.removeAttribute(name);
  }
}

export function getStringAttr(el: HTMLElement, name: string, fallback = ''): string {
  return el.getAttribute(name) ?? fallback;
}

export function getNumericAttr(el: HTMLElement, name: string, fallback: number): number {
  const raw = el.getAttribute(name);
  if (raw === null || raw === '') return fallback;
  const n = Number(raw);
  return Number.isFinite(n) ? n : fallback;
}
