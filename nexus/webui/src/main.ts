/**
 * NEXUS — qalarc investigation workbench. Orchestrator: builds the shell,
 * wires rail / graph / inspector / console / tabs / SSE, owns app state.
 */
import './style.css';
import { api, ApiError } from './api';
import { Console } from './console';
import { clear, debounce, emptyState, h, toast } from './dom';
import { GraphView, etypeColor, type LayoutName } from './graph';
import { GuidePane } from './guide';
import { Inspector } from './inspector';
import { Rail } from './rail';
import { loadSettings, openSettingsModal } from './settings';
import { EventLineStream } from './sse';
import { AnalysisPane, StoryPane, TimelinePane } from './tabs';
import type { BusEvent, Case, CaseGraph, NexusSettings, Task } from './types';

type TabName = 'graph' | 'timeline' | 'analysis' | 'story' | 'guide';

function main(): void {
  let settings: NexusSettings = loadSettings();

  // ------------------------------------------------------------- state
  const state = {
    caseId: null as string | null,
    case: null as Case | null,
    graph: null as CaseGraph | null,
    tab: 'graph' as TabName,
    pathAnchor: null as string | null,
    hiddenEtypes: new Set<string>(),
    lastMiniTaskId: null as string | null,
  };

  // -------------------------------------------------------------- shell
  const appRoot = document.getElementById('app') as HTMLElement;

  const caseNameEl = h('span', { class: 'case-name-top mono', title: 'active case' }, '—');

  const tabsEl = new Map<TabName, HTMLElement>();
  const tabBar = h('nav', { class: 'tabs', role: 'tablist' });
  for (const t of ['graph', 'timeline', 'analysis', 'story', 'guide'] as const) {
    const b = h('button', {
      class: 'tab', role: 'tab', type: 'button', dataset: { tab: t },
      onclick: () => setTab(t),
    }, t);
    tabsEl.set(t, b);
    tabBar.append(b);
  }

  const sseDot = h('span', { class: 'dot dot-off', id: 'sse-dot', title: 'event bus offline' });

  // live "agents working" pill — appears while ≥1 task is running
  const busyCount = h('span', { class: 'mono' }, '0');
  const agentsBusy = h('span', {
    class: 'agents-busy', id: 'agents-busy', hidden: true,
    title: 'agent tasks currently running',
  }, '◈ agents working · ', busyCount);

  const gear = h('button', {
    class: 'btn btn-ghost', type: 'button', title: 'settings', 'aria-label': 'settings',
    onclick: () => {
      openSettingsModal(settings, (s) => {
        settings = s;
        void rail.refreshHealth();
      });
    },
  }, '⚙');

  const toggleLeft = h('button', {
    class: 'btn btn-ghost only-narrow', type: 'button', title: 'toggle cases rail',
    onclick: () => document.body.classList.toggle('show-left'),
  }, '▤');
  const toggleRight = h('button', {
    class: 'btn btn-ghost only-narrow', type: 'button', title: 'toggle inspector',
    onclick: () => document.body.classList.toggle('show-right'),
  }, '▨');

  const topbar = h('header', { id: 'topbar' },
    h('div', { class: 'brand' },
      h('span', { class: 'brand-mark mono' }, 'NEXUS ▚'),
      h('span', { class: 'brand-sub' }, 'qalarc investigation workbench'),
    ),
    h('div', { class: 'topbar-case' }, caseNameEl),
    tabBar,
    agentsBusy,
    h('span', { class: 'flex-spacer' }),
    toggleLeft, toggleRight, sseDot, gear,
  );

  // graph toolbar
  const searchIn = h('input', {
    class: 'input input-sm toolbar-search', type: 'search',
    placeholder: 'filter by label…', 'aria-label': 'filter nodes by label',
  }) as HTMLInputElement;
  const chipsEl = h('div', { class: 'chips', role: 'group', 'aria-label': 'etype filters' });
  const pathBadge = h('span', { class: 'path-badge mono', hidden: true },
    '⇢ path',
    h('button', {
      class: 'btn btn-ghost btn-xs', type: 'button', title: 'clear path',
      onclick: () => clearPath(),
    }, '✕'),
  );
  const layoutGroup = h('div', { class: 'layout-group', role: 'group' });
  const layoutDefs: Array<[LayoutName, string, string]> = [
    ['fcose', 'force', 'fcose force layout'],
    ['grid', 'grid', 'grid layout'],
    ['circle', 'circle', 'circle layout'],
  ];
  for (const [name, label, title] of layoutDefs) {
    layoutGroup.append(h('button', {
      class: 'btn btn-sm layout-btn', type: 'button', dataset: { layout: name }, title,
      onclick: () => graph.runLayout(name),
    }, label));
  }
  layoutGroup.append(h('button', {
    class: 'btn btn-sm', type: 'button', title: 'fit view',
    onclick: () => graph.fit(),
  }, '⤢'));

  const cyContainer = h('div', { id: 'cy' });
  const graphEmpty = h('div', { class: 'graph-empty', hidden: true });
  const graphToolbar = h('div', { class: 'graph-toolbar' },
    searchIn, chipsEl, pathBadge, h('span', { class: 'flex-spacer' }), layoutGroup,
  );
  const paneGraph = h('section', { class: 'pane', id: 'pane-graph' },
    graphToolbar,
    h('div', { class: 'graph-wrap' }, cyContainer, graphEmpty),
  );
  const center = h('main', { id: 'center' }, paneGraph);
  const frame = h('div', { id: 'frame' });

  appRoot.append(topbar, frame);

  // ----------------------------------------------- components (closures are
  // lazy, so construction order below is safe; the cy container must be IN
  // THE DOM before GraphView exists — cytoscape's init runs a probe layout
  // that needs a non-zero container size)
  const rail = new Rail({
    selectCase: (id) => void selectCase(id),
    selectEntity: (id) => navigate(id),
    caseChanged: () => {
      const next = rail.activeCaseId;
      if (next) void selectCase(next);
      else {
        state.caseId = null;
        state.case = null;
        caseNameEl.textContent = '—';
        renderNoCase();
      }
    },
    graphDirty: () => scheduleGraphRefresh(),
    tasksChanged: () => consolePane.expand(state.lastMiniTaskId ?? ''),
    currentCaseId: () => state.caseId,
    currentEntities: () => graph.entities(),
  });

  const inspector = new Inspector({
    caseId: () => state.caseId,
    labelOf: (id) => graph.label(id),
    entities: () => graph.entities(),
    navigate: (id) => navigate(id),
    graphChanged: () => scheduleGraphRefresh(),
    taskStarted: (t) => consolePane.noteCreated(t),
  });

  const consolePane = new Console({
    caseId: () => state.caseId,
    entityLabel: (id) => (id ? graph.label(id) : null),
    onTasks: (tasks) => {
      state.lastMiniTaskId = tasks[0]?.id ?? null;
      rail.refreshTasks(tasks);
      updateAgentsBusy(tasks);
    },
  });

  const timeline = new TimelinePane({
    caseId: () => state.caseId,
    entities: () => graph.entities(),
    navigate: (id) => navigate(id),
  });

  const analysis = new AnalysisPane({
    caseId: () => state.caseId,
    entities: () => graph.entities(),
    labelOf: (id) => graph.label(id),
    navigate: (id) => navigate(id),
    taskStarted: (t) => consolePane.noteCreated(t),
  });

  const story = new StoryPane({
    caseId: () => state.caseId,
    case: () => state.case,
  });

  const guide = new GuidePane();

  center.append(timeline.el, analysis.el, story.el, guide.el);
  frame.append(rail.el, center, inspector.el);
  appRoot.append(consolePane.el);

  // container is attached now — safe to boot cytoscape
  const graph = new GraphView(cyContainer);

  // debug/automation handle (used by smoke tests + console poking)
  (window as unknown as Record<string, unknown>).__nexus = { graph, state };

  // ------------------------------------------------------------ app logic
  async function selectCase(id: string): Promise<void> {
    state.caseId = id;
    rail.setActive(id);
    clearPath();
    inspector.close();
    document.body.classList.remove('show-left');
    state.hiddenEtypes.clear();
    searchIn.value = '';
    await Promise.allSettled([
      fetchGraph(false),
      consolePane.refresh(id),
    ]);
    void timeline.refresh(id);
    try {
      const { case: c } = await api.getCase(id);
      state.case = c;
      caseNameEl.textContent = c.name;
    } catch { /* graph fetch surfaced errors already */ }
  }

  async function fetchGraph(relayout: boolean): Promise<void> {
    if (!state.caseId) return;
    const g = await api.getGraph(state.caseId);
    state.graph = g;
    graph.setGraph(g, { relayout });
    renderChips();
    analysis.refreshFromGraph(g);
    renderGraphEmpty(g.nodes.length === 0);
    if (inspector.isOpen) void inspector.refresh();
  }

  const scheduleGraphRefresh = debounce(() => {
    fetchGraph(false).catch((e) =>
      toast(e instanceof Error ? e.message : String(e), 'warn'));
  }, 700);

  function renderGraphEmpty(empty: boolean): void {
    clear(graphEmpty);
    if (!empty) { graphEmpty.hidden = true; return; }
    graphEmpty.hidden = false;
    graphEmpty.append(emptyState(
      'empty graph',
      'add entities from the left rail, or run a hub scan from an entity inspector',
    ));
  }

  function renderNoCase(): void {
    caseNameEl.textContent = '—';
    state.graph = null;
    graph.setGraph(
      { nodes: [], edges: [], analysis: { degree: {}, pagerank: {}, communities: {}, components: [] } },
      { relayout: true },
    );
    renderChips();
    graphEmpty.hidden = false;
    clear(graphEmpty);
    graphEmpty.append(emptyState('no case selected', 'create a case in the left rail to begin'));
    analysis.refreshFromGraph({ nodes: [], edges: [], analysis: { degree: {}, pagerank: {}, communities: {}, components: [] } });
  }

  function navigate(entityId: string): void {
    if (!graph.entity(entityId)) {
      toast('entity not on this graph', 'warn');
      return;
    }
    inspector.open(entityId);
    graph.focus(entityId);
    if (!document.body.classList.contains('show-right') && window.innerWidth <= 1100) {
      document.body.classList.add('show-right');
    }
  }

  function clearPath(): void {
    state.pathAnchor = null;
    graph.setAnchor(null);
    graph.highlightPath(null);
    pathBadge.hidden = true;
  }

  async function shiftPathTo(id: string): Promise<void> {
    if (!state.pathAnchor) {
      state.pathAnchor = id;
      graph.setAnchor(id);
      toast('path anchor set — shift-click a second node', 'info', 2500);
      return;
    }
    const a = state.pathAnchor;
    state.pathAnchor = null;
    graph.setAnchor(null);
    if (a === id) return;
    if (!state.caseId) return;
    try {
      const { path } = await api.getPath(state.caseId, a, id);
      if (path && path.length > 0) {
        graph.highlightPath(path);
        pathBadge.hidden = false;
        toast(`path found: ${path.length} node${path.length === 1 ? '' : 's'}`, 'ok', 2500);
      } else {
        graph.highlightPath(null);
        pathBadge.hidden = true;
        toast('no path between those nodes', 'warn');
      }
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), 'error');
    }
  }

  function renderChips(): void {
    clear(chipsEl);
    for (const t of graph.etypes()) {
      const on = !state.hiddenEtypes.has(t);
      chipsEl.append(h('button', {
        class: `chip chip-filter${on ? '' : ' off'}`, type: 'button',
        title: `${on ? 'hide' : 'show'} ${t}`,
        onclick: () => {
          if (state.hiddenEtypes.has(t)) state.hiddenEtypes.delete(t);
          else state.hiddenEtypes.add(t);
          renderChips();
          applyFilters();
        },
      },
        h('span', { class: 'dot dot-inline', style: `background:${etypeColor(t)}` }),
        t,
      ));
    }
  }

  function applyFilters(): void {
    graph.setFilters(searchIn.value, state.hiddenEtypes);
  }

  /** Server task truth → topbar pill + pulsing ring on target nodes. */
  function updateAgentsBusy(tasks: Task[]): void {
    const running = tasks.filter((t) => t.status === 'running');
    agentsBusy.hidden = running.length === 0;
    busyCount.textContent = String(running.length);
    graph.setBusy(new Set(
      running.map((t) => t.target).filter((x): x is string =>
        typeof x === 'string' && x.length > 0),
    ));
  }

  function onBusEvent(ev: BusEvent): void {
    switch (ev.type) {
      case 'entity_added':
      case 'case_updated':
        if (ev.case_id && state.caseId && ev.case_id === state.caseId) scheduleGraphRefresh();
        if (ev.type === 'case_updated') void rail.refreshCases();
        break;
      case 'task_update':
        void consolePane.refresh();
        break;
      default:
        break;
    }
  }

  // --------------------------------------------------------------- wiring
  searchIn.addEventListener('input', applyFilters);

  graph.cy.on('tap', 'node', (evt) => {
    const id = evt.target.id();
    const orig = evt.originalEvent as MouseEvent | undefined;
    if (orig?.shiftKey) { void shiftPathTo(id); return; }
    clearPath();
    navigate(id);
  });

  graph.cy.on('tap', (evt) => {
    if (evt.target === graph.cy) {
      clearPath();
      inspector.close();
    }
  });

  window.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if (document.querySelector('.modal-backdrop')) return; // modal handles itself
    if (state.pathAnchor || !pathBadge.hidden) { clearPath(); return; }
    if (inspector.isOpen) inspector.close();
  });

  const bus = new EventLineStream('/api/events', (ev) => onBusEvent(ev), (connected) => {
    sseDot.classList.toggle('dot-up', connected);
    sseDot.classList.toggle('dot-off', !connected);
    sseDot.title = connected ? 'event bus connected' : 'event bus offline (retrying)';
  });
  bus.start();

  function setTab(tab: TabName): void {
    state.tab = tab;
    for (const [name, btn] of tabsEl) {
      btn.classList.toggle('active', name === tab);
      btn.setAttribute('aria-selected', String(name === tab));
    }
    const paneIds: Record<TabName, string> = {
      graph: 'pane-graph', timeline: 'pane-timeline',
      analysis: 'pane-analysis', story: 'pane-story', guide: 'pane-guide',
    };
    for (const pid of Object.values(paneIds)) {
      const el = document.getElementById(pid);
      if (el) el.hidden = pid !== paneIds[tab];
    }
    if (tab === 'timeline') void timeline.refresh();
    if (tab === 'story') void story.refresh();
    if (tab === 'analysis' && state.graph) analysis.refreshFromGraph(state.graph);
    if (tab === 'graph') graph.fit();
  }

  // ----------------------------------------------------------------- boot
  async function boot(): Promise<void> {
    window.setInterval(() => void consolePane.refresh(), 15000);
    try {
      await rail.refreshHealth();
    } catch { /* rendered as down */ }
    try {
      await rail.refreshCases();
      const first = rail.activeCaseId;
      if (first) await selectCase(first);
      else renderNoCase();
    } catch (e) {
      renderNoCase();
      toast(e instanceof ApiError ? e.message : String(e), 'warn');
    }
    void consolePane.refresh();
  }

  void boot();
  setTab('graph');
}

main();
