# nexus-tauri — NEXUS shell

Desktop shell + headless runner for the NEXUS investigation workbench.
One binary, two modes:

| Mode | Build | What it does |
|---|---|---|
| **headless** (default) | `cargo run -p nexus-tauri` | Runs the nexus HTTP server (`nexus-server::serve`), prints the URL, ctrl-c exits cleanly. Needs **no webview libraries**. |
| **desktop** | `cargo run -p nexus-tauri --features desktop` | Boots the same server on an ephemeral `127.0.0.1` port and opens a Tauri 2 webview window pointed at it. |

Contract: `nexus/CONTRACT.md` §1 (env/config) and §7 (tauri shell).

## Flags

```
nexus-tauri [--port N] [--db PATH] [--check] [--headless] [--help]
```

- `--port N` — HTTP port override (beats `$NEXUS_PORT`, default **8801**)
- `--db PATH` — SQLite db override (beats `$NEXUS_DB`)
- `--check` — smoke test: builds `Config`, inits server state on a throwaway
  `/tmp` db, prints `nexus-tauri OK`, exits 0. No listening, no GUI.
- `--headless` — accepted no-op (headless is the default build)
- Desktop mode also accepts `--port` / `--db` (port defaults to ephemeral `:0`).

Env vars are exactly contract §1: `NEXUS_PORT`, `NEXUS_DB`, `NEXUS_HUB_URL`,
`OSINT_HUB_TOKEN`, `FLOWSINT_URL`, `OPENPLANTER_BIN`, `OPENPLANTER_WORKSPACES`,
`ARCHON_URL`, `LAYA_URL`, `NEXUS_LAYA_LOG`. Logging: `RUST_LOG` (default `info`).

## Run headless (no GUI deps required)

```sh
# from the nexus/ workspace root
cargo run -p nexus-tauri                     # → nexus: http://127.0.0.1:8801
cargo run -p nexus-tauri -- --port 9000 --db /tmp/probe.db
cargo run -p nexus-tauri -- --check          # smoke test
```

## Run desktop (dev)

The `desktop` feature compiles in the Tauri shell (`tauri = "2"` +
`tauri-build`); without the feature the crate builds anywhere.

```sh
# from THIS crate dir (crates/nexus-tauri)
npx --yes @tauri-apps/cli@^2 dev --features desktop

# or plain cargo — same result, no node needed
cargo run -p nexus-tauri --features desktop
```

Both open a 1440×900 window (min 1100×700), title
“NEXUS — qalarc investigation workbench”, app id `com.qalarc.nexus`.
The window is created programmatically in `src/desktop.rs`; the UI itself is
served by the embedded axum server (workbench talks same-origin `/api`, no
Tauri IPC, no capabilities needed).

## Release build

```sh
# binary only
cargo build -p nexus-tauri --features desktop --release

# full bundler (deb/appimage per tauri.conf.json targets "all")
npx --yes @tauri-apps/cli@^2 build --features desktop
```

Linux system deps: `webkit2gtk-4.1` (`libwebkit2gtk-4.1-dev`), `libgtk-3-dev`,
`librsvg2-dev` — **present on this machine**. First desktop compile pulls the
whole Tauri stack; expect several minutes.

## Files of interest

- `src/main.rs` — arg parsing, logging init, headless mode, `--check`
- `src/desktop.rs` — desktop mode (feature-gated)
- `build.rs` — runs `tauri_build::build()` **only** when
  `CARGO_FEATURE_DESKTOP` is set
- `tauri.conf.json` — schema v2; `build.frontendDist` points at
  `../../webui/dist` (resolved from this crate root; the UI bundle is embedded
  for codegen only — at runtime the webview loads the server URL). If the ui
  agent's `webui/dist` is ever missing, point `frontendDist` at the hermetic
  local `assets/` dir (trivial placeholder included). `app.windows` is empty
  by design (window created in `src/desktop.rs`).
- `icons/` — 32×32 / 128×128 / 64×64 (`nexus.png`), generated RGBA PNGs
