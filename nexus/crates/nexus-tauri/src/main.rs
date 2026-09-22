//! nexus-tauri — NEXUS shell binary (dual mode).
//!
//! - **HEADLESS** (default, no features): parse flags, init logging, run
//!   `nexus_server::serve` and print the URL. Needs no webview libraries.
//! - **DESKTOP** (`--features desktop`, gated module [`desktop`]): boots the
//!   server on an ephemeral port and opens a Tauri 2 webview window at it.
//!
//! Flags (both modes): `--port N` (overrides `NEXUS_PORT`), `--db PATH`
//! (overrides `NEXUS_DB`), `--check` (smoke test, no GUI), `--headless`
//! (accepted no-op, it is the default), `--help`.
//!
//! Contract: nexus/CONTRACT.md §1 (env) and §7 (tauri shell).

use std::path::PathBuf;

#[cfg(feature = "desktop")]
mod desktop;

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub(crate) struct Args {
    port: Option<u16>,
    db: Option<PathBuf>,
    check: bool,
    help: bool,
}

impl Args {
    fn parse<I: Iterator<Item = String>>(mut it: I) -> anyhow::Result<Self> {
        let mut args = Self::default();
        while let Some(flag) = it.next() {
            match flag.as_str() {
                "--port" | "-p" => {
                    let v = it
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--port needs a value"))?;
                    args.port = Some(
                        v.parse()
                            .map_err(|_| anyhow::anyhow!("--port: not a number: {v}"))?,
                    );
                }
                "--db" => {
                    let v = it.next().ok_or_else(|| anyhow::anyhow!("--db needs a value"))?;
                    args.db = Some(PathBuf::from(v));
                }
                "--check" => args.check = true,
                "--headless" => {} // implied; accepted for symmetry with contract §7
                "--help" | "-h" => args.help = true,
                other => anyhow::bail!("unknown flag: {other} (try --help)"),
            }
        }
        Ok(args)
    }
}

fn print_usage() {
    println!(
        "nexus-tauri — NEXUS shell\n\
         \n\
         USAGE:\n\
         \x20 nexus-tauri [--port N] [--db PATH] [--check] [--headless]\n\
         \n\
         FLAGS:\n\
         \x20 --port N     HTTP port override (default: $NEXUS_PORT or 8801)\n\
         \x20 --db PATH    SQLite db path override (default: $NEXUS_DB)\n\
         \x20 --check      init server state on a throwaway /tmp db, print OK, exit\n\
         \x20 --headless   implied in this build (no tauri feature)\n\
         \x20 --help       this text\n\
         \n\
         BUILD MODES:\n\
         \x20 cargo run -p nexus-tauri                        # headless server\n\
         \x20 cargo run -p nexus-tauri --features desktop     # desktop shell (GUI)"
    );
}

/// Env-filter logging init: `RUST_LOG` wins, default `info`.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// `--check`: build config, init server state on a throwaway /tmp db,
/// print "nexus-tauri OK", exit 0. No listening, no GUI.
pub(crate) async fn run_check() -> anyhow::Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("nexus-tauri-check-{}-{}", std::process::id(), stamp));
    std::fs::create_dir_all(&dir)?;

    let mut cfg = nexus_server::Config::from_env();
    cfg.db_path = dir.join("nexus.db");
    // Contract §4: `init_state(cfg) -> AppState` (no Result; it panics on
    // fatal init errors, which is fine for a smoke test — non-zero exit).
    let _state = nexus_server::init_state(cfg).await;
    println!("nexus-tauri OK");

    // Best-effort cleanup of our own scratch dir; failure is harmless.
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse(std::env::args().skip(1))?;
    if args.help {
        print_usage();
        return Ok(());
    }
    init_tracing();

    #[cfg(feature = "desktop")]
    desktop::run(args)?;

    #[cfg(not(feature = "desktop"))]
    {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(headless(args))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HEADLESS mode (default build)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "desktop"))]
async fn headless(args: Args) -> anyhow::Result<()> {
    if args.check {
        return run_check().await;
    }

    // `serve()` reads NEXUS_PORT itself (contract §1); a --port flag must
    // therefore be injected through the environment.
    if let Some(p) = args.port {
        std::env::set_var("NEXUS_PORT", p.to_string());
    }
    let mut cfg = nexus_server::Config::from_env();
    if let Some(db) = args.db {
        cfg.db_path = db;
    }

    let port = resolve_port(args.port);
    println!("nexus: http://127.0.0.1:{port}");

    // serve() owns the accept loop; we just watch it and translate ctrl-c
    // into a clean exit.
    let server = tokio::spawn(async move { nexus_server::serve(cfg).await });
    tokio::select! {
        res = server => {
            res.map_err(|e| anyhow::anyhow!("server task panicked: {e}"))??;
        }
        _ = tokio::signal::ctrl_c() => {
            eprintln!("nexus: ctrl-c, shutting down");
        }
    }
    Ok(())
}

/// Effective port: flag > NEXUS_PORT > 8801 (contract §1).
#[cfg(not(feature = "desktop"))]
fn resolve_port(flag: Option<u16>) -> u16 {
    flag.or_else(|| std::env::var("NEXUS_PORT").ok().and_then(|v| v.parse().ok()))
        .unwrap_or(8801)
}
