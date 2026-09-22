/**
 * Bottom console: task rows + per-task SSE log streams (cap 500 lines).
 */
import { api } from './api';
import { clear, elapsed, h, timeAgo } from './dom';
import { streamTaskLogs } from './sse';
import type { Task, TaskLogLine } from './types';

const LOG_CAP = 500;

/** braille spinner frames for running task rows (cycled on an interval) */
const GLYPHS = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

export interface ConsoleCallbacks {
  caseId(): string | null;
  entityLabel(id: string | null): string | null;
  onTasks?(tasks: Task[]): void;
}

interface TaskEntry {
  task: Task;
  logs: string[];
  open: boolean;
  ctl: AbortController | null;
}

export class Console {
  readonly el: HTMLElement;
  private bodyEl: HTMLElement;
  private rowsEl: HTMLElement;
  private logPane: HTMLElement;
  private entries = new Map<string, TaskEntry>();
  private activeLogId: string | null = null;
  private collapsed = false;
  private ticker: number;
  private glyphTimer: number;
  private glyphIdx = 0;

  constructor(private readonly cb: ConsoleCallbacks) {
    this.rowsEl = h('div', { class: 'task-rows' });
    this.logPane = h('pre', { class: 'log-pane mono', hidden: true });
    this.bodyEl = h('div', { class: 'console-body' }, this.rowsEl, this.logPane);

    const chevron = h('button', {
      class: 'btn btn-ghost btn-sm console-chevron', type: 'button',
      title: 'collapse / expand', 'aria-expanded': 'true',
      onclick: () => this.toggle(),
    }, '▾');

    this.el = h('footer', { class: 'console', id: 'console' },
      h('div', { class: 'console-head' },
        h('span', { class: 'panel-label' }, 'task console'),
        h('span', { class: 'muted small console-count' }),
        h('span', { class: 'flex-spacer' }),
        chevron,
      ),
      this.bodyEl,
    );

    this.ticker = window.setInterval(() => this.tickElapsed(), 1000);
    this.glyphTimer = window.setInterval(() => this.tickGlyphs(), 120);
  }

  destroy(): void {
    window.clearInterval(this.ticker);
    window.clearInterval(this.glyphTimer);
    for (const e of this.entries.values()) e.ctl?.abort();
  }

  toggle(collapse?: boolean): void {
    this.collapsed = collapse ?? !this.collapsed;
    this.el.classList.toggle('collapsed', this.collapsed);
    const btn = this.el.querySelector<HTMLButtonElement>('.console-chevron');
    if (btn) {
      btn.textContent = this.collapsed ? '▴' : '▾';
      btn.setAttribute('aria-expanded', String(!this.collapsed));
    }
  }

  async refresh(caseId?: string | null): Promise<void> {
    let tasks: Task[];
    try {
      tasks = await api.listTasks(caseId ?? this.cb.caseId() ?? undefined);
    } catch {
      return;
    }
    this.applyTasks(tasks);
  }

  /** Server is source of truth; merge with local log buffers + open state. */
  applyTasks(tasks: Task[]): void {
    const seen = new Set<string>();
    for (const t of tasks) {
      seen.add(t.id);
      const prev = this.entries.get(t.id);
      if (prev) {
        prev.task = t;
      } else {
        this.entries.set(t.id, { task: t, logs: [], open: false, ctl: null });
      }
    }
    // drop tasks that vanished server-side (capped list)
    for (const id of [...this.entries.keys()]) {
      if (!seen.has(id)) this.entries.delete(id);
    }
    this.syncStreams();
    this.render();
    this.cb.onTasks?.(tasks);
  }

  /** Called when a task is created locally. */
  noteCreated(task: Task): void {
    const existing = this.entries.get(task.id);
    if (!existing) {
      this.entries.set(task.id, { task, logs: [], open: true, ctl: null });
      this.activeLogId = task.id;
    }
    this.el.classList.remove('collapsed');
    this.collapsed = false;
    void this.refresh();
  }

  /** Left-rail mini-feed click: open console + expand that task. */
  expand(id: string): void {
    const e = this.entries.get(id);
    if (!id) return;
    if (e) e.open = true;
    this.activeLogId = id;
    this.collapsed = false;
    this.el.classList.remove('collapsed');
    const btn = this.el.querySelector<HTMLButtonElement>('.console-chevron');
    if (btn) { btn.textContent = '▾'; btn.setAttribute('aria-expanded', 'true'); }
    this.render();
  }

  private syncStreams(): void {
    for (const e of this.entries.values()) {
      const running = e.task.status === 'running';
      if (running && !e.ctl) {
        const ctl = new AbortController();
        e.ctl = ctl;
        e.logs.push(`── streaming ${e.task.id} ──`);
        void streamTaskLogs(e.task.id, (line) => this.onLogLine(e, line), ctl.signal)
          .catch(() => { /* stream broke; polling refresh will catch up */ })
          .finally(() => {
            if (e.ctl === ctl) e.ctl = null;
            this.renderIfActive(e.task.id);
          });
      } else if (!running && e.ctl) {
        e.ctl.abort();
        e.ctl = null;
      }
    }
  }

  private onLogLine(e: TaskEntry, line: TaskLogLine): void {
    const lvl = (line.level ?? 'info').toLowerCase();
    const mark = lvl === 'error' ? '!!' : lvl === 'warn' ? ' !' : '· ';
    e.logs.push(`${line.ts ?? ''} ${mark} ${line.msg ?? ''}`);
    if (e.logs.length > LOG_CAP) e.logs.splice(0, e.logs.length - LOG_CAP);
    // a log line implies liveness — light refresh of status text only
    this.renderIfActive(e.task.id);
  }

  private renderIfActive(taskId: string): void {
    if (this.activeLogId !== taskId) return;
    const e = this.entries.get(taskId);
    if (!e) return;
    this.renderLogPane(e);
  }

  private render(): void {
    clear(this.rowsEl);
    const list = [...this.entries.values()]
      .sort((a, b) => b.task.created_at.localeCompare(a.task.created_at))
      .slice(0, 40);

    const count = this.el.querySelector<HTMLElement>('.console-count');
    if (count) {
      const running = list.filter((e) => e.task.status === 'running').length;
      count.textContent = `${list.length} task${list.length === 1 ? '' : 's'}${running ? ` · ${running} running` : ''}`;
    }

    if (list.length === 0) {
      this.rowsEl.append(h('div', { class: 'muted small pad8' },
        'no tasks yet — select an entity and run hub scan / enrich / research from the inspector'));
    }

    for (const e of list) {
      this.rowsEl.append(this.taskRow(e));
    }
    const active = this.activeLogId ? this.entries.get(this.activeLogId) : null;
    if (active && active.open) this.renderLogPane(active);
    else { this.logPane.hidden = true; }
  }

  private taskRow(e: TaskEntry): HTMLElement {
    const t = e.task;
    const ing = t.ingested;
    const ingBits: string[] = [];
    if (ing?.entities) ingBits.push(`+${ing.entities}e`);
    if (ing?.relations) ingBits.push(`+${ing.relations}r`);
    if (ing?.evidence) ingBits.push(`+${ing.evidence}v`);
    const status = t.status === 'running'
      ? h('span', { class: 'task-glyph mono', title: 'running' }, GLYPHS[0])
      : h('span', { class: `dot dot-${t.status === 'done' ? 'up' : t.status === 'error' ? 'down' : 'off'}`, title: t.status });

    const targetLabel = t.target ? (this.cb.entityLabel(t.target) ?? t.target) : '—';

    const row = h('div', {
      class: `task-row st-${t.status}${e.open ? ' open' : ''}`, tabindex: '0',
      'data-task': t.id,
      onclick: () => {
        e.open = !e.open;
        this.activeLogId = e.open ? t.id : this.activeLogId;
        if (e.open && t.logs && t.logs.length > 0 && e.logs.length === 0) {
          e.logs.push(...t.logs.slice(-LOG_CAP)); // seed from snapshot
        }
        this.render();
      },
      onkeydown: (ev: KeyboardEvent) => { if (ev.key === 'Enter') row.click(); },
    },
      status,
      h('span', { class: 'task-kind mono' }, t.kind),
      h('span', { class: 'task-target', title: t.target ?? '' }, targetLabel),
      h('span', { class: 'task-ing mono' }, ingBits.join(' ')),
      h('span', { class: 'task-elapsed mono', 'data-created': t.created_at, 'data-finished': t.finished_at ?? '' },
        elapsed(t.created_at, t.finished_at)),
      h('span', { class: 'task-summary muted', title: t.summary ?? '' }, t.summary ?? (t.status === 'error' ? 'error' : '')),
    );
    return row;
  }

  private renderLogPane(e: TaskEntry): void {
    this.logPane.hidden = false;
    const nearBottom = this.logPane.scrollHeight - this.logPane.scrollTop - this.logPane.clientHeight < 40;
    const text = e.logs.length > 0
      ? e.logs.join('\n')
      : (e.task.logs && e.task.logs.length > 0
        ? e.task.logs.slice(-LOG_CAP).join('\n')
        : (e.task.status === 'queued' ? 'queued…' : 'no log lines yet'));
    this.logPane.textContent = text;
    if (nearBottom) this.logPane.scrollTop = this.logPane.scrollHeight;
  }

  private tickElapsed(): void {
    const nodes = this.el.querySelectorAll<HTMLElement>('.task-elapsed[data-created]');
    for (const n of nodes) {
      if (n.dataset.finished) continue;
      const created = n.dataset.created;
      if (created) n.textContent = elapsed(created, null);
    }
  }

  /** Cycle the braille glyph on every visible running task row. */
  private tickGlyphs(): void {
    const nodes = this.el.querySelectorAll<HTMLElement>('.task-glyph');
    if (nodes.length === 0) return;
    this.glyphIdx = (this.glyphIdx + 1) % GLYPHS.length;
    const g = GLYPHS[this.glyphIdx];
    for (const n of nodes) n.textContent = g;
  }

  ageOfNewest(): string | null {
    const list = [...this.entries.values()].sort((a, b) => b.task.created_at.localeCompare(a.task.created_at));
    return list[0] ? timeAgo(list[0].task.created_at) : null;
  }
}
