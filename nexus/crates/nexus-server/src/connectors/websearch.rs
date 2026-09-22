//! websearch connector — pure pivot-URL builders (no scraping).

use serde_json::json;

pub struct Pivot {
    pub name: String,
    pub url: String,
}

fn q(s: &str) -> String {
    urlencoding_lite(s)
}

/// Minimal percent-encoding for query values (no external crate).
fn urlencoding_lite(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn pivots_for(etype: &str, label: &str, data: &serde_json::Value) -> Vec<Pivot> {
    let l = q(label);
    let mut v = vec![
        Pivot { name: "Google".into(), url: format!("https://www.google.com/search?q=%22{l}%22") },
        Pivot { name: "DuckDuckGo".into(), url: format!("https://duckduckgo.com/?q=%22{l}%22") },
        Pivot { name: "Bing".into(), url: format!("https://www.bing.com/search?q=%22{l}%22") },
        Pivot { name: "Google News".into(), url: format!("https://news.google.com/search?q=%22{l}%22") },
        Pivot { name: "Wikipedia".into(), url: format!("https://en.wikipedia.org/w/index.php?search={l}") },
        Pivot { name: "Reddit".into(), url: format!("https://www.reddit.com/search/?q={l}") },
        Pivot { name: "YouTube".into(), url: format!("https://www.youtube.com/results?search_query={l}") },
    ];
    match etype {
        "username" | "alias" => {
            for (site, pat) in [
                ("GitHub", "https://github.com/{u}"),
                ("X / Twitter", "https://x.com/{u}"),
                ("Instagram", "https://www.instagram.com/{u}/"),
                ("Telegram", "https://t.me/{u}"),
                ("Reddit user", "https://www.reddit.com/user/{u}"),
            ] {
                v.push(Pivot {
                    name: site.into(),
                    url: pat.replace("{u}", label.trim_start_matches('@')),
                });
            }
            v.push(Pivot {
                name: "Sherlock-style dork".into(),
                url: format!("https://www.google.com/search?q=%22{l}%22+(site:github.com+OR+site:x.com+OR+site:t.me+OR+site:reddit.com)"),
            });
        }
        "email" => {
            v.push(Pivot {
                name: "Have I Been Pwned".into(),
                url: format!("https://haveibeenpwned.com/unifiedsearch/{l}"),
            });
            v.push(Pivot {
                name: "Gravatar".into(),
                url: format!("https://en.gravatar.com/{l}.json"),
            });
        }
        "phone" => {
            v.push(Pivot {
                name: "Truecaller".into(),
                url: format!("https://www.truecaller.com/search/au/{l}"),
            });
            v.push(Pivot {
                name: "Google dork".into(),
                url: format!("https://www.google.com/search?q=%22{label}%22+OR+%22{l}%22"),
            });
        }
        "domain" | "subdomain" => {
            v.push(Pivot { name: "crt.sh".into(), url: format!("https://crt.sh/?q={l}") });
            v.push(Pivot { name: "urlscan".into(), url: format!("https://urlscan.io/search/#{l}") });
            v.push(Pivot {
                name: "WHOIS".into(),
                url: format!("https://www.whois.com/whois/{l}"),
            });
        }
        "ip" => {
            v.push(Pivot { name: "Shodan".into(), url: format!("https://www.shodan.io/host/{l}") });
            v.push(Pivot {
                name: "AbuseIPDB".into(),
                url: format!("https://www.abuseipdb.com/check/{l}"),
            });
        }
        "wallet" => {
            v.push(Pivot {
                name: "Blockchair".into(),
                url: format!("https://blockchair.com/search?q={l}"),
            });
            v.push(Pivot { name: "OXT".into(), url: format!("https://oxt.me/address/{l}") });
        }
        "person" | "org" => {
            v.push(Pivot {
                name: "LinkedIn".into(),
                url: format!("https://www.linkedin.com/search/results/all/?keywords={l}"),
            });
            v.push(Pivot {
                name: "OpenCorporates".into(),
                url: format!("https://opencorporates.com/companies?q={l}"),
            });
        }
        "website" => {
            let host = data
                .get("host")
                .and_then(|h| h.as_str())
                .unwrap_or(label);
            v.push(Pivot {
                name: "Wayback".into(),
                url: format!("https://web.archive.org/web/*/{}", q(host)),
            });
        }
        _ => {}
    }
    // keep the ordering stable, cap at 16
    v.truncate(16);
    v
}

pub fn pivots_json(etype: &str, label: &str, data: &serde_json::Value) -> serde_json::Value {
    json!(pivots_for(etype, label, data)
        .into_iter()
        .map(|p| json!({"name": p.name, "url": p.url}))
        .collect::<Vec<_>>())
}
