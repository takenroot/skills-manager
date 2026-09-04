//! Read-only HTTP surface for `skills-manager`.
//!
//! MVP scope (2026-09): exposes the same read-side data the Tauri GUI reads
//! over HTTP, so the React frontend can be served by a plain Vite dev server
//! when the desktop shell is unavailable (headless / remote). All write-side
//! surface (`install`, `deploy`, `remove`, …) still goes through the CLI for
//! now — see `docs/web-mode.md` if that ever changes.
//!
//! Boot the server with the `skills-manager-web` binary:
//! ```bash
//! skills-manager-web --host 127.0.0.1 --port 8765
//! ```

pub mod handlers;
pub mod server;

pub use server::run_server;