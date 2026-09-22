/** Tiny DOM helpers, toast system, formatting utils. No deps. */

export type Child = Node | string | number | boolean | null | undefined;

export function h(
  tag: string,
  attrs?: Record<string, unknown> | null,
  ...children: Array<Child | Child[]>
): HTMLElement {
  const el = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v === null || v === undefined || v === false) continue;
      if (k === 'class') el.className = String(v);
      else if (k === 'text') el.textContent = String(v);
      else if (k === 'html') el.innerHTML = String(v); // only with trusted content
      else if (k.startsWith('on') && typeof v === 'function') {
        el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
      } else if (k === 'dataset' && typeof v === 'object') {
        Object.assign(el.dataset, v as Record<string, string>);
      } else if (v === true) el.setAttribute(k, '');
      else el.setAttribute(k, String(v));
    }
  }
  append(el, children);
  return el;
}

function append(el: HTMLElement, children: Array<Child | Child[]>): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) { append(el, c); continue; }
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
}

export function clear(el: HTMLElement): void {
  while (el.firstChild) el.removeChild(el.firstChild);
}

// ---------------------------------------------------------------- toasts

export type ToastKind = 'info' | 'ok' | 'warn' | 'error';

let toastRoot: HTMLElement | null = null;

function ensureToastRoot(): HTMLElement {
  if (!toastRoot) {
    toastRoot = h('div', { id: 'toasts', 'aria-live': 'polite' });
    document.body.append(toastRoot);
  }
  return toastRoot;
}

let lastToastAt: Record<string, number> = {};

export function toast(msg: string, kind: ToastKind = 'info', ms = 4200): void {
  // rate-limit identical messages (e.g. SSE-driven retries)
  const now = Date.now();
  if (lastToastAt[msg] && now - lastToastAt[msg] < 3000) return;
  lastToastAt[msg] = now;
  const root = ensureToastRoot();
  const t = h('div', { class: `toast toast-${kind}`, role: 'status' },
    h('span', { class: 'toast-msg' }, msg),
    h('button', {
      class: 'toast-x', 'aria-label': 'dismiss', type: 'button',
      onclick: () => t.remove(),
    }, '✕'),
  );
  root.append(t);
  requestAnimationFrame(() => t.classList.add('in'));
  window.setTimeout(() => {
    t.classList.remove('in');
    window.setTimeout(() => t.remove(), 300);
  }, ms);
  // gc the rate-limit map
  if (Object.keys(lastToastAt).length > 64) lastToastAt = {};
}

/** Run a promise; toast errors (esp. ApiError); optional success message. */
export async function guarded<T>(
  p: Promise<T>,
  okMsg?: string | ((result: T) => string),
): Promise<T | undefined> {
  try {
    const r = await p;
    if (okMsg) toast(typeof okMsg === 'function' ? okMsg(r) : okMsg, 'ok');
    return r;
  } catch (e) {
    if (e instanceof Error && e.name === 'AbortError') return undefined;
    toast(e instanceof Error ? e.message : String(e), 'error');
    return undefined;
  }
}

// ------------------------------------------------------------ formatting

/** RFC3339 → "YYYY-MM-DD HH:MM" local. */
export function fmtTs(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ` +
    `${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function timeAgo(iso: string | null | undefined): string {
  if (!iso) return '—';
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return iso;
  const s = Math.max(0, Math.floor((Date.now() - t) / 1000));
  if (s < 45) return 'just now';
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const hr = Math.floor(m / 60);
  if (hr < 24) return `${hr}h ago`;
  const d = Math.floor(hr / 24);
  if (d < 14) return `${d}d ago`;
  return fmtTs(iso);
}

/** Elapsed between created_at and finished_at (or now), human short. */
export function elapsed(created: string, finished?: string | null): string {
  const a = new Date(created).getTime();
  const b = finished ? new Date(finished).getTime() : Date.now();
  if (Number.isNaN(a) || Number.isNaN(b)) return '—';
  let s = Math.max(0, Math.floor((b - a) / 1000));
  const hr = Math.floor(s / 3600); s -= hr * 3600;
  const m = Math.floor(s / 60); s -= m * 60;
  if (hr > 0) return `${hr}h${p2(m)}m`;
  if (m > 0) return `${m}m${p2(s)}s`;
  return `${s}s`;
}

function p2(n: number): string { return String(n).padStart(2, '0'); }

export function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n - 1) + '…' : s;
}

// ---------------------------------------------------------------- modals

/** Generic centered modal. Returns {close}. Backdrop click / Esc close. */
export function openModal(
  title: string,
  build: (body: HTMLElement, close: () => void) => void,
  opts?: { wide?: boolean },
): { close: () => void } {
  const backdrop = h('div', { class: 'modal-backdrop' });
  const close = (): void => {
    backdrop.classList.remove('in');
    window.setTimeout(() => backdrop.remove(), 150);
    document.removeEventListener('keydown', onKey);
  };
  const onKey = (e: KeyboardEvent): void => { if (e.key === 'Escape') close(); };
  document.addEventListener('keydown', onKey);
  const panel = h('div', { class: `modal${opts?.wide ? ' modal-wide' : ''}`, role: 'dialog', 'aria-modal': 'true' },
    h('div', { class: 'modal-head' },
      h('span', { class: 'panel-label' }, title),
      h('button', { class: 'btn btn-ghost btn-sm', onclick: close, 'aria-label': 'close', type: 'button' }, '✕'),
    ),
    ((): HTMLElement => { const b = h('div', { class: 'modal-body' }); build(b, close); return b; })(),
  );
  backdrop.append(panel);
  backdrop.addEventListener('mousedown', (e) => { if (e.target === backdrop) close(); });
  document.body.append(backdrop);
  requestAnimationFrame(() => backdrop.classList.add('in'));
  const first = panel.querySelector<HTMLElement>('input, textarea, select, button');
  first?.focus();
  return { close };
}

/** Prompt-style modal with a single text input. Resolves null on cancel. */
export function inputModal(
  title: string, label: string, placeholder = '',
): Promise<string | null> {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v: string | null): void => {
      if (done) return;
      done = true;
      resolve(v);
      m.close();
    };
    const input = h('input', { class: 'input', placeholder, type: 'text', spellcheck: 'false' }) as HTMLInputElement;
    const m = openModal(title, (body, _close) => {
      body.append(
        h('label', { class: 'field-label' }, label),
        input,
        h('div', { class: 'row row-end gap8' },
          h('button', { class: 'btn', type: 'button', onclick: () => finish(null) }, 'Cancel'),
          h('button', {
            class: 'btn btn-primary', type: 'button',
            onclick: () => finish(input.value.trim()),
          }, '▶ OK'),
        ),
      );
      onEnter(input, () => finish(input.value.trim()));
    });
    void m;
  });
}

/** Native confirm, kept for destructive ops (honest + keyboard accessible). */
export function confirmDialog(msg: string): boolean {
  return window.confirm(msg);
}

// ------------------------------------------------------------ misc utils

export function downloadText(filename: string, text: string, mime = 'text/markdown'): void {
  const blob = new Blob([text], { type: `${mime};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const a = h('a', { href: url, download: filename }) as HTMLAnchorElement;
  document.body.append(a);
  a.click();
  a.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 2000);
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const ta = h('textarea', { style: 'position:fixed;opacity:0' }) as HTMLTextAreaElement;
      ta.value = text;
      document.body.append(ta);
      ta.select();
      const ok = document.execCommand('copy');
      ta.remove();
      return ok;
    } catch {
      return false;
    }
  }
}

/** Debounce helper. */
export function debounce<A extends unknown[]>(fn: (...a: A) => void, ms: number): (...a: A) => void {
  let t: number | undefined;
  return (...a: A) => {
    window.clearTimeout(t);
    t = window.setTimeout(() => fn(...a), ms);
  };
}

/**
 * Submit-on-Enter for inputs/textarea fields: calls fn() when Enter is
 * pressed (Shift+Enter still inserts a newline in textareas). Every text
 * input that performs an action goes through this — Enter must always work.
 */
export function onEnter(el: HTMLElement, fn: () => void): void {
  el.addEventListener('keydown', (e: Event) => {
    const ke = e as KeyboardEvent;
    if (ke.key === 'Enter' && !ke.shiftKey) {
      ke.preventDefault();
      fn();
    }
  });
}

export function emptyState(msg: string, hint?: string): HTMLElement {
  return h('div', { class: 'empty' },
    h('div', { class: 'empty-mark' }, '▚ ▚ ▚'),
    h('div', { class: 'empty-msg' }, msg),
    hint ? h('div', { class: 'empty-hint' }, hint) : null,
  );
}
