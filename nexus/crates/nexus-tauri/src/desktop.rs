//! DESKTOP mode (`--features desktop`), contract §7:
//! `init_state(Config::from_env())` → `build_router` → bind `127.0.0.1:0`
//! via `axum::serve` → ephemeral port → Tauri webview window navigating to
//! `http://127.0.0.1:{port}` (WebviewUrl::External). The workbench UI talks
//! same-origin HTTP to that embedded server; no Tauri IPC is used, so the
//! config carries no capabilities and `app.windows` stays empty (the window
//! is created programmatically below).

use crate::Args;

pub(crate) fn run(args: Args) -> anyhow::Result<()> {
    if args.check {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return rt.block_on(crate::run_check());
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Boot the embedded server first; keep its port for the window URL.
    let port = rt.block_on(boot_server(&args))?;
    println!("nexus: http://127.0.0.1:{port}");

    tauri::Builder::default()
        .setup(move |app| {
            let url = tauri::Url::parse(&format!("http://127.0.0.1:{port}"))?;
            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url))
                .title("NEXUS — qalarc investigation workbench")
                .inner_size(1440.0, 900.0)
                .min_inner_size(1100.0, 700.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("tauri: {e}"))?;

    // Window closed → run() returns → rt drops → server task dies with it.
    Ok(())
}

async fn boot_server(args: &Args) -> anyhow::Result<u16> {
    let mut cfg = nexus_server::Config::from_env();
    if let Some(db) = &args.db {
        cfg.db_path = db.clone();
    }

    let state = nexus_server::init_state(cfg).await;
    let app = nexus_server::build_router(state);

    // Desktop binds ephemeral (contract §7) unless --port was given.
    let bind_port = args.port.unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", bind_port)).await?;
    let port = listener.local_addr()?.port();

    // Serve for the lifetime of the process; log if it ever dies early.
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("nexus server stopped: {e}");
        }
    });
    Ok(port)
}
