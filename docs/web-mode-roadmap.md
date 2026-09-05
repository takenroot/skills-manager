# Web mode — MVP roadmap

> **Status: in progress.** This document is the source of truth for which
> Tauri commands still need a web-mode HTTP binding (`callInvoke` + `WEB_PATH`
> entry in `src/lib/tauri.ts`) for the web UI to feel usable without falling
> back to `skills-manager-cli`.
>
> Each checkbox corresponds to one Tauri command. A command is "done" when:
> 1. A Rust handler exists under `src-tauri/src/web/`,
> 2. The route is mounted in `src-tauri/src/web/server.rs`,
> 3. The wrapper in `src/lib/tauri.ts` uses `callInvoke(...)` instead of
>    the bare `invoke(...)` stub,
> 4. The command is exercised end-to-end via `curl` against
>    `http://127.0.0.1:8766`.

## Scope decisions (locked before starting)

| Decision | Choice |
|---|---|
| Project workspace (13 endpoints) | Out of MVP core — only `getProjects` / `addProject` / `removeProject` ship in Batch 4 |
| Custom agents | Out of MVP |
| Git backup | Out of MVP |
| Tray menu / `appExit` / `restartApp` | Out of MVP — no tray in web mode |
| `logStartupEvent` | Batch 6 — low priority, startup only |

---

## Batch 4 — P0: day-to-day usable

> Without this batch the web UI has buttons that throw
> `"no web-mode binding yet; use skills-manager-cli"` on click.

### Presets

- [ ] `applyPresetToDefault` → `POST /presets/{id}/apply` (Sidebar "应用到默认")

### Projects

- [ ] `getProjects` → `GET /projects`

### Settings

- [ ] `getSettings` → `GET /settings/{key}`
- [ ] `setSettings` → `PUT /settings/{key}`

### Skill updates

- [ ] `checkAllSkillUpdates` → `POST /skills/check-updates`
- [ ] `updateSkill` → `POST /skills/update` (single)
- [ ] `batchUpdateSkills` → `POST /skills/update` (array, dry-run shared with updateSkill)

### App updates

- [ ] `checkAppUpdate` → `GET /app-version`

### Git install flow (InstallSkills page)

- [ ] `installGit` → `POST /skills/install/git`
- [ ] `previewGitInstall` → `POST /skills/install/git/preview`
- [ ] `confirmGitInstall` → `POST /skills/install/git/confirm`
- [ ] `cancelGitPreview` → `POST /skills/install/git/cancel`

**Batch 4 total: ~12 endpoints**

---

## Batch 5 — P1: core management complete

> After this batch almost every button in the existing UI works in web mode.

### skills.sh install flow

- [ ] `installFromSkillssh` → `POST /skills/install/skillssh`
- [ ] `searchSkillssh` → `GET /skillssh/search`
- [ ] `fetchLeaderboard` → `GET /skillssh/leaderboard`

### Tags

- [ ] `getAllTags` → `GET /tags`
- [ ] `renameTag` → `POST /tags/rename`
- [ ] `deleteTag` → `POST /tags/delete`

### Preset CRUD + content

- [ ] `createPreset` → `POST /presets`
- [ ] `updatePreset` → `PUT /presets/{id}`
- [ ] `deletePreset` → `DELETE /presets/{id}`
- [ ] `reorderPresets` → `POST /presets/reorder`
- [ ] `addSkillToPreset` → `POST /presets/{id}/skills`
- [ ] `removeSkillFromPreset` → `POST /presets/{id}/skills/remove`
- [ ] `getPresetSkillOrder` → `GET /presets/{id}/skills`
- [ ] `reorderPresetSkills` → `POST /presets/{id}/skills/reorder`

### Scan + import existing

- [ ] `scanLocalSkills` → `POST /skills/scan-existing`
- [ ] `importExistingSkill` → `POST /skills/import-existing`
- [ ] `importAllDiscovered` → `POST /skills/import-existing-all`
- [ ] `batchImportFolder` → `POST /skills/batch-import-folder`

### Skill detail / diff

- [ ] `getSkillDocument` → `GET /skills/{id}/document`
- [ ] `getSourceSkillDocument` → `GET /skills/{id}/document/source`
- [ ] `getSkillSourceDiff` → `GET /skills/{id}/diff`

**Batch 5 total: ~20 endpoints**

---

## Batch 6 — P1 minor

- [ ] `setToolEnabled` → `POST /tools/{key}/enable`
- [ ] `setAllToolsEnabled` → `POST /tools/enable-all`
- [ ] `getToolOrder` → `GET /tools/order`
- [ ] `setToolOrder` → `POST /tools/order`
- [ ] `logStartupEvent` → `POST /startup-event`

**Batch 6 total: ~5 endpoints**

---

## Batch 7 — P2: advanced

### Project workspace

- [ ] `getProjectSkills` → `GET /projects/{id}/skills`
- [ ] `getProjectAgentTargets` → `GET /projects/{id}/agents`
- [ ] `addProject` → `POST /projects`
- [ ] `removeProject` → `DELETE /projects/{id}`
- [ ] `scanProjects` → `POST /projects/scan`
- [ ] `addLinkedWorkspace` → `POST /projects/linked-workspace`
- [ ] `getProjectSkillDocument` → `GET /projects/{id}/skills/document`
- [ ] `exportSkillToProject` → `POST /projects/{id}/skills/export`
- [ ] `importProjectSkillToCenter` → `POST /projects/{id}/skills/import`
- [ ] `updateProjectSkillToCenter` → `POST /projects/{id}/skills/update-to-center`
- [ ] `updateProjectSkillFromCenter` → `POST /projects/{id}/skills/update-from-center`
- [ ] `toggleProjectSkill` → `POST /projects/{id}/skills/toggle`
- [ ] `deleteProjectSkill` → `POST /projects/{id}/skills/delete`

### Source link management

- [ ] `reimportLocalSkill` → `POST /skills/{id}/reimport`
- [ ] `relinkLocalSkillSource` → `POST /skills/{id}/relink`
- [ ] `detachLocalSkillSource` → `POST /skills/{id}/detach`

### Custom agents

- [ ] `addCustomTool` → `POST /tools/custom`
- [ ] `removeCustomTool` → `DELETE /tools/custom/{key}`
- [ ] `setCustomToolPath` → `PUT /tools/custom/{key}/path`
- [ ] `resetCustomToolPath` → `DELETE /tools/custom/{key}/path`
- [ ] `setCustomToolProjectPath` → `PUT /tools/custom/{key}/project-path`
- [ ] `resetCustomToolProjectPath` → `DELETE /tools/custom/{key}/project-path`

**Batch 7 total: ~22 endpoints**

---

## Batch 8 — polish (no new endpoints)

Pure frontend: stop `AppContext` from error-toasting when a desktop-only
command has no web binding yet.

- [ ] Make `refreshProjects` fail silently in web mode (no `setTranslatedError`).
- [ ] Make `refreshAppUpdate` failure silent in web mode (matches current
      Tauri behavior of "log, never toast" — already wired, just needs
      `getProjects` to not raise).
- [ ] Make `logStartupEvent` rejection non-fatal (already `.catch(() => {})`
      in `AppContext.init` — verify it stays that way after refactors).
- [ ] Decide and document: when a wrapper still has no web binding, do we
      (a) return an empty array / null stub, or (b) throw the current
      "use skills-manager-cli" message? Current answer: (b) for read,
      but (a) for background `refresh*` calls in `AppContext` so they
      don't set `appError`.

---

## Completion criteria for "MVP shipped"

A PR can call itself MVP-ready when:

1. All checkboxes in **Batch 4** + **Batch 8** are done, **and**
2. `README.md` "Web mode (preview)" section is renamed to
   "Web mode" (drop "preview"), **and**
3. The list of "CLI-only operations" in that section is empty, **and**
4. Manual smoke test on the local `skills-manager-web` + Vite dev server
   covers: launch, view presets, apply preset to default, install a
   local-path skill, deploy/undeploy, tag, delete, view projects, view
   settings, trigger an update check.