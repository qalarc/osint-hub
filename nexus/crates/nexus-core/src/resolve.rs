//! Resolution helpers — normalization keys, similarity, candidate extraction.
//! CONTRACT §3 (`resolve.rs`).

use crate::model::EType;
use std::sync::OnceLock;

/// Normalization key for entity dedup (CONTRACT §2 rules):
/// - email → lowercase
/// - domain/subdomain → lowercase, strip leading `www.`
/// - username/alias → lowercase
/// - phone → E.164 digits (leading `+` kept when present)
/// - website → lowercased host+path (scheme + trailing slash stripped)
/// - ip/asn/cidr → verbatim (trimmed)
/// - anything else → trimmed
pub fn normalize(etype: &str, label: &str) -> String {
    let t = label.trim();
    match etype {
        "email" => t.to_lowercase(),
        "domain" | "subdomain" => {
            let mut s = t.to_lowercase();
            if let Some(stripped) = s.strip_prefix("www.") {
                s = stripped.to_string();
            }
            s
        }
        "username" | "alias" => t.to_lowercase(),
        "phone" => {
            let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
            if t.contains('+') {
                format!("+{digits}")
            } else {
                digits
            }
        }
        "website" => {
            let mut s = t.to_lowercase();
            for scheme in ["https://", "http://"] {
                if let Some(stripped) = s.strip_prefix(scheme) {
                    s = stripped.to_string();
                    break;
                }
            }
            while s.ends_with('/') {
                s.pop();
            }
            s
        }
        // ip / asn / cidr verbatim; everything else trimmed
        _ => t.to_string(),
    }
}

/// Jaro-Winkler similarity via the `strsim` crate (1.0 = identical).
pub fn similarity(a: &str, b: &str) -> f64 {
    strsim::jaro_winkler(a, b)
}

const FILE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "pdf", "txt", "md", "html", "htm",
    "js", "ts", "css", "json", "csv", "zip", "tar", "gz", "rar", "7z", "rs", "py", "go", "rb",
    "java", "c", "cpp", "h", "toml", "yaml", "yml", "xml", "mp4", "mp3", "avi", "mov", "wav",
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "exe", "dll", "so", "bin", "iso", "deb", "rpm",
    "lock",
];

/// Maximum number of candidates returned.
pub const MAX_CANDIDATES: usize = 200;

/// Sweep free text for pivotable entities: emails, phones (E.164-ish),
/// domains, ipv4s, bitcoin wallets and usernames from profile-URL patterns
/// (github/twitter/x/instagram/t.me/reddit). Deduplicated, capped at 200.
pub fn extract_candidates(text: &str) -> Vec<(EType, String)> {
    let mut claimed: Vec<(usize, usize)> = Vec::new();
    let mut out: Vec<(EType, String)> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();

    // priority order: email > wallet > ipv4 > profile-username > domain > phone
    for (etype, spans) in [
        (EType::Email, email_matches(text)),
        (EType::Wallet, wallet_matches(text)),
        (EType::Ip, ipv4_matches(text)),
        (EType::Username, profile_matches(text)),
        (EType::Domain, domain_matches(text)),
        (EType::Phone, phone_matches(text)),
    ] {
        for (start, end, value) in spans {
            if claimed.iter().any(|(s, e)| start < *e && *s < end) {
                continue; // overlaps an already-claimed span
            }
            let key = (etype.as_str().to_string(), value.clone());
            if seen.insert(key) {
                claimed.push((start, end));
                out.push((etype, value));
                if out.len() >= MAX_CANDIDATES {
                    return out;
                }
            }
        }
    }
    out
}

fn email_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?i)\b[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,}\b").unwrap())
}

fn wallet_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"\b(?:[13][a-km-zA-HJ-NP-Z1-9]{25,61}|bc1[a-z0-9]{11,71})\b").unwrap()
    })
}

fn ipv4_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap())
}

fn profile_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)\b(?:github\.com|twitter\.com|x\.com|instagram\.com|t\.me|reddit\.com/user)/([A-Za-z0-9_.\-]{1,64})",
        )
        .unwrap()
    })
}

fn domain_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(?:[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}\b").unwrap()
    })
}

fn phone_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\+?\d[\d\s\-().]{5,24}\d").unwrap())
}

fn email_matches(text: &str) -> Vec<(usize, usize, String)> {
    email_re()
        .find_iter(text)
        .map(|m| (m.start(), m.end(), m.as_str().to_lowercase()))
        .collect()
}

fn wallet_matches(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for m in wallet_re().find_iter(text) {
        let raw = m.as_str();
        // contract window: 26..=62 chars total
        if !(26..=62).contains(&raw.len()) {
            continue;
        }
        // boundaries must not be alphanumeric (avoid slicing longer tokens)
        if prev_alnum(text, m.start()) || next_alnum(text, m.end()) {
            continue;
        }
        out.push((m.start(), m.end(), raw.to_string()));
    }
    out
}

fn ipv4_matches(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for m in ipv4_re().find_iter(text) {
        let raw = m.as_str();
        let ok = raw.split('.').all(|o| o.parse::<u16>().map(|v| v <= 255).unwrap_or(false));
        if !ok {
            continue;
        }
        // reject dotted slices of longer dotted sequences ("1.2.3.4.5")
        if prev_char(text, m.start()) == Some('.') {
            continue;
        }
        if next_char(text, m.end()) == Some('.') && next_char(text, m.end() + 1).is_some_and(|c| c.is_ascii_digit() || c.is_ascii_alphabetic()) {
            continue;
        }
        out.push((m.start(), m.end(), raw.to_string()));
    }
    out
}

fn profile_matches(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for caps in profile_re().captures_iter(text) {
        let whole = caps.get(0).unwrap();
        let raw = caps.get(1).unwrap().as_str();
        let raw = raw.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'']);
        if raw.is_empty() || FILE_EXTS.contains(&raw.rsplit('.').next().unwrap_or("").to_lowercase().as_str()) && raw.contains('.') {
            continue;
        }
        out.push((whole.start(), whole.end(), raw.to_lowercase()));
    }
    out
}

fn domain_matches(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for m in domain_re().find_iter(text) {
        let mut host = m.as_str().trim_end_matches('.').to_lowercase();
        if host.starts_with("www.") {
            host = host[4..].to_string();
        }
        let tld = host.rsplit('.').next().unwrap_or("");
        if FILE_EXTS.contains(&tld) {
            continue; // looks like a filename
        }
        out.push((m.start(), m.end(), host));
    }
    out
}

fn phone_matches(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for m in phone_re().find_iter(text) {
        let raw = m.as_str();
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        let n = digits.len();
        if !(8..=15).contains(&n) {
            continue;
        }
        let has_plus = raw.starts_with('+');
        let has_sep = raw.chars().any(|c| matches!(c, ' ' | '-' | '(' | ')' | '.'));
        let accept = if has_plus {
            (8..=15).contains(&n)
        } else {
            has_sep && (9..=15).contains(&n)
        };
        if !accept {
            continue;
        }
        // boundary hygiene: no letters/digits glued to the candidate
        if prev_alnum(text, m.start()) || next_alnum(text, m.end()) {
            continue;
        }
        let value = if has_plus { format!("+{digits}") } else { digits };
        out.push((m.start(), m.end(), value));
    }
    out
}

fn prev_char(text: &str, at: usize) -> Option<char> {
    text[..at].chars().last()
}

fn next_char(text: &str, at: usize) -> Option<char> {
    text[at..].chars().next()
}

fn prev_alnum(text: &str, at: usize) -> bool {
    prev_char(text, at).is_some_and(|c| c.is_alphanumeric())
}

fn next_alnum(text: &str, at: usize) -> bool {
    next_char(text, at).is_some_and(|c| c.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_per_etype() {
        assert_eq!(normalize("email", "  Bob@Example.COM "), "bob@example.com");
        assert_eq!(normalize("domain", "WWW.EvilCorp.IO"), "evilcorp.io");
        assert_eq!(normalize("subdomain", "mail.www.Internal.Org"), "mail.www.internal.org");
        assert_eq!(normalize("username", " JohnDoe "), "johndoe");
        assert_eq!(normalize("website", "HTTPS://Example.COM/Path/"), "example.com/path");
        assert_eq!(normalize("ip", " 10.0.0.1 "), "10.0.0.1");
        assert_eq!(normalize("asn", " AS13335 "), "AS13335"); // verbatim
        assert_eq!(normalize("note", "  free text  "), "free text");
    }

    #[test]
    fn normalize_phone_e164() {
        assert_eq!(normalize("phone", "+1 (415) 555-0100"), "+14155550100");
        assert_eq!(normalize("phone", "+44 20 7123 4567"), "+442071234567");
        assert_eq!(normalize("phone", "555-0100"), "5550100"); // no plus → digits only
        assert_eq!(normalize("phone", "  +79991234567  "), "+79991234567");
    }

    #[test]
    fn similarity_jaro_winkler() {
        assert!((similarity("johndoe", "johndoe") - 1.0).abs() < 1e-9);
        assert!(similarity("johndoe", "john doe") > 0.85);
        assert!(similarity("alice", "bob") < 0.6);
    }

    #[test]
    fn extract_finds_all_kinds_in_paragraph() {
        let text = "Contact bob.smith@Example.COM or +1 (415) 555-0100. \
                    Site https://evil-corp.io/about and mail.evil-corp.io. \
                    Gateway 8.8.8.8 logged. Profile github.com/torvalds seen. \
                    Wallet 1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa cashed out.";
        let cands = extract_candidates(text);
        let has = |t: EType, v: &str| cands.iter().any(|(e, x)| *e == t && x == v);
        assert!(has(EType::Email, "bob.smith@example.com"), "{cands:?}");
        assert!(has(EType::Phone, "+14155550100"), "{cands:?}");
        assert!(has(EType::Domain, "evil-corp.io"), "{cands:?}");
        assert!(has(EType::Domain, "mail.evil-corp.io"), "{cands:?}");
        assert!(has(EType::Ip, "8.8.8.8"), "{cands:?}");
        assert!(has(EType::Username, "torvalds"), "{cands:?}");
        assert!(has(EType::Wallet, "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"), "{cands:?}");
    }

    #[test]
    fn extract_skips_filenames_dates_and_claimed_spans() {
        let text = "Read report.pdf and data.json — not domains. \
                    Version 1.2.3.4.5 rolled. Meeting 2026-09-22 at 10:00. \
                    Email a@b.co and a@b.co again (dedup).";
        let cands = extract_candidates(text);
        let domains: Vec<&str> = cands.iter().filter(|(e, _)| *e == EType::Domain).map(|(_, v)| v.as_str()).collect();
        assert!(!domains.contains(&"report.pdf"), "{domains:?}");
        assert!(!domains.contains(&"data.json"), "{domains:?}");
        assert!(!cands.iter().any(|(e, _)| *e == EType::Ip), "{cands:?}");
        // date must not become a phone
        assert!(!cands.iter().any(|(_, v)| v == "20260922" || v == "2026-09-22"), "{cands:?}");
        // email deduped
        let emails = cands.iter().filter(|(e, _)| *e == EType::Email).count();
        assert_eq!(emails, 1, "{cands:?}");
    }

    #[test]
    fn extract_profile_variants_and_cap() {
        for (url, expected) in [
            ("https://twitter.com/jack", "jack"),
            ("https://x.com/jack", "jack"),
            ("https://instagram.com/fivenight", "fivenight"),
            ("https://t.me/durov", "durov"),
            ("https://reddit.com/user/spez", "spez"),
        ] {
            let cands = extract_candidates(url);
            assert!(
                cands.iter().any(|(e, v)| *e == EType::Username && v == expected),
                "{url} → {cands:?}"
            );
        }
        let flood_text: String = (0..400).map(|i| format!("user{i}@mail.com ")).collect();
        let flood = extract_candidates(&flood_text);
        assert_eq!(flood.len(), MAX_CANDIDATES);
    }
}
