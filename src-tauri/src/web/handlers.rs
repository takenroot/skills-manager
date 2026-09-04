//! HTTP handlers for the read-only web surface.
//!
//! Each handler is a thin wrapper around the same store access the Tauri
//! `#[tauri::command]` versions run, so the JSON shape on the wire is the
//! shape the desktop GUI already consumes — no parallel DTO definitions to
//! drift.
//!
//! Errors flow through `AppError::IntoResponse`, which maps `ErrorKind` to
//! HTTP status codes (NotFound → 404, InvalidInput → 400, others → 500).
//! The body is the serialized `AppError` JSON the frontend already knows how
//! to parse, with an extra top-level `ok: false` to keep the CLI envelope
//! shape consistent (`{"ok": ..., "error": ...}`).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::json;

use crate::commands::presets::{preset_dto, PresetDto};
use crate::commands::skills::{managed_skill_to_dto, ManagedSkillDto};
use crate::commands::tools::ToolInfoDto;
use crate::core::error::AppError;
use crate::core::skill_store::SkillStore;
use crate::core::tool_service;

/// Shared axum state: an `Arc<SkillStore>` (same shape Tauri hands commands).
pub type Store = State<Arc<SkillStore>>;

// ── AppError → HTTP ──────────────────────────────────────────────────────

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self.kind {
            crate::core::error::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            crate::core::error::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
            crate::core::error::ErrorKind::TargetConflict => StatusCode::CONFLICT,
            crate::core::error::ErrorKind::Cancelled => StatusCode::from_u16(499).unwrap(),
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = json!({
            "ok": false,
            "code": format!("{:?}", self.kind).to_lowercase(),
            "kind": self.kind,
            "message": self.message,
        });
        (status, Json(body)).into_response()
    }
}

fn join_blocking<T: Send + 'static>(
    fut: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    tokio::task::spawn_blocking(fut)
}

// ── GET /api/health ──────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub server: &'static str,
    pub version: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        server: "skills-manager-web",
        version: env!("CARGO_PKG_VERSION"),
    })
}

// ── GET /api/tools ───────────────────────────────────────────────────────

pub async fn list_tools(State(store): Store) -> Result<Json<Vec<ToolInfoDto>>, AppError> {
    let store = store.clone();
    let dtos = join_blocking(move || {
        let infos = tool_service::list_tool_info(&store);
        infos
            .into_iter()
            .map(|info| ToolInfoDto {
                key: info.key,
                display_name: info.display_name,
                installed: info.installed,
                skills_dir: info.skills_dir,
                enabled: info.enabled,
                is_custom: info.is_custom,
                has_path_override: info.has_path_override,
                project_relative_skills_dir: info.project_relative_skills_dir,
                has_project_path_override: info.has_project_path_override,
                category: info.category,
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))?;
    Ok(Json(dtos))
}

// ── GET /api/skills ──────────────────────────────────────────────────────

pub async fn list_skills(State(store): Store) -> Result<Json<Vec<ManagedSkillDto>>, AppError> {
    let store = store.clone();
    let dtos = join_blocking(move || {
        let skills = store.get_all_skills().map_err(AppError::db)?;
        let all_targets = store.get_all_targets().map_err(AppError::db)?;
        let tags_map = store.get_tags_map().map_err(AppError::db)?;
        let dtos: Vec<ManagedSkillDto> = skills
            .into_iter()
            .map(|skill| managed_skill_to_dto(&store, skill, &all_targets, &tags_map))
            .collect();
        Ok::<_, AppError>(dtos)
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(dtos))
}

// ── GET /api/skills/:id ───────────────────────────────────────────────────
//
// `:id` may be either the skill id (uuid) or its name (basename of the
// central dir). The desktop app accepts both in `resolve_skill`; mirror
// that here so a URL like `/api/skills/cnb-api` works the same way a CLI
// `skills show cnb-api` does.

pub async fn get_skill(
    Path(id): Path<String>,
    State(store): Store,
) -> Result<Json<ManagedSkillDto>, AppError> {
    let store = store.clone();
    let id_for_blocking = id.clone();
    let dto = join_blocking(move || {
        let skill = store
            .get_skill_by_id(&id_for_blocking)
            .map_err(AppError::db)?
            .or_else(|| {
                store
                    .get_all_skills()
                    .ok()
                    .and_then(|skills| skills.into_iter().find(|s| s.name == id_for_blocking))
            })
            .ok_or_else(|| AppError::not_found(format!("skill not found: {id}")))?;
        let all_targets = store.get_all_targets().map_err(AppError::db)?;
        let tags_map = store.get_tags_map().map_err(AppError::db)?;
        Ok::<_, AppError>(managed_skill_to_dto(
            &store,
            skill,
            &all_targets,
            &tags_map,
        ))
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(dto))
}

// ── GET /api/presets ─────────────────────────────────────────────────────

pub async fn list_presets(State(store): Store) -> Result<Json<Vec<PresetDto>>, AppError> {
    let store = store.clone();
    let dtos = join_blocking(move || {
        let scenarios = store.get_all_scenarios().map_err(AppError::db)?;
        let dtos: Vec<PresetDto> = scenarios
            .into_iter()
            .map(|s| preset_dto(&store, s))
            .collect();
        Ok::<_, AppError>(dtos)
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(dtos))
}

// ── GET /api/presets/active ──────────────────────────────────────────────

pub async fn get_active_preset(State(store): Store) -> Result<Json<Option<PresetDto>>, AppError> {
    let store = store.clone();
    let dto = join_blocking(move || {
        let active_id = store.get_active_scenario_id().map_err(AppError::db)?;
        if let Some(id) = active_id {
            let scenarios = store.get_all_scenarios().map_err(AppError::db)?;
            if let Some(s) = scenarios.into_iter().find(|s| s.id == id) {
                return Ok::<_, AppError>(Some(preset_dto(&store, s)));
            }
        }
        Ok::<_, AppError>(None)
    })
    .await
    .map_err(|e| AppError::db(format!("join error: {e}")))??;
    Ok(Json(dto))
}