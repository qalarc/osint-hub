/* ============================================================================
   qalarc · OSINT Hub — LITE cloud API (Cloudflare Pages Function)
   Route file: functions/api/[[path]].js  →  serves same-origin /api/*

   WHY THIS EXISTS
   ---------------
   The full engine is a FastAPI backend running maigret / holehe / phoneinfoga
   as local subprocesses — impossible on Pages. This Function provides a
   zero-server subset ("lite") so the deployed static frontend stays useful:

     GET  /api/health      → lite health marker (mode:"lite")
     GET  /api/tools       → the two lite tools
     POST /api/lite/check  → synchronous edge username scan (~44 platforms)
     POST /api/lite/pivots → instant Google-dork pivots for a phone number
     *    /api/*           → 501 (full engine not available in lite mode)

   FREE-PLAN SUBREQUEST BUDGET (the design constraint)
   --------------------------------------------------
   Workers/Pages Functions on the Cloudflare FREE plan allow **50 subrequests
   per incoming request**. Every external fetch() counts as one subrequest —
   and a redirect chain counts once PER HOP. Therefore:

     • every probe uses redirect:"manual" → exactly ONE subrequest per site,
       no matter how the target would redirect;
     • the site list is hard-capped at 44 entries (44 < 50, leaving headroom);
     • probes run in chunks of 10 concurrent fetches (Promise.allSettled) to
       stay polite and inside CPU limits (free plan: 10 ms CPU/request —
       awaiting fetches does not consume CPU);
     • each fetch is bounded by AbortSignal.timeout(8000) so a hanging host
       cannot stall the whole request;
     • no redirect is followed, so per-site detection works from the immediate
       status code + Location header (see SITES below).

   DETECTION RULES (conservative by design — verified empirically 2026-09)
   -----------------------------------------------------------------------
   Sites were probed with a known-existing and a known-missing username:
   only platforms whose missing-profile response is reliably distinguishable
   are included. Unreliable platforms that answer HTTP 200 for *any* username
   (instagram, tiktok, twitch, spotify, pinterest, kaggle, pypi, mixcloud,
   imgur, dailymotion, artstation, 500px, slideshare …) are deliberately
   OMITTED — a false "found" is worse than a missing row.
   Rule kinds (per site):
     status    (default) found ⇔ 2xx            (missing ⇒ 404/403/redirect)
     redirUser             found ⇔ 2xx, or 3xx whose Location still contains
                           the username (canonicalisation redirect; missing is
                           a plain 404 on these sites)
     roblox                special: 3xx whose Location is NOT the
                           "request-error" page (missing users bounce there)
     sniff                 2xx AND cheap body markers (tiny JSON pages or the
                           steam error string; body reads skipped if huge)

   No state, no KV, no analytics. A best-effort in-memory per-IP rate limiter
   (30/hour) lives only for the isolate's lifetime — best effort, not a
   guarantee. Plain JS, no build step, no external packages.
============================================================================ */

const VERSION = 'lite-1.0.0';
const PROBE_UA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 OSINT-Hub-Lite/1.0';
const FETCH_TIMEOUT_MS = 8000;
const CHUNK_SIZE = 10;
const RATE_LIMIT = 30;              /* lite POSTs per hour per IP */
const RATE_WINDOW_MS = 60 * 60 * 1000;

/* ------------------------------------------------------------------ tools */

const HEALTH = {
  ok: true,
  version: VERSION,
  tools_installed: 2,
  auth_required: false,
  mode: 'lite'
};

const TOOLS = {
  tools: [
    {
      id: 'cloudcheck',
      name: 'CloudCheck (lite)',
      kind: 'username',
      installed: true,
      description: 'Edge username check across ~40 top platforms (sherlock-style, Cloudflare-native).',
      hint: 'always available on Pages'
    },
    {
      id: 'pivots',
      name: 'Search Pivots',
      kind: 'phone',
      installed: true,
      description: 'Instant Google-dork pivot generation for a phone number.',
      hint: 'always available on Pages'
    }
  ]
};

/* ------------------------------------------------------------------ sites
   u = validated username. "probe" = (existing → code, missing → code).
   List length MUST stay ≤ 45 (free-plan subrequest budget: 50/request).   */

const SITES = [
  /* --- big social / content platforms --- */
  { site: 'GitHub',            url: u => 'https://github.com/' + u },                        /* probe: 200/404 */
  { site: 'GitHub Gist',       url: u => 'https://gist.github.com/' + u },                   /* probe: 200/404 */
  { site: 'GitHub Pages',      url: u => 'https://' + u + '.github.io/' },                   /* probe: 200/404 */
  { site: 'GitLab',            url: u => 'https://gitlab.com/' + u },                        /* probe: 200/302→login (missing) */
  { site: 'X (Twitter)',       url: u => 'https://x.com/' + u },                             /* probe: 200/404 */
  { site: 'YouTube',           url: u => 'https://www.youtube.com/@' + u },                  /* probe: 200/404 */
  { site: 'Medium',            url: u => 'https://medium.com/@' + u },                       /* probe: 200/403 (missing) */
  { site: 'Dev.to',            url: u => 'https://dev.to/' + u },                            /* probe: 200/404 */
  { site: 'DeviantArt',        url: u => 'https://www.deviantart.com/' + u },                /* probe: 200/404 */
  { site: 'Behance',           url: u => 'https://www.behance.net/' + u },                   /* probe: 200/404 */
  { site: 'Dribbble',          url: u => 'https://dribbble.com/' + u,            rule: 'redirUser' }, /* 200|302→/{u}/about, missing 404 */
  { site: 'Flickr',            url: u => 'https://www.flickr.com/people/' + u },             /* probe: 200/404 */
  { site: 'Vimeo',             url: u => 'https://vimeo.com/' + u },                         /* probe: 200/404 */
  { site: 'SoundCloud',        url: u => 'https://soundcloud.com/' + u },                    /* probe: 200/404 */
  { site: 'Last.fm',           url: u => 'https://www.last.fm/user/' + u },                  /* probe: 200/404 */
  { site: 'Tumblr',            url: u => 'https://' + u + '.tumblr.com/' },                  /* probe: 200/404 */
  { site: 'Steam',             url: u => 'https://steamcommunity.com/id/' + u,
    sniff: { absent: ['could not be found'] } },                       /* 200 always; missing body carries the error string */
  { site: 'Telegram',          url: u => 'https://t.me/' + u,
    sniff: { present: ['tgme_page_title'] } },                         /* 200 always; real preview pages embed the title div */
  { site: 'Mastodon',          url: u => 'https://mastodon.social/@' + u },                  /* probe: 200/404 */
  { site: 'Substack',          url: u => 'https://' + u + '.substack.com/' },                /* probe: 200/404 */

  /* --- link-in-bio / monetization --- */
  { site: 'Linktree',          url: u => 'https://linktr.ee/' + u },                         /* probe: 200/404 */
  { site: 'Patreon',           url: u => 'https://www.patreon.com/' + u,          rule: 'redirUser' }, /* 200|308→/{u}, missing 404 */
  { site: 'Gumroad',           url: u => 'https://gumroad.com/' + u,             rule: 'redirUser' }, /* 200|301→/{u}, missing 404 */
  /* --- gaming --- */
  /* NB: omitted on purpose (unverifiable via status codes): instagram,
     tiktok, twitch, spotify, pinterest — all answer 200 for missing profiles. */
  { site: 'Roblox',            url: u => 'https://www.roblox.com/users/profile?username=' + u,
    rule: 'roblox' },                                                  /* 302→/users/{id}/profile, missing 302→request-error */
  { site: 'Minecraft',         url: u => 'https://api.mojang.com/users/profiles/minecraft/' + u }, /* API: 200/404 */

  /* --- dev / code --- */
  { site: 'Replit',            url: u => 'https://replit.com/@' + u,             rule: 'redirUser' }, /* 302→…goto=%2F@{u}, missing 404 */
  { site: 'HackerRank',        url: u => 'https://www.hackerrank.com/profile/' + u },        /* probe: 200/302 (missing) */
  { site: 'Codewars',          url: u => 'https://www.codewars.com/users/' + u },            /* probe: 200/404 */
  { site: 'Docker Hub',        url: u => 'https://hub.docker.com/v2/users/' + u + '/' },     /* API: 200/404 */
  { site: 'RubyGems',          url: u => 'https://rubygems.org/profiles/' + u },             /* probe: 200/404 */
  { site: 'Hacker News',       url: u => 'https://news.ycombinator.com/user?id=' + u,
    sniff: { absent: ['No such user'] } },                             /* 200 always; tiny page, cheap sniff */

  /* --- forums / wikis / Q&A --- */
  { site: 'Wikipedia',         url: u => 'https://en.wikipedia.org/w/api.php?action=query&list=users&ususers=' + u + '&format=json',
    sniff: { absent: ['"missing"'] } },                                /* API: tiny JSON, missing users get "missing":"" */
  { site: 'Keybase',           url: u => 'https://keybase.io/_/api/1.0/user/lookup.json?username=' + u,
    sniff: { present: ['"them":{'] } },                                /* API: tiny JSON, missing → them:null */

  /* --- niche communities --- */
  { site: 'Chess.com',         url: u => 'https://api.chess.com/pub/player/' + u },          /* API: 200/404 */
  { site: 'Lichess',           url: u => 'https://lichess.org/api/user/' + u },              /* API: 200/404 */
  { site: '9GAG',              url: u => 'https://9gag.com/u/' + u },                       /* probe: 200/404 */
  { site: 'Letterboxd',        url: u => 'https://letterboxd.com/' + u + '/' },              /* probe: 200/404 */
  { site: 'MyAnimeList',       url: u => 'https://myanimelist.net/profile/' + u },           /* probe: 200/404 */
  { site: 'Redbubble',         url: u => 'https://www.redbubble.com/people/' + u + '/shop' }, /* probe: 200/404 */
  { site: 'Speaker Deck',      url: u => 'https://speakerdeck.com/' + u },                   /* probe: 200/404 */
  { site: 'Goodreads',         url: u => 'https://www.goodreads.com/' + u,         rule: 'redirUser' }, /* 301→/user/show/…-{u}…, missing 404 */
  { site: 'Disqus',            url: u => 'https://disqus.com/by/' + u + '/',      rule: 'redirUser' }, /* 302→canonical, missing 404 */
  { site: 'OpenStreetMap',     url: u => 'https://www.openstreetmap.org/user/' + u }         /* probe: 200/404 */
];

/* ------------------------------------------------------- phone pivots ---- */

/* +CC → country guess (longest-prefix match). Small by design. */
const CC_TABLE = {
  '1': 'US/CA', '7': 'RU/KZ', '20': 'EG', '27': 'ZA', '30': 'GR', '31': 'NL',
  '32': 'BE', '33': 'FR', '34': 'ES', '36': 'HU', '39': 'IT', '40': 'RO',
  '41': 'CH', '43': 'AT', '44': 'UK', '45': 'DK', '46': 'SE', '47': 'NO',
  '48': 'PL', '49': 'DE', '52': 'MX', '55': 'BR', '61': 'AU', '62': 'ID',
  '63': 'PH', '64': 'NZ', '66': 'TH', '81': 'JP', '82': 'KR', '84': 'VN',
  '86': 'CN', '90': 'TR', '91': 'IN', '92': 'PK', '94': 'LK', '98': 'IR',
  '212': 'MA', '234': 'NG', '254': 'KE', '351': 'PT', '353': 'IE',
  '354': 'IS', '358': 'FI', '370': 'LT', '371': 'LV', '372': 'EE',
  '375': 'BY', '380': 'UA', '971': 'AE', '966': 'SA', '972': 'IL', '995': 'GE'
};

/* Disposable / public-SMS receiver sites — numbers appearing here have been
   used for throwaway verification. Searched via Google site: dorks. */
const DISPOSABLE_SITES = [
  'receive-sms.com', 'receive-smsonline.com', 'sms-online.co', 'smstome.com',
  'smsreceivefree.com', 'receive-a-sms.com', 'receivefreesms.com'
];

function googleDork(query) {
  return 'https://www.google.com/search?q=' + encodeURIComponent(query);
}

/* ------------------------------------------------------- rate limiting --- */

const rateMap = new Map(); /* ip → [timestamps] (isolate-lifetime, best effort) */

function rateAllow(key) {
  const now = Date.now();
  let hits = rateMap.get(key);
  if (!hits) { hits = []; rateMap.set(key, hits); }
  while (hits.length && hits[0] <= now - RATE_WINDOW_MS) hits.shift();
  if (hits.length >= RATE_LIMIT) return false;
  hits.push(now);
  if (rateMap.size > 10000) {
    for (const [k, v] of rateMap) {
      if (!v.length || v[v.length - 1] <= now - RATE_WINDOW_MS) rateMap.delete(k);
    }
  }
  return true;
}

/* ------------------------------------------------------------- helpers --- */

function corsHeaders() {
  return {
    'Access-Control-Allow-Origin': '*',
    'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
    'Access-Control-Allow-Headers': 'Content-Type, Authorization',
    'Access-Control-Max-Age': '86400'
  };
}

function json(status, obj) {
  const headers = corsHeaders();
  headers['Content-Type'] = 'application/json; charset=utf-8';
  headers['Cache-Control'] = 'no-store';
  return new Response(JSON.stringify(obj), { status: status, headers: headers });
}

async function readJsonBody(request) {
  try { return await request.json(); } catch (_) { return null; }
}

function clientIp(request) {
  return request.headers.get('CF-Connecting-IP') || 'anon';
}

/* Does a redirect Location still point at the username? Tokens cover the
   canonical patterns seen in the wild (/u, @u, -u, =u + urlencoded forms).
   NB: very short usernames can substring-collide — acceptable for a heuristic
   tool and far safer than treating every 3xx as "found". */
function locationKeepsUser(loc, user) {
  const l = loc.toLowerCase();
  const u = user.toLowerCase();
  return ['/' + u, '@' + u, '-' + u, '=' + u, '%40' + u, '%2f' + u]
    .some(t => l.includes(t));
}

/* Probe one site. Returns a found-item or null. One subrequest per call. */
async function probeSite(def, user) {
  const url = def.url(user);
  let res;
  try {
    res = await fetch(url, {
      method: 'GET',
      redirect: 'manual',                 /* exactly 1 subrequest, no hop chain */
      headers: { 'User-Agent': PROBE_UA, 'Accept': 'text/html,application/json' },
      signal: AbortSignal.timeout(FETCH_TIMEOUT_MS)
    });
  } catch (_) {
    return null;                          /* timeout / network error → not found */
  }

  const status = res.status;
  if (status >= 200 && status < 300) {
    if (def.sniff) {
      const len = Number(res.headers.get('content-length') || 0);
      if (len > 3000000) return null;     /* huge body → skip, stay conservative */
      let body = '';
      try { body = await res.text(); } catch (_) { return null; }
      if (def.sniff.absent && def.sniff.absent.some(m => body.includes(m))) return null;
      if (def.sniff.present && !def.sniff.present.every(m => body.includes(m))) return null;
    }
    return { site: def.site, url: url, http_status: status };
  }

  if (status >= 300 && status < 400) {
    const loc = res.headers.get('location') || '';
    if (!loc) return null;
    if (def.rule === 'redirUser' && locationKeepsUser(loc, user)) {
      return { site: def.site, url: url, http_status: status };
    }
    if (def.rule === 'roblox' && !/request-error/i.test(loc)) {
      return { site: def.site, url: url, http_status: status };
    }
  }

  return null;                            /* 4xx/5xx/unmatched 3xx → not found */
}

/* --------------------------------------------------------- endpoints ----- */

async function liteCheck(request) {
  if (!rateAllow(clientIp(request))) {
    return json(429, { error: 'rate limited: lite cloud mode allows ' + RATE_LIMIT + ' scans/hour per IP' });
  }
  const body = await readJsonBody(request);
  if (!body || typeof body.value !== 'string') {
    return json(400, { error: 'expected JSON body {"value":"<username>"}' });
  }
  const user = body.value.trim().replace(/^@+/, '');
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(user)) {
    return json(400, { error: 'invalid username (allowed: A-Z a-z 0-9 . _ - , max 64 chars)' });
  }

  const found = [];
  /* chunked concurrency — see subrequest-budget notes in the header */
  for (let i = 0; i < SITES.length; i += CHUNK_SIZE) {
    const chunk = SITES.slice(i, i + CHUNK_SIZE);
    const settled = await Promise.allSettled(chunk.map(s => probeSite(s, user)));
    for (const r of settled) {
      if (r.status === 'fulfilled' && r.value) found.push(r.value);
    }
  }
  return json(200, { found: found });
}

function litePivots(request) {
  if (!rateAllow(clientIp(request))) {
    return json(429, { error: 'rate limited: lite cloud mode allows ' + RATE_LIMIT + ' scans/hour per IP' });
  }
  return readJsonBody(request).then(body => {
    if (!body || typeof body.value !== 'string') {
      return json(400, { error: 'expected JSON body {"value":"+61425228338"}' });
    }
    const raw = body.value.trim();
    const digits = raw.replace(/\D/g, '');
    if (digits.length < 7 || digits.length > 15) {
      return json(400, { error: 'invalid phone number (expected E.164-ish, 7-15 digits)' });
    }
    const e164 = '+' + digits;

    /* country guess — longest prefix first */
    let country = null;
    for (let len = 3; len >= 1; len--) {
      const cc = digits.slice(0, len);
      if (CC_TABLE[cc]) { country = CC_TABLE[cc]; break; }
    }

    const info = [];
    info.push({ key: 'Country (guessed)', value: country || 'unknown' });
    info.push({ key: 'E.164', value: e164 });
    info.push({ key: 'Digits', value: String(digits.length) });

    const d = '"' + digits + '"';
    info.push({ key: 'Google — exact match', url: googleDork(d) });
    info.push({ key: 'Social media (Facebook)', url: googleDork('site:facebook.com ' + d) });
    info.push({ key: 'Social media (X/Twitter)', url: googleDork('site:twitter.com ' + d) });
    info.push({ key: 'Social media (LinkedIn)', url: googleDork('site:linkedin.com ' + d) });
    info.push({ key: 'Social media (Instagram)', url: googleDork('site:instagram.com ' + d) });
    info.push({ key: 'Social media (VK)', url: googleDork('site:vk.com ' + d) });
    info.push({ key: 'Reddit', url: googleDork('site:reddit.com ' + d) });
    info.push({ key: 'WhatsApp — open chat', url: 'https://wa.me/' + digits });
    DISPOSABLE_SITES.forEach(s => {
      info.push({ key: 'Disposable SMS — ' + s, url: googleDork('site:' + s + ' ' + d) });
    });
    info.push({ key: 'DuckDuckGo — exact match', url: 'https://duckduckgo.com/?q=' + encodeURIComponent(d) });
    info.push({ key: 'Bing — exact match', url: 'https://www.bing.com/search?q=' + encodeURIComponent(d) });

    return json(200, { info: info });
  });
}

/* ------------------------------------------------------------- routing --- */

export async function onRequest(context) {
  const request = context.request;
  const url = new URL(request.url);
  const path = (url.pathname || '/').replace(/\/+$/, '') || '/';
  const method = request.method.toUpperCase();

  if (method === 'OPTIONS') {
    return new Response(null, { status: 204, headers: corsHeaders() });
  }

  if (path === '/api/health' && method === 'GET') {
    return json(200, HEALTH);
  }
  if (path === '/api/tools' && method === 'GET') {
    return json(200, TOOLS);
  }
  if (path === '/api/lite/check' && method === 'POST') {
    return liteCheck(request);
  }
  if (path === '/api/lite/pivots' && method === 'POST') {
    return litePivots(request);
  }

  /* everything else — /api/scan, /api/jobs/*, wrong methods, bare /api —
     the full engine simply is not here. */
  return json(501, {
    error: 'full engine not available in lite mode — run the FastAPI backend for maigret/holehe/phoneinfoga'
  });
}
