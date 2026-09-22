/**
 * Left rail: cases list, connectors health, tasks mini-feed, add-entity form.
 */
import { api } from './api';
import { clear, emptyState, h, guarded, timeAgo, toast } from './dom';
import { ETYPES, type Case, type Entity, type Health, type Task } from './types';

export interface RailCallbacks {
  selectCase(id: string): void;
  selectEntity(id: string): void;
  caseChanged(): void;
  graphDirty(): void;
  tasksChanged(): void;
  currentCaseId(): string | null;
  currentEntities(): Entity[];
}

const STATUS_DOT: Record<string, string> = {
  active: 'dot dot-up',
  paused: 'dot dot-warn',
  archived: 'dot dot-off',
  closed: 'dot dot-off',
};

export class Rail {
  readonly el: HTMLElement;
  private casesList: HTMLElement;
  private healthList: HTMLElement;
  private tasksList: HTMLElement;
  private entityForm: HTMLFormElement;
  private etypeSelect: HTMLSelectElement;
  private labelInput: HTMLInputElement;
  private activeId: string | null = null;
  private cases: Case[] = [];

  constructor(private readonly cb: RailCallbacks) {
    this.el = h('aside', { class: 'rail rail-left', id: 'rail-left' });

    // ---- cases -------------------------------------------------------
    this.casesList = h('div', { class: 'cases-list', role: 'list' });
    const newName = h('input', {
      class: 'input input-sm', type: 'text', placeholder: 'new case name…',
      spellcheck: 'false', 'aria-label': 'new case name',
    }) as HTMLInputElement;
    const newForm = h('form', { class: 'row gap6 new-case-row' },
      newName,
      h('button', { class: 'btn btn-sm btn-primary', type: 'submit', title: 'create case' }, '+'),
    );
    newForm.addEventListener('submit', (e) => {
      e.preventDefault();
      const name = newName.value.trim();
      if (!name) return;
      newName.value = '';
      void guarded(api.createCase({ name }), (c) => {
        void this.refreshCases(c.id);
        return `case created: ${c.name}`;
      });
    });

    this.el.append(
      h('section', { class: 'panel panel-rail' },
        h('header', { class: 'panel-head' }, h('span', { class: 'panel-label' }, 'cases')),
        newForm,
        this.casesList,
      ),
    );

    // ---- connectors ----------------------------------------------------
    this.healthList = h('div', { class: 'health-list' });
    this.el.append(
      h('section', { class: 'panel panel-rail' },
        h('header', { class: 'panel-head' },
          h('span', { class: 'panel-label' }, 'connectors'),
          h('button', {
            class: 'btn btn-ghost btn-sm', type: 'button', title: 'refresh health',
            onclick: () => void this.refreshHealth(),
          }, '↻'),
        ),
        this.healthList,
      ),
    );

    // ---- tasks mini-feed -------------------------------------------------
    this.tasksList = h('div', { class: 'minitasks' });
    this.el.append(
      h('section', { class: 'panel panel-rail' },
        h('header', { class: 'panel-head' }, h('span', { class: 'panel-label' }, 'tasks')),
        this.tasksList,
      ),
    );

    // ---- add entity ------------------------------------------------------
    this.etypeSelect = h('select', { class: 'input input-sm', 'aria-label': 'entity type' }) as HTMLSelectElement;
    for (const t of ETYPES) {
      this.etypeSelect.append(h('option', { value: t }, t));
    }
    this.etypeSelect.value = 'username';
    this.labelInput = h('input', {
      class: 'input input-sm', type: 'text', placeholder: 'label…',
      spellcheck: 'false', 'aria-label': 'entity label',
    }) as HTMLInputElement;
    this.entityForm = h('form', { class: 'add-entity-form' },
      this.etypeSelect,
      this.labelInput,
      h('button', { class: 'btn btn-sm btn-primary btn-block', type: 'submit' }, '+ entity'),
    ) as HTMLFormElement;
    this.entityForm.addEventListener('submit', (e) => {
      e.preventDefault();
      this.submitEntity();
    });
    this.el.append(
      h('section', { class: 'panel panel-rail' },
        h('header', { class: 'panel-head' }, h('span', { class: 'panel-label' }, 'add entity')),
        this.entityForm,
      ),
    );
  }

  get isOpen(): boolean { return true; }

  get activeCaseId(): string | null { return this.activeId; }

  private submitEntity(): void {
    const caseId = this.cb.currentCaseId();
    const label = this.labelInput.value.trim();
    if (!caseId) { toast('select a case first', 'warn'); return; }
    if (!label) { toast('label required', 'warn'); return; }
    const etype = this.etypeSelect.value;
    this.labelInput.value = '';
    void guarded(api.addEntity(caseId, { etype, label }), (r) =>
      r.deduplicated ? `"${label}" merged with existing entity` : `entity added: ${label}`);
  }

  async refreshCases(selectId?: string): Promise<void> {
    const cases = await api.listCases();
    this.cases = cases;
    if (selectId) this.activeId = selectId;
    else if (!cases.some((c) => c.id === this.activeId)) {
      this.activeId = cases[0]?.id ?? null;
    }
    this.renderCases();
    const target = this.activeId;
    if (selectId && target === selectId) this.cb.selectCase(target);
  }

  setActive(id: string | null): void {
    this.activeId = id;
    this.renderCases();
  }

  private renderCases(): void {
    clear(this.casesList);
    if (this.cases.length === 0) {
      this.casesList.append(emptyState('no cases', 'create one above to begin'));
      return;
    }
    for (const c of this.cases) {
      const row = h('div', {
        class: `case-row${c.id === this.activeId ? ' active' : ''}`,
        role: 'listitem', tabindex: '0',
        onclick: () => this.cb.selectCase(c.id),
        onkeydown: (e: KeyboardEvent) => { if (e.key === 'Enter') this.cb.selectCase(c.id); },
      },
        h('span', { class: STATUS_DOT[c.status] ?? 'dot dot-off', title: c.status }),
        h('span', { class: 'case-name', title: c.name }, c.name),
        h('span', { class: 'case-updated' }, timeAgo(c.updated_at)),
        h('button', {
          class: 'btn btn-ghost btn-xs case-del', type: 'button', title: 'delete case',
          onclick: (e: Event) => {
            e.stopPropagation();
            if (!window.confirm(`Delete case "${c.name}" and all its data?`)) return;
            void guarded(api.deleteCase(c.id), () => {
              void this.refreshCases();
              this.cb.caseChanged();
              return 'case deleted';
            });
          },
        }, '✕'),
      );
      this.casesList.append(row);
    }
  }

  async refreshHealth(): Promise<void> {
    let health: Health;
    try {
      health = await api.health();
    } catch {
      this.renderHealth(null);
      return;
    }
    this.renderHealth(health);
  }

  private renderHealth(health: Health | null): void {
    clear(this.healthList);
    const connectors: Array<[string, string]> = health
      ? Object.entries(health.connectors)
      : ['hub', 'flowsint', 'openplanter', 'archon', 'laya'].map((k) => [k, 'down']);
    for (const [name, state] of connectors) {
      const cls = state === 'up' ? 'dot-up' : state === 'down' ? 'dot-down' : 'dot-off';
      this.healthList.append(
        h('div', {
          class: 'health-row', title: `${name}: ${state}`,
        },
          h('span', { class: `dot ${cls}` }),
          h('span', { class: 'health-name' }, name),
          h('span', { class: `health-state state-${state}` }, state),
        ),
      );
    }
  }

  refreshTasks(tasks: Task[]): void {
    clear(this.tasksList);
    const last = tasks.slice(0, 5);
    if (last.length === 0) {
      this.tasksList.append(h('div', { class: 'muted small pad6' }, 'no tasks yet — run one from the inspector'));
      return;
    }
    for (const t of last) {
      const row = h('div', {
        class: `minitask st-${t.status}`, tabindex: '0',
        title: `${t.kind} — ${t.status}${t.summary ? ` (${t.summary})` : ''}`,
        onclick: () => this.cb.tasksChanged(),
        onkeydown: (e: KeyboardEvent) => { if (e.key === 'Enter') this.cb.tasksChanged(); },
      },
        h('span', { class: 'dot' }, ),
        h('span', { class: 'minitask-kind' }, t.kind),
        h('span', { class: 'minitarget' }, t.target ? this.shortTarget(t.target) : ''),
      );
      this.tasksList.append(row);
    }
  }

  private shortTarget(id: string): string {
    const e = this.cb.currentEntities().find((x) => x.id === id);
    return e ? e.label : id;
  }

  focusEntityForm(): void {
    this.labelInput.focus();
  }

  prefillEntity(etype: string, label: string): void {
    if ((ETYPES as readonly string[]).includes(etype)) this.etypeSelect.value = etype;
    this.labelInput.value = label;
  }
}
