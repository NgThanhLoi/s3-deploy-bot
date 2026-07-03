# Fast Deploy Presets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Telegram-managed Fast Deploy presets with create, run, edit, and delete support.

**Architecture:** Add a small persistent preset store backed by `app.data_dir/fast_deploy_presets.json`, then extend the existing Telegram deploy session with Fast Deploy management steps. Running a preset reuses the current confirm and job creation path instead of adding a second deploy pipeline.

**Tech Stack:** Rust, Tokio, Teloxide inline keyboards, Serde JSON, existing config/auth/session/job modules.

---

## File Structure

- Create `src/fast_preset.rs`: data model, JSON load/save, per-user CRUD, validation helpers.
- Modify `src/main.rs`: register the new module.
- Modify `src/commands.rs`: add `/fast`, Fast Deploy callback handlers, preset CRUD screens, and tests.
- Modify `src/menu.rs`: add Fast Deploy keyboards.
- Modify `src/session.rs`: extend deploy session state to support preset creation/editing without creating a separate session store.
- Modify `Cargo.toml` if `serde_json` is not already present.
- Modify `.gitignore`: ignore runtime preset JSON only if data files are not already ignored.
- Modify `README.md`: document Telegram Fast Deploy preset behavior.

## Task 1: Preset Storage

**Files:**
- Create: `src/fast_preset.rs`
- Modify: `src/main.rs`
- Test: `src/fast_preset.rs`

- [ ] **Step 1: Write failing storage tests**

Add tests covering create/list/update/delete and max 10 presets per user:

```rust
#[tokio::test]
async fn store_lists_only_presets_for_owner() {
    let dir = tempfile::tempdir().unwrap();
    let store = FastPresetStore::new(dir.path().join("fast_deploy_presets.json"));

    store
        .create(1, NewFastPreset {
            name: "WebPOS staging".into(),
            project: "webpos".into(),
            environment: "staging".into(),
            branch: "s3-retail-prod".into(),
            action: FastPresetAction::Deploy,
        })
        .await
        .unwrap();
    store
        .create(2, NewFastPreset {
            name: "Other".into(),
            project: "api".into(),
            environment: "staging".into(),
            branch: "develop".into(),
            action: FastPresetAction::Build,
        })
        .await
        .unwrap();

    let mine = store.list_for_owner(1).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].name, "WebPOS staging");
}
```

- [ ] **Step 2: Verify red**

Run:

```bash
cargo test fast_preset
```

Expected: fail because `fast_preset` module/types do not exist.

- [ ] **Step 3: Implement storage**

Create `FastPreset`, `NewFastPreset`, `FastPresetAction`, and `FastPresetStore`. Use an internal `tokio::sync::Mutex<()>` around file read/write to serialize writes. Store JSON as:

```json
{
  "version": 1,
  "presets": []
}
```

Public API:

```rust
impl FastPresetStore {
    pub fn new(path: PathBuf) -> Self;
    pub async fn list_for_owner(&self, owner_user_id: i64) -> Result<Vec<FastPreset>>;
    pub async fn get_for_owner(&self, owner_user_id: i64, preset_id: &str) -> Result<Option<FastPreset>>;
    pub async fn create(&self, owner_user_id: i64, input: NewFastPreset) -> Result<FastPreset>;
    pub async fn update(&self, owner_user_id: i64, preset_id: &str, input: NewFastPreset) -> Result<FastPreset>;
    pub async fn delete(&self, owner_user_id: i64, preset_id: &str) -> Result<bool>;
}
```

- [ ] **Step 4: Verify green**

Run:

```bash
cargo test fast_preset
```

Expected: preset storage tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/fast_preset.rs src/main.rs Cargo.toml Cargo.lock
git commit -m "Add fast deploy preset storage"
```

## Task 2: Session State And Validation

**Files:**
- Modify: `src/session.rs`
- Modify: `src/commands.rs`
- Test: `src/commands.rs`

- [ ] **Step 1: Write failing validation tests**

Add tests that confirm a preset is rejected when its deploy target is missing, branch is not allowed, or user lacks deploy permission.

- [ ] **Step 2: Verify red**

Run:

```bash
cargo test fast_preset_validation
```

Expected: fail because validation helpers and session state are not present.

- [ ] **Step 3: Extend session state**

Add session steps:

```rust
FastPresetList,
FastPresetManageList,
FastPresetManageOne,
FastPresetCreateName,
FastPresetEditField,
FastPresetDeleteConfirm,
```

Add optional fields:

```rust
pub fast_preset_id: Option<String>,
pub fast_preset_name: Option<String>,
pub fast_preset_editing: bool,
```

- [ ] **Step 4: Add validation helper**

In `commands.rs`, add:

```rust
fn validate_preset_for_user(
    ctx: &AuthContext,
    config: &Config,
    preset: &FastPreset,
) -> Result<DeployAction, String>
```

It converts preset action to `DeployAction`, verifies target/project/environment/branch, then calls `check_action_permission`.

- [ ] **Step 5: Verify green**

Run:

```bash
cargo test fast_preset_validation
```

Expected: validation tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/session.rs src/commands.rs
git commit -m "Validate fast deploy presets"
```

## Task 3: Fast Deploy Menus

**Files:**
- Modify: `src/menu.rs`
- Test: `src/commands.rs`

- [ ] **Step 1: Write failing keyboard tests**

Test that Fast Deploy list renders preset buttons plus create/manage controls, and that manage-one renders run/edit/delete/back.

- [ ] **Step 2: Verify red**

Run:

```bash
cargo test fast_preset_keyboard
```

Expected: fail because new menu functions do not exist.

- [ ] **Step 3: Implement menu functions**

Add:

```rust
pub fn fast_preset_list_keyboard(presets: &[FastPreset], has_config_default: bool) -> InlineKeyboardMarkup;
pub fn fast_preset_manage_keyboard(presets: &[FastPreset]) -> InlineKeyboardMarkup;
pub fn fast_preset_manage_one_keyboard(preset_id: &str) -> InlineKeyboardMarkup;
pub fn fast_preset_edit_field_keyboard(preset_id: &str) -> InlineKeyboardMarkup;
pub fn fast_preset_delete_confirm_keyboard(preset_id: &str) -> InlineKeyboardMarkup;
```

Use callback prefixes:

- `fast:run:{id}`
- `fast:default`
- `fast:create`
- `fast:manage`
- `fast:manage_one:{id}`
- `fast:edit:{id}`
- `fast:delete:{id}`
- `fast:delete_yes:{id}`
- `fast:delete_no:{id}`

- [ ] **Step 4: Verify green**

Run:

```bash
cargo test fast_preset_keyboard
```

Expected: keyboard tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/menu.rs src/commands.rs
git commit -m "Add fast deploy preset menus"
```

## Task 4: Telegram Flow

**Files:**
- Modify: `src/commands.rs`
- Modify: `src/session.rs`
- Test: `src/commands.rs`

- [ ] **Step 1: Write failing callback tests**

Add tests for callback routing:

- `quick:deploy` opens preset list instead of requiring config.
- `fast:run:{id}` loads preset into session and moves to `Confirm`.
- `fast:delete_yes:{id}` removes only the current user's preset.

- [ ] **Step 2: Verify red**

Run:

```bash
cargo test fast_preset_flow
```

Expected: fail because callbacks are not implemented.

- [ ] **Step 3: Add `/fast` command**

Extend `Command`:

```rust
#[command(description = "Open Fast Deploy presets")]
Fast,
```

Route it in `bot.rs` to a new `handle_fast` that authenticates, checks build permission, creates or reuses a session, then shows the Fast Deploy list.

- [ ] **Step 4: Change `/deploy` Fast Deploy button behavior**

`menu::environment_keyboard` should always include `⚡ Fast deploy`. In callback handling, `quick:deploy` should call `show_fast_preset_list`.

- [ ] **Step 5: Implement run/delete/manage callbacks**

Add handlers for `fast:*` callback prefixes before the normal deploy-step match. Running a preset should:

```rust
session.environment_key = Some(preset.environment.clone());
session.project_key = Some(preset.project.clone());
session.branch = Some(preset.branch.clone());
session.action = Some(action);
session.set_step(SessionStep::Confirm);
```

Then call `show_step`.

- [ ] **Step 6: Implement create/edit using existing wizard choices**

Create flow:

1. Ask name by text message.
2. Select environment.
3. Select project.
4. Select branch.
5. Select action.
6. Show `▶️ Chạy ngay`, `💾 Lưu`, `❌ Hủy`.

Edit flow loads an existing preset into session, then lets the user select field to change and saves the updated preset after validation.

- [ ] **Step 7: Verify green**

Run:

```bash
cargo test fast_preset_flow
cargo test
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/commands.rs src/session.rs src/menu.rs src/fast_preset.rs
git commit -m "Add Telegram fast deploy preset flow"
```

## Task 5: AppState Wiring And Docs

**Files:**
- Modify: `src/commands.rs`
- Modify: `src/bot.rs`
- Modify: `README.md`
- Modify: `config.example.toml`

- [ ] **Step 1: Wire store into AppState**

Add:

```rust
pub fast_preset_store: FastPresetStore,
```

Initialize it from:

```rust
config.app.data_dir.join("fast_deploy_presets.json")
```

- [ ] **Step 2: Document usage**

README should explain:

- `/fast`
- `⚡ Fast deploy`
- Per-user presets
- Runtime JSON storage path
- Existing `[quick_deploy]` is optional fallback/default

- [ ] **Step 3: Run full verification**

Run:

```bash
cargo fmt
cargo test
cargo clippy
```

Expected: all commands exit 0.

- [ ] **Step 4: Commit**

```bash
git add README.md config.example.toml src
git commit -m "Document fast deploy presets"
```

## Self-Review

- Spec coverage: create, run, edit, delete, many presets per user, permission checks, production double confirm, config fallback, runtime storage are covered.
- Placeholder scan: no TBD/TODO placeholders.
- Type consistency: plan consistently uses `FastPreset`, `NewFastPreset`, `FastPresetAction`, and `FastPresetStore`.
