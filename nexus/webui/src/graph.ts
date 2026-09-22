/**
 * Cytoscape graph wrapper: etype color map, path-mode highlighting,
 * filters, layouts. All UI-specific policy lives here; main.ts drives it.
 */
import cytoscape from 'cytoscape';
import fcose from 'cytoscape-fcose';
import type { CaseGraph, Entity } from './types';
import { ETYPES } from './types';

// fcose is a plain-js extension without usable typings → register as any.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
(cytoscape as any).use(fcose);

/** Etype → node color (CONTRACT §6 / UI spec). Unlisted etypes → gray. */
export const ETYPE_COLORS: Record<string, string> = {
  person: '#f472b6',
  org: '#fb923c',
  username: '#a78bfa',
  email: '#60a5fa',
  phone: '#38bdf8',
  domain: '#4ade80',
  subdomain: '#86efac',
  ip: '#facc15',
  website: '#2dd4bf',
  social_profile: '#c084fc',
  wallet: '#f59e0b',
  image: '#f87171',
  document: '#94a3b8',
  article: '#e879f9',
  event: '#fbbf24',
  location: '#34d399',
  topic: '#e2e8f0',
  note: '#64748b',
};

export function etypeColor(etype: string): string {
  return ETYPE_COLORS[etype] ?? '#6b7280';
}

export type LayoutName = 'fcose' | 'grid' | 'circle';

const STYLES = [
  {
    selector: 'node',
    style: {
      label: 'data(label)',
      width: 'data(size)',
      height: 'data(size)',
      'background-color': 'data(color)',
      'background-opacity': 0.92,
      'border-width': 1,
      'border-color': '#0a0e14',
      'border-opacity': 0.9,
      // label under the node
      'text-valign': 'bottom',
      'text-halign': 'center',
      'text-margin-y': 7,
      'font-family': 'ui-monospace, SFMono-Regular, SF Mono, Menlo, Consolas, monospace',
      'font-size': 10,
      color: '#93a4bb',
      'text-background-color': '#0a0e14',
      'text-background-opacity': 0.72,
      'text-background-padding': 2,
      'text-background-shape': 'roundrectangle',
      'text-wrap': 'ellipsis',
      'text-max-width': 110,
      'min-zoomed-font-size': 5,
      'transition-property': 'opacity, border-color, border-width',
      'transition-duration': '160ms',
      'active-bg-size': 0,
    },
  },
  {
    selector: 'node.pinned',
    style: { 'border-color': '#fbbf24', 'border-width': 2 },
  },
  {
    selector: 'node.anchor',
    style: {
      'border-color': '#fbbf24',
      'border-width': 2,
      'border-style': 'dashed',
    },
  },
  {
    selector: 'node:selected',
    style: {
      'border-color': '#a78bfa',
      'border-width': 2,
      'border-opacity': 1,
      color: '#e2e8f0',
    },
  },
  {
    selector: 'edge',
    style: {
      label: 'data(rel)',
      width: 1,
      'line-color': 'rgba(51,65,85,0.50)',
      'line-opacity': 0.85,
      'curve-style': 'bezier',
      'target-arrow-shape': 'triangle',
      'target-arrow-color': 'rgba(51,65,85,0.50)',
      'arrow-scale': 0.6,
      'font-family': 'ui-monospace, SFMono-Regular, SF Mono, Menlo, Consolas, monospace',
      'font-size': 9,
      'text-opacity': 0,
      'text-background-color': '#0a0e14',
      'text-background-opacity': 0.85,
      'text-background-padding': 2,
      'text-background-shape': 'roundrectangle',
      color: '#8ea0b5',
      'transition-property': 'line-color, width, text-opacity, opacity',
      'transition-duration': '160ms',
      'active-bg-size': 0,
    },
  },
  {
    selector: 'edge.hover-label',
    style: { 'text-opacity': 1 },
  },
  {
    selector: 'edge.onpath',
    style: {
      'line-color': '#a78bfa',
      'target-arrow-color': '#a78bfa',
      width: 2.2,
      'text-opacity': 1,
      color: '#c4b5fd',
      'line-opacity': 1,
    },
  },
  {
    selector: 'node.onpath',
    style: { 'border-color': '#a78bfa', 'border-width': 2 },
  },
  {
    // agent working on this node — pulsing purple ring (class toggled 800ms)
    selector: 'node.busy',
    style: { 'border-color': '#a78bfa', 'border-width': 2, 'border-opacity': 1 },
  },
  {
    selector: 'node.busy.busy-pulse',
    style: { 'border-color': '#c4b5fd', 'border-width': 5 },
  },
  {
    selector: '.dim',
    style: { opacity: 0.12 },
  },
  {
    selector: '.filtered',
    style: { display: 'none' },
  },
] as unknown as cytoscape.StylesheetJson;

export interface GraphFilterState {
  search: string;
  hiddenEtypes: Set<string>;
}

function layoutOpts(name: LayoutName): cytoscape.LayoutOptions {
  if (name === 'grid') {
    return { name: 'grid', fit: true, padding: 48, animate: true, animationDuration: 300 } as cytoscape.LayoutOptions;
  }
  if (name === 'circle') {
    return { name: 'circle', fit: true, padding: 48, animate: true, animationDuration: 300 } as cytoscape.LayoutOptions;
  }
  return {
    name: 'fcose',
    quality: 'default',
    randomize: true,
    animate: true,
    animationDuration: 450,
    padding: 48,
    nodeRepulsion: 7000,
    idealEdgeLength: () => 90,
  } as unknown as cytoscape.LayoutOptions;
}

export class GraphView {
  readonly cy: cytoscape.Core;
  private layoutName: LayoutName = 'fcose';
  private filters: GraphFilterState = { search: '', hiddenEtypes: new Set() };
  private entityById = new Map<string, Entity>();
  private ro: ResizeObserver | null = null;
  private busyIds = new Set<string>();
  private busyTimer: number | null = null;

  constructor(container: HTMLElement) {
    this.cy = cytoscape({
      container,
      elements: [],
      style: STYLES,
      wheelSensitivity: 0.22,
      maxZoom: 3,
      minZoom: 0.08,
      boxSelectionEnabled: true,
    });
    // keep canvas sized with its flex container
    if (typeof ResizeObserver !== 'undefined') {
      this.ro = new ResizeObserver(() => this.cy.resize());
      this.ro.observe(container);
    }
    // edge labels on hover
    this.cy.on('mouseover', 'edge', (e) => e.target.addClass('hover-label'));
    this.cy.on('mouseout', 'edge', (e) => e.target.removeClass('hover-label'));
  }

  destroy(): void {
    if (this.busyTimer !== null) window.clearInterval(this.busyTimer);
    this.busyTimer = null;
    this.ro?.disconnect();
    this.cy.destroy();
  }

  get currentLayout(): LayoutName {
    return this.layoutName;
  }

  entities(): Entity[] {
    return [...this.entityById.values()];
  }

  entity(id: string): Entity | undefined {
    return this.entityById.get(id);
  }

  label(id: string): string {
    return this.entityById.get(id)?.label ?? id;
  }

  etypes(): string[] {
    const present = new Set<string>();
    this.entityById.forEach((e) => { present.add(String(e.etype)); });
    const known = (ETYPES as readonly string[]).filter((t) => present.has(t));
    const extra = [...present].filter((t) => !known.includes(t)).sort();
    return [...known, ...extra];
  }

  /**
   * Replace graph data. Preserves positions of surviving nodes (fresh SSE
   * data doesn't nuke the analyst's arrangement) unless `relayout`.
   */
  setGraph(g: CaseGraph, opts?: { relayout?: boolean }): void {
    const prevPos = new Map<string, cytoscape.Position>();
    const prevSelected = new Set<string>();
    this.cy.nodes().forEach((n) => { prevPos.set(n.id(), { ...n.position() }); });
    this.cy.$(':selected').forEach((n) => { prevSelected.add(n.id()); });

    this.entityById = new Map(g.nodes.map((n) => [n.id, n]));

    // node size from pagerank (fallback degree) — 18..40px
    const pr = g.analysis?.pagerank ?? {};
    const deg = g.analysis?.degree ?? {};
    let prMax = 0;
    for (const v of Object.values(pr)) prMax = Math.max(prMax, v);
    const sizeOf = (id: string): number => {
      const v = pr[id] ?? 0;
      if (prMax > 0 && v > 0) return 18 + 22 * (v / prMax);
      const d = deg[id] ?? 0;
      return Math.min(38, 18 + d * 2);
    };

    const els: cytoscape.ElementDefinition[] = g.nodes.map((n) => ({
      group: 'nodes',
      data: {
        id: n.id,
        label: n.label,
        etype: String(n.etype),
        color: etypeColor(String(n.etype)),
        size: sizeOf(n.id),
      },
      classes: n.pinned ? 'pinned' : '',
    }));
    const seenEdge = new Set<string>();
    for (const e of g.edges) {
      if (!this.entityById.has(e.src) || !this.entityById.has(e.dst)) continue;
      const key = `${e.src}|${e.dst}|${e.rel}`;
      if (seenEdge.has(key)) continue;
      seenEdge.add(key);
      els.push({
        group: 'edges',
        data: { id: e.id, source: e.src, target: e.dst, rel: e.rel },
      });
    }

    this.cy.batch(() => {
      this.cy.elements().remove();
      this.cy.add(els);
      if (!opts?.relayout) {
        this.cy.nodes().forEach((n) => {
          const p = prevPos.get(n.id());
          if (p) { n.position(p); return; }
          // park new nodes near a connected survivor (or the origin)
          let base: { x: number; y: number } = { x: 0, y: 0 };
          const firstEdge = n.connectedEdges().first();
          if (firstEdge.inside()) {
            const other = firstEdge.connectedNodes().not(n).first();
            if (other.inside() && other.isNode()) {
              base = (other as unknown as cytoscape.NodeSingular).position();
            }
          }
          const seed = (n.id().length * 37) % 60;
          n.position({ x: base.x + seed - 30, y: base.y + ((seed * 7) % 60) - 30 });
        });
      }
    });

    if (opts?.relayout || prevPos.size === 0) this.runLayout();
    else this.applyFilters();

    // restore selection where node survived
    this.cy.batch(() => {
      for (const id of prevSelected) {
        const n = this.cy.getElementById(id);
        if (n.inside() && n.isNode()) { n.select(); }
      }
    });

    // elements were rebuilt — re-arm the pulsing ring on busy targets
    if (this.busyIds.size > 0) this.setBusy(this.busyIds);
  }

  /**
   * Nodes with an agent task in flight get a pulsing ring. Cytoscape draws to
   * canvas (no CSS animation), so the pulse is a class swap on an 800ms
   * timer: .busy ↔ .busy.busy-pulse (thin ↔ thick purple border).
   */
  setBusy(ids: Iterable<string>): void {
    this.busyIds = new Set(ids);
    this.busyIds.delete('');
    this.cy.nodes().removeClass('busy');
    this.cy.nodes().removeClass('busy-pulse');
    for (const id of this.busyIds) {
      const n = this.cy.getElementById(id);
      if (n.inside() && n.isNode()) n.addClass('busy');
    }
    if (this.busyIds.size > 0 && this.busyTimer === null) {
      this.busyTimer = window.setInterval(() => {
        this.cy.nodes('.busy').toggleClass('busy-pulse');
      }, 800);
    } else if (this.busyIds.size === 0 && this.busyTimer !== null) {
      window.clearInterval(this.busyTimer);
      this.busyTimer = null;
    }
  }

  runLayout(name?: LayoutName): void {
    if (name) this.layoutName = name;
    if (this.cy.nodes().length === 0) return;
    this.applyFilters();
    const l = this.cy.layout(layoutOpts(this.layoutName));
    l.one('layoutstop', () => {
      this.cy.fit(undefined, 48);
    });
    l.run();
  }

  fit(): void {
    window.setTimeout(() => {
      if (this.cy.nodes().length > 0) this.cy.fit(undefined, 48);
    }, 30);
  }

  setFilters(search: string, hiddenEtypes: Set<string>): void {
    this.filters = { search, hiddenEtypes };
    this.applyFilters();
  }

  applyFilters(): void {
    const s = this.filters.search.trim().toLowerCase();
    const hidden = this.filters.hiddenEtypes;
    this.cy.batch(() => {
      this.cy.nodes().forEach((n) => {
        const label = String(n.data('label') ?? '').toLowerCase();
        const etype = String(n.data('etype') ?? '');
        const off = hidden.has(etype) || (s.length > 0 && !label.includes(s));
        n.removeClass('filtered');
        if (off) n.addClass('filtered');
      });
      this.cy.edges().forEach((e) => {
        e.removeClass('filtered');
        if (e.source().hasClass('filtered') || e.target().hasClass('filtered')) {
          e.addClass('filtered');
        }
      });
    });
  }

  /** Path mode. ids=null clears. Others get `dim`. */
  highlightPath(ids: string[] | null): void {
    const all = this.cy.elements();
    all.removeClass(['onpath', 'dim', 'anchor']);
    if (!ids || ids.length === 0) return;
    const set = new Set(ids);
    const nodes = this.cy.nodes().filter((n) => set.has(n.id()));
    const edges = this.cy.edges().filter((e) =>
      set.has(e.source().id()) && set.has(e.target().id()));
    const onpath = nodes.union(edges);
    all.not(onpath).addClass('dim');
    onpath.addClass('onpath');
    if (nodes.length > 0) {
      this.cy.animate({ fit: { eles: nodes, padding: 70 } }, { duration: 250 });
    }
  }

  setAnchor(id: string | null): void {
    this.cy.nodes().removeClass('anchor');
    if (id) this.cy.getElementById(id).addClass('anchor');
  }

  /** Select + pan to a node (does not fire app navigation). */
  focus(id: string): void {
    const n = this.cy.getElementById(id);
    if (!n.inside()) return;
    n.select();
    this.cy.animate({ center: { eles: n } }, { duration: 220 });
  }

  clearSelection(): void {
    this.cy.$(':selected').unselect();
  }
}
