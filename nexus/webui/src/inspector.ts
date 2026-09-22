/**
 * Right inspector drawer: entity fields, data JSON, relations, evidence,
 * wiki doc editor, connector actions, pivots, merge, delete.
 */
import { api, type MergePreview } from './api';
import {
  clear, emptyState, fmtTs, guarded, h, inputModal, openModal, toast, truncate,
} from './dom';
import { etypeColor } from './graph';
import {
  type Entity, type EntityDetail, type PivotSuggestion, type Relation, type Task,
} from './types';

export interface InspectorCallbacks {
  caseId(): string | null;
  labelOf(id: string): string;
  entities(): Entity[];
  navigate(entityId: string): void;
  graphChanged(): void;
  taskStarted(task: Task): void;
}

/** hub_scan kind for an entity's etype */
function scanKindFor(etype: string): string {
  if (etype === 'email') return 'email';
  if (etype === 'phone') return 'phone';
  if (etype === 'image') return 'image';
  return 'username'; // username / alias / person / fallback
}

/** flowsint enrichers suggested by etype */
function enrichersFor(etype: string): string[] {
  switch (etype) {
    case 'domain':
    case 'subdomain':
      return ['subdomain_discovery', 'dns_lookup', 'whois_lookup'];
    case 'ip':
      return ['reverse_dns', 'geoip', 'ports'];
    case 'website':
      return ['http_headers', 'tech_stack'];
    case 'email':
      return ['breach_check', 'gravatar'];
    case 'phone':
      return ['phone_lookup', 'carrier'];
    case 'username':
    case 'person':
      return ['account_search'];
    case 'wallet':
      return ['balance', 'tx_history'];
    default:
      return ['basic'];
  }
}

/** map a pivot action like "flowsint_enrich:subdomain_discovery" to a task */
export function pivotToTask(
  action: string, entity: Entity,
): { kind: string; params: Record<string, unknown> } {
  const i = action.indexOf(':');
  const head = i >= 0 ? action.slice(0, i) : action;
  const rest = i >= 0 ? action.slice(i + 1) : null;
  switch (head) {
    case 'hub_scan':
      return { kind: 'hub_scan', params: { scan: rest ?? 'username', value: entity.label } };
    case 'flowsint_enrich':
      return { kind: 'flowsint_enrich', params: { enrichers: rest ? [rest] : enrichersFor(String(entity.etype)) } };
    case 'openplanter_research':
      return { kind: 'openplanter_research', params: rest ? { question: rest } : {} };
    case 'archon_ground':
      return { kind: 'archon_ground', params: rest ? { question: rest } : {} };
    case 'web_pivots':
      return { kind: 'web_pivots', params: {} };
    default:
      return { kind: head, params: rest ? { scan: rest } : {} };
  }
}

export class Inspector {
  readonly el: HTMLElement;
  private entityId: string | null = null;
  private detail: EntityDetail | null = null;
  private subtab: 'inspect' | 'wiki' = 'inspect';

  private headEl: HTMLElement;
  private bodyEl: HTMLElement;
  private closeBtn: HTMLElement;

  constructor(private readonly cb: InspectorCallbacks) {
    this.headEl = h('div', { class: 'inspector-head' });
    this.bodyEl = h('div', { class: 'inspector-body' });
    this.closeBtn = h('button', {
      class: 'btn btn-ghost btn-sm', type: 'button', title: 'close',
      onclick: () => this.close(),
    }, '✕');
    this.el = h('aside', { class: 'rail rail-right', id: 'rail-right', hidden: true },
      h('div', { class: 'panel panel-rail panel-fill' },
        h('header', { class: 'panel-head' },
          h('span', { class: 'panel-label' }, 'inspector'),
          this.closeBtn,
        ),
        this.headEl,
        this.bodyEl,
      ),
    );
  }

  get isOpen(): boolean { return !this.el.hidden; }
  get currentId(): string | null { return this.entityId; }

  open(id: string): void {
    this.entityId = id;
    this.el.hidden = false;
    this.el.classList.add('open');
    void this.load();
  }

  close(): void {
    this.entityId = null;
    this.detail = null;
    this.el.hidden = true;
    this.el.classList.remove('open');
  }

  /** re-fetch if open; tolerate the entity having vanished; never clobber active typing */
  async refresh(): Promise<void> {
    if (!this.entityId) return;
    const ae = document.activeElement as HTMLElement | null;
    if (ae && this.el.contains(ae) && (ae.tagName === 'TEXTAREA' || ae.tagName === 'INPUT')) return;
    try {
      await api.getEntity(this.entityId);
    } catch {
      this.close();
      return;
    }
    await this.load();
  }

  private async load(): Promise<void> {
    if (!this.entityId) return;
    const id = this.entityId;
    let d: EntityDetail;
    try {
      d = await api.getEntity(id);
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), 'error');
      this.close();
      return;
    }
    if (this.entityId !== id) return; // switched away meanwhile
    this.detail = d;
    this.renderHead(d.entity);
    if (this.subtab === 'wiki') this.renderWiki(d);
    else this.renderInspect(d);
  }

  // ------------------------------------------------------------ header
  private renderHead(e: Entity): void {
    clear(this.headEl);
    const color = etypeColor(String(e.etype));
    this.headEl.append(
      h('div', { class: 'inspector-title' },
        h('span', { class: 'dot', style: `background:${color};box-shadow:0 0 6px ${color}66` }),
        h('span', { class: 'inspector-label', title: e.label }, e.label),
      ),
      h('div', { class: 'muted small mono' }, `${e.id} · ${e.etype}`),
    );
  }

  // ---------------------------------------------------------- inspect tab
  private renderInspect(d: EntityDetail): void {
    const e = d.entity;
    clear(this.bodyEl);
    this.bodyEl.append(this.subtabs());

    // fields
    const labelInput = h('input', { class: 'input input-sm', type: 'text', value: e.label, spellcheck: 'false' }) as HTMLInputElement;
    const commitLabel = (): void => {
      const v = labelInput.value.trim();
      if (!v || v === e.label) return;
      void guarded(api.patchEntity(e.id, { label: v }).then(() => { this.cb.graphChanged(); }), 'label updated');
    };
    labelInput.addEventListener('blur', commitLabel);
    labelInput.addEventListener('keydown', (ev) => { if (ev.key === 'Enter') labelInput.blur(); });

    // etype is contract-fixed at creation (PATCH has no etype field) → read-only chip
    const etypeChip = h('span', {
      class: 'chip chip-etype mono',
      title: 'etype is fixed at creation (server dedup key)',
      style: `border-color:${etypeColor(String(e.etype))}55`,
    }, String(e.etype));

    const confRange = h('input', { class: 'range', type: 'range', min: '0', max: '1', step: '0.05', value: String(e.confidence) }) as HTMLInputElement;
    const confVal = h('span', { class: 'mono small' }, e.confidence.toFixed(2));
    confRange.addEventListener('input', () => { confVal.textContent = Number(confRange.value).toFixed(2); });
    confRange.addEventListener('change', () => {
      void guarded(api.patchEntity(e.id, { confidence: Number(confRange.value) }), 'confidence updated');
    });

    const pinnedBtn = h('button', {
      class: `btn btn-sm${e.pinned ? ' btn-primary' : ''}`, type: 'button',
      onclick: () => {
        void guarded(api.patchEntity(e.id, { pinned: !e.pinned }).then(() => {
          void this.load(); this.cb.graphChanged();
        }), e.pinned ? 'unpinned' : 'pinned');
      },
    }, e.pinned ? '★ pinned' : '☆ pinned');

    // data JSON
    const dataTa = h('textarea', {
      class: 'textarea textarea-sm mono', rows: '5', spellcheck: 'false',
    }) as HTMLTextAreaElement;
    dataTa.value = JSON.stringify(e.data ?? {}, null, 2);
    const applyData = h('button', {
      class: 'btn btn-sm', type: 'button',
      onclick: () => {
        try {
          const parsed: unknown = JSON.parse(dataTa.value);
          if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
            throw new Error('data must be a JSON object');
          }
          void guarded(api.patchEntity(e.id, { data: parsed as Record<string, unknown> }), 'data saved');
        } catch (err) {
          toast(err instanceof Error ? err.message : String(err), 'error');
        }
      },
    }, 'apply data');

    // relations
    const relList = h('div', { class: 'rel-list' });
    if (d.relations.length === 0) {
      relList.append(h('div', { class: 'muted small pad6' }, 'no relations'));
    }
    for (const r of d.relations) {
      relList.append(this.relRow(r, e.id));
    }

    // evidence
    const evList = h('div', { class: 'ev-list' });
    if (d.evidence.length === 0) {
      evList.append(h('div', { class: 'muted small pad6' }, 'no evidence attached'));
    }
    for (const v of d.evidence) {
      evList.append(
        h('div', { class: 'ev-item' },
          h('div', { class: 'row gap6' },
            h('span', { class: 'chip' }, v.kind),
            h('span', { class: 'muted small' }, fmtTs(v.ts)),
          ),
          v.url
            ? h('a', { class: 'ev-title', href: v.url, target: '_blank', rel: 'noopener noreferrer' },
              truncate(v.title ?? v.url, 80))
            : h('span', { class: 'ev-title' }, truncate(v.title ?? v.id, 80)),
          v.snippet ? h('div', { class: 'ev-snippet' }, truncate(v.snippet, 180)) : null,
        ),
      );
    }

    this.bodyEl.append(
      h('div', { class: 'grid2 gap6' }, labelInput, etypeChip),
      h('div', { class: 'row gap8 conf-row' },
        h('span', { class: 'field-label' }, 'conf'), confRange, confVal, pinnedBtn,
      ),
      h('div', { class: 'field' },
        h('span', { class: 'field-label' }, 'data (json)'),
        dataTa, applyData,
      ),
      h('div', { class: 'field' },
        h('span', { class: 'panel-label' }, `relations · ${d.relations.length}`),
        relList,
      ),
      h('div', { class: 'field' },
        h('span', { class: 'panel-label' }, `evidence · ${d.evidence.length}`),
        evList,
      ),
      this.actionsSection(e),
    );
  }

  private relRow(r: Relation, selfId: string): HTMLElement {
    const other = r.src === selfId ? r.dst : r.src;
    const dir = r.src === selfId ? '→' : '←';
    return h('div', {
      class: 'rel-item', tabindex: '0', title: `rel ${dir} ${this.cb.labelOf(other)}`,
      onclick: () => this.cb.navigate(other),
      onkeydown: (e: KeyboardEvent) => { if (e.key === 'Enter') this.cb.navigate(other); },
    },
      h('span', { class: 'rel-dir' }, dir),
      h('span', { class: 'chip chip-rel' }, r.rel),
      h('span', { class: 'rel-other' }, truncate(this.cb.labelOf(other), 34)),
    );
  }

  // ------------------------------------------------------------- actions
  private actionsSection(e: Entity): HTMLElement {
    const caseId = this.cb.caseId();
    const post = (kind: string, params: Record<string, unknown>, okMsg: string): void => {
      if (!caseId) { toast('no case selected', 'warn'); return; }
      void guarded(api.createTask(caseId, { kind, target: e.id, params }).then((t) => {
        this.cb.taskStarted(t);
        return t;
      }), okMsg);
    };

    const askQuestion = (title: string, kind: string): void => {
      void inputModal(title, 'question', 'what should the agent dig into?').then((q) => {
        if (q === null) return;
        if (!q) { toast('question required', 'warn'); return; }
        post(kind, { question: q }, `${kind} task queued`);
      });
    };

    const pivotsWrap = h('div', { class: 'pivots-wrap', hidden: true });
    const togglePivots = async (): Promise<void> => {
      if (!caseId) return;
      if (!pivotsWrap.hidden) { pivotsWrap.hidden = true; return; }
      pivotsWrap.hidden = false;
      clear(pivotsWrap);
      pivotsWrap.append(h('div', { class: 'muted small pad6' }, 'loading pivots…'));
      try {
        const { pivots } = await api.getPivots(caseId, e.id);
        clear(pivotsWrap);
        if (pivots.length === 0) {
          pivotsWrap.append(h('div', { class: 'muted small pad6' }, 'no pivot suggestions'));
        }
        for (const p of pivots) pivotsWrap.append(this.pivotCard(p, e));
      } catch (err) {
        clear(pivotsWrap);
        pivotsWrap.append(h('div', { class: 'small pad6 error-text' }, String(err instanceof Error ? err.message : err)));
      }
    };

    // merge modal (side-by-side comparison + preview verdict)
    const mergeInto = (): void => this.openMergeModal(e);

    // hub scans only fit identifier entities — names/orgs go to research+pivots
    const scannable = ['username', 'alias', 'email', 'phone', 'image'].includes(String(e.etype));
    const actionButtons: HTMLElement[] = [];
    if (scannable) {
      actionButtons.push(h('button', {
        class: 'btn btn-sm', type: 'button',
        title: `hub multi-scan (${scanKindFor(String(e.etype))})`,
        onclick: () => post('hub_scan', { scan: scanKindFor(String(e.etype)), value: e.label }, 'hub_scan queued'),
      }, '▶ hub scan'));
    }

    return h('div', { class: 'field actions-field' },
      h('span', { class: 'panel-label' }, 'actions'),
      h('div', { class: 'actions-grid' },
        ...actionButtons,
        h('button', {
          class: 'btn btn-sm', type: 'button',
          title: `flowsint enrichers: ${enrichersFor(String(e.etype)).join(', ')}`,
          onclick: () => post('flowsint_enrich', { enrichers: enrichersFor(String(e.etype)) }, 'flowsint_enrich queued'),
        }, '▶ flowsint enrich'),
        h('button', { class: 'btn btn-sm', type: 'button', onclick: () => askQuestion('openplanter research', 'openplanter_research') }, '▶ openplanter research'),
        h('button', { class: 'btn btn-sm', type: 'button', onclick: () => askQuestion('archon ground', 'archon_ground') }, '▶ archon ground'),
        h('button', {
          class: 'btn btn-sm', type: 'button', onclick: () => void togglePivots(),
        }, '◇ pivots'),
        h('button', { class: 'btn btn-sm', type: 'button', onclick: () => post('web_pivots', {}, 'web_pivots queued') }, '▶ web pivots'),
        h('button', { class: 'btn btn-sm', type: 'button', onclick: () => mergeInto() }, '⇉ merge…'),
        h('button', {
          class: 'btn btn-sm btn-danger', type: 'button',
          onclick: () => {
            if (!window.confirm(`Delete entity "${e.label}"? Its relations detach; evidence stays on the case.`)) return;
            void guarded(api.deleteEntity(e.id), () => {
              this.close();
              this.cb.graphChanged();
              return 'entity deleted';
            });
          },
        }, '✕ delete'),
      ),
      pivotsWrap,
    );
  }

  // ---------------------------------------------------------- merge modal
  /**
   * Side-by-side merge UI: both entities as cards, live same-entity preview
   * (similarity meter + colored verdict + plain-language sentence), a
   * direction selector (who survives), and a guarded Merge button — disabled
   * for `distinct` unless "merge anyway" is ticked. Enter submits.
   */
  private openMergeModal(e: Entity): void {
    const others = this.cb.entities().filter((x) => x.id !== e.id);
    if (others.length === 0) { toast('no other entities to merge with', 'warn'); return; }

    const COPY: Record<string, { badge: string; cls: string; line: string }> = {
      same: {
        badge: 'SAME', cls: 'v-same',
        line: 'These look like the same real-world entity. Merging re-points all relations and evidence to the survivor and records an audit trail — the duplicate is removed.',
      },
      related: {
        badge: 'RELATED', cls: 'v-related',
        line: 'Possible overlap. Review the cards before merging.',
      },
      distinct: {
        badge: 'DISTINCT', cls: 'v-distinct',
        line: 'These look like different entities. Merging is not recommended.',
      },
    };

    let direction: 'a' | 'b' = 'a'; // a = inspected entity survives
    let candidate: Entity = others[0];
    let seq = 0; // stale-response guard
    let lastVerdict: string | null = null;

    const survivor = (): Entity => (direction === 'a' ? e : candidate);
    const absorbed = (): Entity => (direction === 'a' ? candidate : e);

    // ---- candidate picker -------------------------------------------
    const sel = h('select', { class: 'input input-sm', 'aria-label': 'merge candidate' }) as HTMLSelectElement;
    for (const o of others) {
      sel.append(h('option', { value: o.id }, `${o.etype}: ${truncate(o.label, 36)}`));
    }
    sel.value = candidate.id;
    sel.addEventListener('change', () => {
      const hit = others.find((o) => o.id === sel.value);
      if (hit && hit.id !== candidate.id) { candidate = hit; void refresh(); }
    });

    // ---- direction selector (merge into A / merge into B) ------------
    const dirA = h('button', { class: 'btn btn-sm merge-dir-btn', type: 'button', title: 'the inspector entity survives' });
    const dirB = h('button', { class: 'btn btn-sm merge-dir-btn', type: 'button', title: 'the picked candidate survives' });
    dirA.addEventListener('click', () => { if (direction !== 'a') { direction = 'a'; void refresh(); } });
    dirB.addEventListener('click', () => { if (direction !== 'b') { direction = 'b'; void refresh(); } });

    // ---- preview panel + footer controls ------------------------------
    const cardsEl = h('div', { class: 'merge-cards' });
    const verdictEl = h('div', { class: 'merge-verdict-panel' },
      h('div', { class: 'muted small' }, 'checking similarity…'));

    const mergeAnyway = h('input', { type: 'checkbox' }) as HTMLInputElement;
    const anywayLabel = h('label', { class: 'merge-anyway small', hidden: true },
      mergeAnyway, 'merge anyway — I reviewed the cards');
    mergeAnyway.addEventListener('change', () => {
      if (lastVerdict === 'distinct') mergeBtn.disabled = !mergeAnyway.checked;
    });

    const cancelBtn = h('button', { class: 'btn', type: 'button' }, 'Cancel') as HTMLButtonElement;
    const mergeBtn = h('button', { class: 'btn btn-primary', type: 'submit' }, '⇉ merge') as HTMLButtonElement;
    mergeBtn.disabled = true;

    const entityCard = (d: EntityDetail, tag: 'A' | 'B'): HTMLElement => {
      const en = d.entity;
      const color = etypeColor(String(en.etype));
      const fields = Object.entries(en.data ?? {}).slice(0, 4);
      return h('div', { class: 'merge-card' },
        h('div', { class: 'row gap6' },
          h('span', { class: 'dot', style: `background:${color};box-shadow:0 0 6px ${color}66`, title: String(en.etype) }),
          h('span', { class: 'chip chip-etype mono' }, String(en.etype)),
          h('span', { class: 'merge-tag mono', title: tag === 'A' ? 'inspector entity' : 'candidate' }, tag),
        ),
        h('div', { class: 'merge-card-label', title: en.label }, en.label),
        fields.length > 0
          ? h('div', { class: 'merge-card-fields mono small' },
            fields.map(([k, v]) => h('div', { class: 'merge-kv' },
              h('span', { class: 'merge-k' }, `${k}:`),
              h('span', { class: 'merge-v' }, truncate(JSON.stringify(v) ?? '', 42)))))
          : h('div', { class: 'muted small' }, 'no data fields'),
        h('div', { class: 'row gap8 muted small mono merge-counts' },
          h('span', null, `rels · ${d.relations.length}`),
          h('span', null, `evidence · ${d.evidence.length}`),
        ),
      );
    };

    const renderVerdict = (pv: MergePreview): void => {
      lastVerdict = String(pv.verdict);
      const v = COPY[pv.verdict] ?? COPY.related;
      const pct = Math.max(0, Math.min(100, Math.round(Number(pv.similarity) * 100)));
      const confPct = Math.max(0, Math.min(100, Math.round(Number(pv.confidence) * 100)));
      clear(verdictEl);
      verdictEl.append(
        h('div', { class: 'row merge-sim-row' },
          h('span', { class: 'field-label' }, 'similarity'),
          h('div', {
            class: 'merge-simbar', role: 'meter', 'aria-valuemin': '0',
            'aria-valuemax': '100', 'aria-valuenow': String(pct),
          }, h('div', { class: 'merge-simbar-fill', style: `width:${pct}%` })),
          h('span', { class: 'mono small' }, `${pct}%`),
        ),
        h('div', { class: 'row gap8' },
          h('span', { class: `merge-badge ${v.cls}` }, v.badge),
          h('span', { class: 'muted small mono' }, `confidence ${confPct}% · ${String(pv.engine)}`),
        ),
        h('div', { class: 'merge-sentence' }, v.line),
      );
      const distinct = lastVerdict === 'distinct';
      mergeBtn.disabled = distinct && !mergeAnyway.checked;
      anywayLabel.hidden = !distinct;
    };

    const refresh = async (): Promise<void> => {
      const my = ++seq;
      dirA.classList.toggle('active-dir', direction === 'a');
      dirB.classList.toggle('active-dir', direction === 'b');
      dirA.textContent = `into A · ${truncate(e.label, 18)}`;
      dirB.textContent = `into B · ${truncate(candidate.label, 18)}`;

      lastVerdict = null;
      mergeBtn.disabled = true;
      anywayLabel.hidden = true;
      mergeAnyway.checked = false;
      clear(verdictEl);
      verdictEl.append(h('div', { class: 'muted small' }, 'checking similarity…'));

      try {
        const [da, db] = await Promise.all([api.getEntity(e.id), api.getEntity(candidate.id)]);
        if (my !== seq) return; // candidate/direction changed meanwhile
        clear(cardsEl);
        cardsEl.append(entityCard(da, 'A'), entityCard(db, 'B'));
      } catch (err) {
        if (my !== seq) return;
        clear(cardsEl);
        cardsEl.append(h('div', { class: 'error-text small pad8' },
          String(err instanceof Error ? err.message : err)));
        return;
      }

      try {
        const pv = await api.mergePreview(survivor().id, absorbed().id);
        if (my !== seq) return;
        renderVerdict(pv);
      } catch (err) {
        if (my !== seq) return;
        clear(verdictEl);
        verdictEl.append(h('div', { class: 'error-text small' },
          `preview failed: ${err instanceof Error ? err.message : err}`));
      }
    };

    const doMerge = (): void => {
      if (mergeBtn.disabled) return;
      const sv = survivor();
      const ab = absorbed();
      void guarded(api.mergeEntity(sv.id, ab.id).then((merged) => {
        this.cb.graphChanged(); // refresh graph
        this.open(merged.id); // refresh inspector on the survivor
        m.close();
        return merged;
      }), `merged "${ab.label}" into "${sv.label}" — audit trail kept`);
    };

    const form = h('form', { class: 'merge-form' },
      h('div', { class: 'field' },
        h('span', { class: 'field-label' }, 'duplicate candidate'),
        h('div', { class: 'merge-controls' }, sel, dirA, dirB),
      ),
      cardsEl,
      verdictEl,
      anywayLabel,
      h('div', { class: 'row row-end gap8' }, cancelBtn, mergeBtn),
    );
    form.addEventListener('submit', (ev) => { ev.preventDefault(); doMerge(); });
    cancelBtn.addEventListener('click', () => m.close());

    const m = openModal('merge entity', (body) => { body.append(form); }, { wide: true });
    void refresh();
  }

  private pivotCard(p: PivotSuggestion, e: Entity): HTMLElement {
    const caseId = this.cb.caseId();
    return h('div', { class: 'pivot-card' },
      h('div', { class: 'row gap6' },
        h('span', { class: 'chip chip-action mono' }, p.action),
        h('span', {
          class: 'pivot-priority mono', title: `priority ${p.priority.toFixed(2)}`,
        }, p.priority.toFixed(2)),
      ),
      h('div', { class: 'pivot-reason muted small' }, p.reason),
      h('button', {
        class: 'btn btn-sm btn-primary', type: 'button',
        onclick: () => {
          if (!caseId) return;
          const { kind, params } = pivotToTask(p.action, e);
          void guarded(api.createTask(caseId, { kind, target: e.id, params }).then((t) => {
            this.cb.taskStarted(t);
            return t;
          }), `${kind} queued from pivot`);
        },
      }, 'run'),
    );
  }

  // -------------------------------------------------------------- wiki tab
  private renderWiki(d: EntityDetail): void {
    const e = d.entity;
    clear(this.bodyEl);
    this.bodyEl.append(this.subtabs());

    const ta = h('textarea', {
      class: 'textarea wiki-ta mono', spellcheck: 'false',
      placeholder: '# wiki doc\n\nrender generates the skeleton; prose under Summary / Open questions is preserved.',
    }) as HTMLTextAreaElement;
    ta.value = d.doc ?? '';

    const meta = h('div', { class: 'muted small' }, d.doc ? 'doc loaded' : 'no doc yet — hit render');

    this.bodyEl.append(
      h('div', { class: 'row gap6' },
        h('button', {
          class: 'btn btn-sm', type: 'button',
          onclick: () => {
            void guarded(api.renderDoc(e.id).then((doc) => {
              ta.value = doc;
              meta.textContent = 'rendered from graph facts';
              return doc;
            }), 'doc rendered');
          },
        }, '⟐ render'),
        h('button', {
          class: 'btn btn-sm btn-primary', type: 'button',
          onclick: () => {
            void guarded(api.patchDoc(e.id, ta.value).then((doc) => {
              meta.textContent = `saved ${fmtTs(new Date().toISOString())}`;
              return doc;
            }), 'doc saved');
          },
        }, '⌸ save'),
      ),
      meta,
      ta,
    );
  }

  private subtabs(): HTMLElement {
    const mk = (key: 'inspect' | 'wiki', label: string): HTMLElement =>
      h('button', {
        class: `subtab${this.subtab === key ? ' active' : ''}`, type: 'button',
        onclick: () => {
          if (this.subtab === key || !this.detail) return;
          this.subtab = key;
          if (key === 'wiki') this.renderWiki(this.detail);
          else this.renderInspect(this.detail);
        },
      }, label);
    return h('div', { class: 'subtabs' }, mk('inspect', 'INSPECT'), mk('wiki', 'WIKI DOC'));
  }
}

export function inspectorEmptyState(): HTMLElement {
  return emptyState('no selection', 'click a node to inspect · shift-click two nodes for path mode');
}
