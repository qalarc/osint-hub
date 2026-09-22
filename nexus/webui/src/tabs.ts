/**
 * Tab panes: Timeline, Analysis, Story (dossier + brief, md-rendered).
 */
import { api } from './api';
import {
  clear, copyText, downloadText, emptyState, fmtTs, guarded, h, toast, truncate,
} from './dom';
import { pivotToTask } from './inspector';
import { etypeColor } from './graph';
import { renderMarkdownInto } from './md';
import type { Case, CaseGraph, Entity, Task, TimelineEvent } from './types';

// ------------------------------------------------------------- TIMELINE

export class TimelinePane {
  readonly el: HTMLElement;
  private listEl: HTMLElement;
  private caseId: string | null = null;

  constructor(private readonly cb: {
    caseId(): string | null;
    entities(): Entity[];
    navigate(entityId: string): void;
  }) {
    this.listEl = h('div', { class: 'timeline-list' });
    const kindSel = h('select', { class: 'input input-sm', 'aria-label': 'event kind' }) as HTMLSelectElement;
    for (const k of ['scan', 'note', 'sighting', 'enrichment', 'research', 'report', 'manual']) {
      kindSel.append(h('option', { value: k }, k));
    }
    const titleIn = h('input', {
      class: 'input input-sm', type: 'text', placeholder: 'event title…', spellcheck: 'false',
    }) as HTMLInputElement;
    const entitySel = h('select', { class: 'input input-sm', 'aria-label': 'linked entity (optional)' }) as HTMLSelectElement;
    entitySel.append(h('option', { value: '' }, '— entity —'));
    const srcIn = h('input', {
      class: 'input input-sm', type: 'text', placeholder: 'source (optional)', spellcheck: 'false',
    }) as HTMLInputElement;

    const form = h('form', { class: 'row gap6 tl-form' }, kindSel, titleIn, entitySel, srcIn,
      h('button', { class: 'btn btn-sm btn-primary', type: 'submit' }, '+ event'));
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      const cid = this.cb.caseId();
      const title = titleIn.value.trim();
      if (!cid) { toast('select a case first', 'warn'); return; }
      if (!title) { toast('event title required', 'warn'); return; }
      void guarded(api.addTimelineEvent(cid, {
        kind: kindSel.value,
        title,
        entity_id: entitySel.value || null,
        source: srcIn.value.trim() || undefined,
      }).then((ev) => {
        titleIn.value = '';
        void this.refresh(cid);
        return ev;
      }), 'event added');
    });

    this.el = h('section', { class: 'pane', id: 'pane-timeline', hidden: true },
      h('header', { class: 'pane-head' },
        h('span', { class: 'panel-label' }, 'timeline'),
        form,
      ),
      h('div', { class: 'pane-body' }, this.listEl),
    );
  }

  async refresh(caseId?: string | null): Promise<void> {
    const cid = caseId ?? this.cb.caseId();
    if (!cid) {
      clear(this.listEl);
      this.listEl.append(emptyState('no case selected', 'pick or create a case in the left rail'));
      return;
    }
    this.caseId = cid;
    let events: TimelineEvent[];
    try {
      events = await api.getTimeline(cid);
    } catch (e) {
      clear(this.listEl);
      this.listEl.append(h('div', { class: 'error-text small pad8' }, String(e instanceof Error ? e.message : e)));
      return;
    }
    // entity select options
    const sel = this.el.querySelector<HTMLSelectElement>('select[aria-label="linked entity (optional)"]');
    if (sel) {
      const cur = sel.value;
      clear(sel);
      sel.append(h('option', { value: '' }, '— entity —'));
      for (const en of this.cb.entities()) {
        sel.append(h('option', { value: en.id }, `${en.etype}:${truncate(en.label, 30)}`));
      }
      sel.value = cur;
    }

    clear(this.listEl);
    if (events.length === 0) {
      this.listEl.append(emptyState('no events yet', 'connector runs land here; add one manually above'));
      return;
    }
    for (const ev of events) this.listEl.append(this.eventRow(ev));
  }

  private eventRow(ev: TimelineEvent): HTMLElement {
    const entityLabel = ev.entity_id ? this.cb.entities().find((x) => x.id === ev.entity_id) : undefined;
    return h('div', { class: 'tl-item' },
      h('div', { class: 'tl-dot' }),
      h('div', { class: 'tl-body' },
        h('div', { class: 'row gap6' },
          h('span', { class: 'chip' }, ev.kind),
          h('span', { class: 'mono small muted' }, fmtTs(ev.ts)),
          ev.source ? h('span', { class: 'muted small' }, `· ${ev.source}`) : null,
        ),
        h('div', { class: 'tl-title' }, ev.title),
        entityLabel
          ? h('a', {
            class: 'tl-entity', href: '#', role: 'button',
            onclick: (e: Event) => { e.preventDefault(); this.cb.navigate(entityLabel.id); },
          },
            h('span', { class: 'dot dot-inline', style: `background:${etypeColor(String(entityLabel.etype))}` }),
            entityLabel.label)
          : null,
      ),
    );
  }

  currentCase(): string | null { return this.caseId; }
}

// ------------------------------------------------------------- ANALYSIS

export class AnalysisPane {
  readonly el: HTMLElement;
  private bodyEl: HTMLElement;
  private graph: CaseGraph | null = null;

  constructor(private readonly cb: {
    caseId(): string | null;
    entities(): Entity[];
    labelOf(id: string): string;
    navigate(entityId: string): void;
    taskStarted(task: Task): void;
  }) {
    this.bodyEl = h('div', { class: 'pane-body analysis-body' });
    this.el = h('section', { class: 'pane', id: 'pane-analysis', hidden: true },
      h('header', { class: 'pane-head' },
        h('span', { class: 'panel-label' }, 'analysis'),
        h('span', { class: 'muted small' }, 'pagerank · degree · communities · pivots'),
      ),
      this.bodyEl,
    );
  }

  refreshFromGraph(g: CaseGraph): void {
    this.graph = g;
    this.render();
  }

  private barGlyph(frac: number): string {
    const filled = Math.max(0, Math.min(10, Math.round(frac * 10)));
    return '▰'.repeat(filled) + '▱'.repeat(10 - filled);
  }

  private render(): void {
    const g = this.graph;
    clear(this.bodyEl);
    if (!g || g.nodes.length === 0) {
      this.bodyEl.append(emptyState('nothing to analyze yet', 'add entities and relations to the case'));
      return;
    }
    const ents = this.cb.entities();
    const labelOf = (id: string): string => ents.find((x) => x.id === id)?.label ?? this.cb.labelOf(id);
    const etypeOf = (id: string): string => String(ents.find((x) => x.id === id)?.etype ?? '');

    // ---- centrality ---------------------------------------------------
    const pr = g.analysis.pagerank ?? {};
    const deg = g.analysis.degree ?? {};
    const ranked = Object.entries(pr).sort((a, b) => b[1] - a[1]).slice(0, 15);
    const prMax = ranked[0]?.[1] ?? 1;
    const table = h('table', { class: 'data-table' },
      h('thead', {}, h('tr', {},
        h('th', {}, '#'), h('th', {}, 'entity'), h('th', {}, 'etype'),
        h('th', { class: 'num' }, 'deg'), h('th', { class: 'num' }, 'pagerank'), h('th', {}, '')),
      ),
    );
    const tbody = h('tbody');
    ranked.forEach(([id, score], i) => {
      tbody.append(h('tr', {},
        h('td', { class: 'mono muted' }, String(i + 1)),
        h('td', { class: 'mono' }, truncate(labelOf(id), 34)),
        h('td', {}, h('span', { class: 'chip', style: `border-color:${etypeColor(etypeOf(id))}55` }, etypeOf(id))),
        h('td', { class: 'num mono' }, String(deg[id] ?? 0)),
        h('td', { class: 'num mono' }, score.toFixed(4)),
        h('td', { class: 'bar-glyph mono' }, this.barGlyph(prMax > 0 ? score / prMax : 0)),
      ));
    });
    table.append(tbody);

    // ---- communities --------------------------------------------------
    const comm = g.analysis.communities ?? {};
    const groups = new Map<string, string[]>();
    for (const [id, cidv] of Object.entries(comm)) {
      const key = String(cidv);
      const arr = groups.get(key) ?? [];
      arr.push(id);
      groups.set(key, arr);
    }
    const comps = g.analysis.components ?? [];
    const commEl = h('div', { class: 'comm-wrap' },
      h('div', { class: 'muted small' },
        `${comps.length} connected component${comps.length === 1 ? '' : 's'} (${comps.map((c) => c.length).join(', ')})`),
      ...(groups.size === 0
        ? [h('div', { class: 'muted small pad6' }, 'no communities computed')]
        : [...groups.entries()]
          .sort((a, b) => b[1].length - a[1].length)
          .map(([cid, ids]) => h('div', { class: 'comm-group' },
            h('div', { class: 'panel-label' }, `community ${cid} · ${ids.length}`),
            h('div', { class: 'comm-members' },
              ...ids.map((id) => h('span', {
                class: 'chip chip-node', title: id,
                onclick: () => this.cb.navigate(id),
              },
              h('span', { class: 'dot dot-inline', style: `background:${etypeColor(etypeOf(id))}` }),
              truncate(labelOf(id), 26)))),
          ))),
    );

    // ---- pivots ---------------------------------------------------------
    const pivotsBox = h('div', { class: 'pivots-box' });
    const sel = h('select', { class: 'input input-sm', 'aria-label': 'pivot entity' }) as HTMLSelectElement;
    const pivotEntities = [...ents].sort((a, b) => (pr[b.id] ?? 0) - (pr[a.id] ?? 0)).slice(0, 15);
    for (const e of pivotEntities) sel.append(h('option', { value: e.id }, `${e.etype}:${truncate(e.label, 26)}`));
    const pivotList = h('div', { class: 'pivots-wrap open' });
    const loadPivots = (): void => {
      const cid = this.cb.caseId();
      const eid = sel.value;
      if (!cid || !eid) return;
      clear(pivotList);
      pivotList.append(h('div', { class: 'muted small pad6' }, 'loading…'));
      void guarded(api.getPivots(cid, eid).then(({ pivots }) => {
        clear(pivotList);
        const ent = ents.find((x) => x.id === eid);
        if (!ent) return;
        if (pivots.length === 0) pivotList.append(h('div', { class: 'muted small pad6' }, 'no pivot suggestions for this entity'));
        for (const p of pivots) pivotList.append(this.pivotCard(p, ent, cid));
        return pivots;
      }));
    };
    sel.addEventListener('change', loadPivots);
    pivotsBox.append(
      h('div', { class: 'row gap6' }, sel,
        h('button', { class: 'btn btn-sm', type: 'button', onclick: loadPivots }, '↻')),
      pivotList,
    );

    this.bodyEl.append(
      h('div', { class: 'analysis-grid' },
        h('div', { class: 'panel panel-sub' },
          h('header', { class: 'panel-head' }, h('span', { class: 'panel-label' }, 'centrality · top 15 pagerank')),
          h('div', { class: 'table-scroll' }, table),
        ),
        h('div', { class: 'stack' },
          h('div', { class: 'panel panel-sub' },
            h('header', { class: 'panel-head' }, h('span', { class: 'panel-label' }, 'communities')),
            commEl,
          ),
          h('div', { class: 'panel panel-sub' },
            h('header', { class: 'panel-head' }, h('span', { class: 'panel-label' }, 'pivot suggestions')),
            pivotsBox,
          ),
        ),
      ),
    );
    loadPivots();
  }

  private pivotCard(p: { entity_id: string; action: string; reason: string; priority: number }, ent: Entity, caseId: string): HTMLElement {
    return h('div', { class: 'pivot-card' },
      h('div', { class: 'row gap6' },
        h('span', { class: 'chip chip-action mono' }, p.action),
        h('span', { class: 'pivot-priority mono', title: `priority ${p.priority.toFixed(2)}` }, p.priority.toFixed(2)),
      ),
      h('div', { class: 'pivot-reason muted small' }, p.reason),
      h('button', {
        class: 'btn btn-sm btn-primary', type: 'button',
        onclick: () => {
          const { kind, params } = pivotToTask(p.action, ent);
          void guarded(api.createTask(caseId, { kind, target: ent.id, params }).then((t) => {
            this.cb.taskStarted(t);
            return t;
          }), `${kind} queued from pivot`);
        },
      }, 'run'),
    );
  }
}

// ---------------------------------------------------------------- STORY

export class StoryPane {
  readonly el: HTMLElement;
  private dossierEl: HTMLElement;
  private briefEl: HTMLElement;
  private dossierSrc = '';
  private briefSrc = '';

  constructor(private readonly cb: { caseId(): string | null; case(): Case | null }) {
    this.dossierEl = h('div', { class: 'md' });
    this.briefEl = h('div', { class: 'md' });

    const pushBtn = h('button', {
      class: 'btn btn-sm', type: 'button', title: 'POST /api/archon/push_case',
      onclick: () => {
        const cid = this.cb.caseId();
        if (!cid) return;
        void guarded(api.archonPushCase({ case_id: cid }).then((r) => {
          const name = r.sandbox && typeof r.sandbox === 'object' && 'name' in r.sandbox
            ? String((r.sandbox as Record<string, unknown>).name) : 'sandbox';
          return `pushed to archon: ${name}`;
        }), undefined);
      },
    }, '⇪ push case to archon');

    this.el = h('section', { class: 'pane', id: 'pane-story', hidden: true },
      h('header', { class: 'pane-head' },
        h('span', { class: 'panel-label' }, 'story'),
        h('span', { class: 'flex-spacer' }),
        pushBtn,
      ),
      h('div', { class: 'pane-body story-body' },
        this.docPanel('dossier', this.dossierEl, () => this.dossierSrc, 'dossier'),
        this.docPanel('article brief', this.briefEl, () => this.briefSrc, 'brief'),
      ),
    );
  }

  private docPanel(title: string, into: HTMLElement, src: () => string, fileTag: string): HTMLElement {
    return h('div', { class: 'panel panel-sub story-panel' },
      h('header', { class: 'panel-head' },
        h('span', { class: 'panel-label' }, title),
        h('span', { class: 'flex-spacer' }),
        h('button', {
          class: 'btn btn-ghost btn-sm', type: 'button', title: 'copy markdown',
          onclick: () => {
            void copyText(src()).then((ok) => toast(ok ? `${title} copied` : 'copy failed', ok ? 'ok' : 'error'));
          },
        }, '⧉ copy'),
        h('button', {
          class: 'btn btn-ghost btn-sm', type: 'button', title: 'download .md',
          onclick: () => {
            const c = this.cb.case();
            downloadText(`${c?.slug ?? 'case'}-${fileTag}.md`, src());
          },
        }, '⭳ .md'),
      ),
      h('div', { class: 'story-scroll' }, into),
    );
  }

  async refresh(caseId?: string | null): Promise<void> {
    const cid = caseId ?? this.cb.caseId();
    if (!cid) {
      this.dossierEl.innerHTML = '';
      this.briefEl.innerHTML = '';
      this.dossierEl.append(emptyState('no case selected'));
      this.briefEl.append(emptyState('no case selected'));
      return;
    }
    const [dossier, brief] = await Promise.allSettled([
      api.dossierMd(cid), api.briefMd(cid),
    ]);
    if (dossier.status === 'fulfilled') {
      this.dossierSrc = dossier.value;
      renderMarkdownInto(this.dossierEl, dossier.value);
    } else {
      this.dossierSrc = '';
      renderMarkdownInto(this.dossierEl, `> dossier unavailable: ${dossier.reason}`);
    }
    if (brief.status === 'fulfilled') {
      this.briefSrc = brief.value;
      renderMarkdownInto(this.briefEl, brief.value);
    } else {
      this.briefSrc = '';
      renderMarkdownInto(this.briefEl, `> brief unavailable: ${brief.reason}`);
    }
  }
}
