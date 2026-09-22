//! nexus-server CLI: `--port N`, `--db PATH`, `--check`, `--help`.

use nexus_server::Config;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut port_override: Option<u16> = None;
    let mut db_override: Option<std::path::PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" if i + 1 < args.len() => {
                port_override = args[i + 1].parse::<u16>().ok();
                if port_override.is_none() {
                    eprintln!("nexus-server: --port needs a number");
                    std::process::exit(2);
                }
                i += 2;
            }
            "--db" if i + 1 < args.len() => {
                db_override = Some(std::path::PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--check" => {
                let mut cfg = Config::from_env();
                cfg.db_path = std::env::temp_dir().join(format!(
                    "nexus-check-{}-{}.db",
                    std::process::id(),
                    chrono::Utc::now().timestamp_millis()
                ));
                cfg.config_path = None; // don't persist anything during check
                let rt = tokio::runtime::Runtime::new().expect("tokio rt");
                let state = rt.block_on(nexus_server::init_state(cfg));
                let cases = state.store.list_cases().map(|c| c.len()).unwrap_or(0);
                println!("nexus-server OK ({cases} cases)");
                std::process::exit(0);
            }
            "--help" | "-h" => {
                println!(
                    "nexus-server — NEXUS investigation workbench API\n\n\
                     USAGE: nexus-server [--port N] [--db PATH] [--check]\n\n\
                     ENV: NEXUS_DB, NEXUS_PORT, NEXUS_HUB_URL, OSINT_HUB_TOKEN,\n\
                          FLOWSINT_URL, OPENPLANTER_BIN, OPENPLANTER_WORKSPACES,\n\
                          ARCHON_URL, LAYA_URL, JEV_URL, JEV_API_KEY, NEXUS_CONFIG"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("nexus-server: unknown arg {other:?} (try --help)");
                std::process::exit(2);
            }
        }
    }

    let mut cfg = Config::from_env();
    if let Some(p) = port_override {
        cfg.port = p;
    }
    if let Some(db) = db_override {
        cfg.db_path = db;
    }

    let rt = tokio::runtime::Runtime::new().expect("nexus: tokio runtime");
    if let Err(e) = rt.block_on(nexus_server::serve(cfg)) {
        eprintln!("nexus-server: fatal: {e}");
        std::process::exit(1);
    }
}
