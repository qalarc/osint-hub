/**
 * Mirror of CONTRACT.md §2/§4 JSON shapes — snake_case, exactly as the
 * nexus-server emits them. Do not rename fields; the contract is binding.
 */

export type EType =
  | 'person' | 'org' | 'username' | 'alias' | 'email' | 'phone'
  | 'domain' | 'subdomain' | 'ip' | 'asn' | 'cidr' | 'website'
  | 'social_profile' | 'wallet' | 'transaction' | 'image' | 'document'
  | 'article' | 'event' | 'location' | 'topic' | 'note';

export const ETYPES: readonly EType[] = [
  'person', 'org', 'username', 'alias', 'email', 'phone',
  'domain', 'subdomain', 'ip', 'asn', 'cidr', 'website',
  'social_profile', 'wallet', 'transaction', 'image', 'document',
  'article', 'event', 'location', 'topic', 'note',
];

export const REL_KINDS: readonly string[] = [
  'uses', 'owns', 'resolves_to', 'subdomain_of', 'registered_to', 'mentions',
  'contacted', 'same_as', 'paid_to', 'located_in', 'member_of', 'works_at',
  'sourced_from', 'derived_from', 'related_to', 'posted_on',
];

export const EVIDENCE_KINDS: readonly string[] = [
  'tool_result', 'web_fetch', 'note', 'wiki_doc', 'archon_answer',
  'flowsint_enricher', 'openplanter_finding', 'manual',
];

export type ConnectorName = 'hub' | 'flowsint' | 'openplanter' | 'archon' | 'laya';
export type ConnectorState = 'up' | 'down' | 'unset';
export type TaskStatus = 'queued' | 'running' | 'done' | 'error';

export interface FreeJson {
  [k: string]: unknown;
}

export interface Entity {
  id: string;
  case_id: string;
  etype: EType | string;
  label: string;
  data: FreeJson;
  confidence: number;
  pinned: boolean;
  first_seen: string;
  last_seen: string;
}

export interface Relation {
  id: string;
  case_id: string;
  src: string;
  dst: string;
  rel: string;
  weight: number;
  confidence: number;
  source: string | null;
  evidence: { title?: string; url?: string } | null;
  created_at: string;
}

export interface Evidence {
  id: string;
  case_id: string;
  subject: string;
  kind: string;
  title: string | null;
  url: string | null;
  snippet: string | null;
  raw: FreeJson;
  confidence: number;
  ts: string;
}

export interface TimelineEvent {
  id: string;
  case_id: string;
  ts: string;
  entity_id: string | null;
  kind: string;
  title: string;
  detail: FreeJson;
  source: string | null;
}

export interface Case {
  id: string;
  name: string;
  slug: string;
  status: string;
  notes: string;
  tags: string[];
  created_at: string;
  updated_at: string;
}

export interface CaseStats {
  entities: number;
  relations: number;
  evidence: number;
  tasks: number;
}

export interface GraphAnalysis {
  degree: Record<string, number>;
  pagerank: Record<string, number>;
  communities: Record<string, number>;
  components: string[][];
}

export interface CaseGraph {
  nodes: Entity[];
  edges: Relation[];
  analysis: GraphAnalysis;
}

export interface PivotSuggestion {
  entity_id: string;
  action: string;
  reason: string;
  priority: number;
}

export interface TaskIngested {
  entities?: number;
  relations?: number;
  evidence?: number;
}

export interface Task {
  id: string;
  case_id: string;
  kind: string;
  status: TaskStatus;
  target: string | null;
  params: FreeJson;
  created_at: string;
  finished_at: string | null;
  summary: string | null;
  ingested: TaskIngested | null;
  logs?: string[];
}

export interface TaskLogLine {
  task_id: string;
  level: string;
  msg: string;
  ts: string;
}

export interface Health {
  ok: boolean;
  version: string;
  connectors: Record<ConnectorName, ConnectorState>;
}

export interface EntityDetail {
  entity: Entity;
  relations: Relation[];
  evidence: Evidence[];
  doc: string | null;
}

export interface LayaRanked {
  entity_id: string;
  score: number;
  verdict: string;
  reason: string;
}

/** localStorage `nexus_settings` shape (all strings; '' = unset). Mirrors the
 *  keys /api/admin/config accepts (secrets are sent but redacted in replies). */
export interface NexusSettings {
  hub_url: string;
  hub_token: string;
  flowsint_url: string;
  openplanter_bin: string;
  archon_url: string;
  laya_url: string;
  jev_url: string;
  jev_api_key: string;
}

export interface BusEvent {
  type: 'entity_added' | 'case_updated' | 'task_update' | string;
  case_id?: string | null;
  task_id?: string | null;
  [k: string]: unknown;
}
