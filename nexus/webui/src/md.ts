/**
 * Minimal in-house markdown renderer (no deps).
 * Supports: h1–h6, bold/italic/strike, inline + fenced code, links,
 * pipe tables, hr, ordered/unordered lists (with nesting), paragraphs.
 * Everything is HTML-escaped before inline transforms; link schemes are
 * whitelisted (http/https/mailto/relative). Returns an HTML string.
 */

const ESC_MAP: Record<string, string> = {
  '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
};

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ESC_MAP[c] ?? c);
}

function safeUrl(href: string): string | null {
  const t = href.trim();
  if (/^https?:\/\//i.test(t)) return t;
  if (/^mailto:[^\s@]+@[^\s@]+$/i.test(t)) return t;
  if (/^\/(?!\/)/.test(t)) return t; // same-origin relative, not protocol-relative
  if (/^#[\w-]*$/.test(t)) return t;
  return null;
}

interface CodeSpan { placeholder: string; html: string }

function inline(raw: string): string {
  let s = esc(raw);
  const codes: CodeSpan[] = [];
  s = s.replace(/`([^`\n]+)`/g, (_m, c: string) => {
    const ph = `\u0000${codes.length}\u0000`;
    codes.push({ placeholder: ph, html: `<code>${c}</code>` });
    return ph;
  });
  // links: [text](url)
  s = s.replace(/\[([^\]\n]*)\]\(([^)\s]+)\)/g, (_m, text: string, href: string) => {
    const url = safeUrl(href);
    if (!url) return text;
    return `<a href="${url}" target="_blank" rel="noopener noreferrer">${text}</a>`;
  });
  // auto-link bare urls — intentional: only http(s), keeps dossiers clickable
  s = s.replace(/(^|[\s(])(https?:\/\/[^\s<)]+)/g, (_m, pre: string, url: string) =>
    `${pre}<a href="${url}" target="_blank" rel="noopener noreferrer">${url}</a>`);
  s = s.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>');
  s = s.replace(/(^|[^\w*])\*([^*\n]+)\*(?=[^\w*]|$)/g, '$1<em>$2</em>');
  s = s.replace(/(^|\s)_([^_\n]+)_(?=\s|[.,;:!?)]|$)/g, '$1<em>$2</em>');
  s = s.replace(/~~([^~\n]+)~~/g, '<del>$1</del>');
  for (const c of codes) s = s.split(c.placeholder).join(c.html);
  return s;
}

const HR_RE = /^\s{0,3}(?:-[^\S\n]*-[^\S\n]*-|_[^\S\n]*_[^\S\n]*_|\*[^\S\n]*\*[^\S\n]*\*)[\s*_-]*$/;
const LIST_ITEM_RE = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TABLE_SEP_RE = /^\s*\|?\s*:?-{2,}:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*$/;

/**
 * Anchor slug for h2/h3 (github-ish): lowercase, markdown tokens stripped,
 * non-alphanumerics collapsed to '-', deduped per render. Returns an id that
 * is safe for both the id attribute and querySelector('#…').
 */
function slugifyHeading(raw: string, used: Set<string>): string {
  let s = raw
    .toLowerCase()
    .replace(/[`*_~[\]()!#]/g, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  if (!s) s = 'section';
  if (/^[0-9]/.test(s)) s = `s-${s}`;
  let id = s;
  for (let n = 2; used.has(id); n++) id = `${s}-${n}`;
  used.add(id);
  return id;
}

function splitRow(line: string): string[] {
  let t = line.trim();
  if (t.startsWith('|')) t = t.slice(1);
  if (t.endsWith('|')) t = t.slice(0, -1);
  return t.split('|').map((c) => c.trim());
}

function parseList(lines: string[], start: number): { html: string; next: number } {
  const out: string[] = [];
  const stack: Array<{ indent: number; ordered: boolean }> = [];
  const closeTop = (): void => {
    const top = stack.pop();
    if (top) out.push(top.ordered ? '</ol>' : '</ul>');
  };
  let i = start;
  for (; i < lines.length; i++) {
    const m = LIST_ITEM_RE.exec(lines[i]);
    if (!m) break;
    const indent = m[1].replace(/\t/g, '  ').length;
    const ordered = /\d/.test(m[2][0]);
    const content = inline(m[3]);
    for (;;) {
      if (stack.length === 0) {
        stack.push({ indent, ordered });
        out.push(ordered ? '<ol>' : '<ul>');
        break;
      }
      const top = stack[stack.length - 1];
      if (indent > top.indent + 1) {
        stack.push({ indent, ordered });
        out.push(ordered ? '<ol>' : '<ul>');
        continue;
      }
      if (indent < top.indent - 1 || (indent <= top.indent - 1)) { closeTop(); continue; }
      break;
    }
    out.push(`<li>${content}</li>`);
  }
  while (stack.length > 0) closeTop();
  return { html: out.join(''), next: i };
}

export function renderMarkdown(src: string): string {
  const lines = src.replace(/\r\n?/g, '\n').split('\n');
  const out: string[] = [];
  let i = 0;
  let para: string[] = [];
  const headingIds = new Set<string>();

  const flushPara = (): void => {
    if (para.length > 0) {
      out.push(`<p>${inline(para.join(' '))}</p>`);
      para = [];
    }
  };

  while (i < lines.length) {
    const line = lines[i];

    if (!line.trim()) { flushPara(); i++; continue; }

    // fenced code
    const fence = /^\s*```(.*)$/.exec(line);
    if (fence) {
      flushPara();
      const lang = fence[1].trim();
      const body: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```\s*$/.test(lines[i])) {
        body.push(lines[i]);
        i++;
      }
      i++; // closing fence (or EOF)
      const cls = lang ? ` class="lang-${esc(lang)}"` : '';
      out.push(`<pre><code${cls}>${esc(body.join('\n'))}</code></pre>`);
      continue;
    }

    // heading (h2/h3 get a slug id for in-page anchors, e.g. the guide TOC)
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      flushPara();
      const lvl = heading[1].length;
      const inner = inline(heading[2]);
      if (lvl === 2 || lvl === 3) {
        const id = slugifyHeading(heading[2], headingIds);
        out.push(`<h${lvl} id="${esc(id)}">${inner}</h${lvl}>`);
      } else {
        out.push(`<h${lvl}>${inner}</h${lvl}>`);
      }
      i++;
      continue;
    }

    // hr
    if (HR_RE.test(line)) {
      flushPara();
      out.push('<hr>');
      i++;
      continue;
    }

    // table (header row + separator row)
    if (line.includes('|') && i + 1 < lines.length && TABLE_SEP_RE.test(lines[i + 1])) {
      flushPara();
      const header = splitRow(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].includes('|') && lines[i].trim()) {
        rows.push(splitRow(lines[i]));
        i++;
      }
      const th = header.map((c) => `<th>${inline(c)}</th>`).join('');
      const trs = rows.map((r) => {
        const tds = header.map((_, ci) => `<td>${inline(r[ci] ?? '')}</td>`).join('');
        return `<tr>${tds}</tr>`;
      }).join('');
      out.push(`<div class="md-table-wrap"><table><thead><tr>${th}</tr></thead><tbody>${trs}</tbody></table></div>`);
      continue;
    }

    // lists
    if (LIST_ITEM_RE.test(line)) {
      flushPara();
      const res = parseList(lines, i);
      out.push(res.html);
      i = res.next;
      continue;
    }

    // blockquote (simple: consecutive > lines)
    if (/^\s{0,3}>\s?/.test(line)) {
      flushPara();
      const body: string[] = [];
      while (i < lines.length && /^\s{0,3}>\s?/.test(lines[i])) {
        body.push(lines[i].replace(/^\s{0,3}>\s?/, ''));
        i++;
      }
      out.push(`<blockquote><p>${inline(body.join(' '))}</p></blockquote>`);
      continue;
    }

    // paragraph line
    para.push(line.trim());
    i++;
  }
  flushPara();
  return out.join('\n');
}

/** Render markdown into an element (safe: escaped + scheme-whitelisted). */
export function renderMarkdownInto(el: HTMLElement, src: string): void {
  el.innerHTML = renderMarkdown(src);
}
