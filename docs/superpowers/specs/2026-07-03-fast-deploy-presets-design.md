# Fast Deploy Presets Design

## Goal

Make Fast Deploy configurable from Telegram instead of requiring `config.toml` edits and service restarts.

## Scope

Fast Deploy supports many presets per Telegram user. Each preset has a stable id, display name, project, environment, branch, and action. Users can create, run, edit, and delete their own presets from Telegram.

## UX

`/deploy` continues to open the normal deploy wizard. Its first screen always shows `⚡ Fast deploy`.

When pressed, Fast Deploy shows:

- Existing presets owned by the current user.
- `➕ Tạo preset mới`.
- `⚙️ Quản lý preset`.
- `⬅️ Quay lại` and `❌ Hủy`.

Selecting a preset loads its fields into the normal deploy session and moves to the existing confirm screen. Production environments still use the existing double-confirm step.

Create and edit flows reuse the existing choices for project, environment, branch, and action. A preset can also be run immediately without saving. Editing supports changing name, project, environment, branch, and action. Deleting requires confirmation.

## Storage

Presets are stored under `app.data_dir` in `fast_deploy_presets.json`. The file is runtime state, not static config, so changing presets does not require service restart.

Storage is scoped by `owner_user_id`. A user cannot list, run, edit, or delete another user's presets.

The initial implementation allows up to 10 presets per user. This is enforced in code to keep the Telegram keyboard usable.

## Validation And Permission

Every create, edit, and run validates:

- Project exists.
- Environment exists.
- Deploy target exists for project and environment.
- Branch is valid for the selected project's repository. Quick branch buttons use `main_branch` and `quick_branches`; manual branch input uses existing manual branch validation.
- Action is `build` or `deploy`.
- The current user has permission for the requested action and environment.

Validation happens when saving and again when running. Rechecking on run protects against config changes after a preset was saved.

## Backward Compatibility

The existing optional `[quick_deploy]` config remains as a fallback/default. If no runtime presets exist for a user and `[quick_deploy]` is enabled, the Fast Deploy screen offers it as a runnable default and allows saving it as a user preset.

## Non-Goals

- No shared team presets in this version.
- No arbitrary branch fetch validation against remote before save. Remote branch failures remain visible in the job runner.
- No database migration. JSON storage is enough for small per-user preset lists.
