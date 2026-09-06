# Privacy & Use Notice — OSINT Hub (osint.qalarc.com)

_Owner: qalarc · Contact: team@qalarc.com · Last updated: 2026-09-07_

## What this service does

OSINT Hub checks identifiers you enter (a username, phone number, or email address) against
publicly accessible web pages and platform profiles, and shows which platforms appear to have
an account matching that identifier. It is a lookup convenience layer over open-source tools
(sherlock, maigret, phoneinfoga, holehe, ignorant — and a Cloudflare-native "lite" checker).

## What is processed

| Mode | Stored? | Details |
|---|---|---|
| **Lite (Cloudflare)** | **Nothing persisted** | Queries are processed in-memory at the edge; no results database, no accounts, no analytics. IPs are used transiently for rate limiting only. |
| **Full (self-hosted backend)** | Job records on the operator's server | Identifier + results are written to job files for the session, auto-pruned (default: last 200 jobs). Operators should publish their own retention period. |

- **No cookies, no analytics, no tracking.** UI settings (API base URL, optional token) are stored
  in your browser's localStorage only and never leave your device.
- Checks are performed **from server IPs** (Cloudflare edge or the operator's backend); the
  platforms being checked may log those requests.
- We do **not** compile dossiers, sell data, or build a face/image index.

## Accuracy

Results are **heuristic** — false positives and false negatives are common (platforms change
behaviour, rate-limit, or return ambiguous responses). Findings are "worth checking", not
evidence. Do not use results for employment, tenancy, credit or other consequential decisions
about a person.

## Data subjects

If information about you appears via this service, you can ask us to confirm what (if anything)
was stored (in lite mode: nothing) and to update this notice: **team@qalarc.com**. The profiles
we surface are public web pages controlled by their platforms — removal requests belong with
those platforms. We respond to access/errection requests consistent with the Australian Privacy
Act, GDPR and CCPA where applicable.

## Acceptable use

Authorized research, exposure assessment, identity verification and security work only.
Prohibited: stalking, harassment, doxxing, discrimination, bulk/enumeration scraping, or any
unlawful purpose. Platform terms of service apply to automated checks. Users are responsible
for lawful use in their jurisdiction.
