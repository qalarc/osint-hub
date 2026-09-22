/**
 * Settings modal:
 * - connector fields (localStorage `nexus_settings` → POST /api/admin/config;
 *   health refreshed after save by the caller),
 * - browser-connection fields (localStorage `nexus_api_base` + `nexus_token`):
 *   API-base override for hosted UIs and the bearer token for NEXUS_TOKEN-
 *   gated servers. Base changes need a reload — handled here.
 */
import { API_BASE, api, normalizeBase, saveConnection, storedToken } from './api';
import { h, openModal, toast } from './dom';
import type { NexusSettings } from './types';

const LS_KEY = 'nexus_settings';

const FIELDS: Array<{ key: keyof NexusSettings; label: string; ph: string; secret?: boolean }> = [
  { key: 'hub_url', label: 'hub_url', ph: '127.0.0.1:8799' },
  { key: 'hub_token', label: 'hub_token', ph: 'bearer token (optional)', secret: true },
  { key: 'flowsint_url', label: 'flowsint_url', ph: '127.0.0.1:5173' },
  { key: 'openplanter_bin', label: 'openplanter_bin', ph: 'openplanter-agent (auto-discover)' },
  { key: 'archon_url', label: 'archon_url', ph: '127.0.0.1:7843' },
  { key: 'laya_url', label: 'laya_url', ph: 'host:port  (empty = heuristic)' },
  { key: 'jev_url', label: 'jev_url', ph: 'typesafe systemone endpoint (see server JEV_URL)' },
  { key: 'jev_api_key', label: 'jev_api_key', ph: 'typesafe API key (optional)', secret: true },
];

const DEFAULTS: NexusSettings = {
  hub_url: '', hub_token: '', flowsint_url: '',
  openplanter_bin: '', archon_url: '', laya_url: '',
  jev_url: '', jev_api_key: '',
};

export function loadSettings(): NexusSettings {
  const base = { ...DEFAULTS };
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (!raw) return base;
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === 'object') {
      for (const f of FIELDS) {
        const v = (parsed as Record<string, unknown>)[f.key];
        if (typeof v === 'string') base[f.key] = v;
      }
    }
  } catch { /* corrupted settings — ignore */ }
  return base;
}

export function saveSettingsLocal(s: NexusSettings): void {
  localStorage.setItem(LS_KEY, JSON.stringify(s));
}

export function openSettingsModal(
  current: NexusSettings,
  onSaved: (s: NexusSettings) => void,
): void {
  const inputs: Partial<Record<keyof NexusSettings, HTMLInputElement>> = {};
  const effPre = h('pre', { class: 'settings-effective', hidden: true });

  const apiBaseIn = h('input', {
    class: 'input', type: 'text', value: API_BASE,
    placeholder: 'scheme://host:port of the nexus server  (?api= wins; empty = same-origin)',
    spellcheck: 'false', autocomplete: 'off',
  }) as HTMLInputElement;
  const tokenIn = h('input', {
    class: 'input', type: 'password', value: storedToken(),
    placeholder: 'bearer token if NEXUS_TOKEN gate is on (optional)',
    spellcheck: 'false', autocomplete: 'off',
  }) as HTMLInputElement;

  const modal = openModal('settings', (body, close) => {
    const fieldsGrid = h('div', { class: 'settings-grid' });
    for (const f of FIELDS) {
      const input = h('input', {
        class: 'input', type: f.secret ? 'password' : 'text',
        value: current[f.key], placeholder: f.ph,
        spellcheck: 'false', autocomplete: 'off',
        'data-key': f.key,
      }) as HTMLInputElement;
      inputs[f.key] = input;
      fieldsGrid.append(
        h('label', { class: 'settings-field' },
          h('span', { class: 'field-label' }, f.label),
          input,
        ),
      );
    }
    const connForm = h('div', { class: 'settings-grid' },
      h('label', { class: 'settings-field' },
        h('span', { class: 'field-label' }, 'nexus_api_base'),
        apiBaseIn,
      ),
      h('label', { class: 'settings-field' },
        h('span', { class: 'field-label' }, 'nexus_token'),
        tokenIn,
      ),
    );
    const save = async (): Promise<void> => {
      const next = { ...current };
      for (const f of FIELDS) next[f.key] = inputs[f.key]?.value.trim() ?? '';
      try {
        const payload: Record<string, string | null> = {};
        for (const f of FIELDS) payload[f.key] = next[f.key] === '' ? null : next[f.key];
        const effective = await api.adminConfig(payload);
        saveSettingsLocal(next);
        saveConnection(apiBaseIn.value, tokenIn.value);
        effPre.hidden = false;
        effPre.textContent = `effective (redacted):\n${JSON.stringify(effective, null, 2)}`;
        if (normalizeBase(apiBaseIn.value) !== API_BASE) {
          toast('api base changed — reloading…', 'ok');
          window.setTimeout(() => window.location.reload(), 600);
          return;
        }
        toast('settings applied', 'ok');
        onSaved(next);
        window.setTimeout(() => close(), 500);
      } catch (e) {
        toast(e instanceof Error ? e.message : String(e), 'error');
      }
    };

    // form wrapper so Enter in any field saves (and the Save button submits)
    const form = h('form', { class: 'settings-form' },
      h('div', { class: 'panel-label settings-section' }, 'connectors — server overlay (POST /api/admin/config)'),
      fieldsGrid,
      h('div', { class: 'panel-label settings-section' }, 'connection — this browser only (localStorage)'),
      connForm,
      h('div', { class: 'muted small settings-note' },
        `active API base: ${API_BASE === '' ? 'same-origin' : API_BASE}. Connector fields are stored in localStorage (nexus_settings) and applied to the server via POST /api/admin/config — env vars remain the defaults, empty fields are sent as null, secrets are redacted in responses. nexus_api_base + nexus_token stay in this browser; a changed api base reloads the page. /api/health stays open when the token gate is on.`,
      ),
      effPre,
      h('div', { class: 'row row-end gap8' },
        h('button', { class: 'btn', type: 'button', onclick: () => close() }, 'Cancel'),
        h('button', { class: 'btn btn-primary', type: 'submit' }, '▶ Save'),
      ),
    );
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      void save();
    });
    body.append(form);
  }, { wide: true });
  void modal;
}
