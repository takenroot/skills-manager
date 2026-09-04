//! Web-only entry point. Reads the same SQLite database the desktop app
//! uses and serves it over HTTP — the React frontend can be served by a
//! plain `npm run dev` and proxy `/api/*` here.
//!
//! MVP scope: read-only. All write operations still go through
//! `skills-manager-cli` (see `skills-manager-cli --help`).
//!
//! Usage:
//!   skills-manager-web [--host 127.0.0.1] [--port 8765] [--skills-root <path>]
//!
//! `--skills-root` mirrors the CLI flag and lets you point the server at a
//! non-default repo, the same way `--skills-root /path/to/repo` does for
//! `skills-manager-cli`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use app_lib::core::{app_state, central_repo};
use app_lib::web;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "skills-manager-web", version, about = "Read-only HTTP surface for skills-manager")]
struct Args {
    /// Bind address. Default 127.0.0.1 (localhost only — the CORS layer is
    /// permissive but the listener itself does not authenticate, so do not
    /// expose 0.0.0.0 without a reverse proxy in front).
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value_t = 8765)]
    port: u16,

    /// Operate on a different skills root. Mirrors the CLI's `--skills-root`.
    /// The app's DB and metadata live under `~/.skills-manager/external/<name>-<hash>/`,
    /// the same as when the CLI uses this flag.
    #[arg(long)]
    skills_root: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(skills_root) = &Args::parse().skills_root {
        let base = central_repo::external_base_dir(skills_root);
        central_repo::set_runtime_base_dir_override(Some(base));
        central_repo::set_runtime_skills_dir_override(Some(skills_root.clone()));
    }

    let store = app_state::initialize_cli_store()
        .context("failed to initialize skill store")?;

    let args = Args::parse();
    web::run_server(&args.host, args.port, store).await
}