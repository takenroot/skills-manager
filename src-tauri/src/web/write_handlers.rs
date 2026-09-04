//! HTTP write surface for the web mode (MVP, batch 2 + batch 3).
//!
//! Batch 2: deploy / undeploy / install / remove / tag — the most common
//! mutating Tauri commands, mapped to POST endpoints that the React
//! frontend can drive without round-tripping through `skills-manager-cli`.
//!
//! Batch 3: per-agent local skill directory surface (list / import /
//! delete / update) so the WorkspaceView is functional in web mode.
//!
//! All endpoints share `AppError::IntoResponse` mapping from `handlers.rs`.
//!
//! No authentication: the server is intended for localhost use (or behind
//! a reverse proxy the user trusts). Every endpoint mutates the same
//! SQLite the desktop GUI reads.
//!
//! Source types for `install`: only local-path install. Git / skills.sh
//! installs need network IO and stay CLI-only for now.

use std::path::{Component, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::commands::skills::{self as cmd, InstallSourceMetadata};
use crate::core::content_hash;
use crate::core::error::AppError;
use crate::core::installer;
use crate::core::repo_lock::RepoLock;
use crate::core::scenario_service::{self, BatchApplyMode};
use crate::core::skill_store::SkillStore;
use crate::core::sync_engine::{self, ReplacePolicy, SyncMode};
use crate::core::tool_adapters::{self, ToolAdapter};

pub type Store = State<Arc<SkillStore>>;

fn join_blocking<T: Send + 'static>(
    fut: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    tokio::task::spawn_blocking(fut)
}

/// Resolve CLI-style references (`id`, `name`, or central-path basename)
/// into `SkillRecord`s, mirroring the desktop app's `resolve_skill`.
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
        let matched = all.iter().find(|s| {
            s.id == *reference
                || s.name == *reference
                || PathBuf::from(&s.central_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    == Some(reference.as_str())
        });
        match matched {
            Some(s) => {
                let owned = s.clone();
                if !out.iter().any(|existing: &crate::core::skill_store::SkillRecord| {
                    existing.id == owned.id
                }) {
                    out.push(owned);
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
    pub references: Vec<String>,
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
        let _lock = RepoLock::acquire_foreground("web install local").map_err(AppError::db)?;
        let install_result = installer::install_from_local(&path, req.name.as_deref())
            .map_err(AppError::io)?;
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
        let skill_id =
            cmd::store_installed_skill_unlocked(&store, &install_result, &metadata, None)?;
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
    let result = join_blocking(move || {
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

#[derive(Deserialize)]
pub struct TagRequest {
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

// ── batch 3: per-agent local skill directory ────────────────────────────

/// Resolve `agent_key` to its on-disk skills directory (already honors
/// custom-tool path overrides via `find_adapter_with_store`).
fn resolve_agent(store: &SkillStore, agent_key: &str) -> Result<ToolAdapter, AppError> {
    tool_adapters::find_adapter_with_store(store, agent_key)
        .ok_or_else(|| AppError::not_found(format!("unknown agent: {agent_key}")))
}

/// Join `skills_dir` with a relative path, refusing `..` or absolute
/// paths. The prefix check is **lexical** (not `canonicalize`): the
/// agent's `~/.claude/skills/cnb-api` is typically a symlink to the
/// central library, and `Path::canonicalize` on the joined path
/// resolves that symlink to `~/.skills-manager/skills/cnb-api`, which
/// would not `starts_with( ~/.claude/skills )`. Lexical containment
/// catches the actual hazards (`..`, absolute paths) without that
/// false positive.
fn safe_join_local(skills_dir: &PathBuf, relative: &str) -> Result<PathBuf, AppError> {
    if relative.is_empty() {
        return Err(AppError::invalid_input("relative_path is empty"));
    }
    let p = PathBuf::from(relative);
    if p.is_absolute() {
        return Err(AppError::invalid_input(format!(
            "absolute paths are not allowed: {relative}"
        )));
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(AppError::invalid_input(format!(
            "path may not contain '..': {relative}"
        )));
    }
    let joined = skills_dir.join(&p);
    if !joined.starts_with(skills_dir) {
        return Err(AppError::invalid_input(format!(
            "path escapes skills_dir: {relative}"
        )));
    }
    Ok(joined)
}

#[derive(Serialize)]
struct LocalSkillDto {
    name: String,
    relative_path: String,
    has_skill_md: bool,
    is_managed: bool,
    managed_skill_id: Option<String>,
    content_hash: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ListAgentResponse {
    agent: String,
    skills_dir: String,
    local_skills: Vec<LocalSkillDto>,
}

/// `GET /api/agents/{key}/local-skills`
pub(crate) async fn list_agent_local_skills(
    Path(agent_key): Path<String>,
    State(store): Store,
) -> Result<Json<ListAgentResponse>, AppError> {
    let store = store.clone();
    let result = join_blocking(move || {
        let adapter = resolve_agent(&store, &agent_key)?;
        let skills_dir = adapter.skills_dir();
        let canonical = skills_dir
            .canonicalize()
            .unwrap_or_else(|_| skills_dir.clone());

        let managed_skills = store.get_all_skills().map_err(AppError::db)?;

        let mut local_skills: Vec<LocalSkillDto> = Vec::new();
        let entries = match std::fs::read_dir(&canonical) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ListAgentResponse {
                    agent: adapter.key,
                    skills_dir: canonical.to_string_lossy().to_string(),
                    local_skills,
                });
            }
            Err(e) => return Err(AppError::io(e)),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let relative_path = match path.strip_prefix(&canonical) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => continue,
            };
            if relative_path.is_empty() || relative_path.contains("..") {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let has_skill_md =
                path.join("SKILL.md").is_file() || path.join("skill.md").is_file();
            let (is_managed, managed_skill_id) = managed_skills
                .iter()
                .find(|s| s.name == name)
                .map(|s| (true, Some(s.id.clone())))
                .unwrap_or((false, None));
            let hash = if has_skill_md {
                content_hash::hash_directory(&path).ok()
            } else {
                None
            };
            local_skills.push(LocalSkillDto {
                name,
                relative_path,
                has_skill_md,
                is_managed,
                managed_skill_id,
                content_hash: hash,
            });
        }
        local_skills.sort_by(|a, b| a.name.cmp(&b.name));
        Ok::<_, AppError>(ListAgentResponse {
            agent: adapter.key,
            skills_dir: canonical.to_string_lossy().to_string(),
            local_skills,
        })
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub(crate) struct AgentPathBody {
    #[serde(default)]
    relative_path: Option<String>,
    #[serde(default)]
    relative_paths: Option<Vec<String>>,
}

fn extract_paths(body: &AgentPathBody) -> Result<Vec<String>, AppError> {
    let mut out: Vec<String> = Vec::new();
    if let Some(p) = &body.relative_path {
        if !p.is_empty() {
            out.push(p.clone());
        }
    }
    if let Some(ps) = &body.relative_paths {
        out.extend(ps.iter().filter(|p| !p.is_empty()).cloned());
    }
    if out.is_empty() {
        return Err(AppError::invalid_input(
            "relative_path or relative_paths required",
        ));
    }
    Ok(out)
}

#[derive(Serialize)]
struct PathResult {
    relative_path: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    central_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct AgentMutateResponse {
    results: Vec<PathResult>,
    succeeded: usize,
    failed: usize,
}

/// `POST /api/agents/{key}/local-skills/import`
pub(crate) async fn import_agent_local_skills(
    Path(agent_key): Path<String>,
    State(store): Store,
    Json(body): Json<AgentPathBody>,
) -> Result<Json<AgentMutateResponse>, AppError> {
    let store = store.clone();
    let paths = extract_paths(&body)?;
    let result = join_blocking(move || {
        let adapter = resolve_agent(&store, &agent_key)?;
        let skills_dir = adapter.skills_dir();
        let mut results = Vec::new();
        let mut succeeded = 0usize;
        let mut failed = 0usize;

        for rel in paths {
            let local_path = match safe_join_local(&skills_dir, &rel) {
                Ok(p) => p,
                Err(e) => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: false,
                        skill_id: None,
                        central_path: None,
                        reason: Some(e.to_string()),
                    });
                    failed += 1;
                    continue;
                }
            };
            let outcome: anyhow::Result<(String, String)> = (|| {
                let _lock = RepoLock::acquire_foreground("web import local skill")
                    .map_err(AppError::db)?;
                let metadata = InstallSourceMetadata {
                    source_type: "import".to_string(),
                    source_ref: Some(local_path.to_string_lossy().to_string()),
                    source_ref_resolved: None,
                    source_subpath: None,
                    source_branch: None,
                    source_revision: None,
                    remote_revision: None,
                    update_status: "local_only".to_string(),
                };
                let install_result = installer::install_from_local(&local_path, None)
                    .map_err(AppError::io)?;
                let skill_id = cmd::store_installed_skill_unlocked(
                    &store,
                    &install_result,
                    &metadata,
                    None,
                )?;
                Ok((
                    skill_id,
                    install_result.central_path.to_string_lossy().to_string(),
                ))
            })();
            match outcome {
                Ok((skill_id, central_path)) => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: true,
                        skill_id: Some(skill_id),
                        central_path: Some(central_path),
                        reason: None,
                    });
                    succeeded += 1;
                }
                Err(e) => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: false,
                        skill_id: None,
                        central_path: None,
                        reason: Some(format!("{e:#}")),
                    });
                    failed += 1;
                }
            }
        }
        Ok::<_, AppError>(AgentMutateResponse {
            results,
            succeeded,
            failed,
        })
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(result))
}

/// `POST /api/agents/{key}/local-skills/delete`
pub(crate) async fn delete_agent_local_skills(
    Path(agent_key): Path<String>,
    State(store): Store,
    Json(body): Json<AgentPathBody>,
) -> Result<Json<AgentMutateResponse>, AppError> {
    let store = store.clone();
    let paths = extract_paths(&body)?;
    let result = join_blocking(move || {
        let adapter = resolve_agent(&store, &agent_key)?;
        let skills_dir = adapter.skills_dir();
        let mut results = Vec::new();
        let mut succeeded = 0usize;
        let mut failed = 0usize;
        for rel in paths {
            let local_path = match safe_join_local(&skills_dir, &rel) {
                Ok(p) => p,
                Err(e) => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: false,
                        skill_id: None,
                        central_path: None,
                        reason: Some(e.to_string()),
                    });
                    failed += 1;
                    continue;
                }
            };
            match std::fs::remove_dir_all(&local_path) {
                Ok(()) => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: true,
                        skill_id: None,
                        central_path: None,
                        reason: None,
                    });
                    succeeded += 1;
                }
                Err(e) => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: false,
                        skill_id: None,
                        central_path: None,
                        reason: Some(format!("{e}")),
                    });
                    failed += 1;
                }
            }
        }
        Ok::<_, AppError>(AgentMutateResponse {
            results,
            succeeded,
            failed,
        })
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub(crate) struct UpdateBody {
    #[serde(default)]
    relative_paths: Vec<String>,
    #[serde(default)]
    force: bool,
}

/// `POST /api/agents/{key}/local-skills/update`
pub(crate) async fn update_agent_local_skills(
    Path(agent_key): Path<String>,
    State(store): Store,
    Json(body): Json<UpdateBody>,
) -> Result<Json<AgentMutateResponse>, AppError> {
    if body.relative_paths.is_empty() {
        return Err(AppError::invalid_input("relative_paths required"));
    }
    let store = store.clone();
    let result = join_blocking(move || {
        let adapter = resolve_agent(&store, &agent_key)?;
        let skills_dir = adapter.skills_dir();
        let mut results = Vec::new();
        let mut succeeded = 0usize;
        let mut failed = 0usize;
        for rel in body.relative_paths {
            let local_path = match safe_join_local(&skills_dir, &rel) {
                Ok(p) => p,
                Err(e) => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: false,
                        skill_id: None,
                        central_path: None,
                        reason: Some(e.to_string()),
                    });
                    failed += 1;
                    continue;
                }
            };

            let skill = store
                .get_all_skills()
                .map_err(AppError::db)?
                .into_iter()
                .find(|s| {
                    s.name == rel
                        || PathBuf::from(&s.central_path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            == Some(rel.as_str())
                });
            let skill = match skill {
                Some(s) => s,
                None => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: false,
                        skill_id: None,
                        central_path: None,
                        reason: Some(
                            "skill not in central library; import first".to_string(),
                        ),
                    });
                    failed += 1;
                    continue;
                }
            };
            let source = PathBuf::from(&skill.central_path);
            let policy = if body.force {
                ReplacePolicy::UserConfirmed
            } else {
                ReplacePolicy::NoClobber
            };
            match sync_engine::sync_skill(&source, &local_path, SyncMode::Copy, policy) {
                Ok(_) => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: true,
                        skill_id: None,
                        central_path: None,
                        reason: None,
                    });
                    succeeded += 1;
                }
                Err(e) => {
                    results.push(PathResult {
                        relative_path: rel,
                        ok: false,
                        skill_id: None,
                        central_path: None,
                        reason: Some(format!("{e:#}")),
                    });
                    failed += 1;
                }
            }
        }
        Ok::<_, AppError>(AgentMutateResponse {
            results,
            succeeded,
            failed,
        })
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(result))
}

// ── router glue ───────────────────────────────────────────────────────

use axum::routing::{get, post};
use axum::Router;

pub fn write_routes() -> Router<Arc<SkillStore>> {
    Router::new()
        .route("/api/skills/deploy", post(deploy_skills))
        .route("/api/skills/undeploy", post(undeploy_skills))
        .route("/api/skills/install", post(install_skill))
        .route("/api/skills/remove", post(remove_skills))
        .route("/api/skills/tag", post(set_tags))
        .route(
            "/api/agents/{key}/local-skills",
            get(list_agent_local_skills),
        )
        .route(
            "/api/agents/{key}/local-skills/import",
            post(import_agent_local_skills),
        )
        .route(
            "/api/agents/{key}/local-skills/delete",
            post(delete_agent_local_skills),
        )
        .route(
            "/api/agents/{key}/local-skills/update",
            post(update_agent_local_skills),
        )
}