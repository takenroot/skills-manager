//! HTTP write surface for the web mode (MVP, batch 2).
//!
//! Exposes the most common mutating Tauri commands as POST endpoints, so the
//! web frontend can install, deploy, and tag skills without round-tripping
//! through `skills-manager-cli`. All endpoints share the same
//! `AppError::IntoResponse` mapping as the read surface — see
//! `handlers.rs` for the status code table.
//!
//! No authentication: the server is intended for localhost use (or behind a
//! reverse proxy the user trusts). Every endpoint mutates the same SQLite
//! the desktop GUI reads, so the file-watcher's eventual refresh will see
//! the change too.
//!
//! Source types for `install`: this batch only exposes local-path install.
//! Git / skills.sh installs need network IO and stay CLI-only for now —
//! they throw `AppError::invalid_input` if requested over HTTP.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::commands::skills::{self as cmd, BatchDeleteSkillsResult, InstallSourceMetadata};
use crate::core::error::AppError;
use crate::core::installer;
use crate::core::repo_lock::RepoLock;
use crate::core::scenario_service::{self, BatchApplyMode};
use crate::core::skill_store::SkillStore;

pub type Store = State<Arc<SkillStore>>;

fn join_blocking<T: Send + 'static>(
    fut: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    tokio::task::spawn_blocking(fut)
}

/// Resolve a list of CLI-style references (`id` or `name` or `central_path` basenames)
/// into `SkillRecord`s, mirroring the desktop app's `resolve_skill` behavior.
/// This is intentionally inlined rather than calling the CLI's `resolve_skill`
/// helper, which lives in `bin/skills-manager-cli.rs` and is not pub.
fn resolve_references(
    store: &SkillStore,
    references: &[String],
) -> Result<Vec<crate::core::skill_store::SkillRecord>, AppError> {
    if references.is_empty() {
        return Err(AppError::invalid_input("at least one reference is required"));
    }
    let all = store.get_all_skills().map_err(AppError::db)?;
    let mut out = Vec::new();
    for reference in references {
        let match_ = all
            .iter()
            .find(|s| {
                s.id == *reference
                    || s.name == *reference
                    || PathBuf::from(&s.central_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        == Some(reference.as_str())
            })
            .cloned();
        match match_ {
            Some(s) => {
                if !out.iter().any(|existing: &crate::core::skill_store::SkillRecord| {
                    existing.id == s.id
                }) {
                    out.push(s);
                }
            }
            None => return Err(AppError::not_found(format!("skill not found: {reference}"))),
        }
    }
    Ok(out)
}

// ── POST /api/skills/deploy ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DeployRequest {
    /// CLI-style references: skill id (uuid), name, or central-path basename.
    pub references: Vec<String>,
    /// Tool keys (e.g. `claude_code`, `codex`). Empty array is rejected —
    /// mirror the CLI's `bail!("no agent key provided")` behavior.
    pub agents: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize)]
pub struct DeployResponse {
    pub pair_count: usize,
    pub dry_run: bool,
}

pub async fn deploy_skills(
    State(store): Store,
    Json(req): Json<DeployRequest>,
) -> Result<Json<DeployResponse>, AppError> {
    let store = store.clone();
    let result = join_blocking(move || {
        if req.agents.is_empty() {
            return Err(AppError::invalid_input(
                "no agent key provided in `agents`",
            ));
        }
        let skills = resolve_references(&store, &req.references)?;
        let skill_ids: Vec<String> = skills.iter().map(|s| s.id.clone()).collect();
        let agent_keys = req.agents.clone();
        let pair_count = skill_ids.len() * agent_keys.len();
        if !req.dry_run {
            scenario_service::apply_skills_to_tools(
                &store,
                &skill_ids,
                &agent_keys,
                BatchApplyMode::Add,
            )?;
        }
        Ok::<_, AppError>(DeployResponse {
            pair_count,
            dry_run: req.dry_run,
        })
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(result))
}

// ── POST /api/skills/undeploy ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct UndeployRequest {
    pub references: Vec<String>,
    pub agents: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize)]
pub struct UndeployResponse {
    pub pair_count: usize,
    pub dry_run: bool,
}

pub async fn undeploy_skills(
    State(store): Store,
    Json(req): Json<UndeployRequest>,
) -> Result<Json<UndeployResponse>, AppError> {
    let store = store.clone();
    let result = join_blocking(move || {
        let skills = resolve_references(&store, &req.references)?;
        let skill_ids: Vec<String> = skills.iter().map(|s| s.id.clone()).collect();
        // For undeploy, an empty agent list means "all currently-deployed tools
        // for these skills". Mirror the CLI's `select_agent_keys_for_removal`.
        let agent_keys: Vec<String> = if req.agents.is_empty() {
            let targets = store.get_all_targets().map_err(AppError::db)?;
            let mut keys: std::collections::BTreeSet<String> = Default::default();
            for target in targets
                .iter()
                .filter(|t| skill_ids.contains(&t.skill_id) && t.status == "ok")
            {
                keys.insert(target.tool.clone());
            }
            keys.into_iter().collect()
        } else {
            req.agents.clone()
        };
        let pair_count = skill_ids.len() * agent_keys.len();
        if !req.dry_run {
            scenario_service::apply_skills_to_tools(
                &store,
                &skill_ids,
                &agent_keys,
                BatchApplyMode::Remove,
            )?;
        }
        Ok::<_, AppError>(UndeployResponse {
            pair_count,
            dry_run: req.dry_run,
        })
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(result))
}

// ── POST /api/skills/install ───────────────────────────────────────────
//
// MVP scope: local-path install only. Git / skills.sh installs need
// network IO and stay CLI-only — refuse them with a clear message.

#[derive(Deserialize)]
pub struct InstallRequest {
    pub source_path: String,
    pub name: Option<String>,
}

#[derive(Serialize)]
pub struct InstallResponse {
    pub skill_id: String,
    pub name: String,
    pub central_path: String,
}

pub async fn install_skill(
    State(store): Store,
    Json(req): Json<InstallRequest>,
) -> Result<Json<InstallResponse>, AppError> {
    let store = store.clone();
    let result = join_blocking(move || {
        let path = PathBuf::from(&req.source_path);
        if !path.exists() {
            return Err(AppError::invalid_input(format!(
                "local path does not exist: {}",
                path.display()
            )));
        }
        let _lock = RepoLock::acquire_foreground("web install local")
            .map_err(AppError::db)?;
        let install_result =
            installer::install_from_local(&path, req.name.as_deref()).map_err(AppError::io)?;
        let metadata = InstallSourceMetadata {
            source_type: "local".to_string(),
            source_ref: Some(req.source_path.clone()),
            source_ref_resolved: None,
            source_subpath: None,
            source_branch: None,
            source_revision: None,
            remote_revision: None,
            update_status: "local_only".to_string(),
        };
        let skill_id = cmd::store_installed_skill_unlocked(
            &store,
            &install_result,
            &metadata,
            None,
        )?;
        Ok::<_, AppError>(InstallResponse {
            skill_id,
            name: install_result.name,
            central_path: install_result.central_path.to_string_lossy().to_string(),
        })
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(result))
}

// ── POST /api/skills/remove ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RemoveRequest {
    /// CLI-style references: skill id, name, or central-path basename.
    pub references: Vec<String>,
}

#[derive(Serialize)]
pub struct RemoveResponse {
    pub deleted: usize,
    pub failed: Vec<String>,
}

pub async fn remove_skills(
    State(store): Store,
    Json(req): Json<RemoveRequest>,
) -> Result<Json<RemoveResponse>, AppError> {
    if req.references.is_empty() {
        return Err(AppError::invalid_input("at least one reference is required"));
    }
    let store = store.clone();
    let result: BatchDeleteSkillsResult = join_blocking(move || {
        let skills = resolve_references(&store, &req.references)?;
        let ids: Vec<String> = skills.iter().map(|s| s.id.clone()).collect();
        cmd::delete_managed_skills_by_ids(&store, &ids)
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(RemoveResponse {
        deleted: result.deleted,
        failed: result.failed,
    }))
}

// ── POST /api/skills/tag ───────────────────────────────────────────────
//
// Replace tag set for one skill. Replaces; does not merge. The
// desktop GUI already shows the resulting set and re-renders.

#[derive(Deserialize)]
pub struct TagRequest {
    /// Skill reference (id, name, or central-path basename).
    pub reference: String,
    pub tags: Vec<String>,
}

#[derive(Serialize)]
pub struct TagResponse {
    pub skill_id: String,
    pub name: String,
    pub tags: Vec<String>,
}

pub async fn set_tags(
    State(store): Store,
    Json(req): Json<TagRequest>,
) -> Result<Json<TagResponse>, AppError> {
    let store = store.clone();
    let result = join_blocking(move || {
        let skills = resolve_references(&store, std::slice::from_ref(&req.reference))?;
        let skill = skills.into_iter().next().ok_or_else(|| {
            AppError::not_found(format!("skill not found: {}", req.reference))
        })?;
        cmd::set_skill_tags_internal(&store, &skill.id, &req.tags)?;
        // Re-read tags from store so we return the canonical (trimmed, deduped) set.
        let tags_map = store.get_tags_map().map_err(AppError::db)?;
        let tags = tags_map.get(&skill.id).cloned().unwrap_or_default();
        Ok::<_, AppError>(TagResponse {
            skill_id: skill.id,
            name: skill.name,
            tags,
        })
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(result))
}

// ── router glue ───────────────────────────────────────────────────────

use axum::routing::post;
use axum::Router;

pub fn write_routes() -> Router<Arc<SkillStore>> {
    Router::new()
        .route("/api/skills/deploy", post(deploy_skills))
        .route("/api/skills/undeploy", post(undeploy_skills))
        .route("/api/skills/install", post(install_skill))
        .route("/api/skills/remove", post(remove_skills))
        .route("/api/skills/tag", post(set_tags))
}