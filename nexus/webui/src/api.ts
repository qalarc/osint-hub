/**
 * Typed API client for the nexus-server HTTP surface (CONTRACT.md §4).
 * Every fetch is cache: no-store; `{"error": ...}` bodies become ApiError
 * for the UI to toast.
 *
 * Connection settings (browser-local, NOT part of nexus_settings):
 * - API base — resolved once at module load: `?api=<url>` query param
 *   (persisted to localStorage) → localStorage `nexus_api_base` → same-origin.
 *   Lets a statically-hosted UI talk to a remote nexus server.
 * - Bearer token — localStorage `nexus_token`; attached as
 *   `Authorization: Bearer …` to every API + SSE request when set
 *   (server gate: NEXUS_TOKEN env; /api/health stays open).
 */
import type {
  Case, CaseGraph, Entity, EntityDetail, Evidence, Health, LayaRanked,
  PivotSuggestion, Relation, Task, TimelineEvent, NexusSettings,
} from './types';

const LS_API_BASE = 'nexus_api_base';
const LS_TOKEN = 'nexus_token';

/** Persist browser-connection fields (settings modal Save). Base change needs a reload. */
export function saveConnection(apiBase: string, token: string): void {
  try {
    localStorage.setItem(LS_API_BASE, normalizeBase(apiBase));
    localStorage.setItem(LS_TOKEN, token.trim());
  } catch { /* storage unavailable */ }
}

function resolveApiBase(): string {
  try {
    const q = new URLSearchParams(window.location.search).get('api');
    if (q !== null) {
      const b = normalizeBase(q);
      localStorage.setItem(LS_API_BASE, b); // sticky: '' = same-origin
      return b;
    }
    return normalizeBase(localStorage.getItem(LS_API_BASE));
  } catch {
    return ''; // no storage / no window — same-origin default
  }
}

/** Resolved API base ('' = same-origin). Change requires a reload (settings modal handles it). */
export const API_BASE: string = resolveApiBase();

/** Prefix a root-relative path ('/api/…') with the configured base. */
export function withBase(path: string): string {
  return `${API_BASE}${path}`;
}

/** Same normalization resolveApiBase applies (trim + strip trailing slashes). */
export function normalizeBase(v: string | null | undefined): string {
  return (v ?? '').trim().replace(/\/+$/, '');
}

/** `Authorization: Bearer …` headers for the nexus server when a token is set. */
export function authHeaders(): Record<string, string> {
  try {
    const token = (localStorage.getItem(LS_TOKEN) ?? '').trim();
    if (token) return { authorization: `Bearer ${token}` };
  } catch { /* storage unavailable */ }
  return {};
}

/** Currently stored bearer token ('' = none) — for the settings modal display. */
export function storedToken(): string {
  try { return localStorage.getItem(LS_TOKEN) ?? ''; } catch { return ''; }
}

export class ApiError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

function errFromBody(status: number, body: unknown): ApiError {
  if (body && typeof body === 'object' && 'error' in body) {
    const e = (body as { error: unknown }).error;
    if (typeof e === 'string' && e) return new ApiError(status, e);
  }
  return new ApiError(status, `${status} ${STATUS_TEXT[status] ?? ''}`.trim());
}

const STATUS_TEXT: Record<number, string> = {
  400: 'bad request', 401: 'unauthorized', 403: 'forbidden', 404: 'not found',
  409: 'conflict', 422: 'unprocessable', 500: 'internal error', 502: 'bad gateway',
  503: 'unavailable',
};

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const headers: Record<string, string> = { ...authHeaders() };
  if (init?.headers) {
    for (const [k, v] of new Headers(init.headers)) headers[k.toLowerCase()] = v;
  }
  let res: Response;
  try {
    res = await fetch(withBase(`/api${path}`), { cache: 'no-store', ...init, headers });
  } catch (e) {
    throw new ApiError(0, `network error — is the nexus server up? (${String(e)})`);
  }
  const text = await res.text();
  let body: unknown = null;
  if (text.length > 0) {
    try { body = JSON.parse(text); } catch { body = null; }
  }
  if (!res.ok) throw errFromBody(res.status, body);
  return body as T;
}

async function requestText(path: string): Promise<string> {
  let res: Response;
  try {
    res = await fetch(withBase(`/api${path}`), { cache: 'no-store', headers: authHeaders() });
  } catch (e) {
    throw new ApiError(0, `network error — is the nexus server up? (${String(e)})`);
  }
  const text = await res.text();
  if (!res.ok) {
    let body: unknown = null;
    try { body = JSON.parse(text); } catch { body = null; }
    throw errFromBody(res.status, body);
  }
  return text;
}

function json(method: string, payload?: unknown): RequestInit {
  return {
    method,
    headers: { 'content-type': 'application/json' },
    body: payload === undefined ? undefined : JSON.stringify(payload),
  };
}

const enc = encodeURIComponent;

export interface NewEntityBody {
  etype: string;
  label: string;
  data?: Record<string, unknown>;
  confidence?: number;
}

export interface NewTaskBody {
  kind: string;
  target?: string | null;
  params?: Record<string, unknown>;
}

/** POST /api/entities/{id}/merge/preview reply (CONTRACT.md §4). */
export interface MergePreview {
  similarity: number;
  verdict: 'same' | 'related' | 'distinct' | string;
  confidence: number;
  engine: string;
}

export const api = {
  // ---- health / admin -------------------------------------------------
  health(): Promise<Health> {
    return request<Health>('/health');
  },
  adminConfig(patch: Partial<NexusSettings>): Promise<Record<string, unknown>> {
    return request<Record<string, unknown>>('/admin/config', json('POST', patch));
  },

  // ---- cases -----------------------------------------------------------
  listCases(): Promise<Case[]> {
    return request<{ cases: Case[] }>('/cases').then((r) => r.cases);
  },
  createCase(body: { name: string; notes?: string; tags?: string[] }): Promise<Case> {
    return request<{ case: Case }>('/cases', json('POST', body)).then((r) => r.case);
  },
  getCase(id: string): Promise<{ case: Case; stats: Record<string, number> }> {
    return request<{ case: Case; stats: Record<string, number> }>(`/cases/${enc(id)}`);
  },
  patchCase(id: string, body: { status?: string; notes?: string }): Promise<Case> {
    return request<{ case: Case }>(`/cases/${enc(id)}`, json('PATCH', body)).then((r) => r.case);
  },
  deleteCase(id: string): Promise<{ ok: boolean }> {
    return request<{ ok: boolean }>(`/cases/${enc(id)}`, json('DELETE'));
  },
  getGraph(caseId: string): Promise<CaseGraph> {
    return request<CaseGraph>(`/cases/${enc(caseId)}/graph`);
  },
  getPath(caseId: string, a: string, b: string): Promise<{ path: string[] | null }> {
    return request<{ path: string[] | null }>(
      `/cases/${enc(caseId)}/path?a=${enc(a)}&b=${enc(b)}`,
    );
  },
  getPivots(caseId: string, entityId: string): Promise<{ pivots: PivotSuggestion[] }> {
    return request<{ pivots: PivotSuggestion[] }>(
      `/cases/${enc(caseId)}/pivots?entity=${enc(entityId)}`,
    );
  },

  // ---- entities --------------------------------------------------------
  addEntity(caseId: string, body: NewEntityBody): Promise<{ entity: Entity; deduplicated: boolean }> {
    return request<{ entity: Entity; deduplicated: boolean }>(
      `/cases/${enc(caseId)}/entities`, json('POST', body),
    );
  },
  getEntity(id: string): Promise<EntityDetail> {
    return request<EntityDetail>(`/entities/${enc(id)}`);
  },
  patchEntity(id: string, patch: {
    label?: string; data?: Record<string, unknown>; pinned?: boolean; confidence?: number;
  }): Promise<Entity> {
    return request<{ entity: Entity }>(`/entities/${enc(id)}`, json('PATCH', patch)).then((r) => r.entity);
  },
  deleteEntity(id: string): Promise<{ ok: boolean }> {
    return request<{ ok: boolean }>(`/entities/${enc(id)}`, json('DELETE'));
  },
  mergeEntity(id: string, other: string): Promise<Entity> {
    return request<{ entity: Entity }>(`/entities/${enc(id)}/merge`, json('POST', { other }))
      .then((r) => r.entity);
  },
  /** Same-entity verdict WITHOUT merging — powers the merge modal preview. */
  mergePreview(id: string, other: string): Promise<MergePreview> {
    return request<MergePreview>(`/entities/${enc(id)}/merge/preview`, json('POST', { other }));
  },

  // ---- relations / evidence / timeline / docs ---------------------------
  addRelation(body: {
    case_id: string; src: string; dst: string; rel: string;
    weight?: number; confidence?: number; source?: string;
    evidence?: { title?: string; url?: string };
  }): Promise<Relation> {
    return request<{ relation: Relation }>('/relations', json('POST', body)).then((r) => r.relation);
  },
  deleteRelation(id: string): Promise<{ ok: boolean }> {
    return request<{ ok: boolean }>(`/relations/${enc(id)}`, json('DELETE'));
  },
  addEvidence(body: {
    case_id: string; subject: string; kind: string;
    title?: string; url?: string; snippet?: string;
    raw?: Record<string, unknown>; confidence?: number;
  }): Promise<Evidence> {
    return request<{ evidence: Evidence }>('/evidence', json('POST', body)).then((r) => r.evidence);
  },
  listEvidence(entityId: string): Promise<Evidence[]> {
    return request<{ evidence: Evidence[] }>(`/entities/${enc(entityId)}/evidence`).then((r) => r.evidence);
  },
  getTimeline(caseId: string): Promise<TimelineEvent[]> {
    return request<{ events: TimelineEvent[] }>(`/cases/${enc(caseId)}/timeline`).then((r) => r.events);
  },
  addTimelineEvent(caseId: string, body: {
    ts?: string; entity_id?: string | null; kind: string; title: string;
    detail?: Record<string, unknown>; source?: string;
  }): Promise<TimelineEvent> {
    return request<{ event: TimelineEvent }>(`/cases/${enc(caseId)}/timeline`, json('POST', body))
      .then((r) => r.event);
  },
  getDoc(entityId: string): Promise<string> {
    return request<{ doc: string }>(`/entities/${enc(entityId)}/doc`).then((r) => r.doc);
  },
  patchDoc(entityId: string, markdown: string): Promise<string> {
    return request<{ doc: string }>(`/entities/${enc(entityId)}/doc`, json('PATCH', { markdown }))
      .then((r) => r.doc);
  },
  renderDoc(entityId: string): Promise<string> {
    return request<{ doc: string }>(`/entities/${enc(entityId)}/doc/render`, json('POST'))
      .then((r) => r.doc);
  },

  // ---- tasks -----------------------------------------------------------
  createTask(caseId: string, body: NewTaskBody): Promise<Task> {
    return request<{ task: Task }>(`/cases/${enc(caseId)}/tasks`, json('POST', body), )
      .then((r) => r.task);
  },
  listTasks(caseId?: string): Promise<Task[]> {
    const q = caseId ? `?case_id=${enc(caseId)}` : '';
    return request<{ tasks: Task[] }>(`/tasks${q}`).then((r) => r.tasks);
  },
  getTask(id: string): Promise<Task> {
    return request<{ task: Task }>(`/tasks/${enc(id)}`).then((r) => r.task);
  },

  // ---- reports / interchange / laya / archon ----------------------------
  dossierMd(caseId: string): Promise<string> {
    return requestText(`/cases/${enc(caseId)}/dossier.md`);
  },
  privacyMd(): Promise<string> {
    return requestText('/privacy.md');
  },
  briefMd(caseId: string): Promise<string> {
    return requestText(`/cases/${enc(caseId)}/brief.md`);
  },
  exportCase(caseId: string): Promise<unknown> {
    return request<unknown>(`/cases/${enc(caseId)}/export.json`);
  },
  importCase(exportShape: unknown): Promise<Case> {
    return request<{ case: Case }>('/cases/import', json('POST', exportShape)).then((r) => r.case);
  },
  layaTriage(caseId: string): Promise<{ ranked: LayaRanked[]; engine: string }> {
    return request<{ ranked: LayaRanked[]; engine: string }>('/laya/triage', json('POST', { case_id: caseId }));
  },
  archonStatus(): Promise<{ up: boolean; sandboxes: Array<Record<string, unknown>> }> {
    return request<{ up: boolean; sandboxes: Array<Record<string, unknown>> }>('/archon/status');
  },
  archonAsk(body: {
    question: string; case_id?: string; sandbox?: string; top_k?: number;
  }): Promise<{ ok: boolean; answer: string; citations: unknown[]; evidence: Evidence | null }> {
    return request<{ ok: boolean; answer: string; citations: unknown[]; evidence: Evidence | null }>(
      '/archon/ask', json('POST', body),
    );
  },
  archonPushCase(body: { case_id: string; sandbox?: string }): Promise<{ ok: boolean; sandbox: Record<string, unknown> | null }> {
    return request<{ ok: boolean; sandbox: Record<string, unknown> | null }>(
      '/archon/push_case', json('POST', body),
    );
  },
};
