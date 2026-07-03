# S3 Deploy Bot

Telegram Deploy Bot for ASP.NET WebForms on Windows Server.

> **⚠️ Current status: MVP deploy flow**
>
> The bot can queue an in-memory job that clones a repo branch, runs MSBuild FileSystem publish,
> deletes configured sensitive build output, zips the current IIS directory, copies build output
> over IIS with robocopy overlay mode, optionally recycles the app pool, and cleans the job workspace.
> Persistent SQLite job history, Windows service mode, and rollback are still future work.

## Setup

### 1. Set Telegram Bot Token

```bash
# Windows (Command Prompt)
set TELEGRAM_BOT_TOKEN=your_bot_token_here

# Windows (PowerShell)
$env:TELEGRAM_BOT_TOKEN = "your_bot_token_here"
```

The environment variable name can be customized in `config.toml` under `[telegram]` → `bot_token_env`.

### 2. Configure

Copy `config.example.toml` to `config.toml` and edit:

```bash
cp config.example.toml config.toml
```

Key settings:
- `[telegram].allowed_chat_ids` — list of chat IDs allowed to use the bot
- `[[users]]` — define users with their Telegram user ID
- `[roles.*]` — define permissions per role
- `[[environments]]` — define environments (staging, production, etc.)
- `[[repositories]]` — define repo URLs and branch rules
- `[[projects]]` — define buildable projects inside repositories
- `[[deploy_targets]]` — map project + environment to IIS paths

### 3. Run

```bash
cargo run -- -c config.toml
```

Or with default config path:

```bash
cargo run
```

## Getting Your User ID / Chat ID

1. Start a private chat with your bot on Telegram.
2. Send `/start` — the bot will show your user ID and chat ID.
3. Add those IDs to `config.toml`:
   - `allowed_chat_ids` — your chat ID
   - `[[users]]` — your user ID

Alternatively, send `/whoami` to see your current user info and permissions.

## Configuration Structure

```toml
[app]                    # App metadata
[telegram]               # Bot token env var, allowed chat IDs
[[users]]                # User definitions (id, name, role)
[roles.*]                # Permission sets per role
[tools]                  # Paths to git, msbuild, robocopy, 7z, appcmd
[defaults]               # Timeouts, limits
[[environments]]         # Environment definitions (key, name, double_confirm)
[[repositories]]         # Repo URLs and branch config/patterns
[[projects]]             # Project build settings inside repositories
[[deploy_targets]]       # Project + environment → IIS path mapping
```

## Permission Model

| Permission | Description |
|---|---|
| `can_build` | Can start deploy wizard and run Build Only |
| `can_deploy_staging` | Can deploy to staging environments |
| `can_deploy_production` | Can deploy to production environments |
| `can_rollback` | Can rollback deployments |
| `can_view_logs` | Can view deploy logs and status |
| `can_cancel_jobs` | Can cancel running jobs |

- `/deploy` requires `can_build` to open the wizard.
- **Build Only** action requires `can_build`.
- **Backup + Deploy IIS** to staging requires `can_deploy_staging`.
- **Backup + Deploy IIS** to production requires `can_deploy_production`.

## Branch Validation

Manual branch input is validated against these rules:

1. Trimmed, not empty
2. Max 120 characters
3. No whitespace
4. No forbidden characters: `; & | > < " '`
5. No `..` (path traversal)
6. No backslash `\`
7. Must not start or end with `/`
8. No `//`
9. Must match at least one `manual_branch_patterns` (e.g., `feature/*`, `hotfix/*`)
10. Must not match any `forbidden_branch_patterns` (e.g., `backup/*`)

## Quick Branch Keyboard

- The `main_branch` (from repository config) is always shown first with a ⭐ star.
- `quick_branches` are shown after, excluding duplicates of `main_branch`.
- If `manual_branch_enabled` is true, a "✍️ Enter branch" button is shown.

## Deploy Flow

1. Keep a persistent bare mirror cache under `workspace_root/repos/{repo-key}.git`.
2. Fetch `origin` before each job and create a fresh detached worktree under `workspace_root/jobs/{job_id}/{repo-key}-worktree`.
3. Publish the selected project with MSBuild into `{project-key}-build`.
4. Delete configured `delete_from_build` entries from the build output.
5. For deploy actions, zip the current IIS directory to `backup_root/{environment}/yyyy-MM-dd/{project}-HH-MM-SS.zip`.
6. Copy build output to IIS with `robocopy /E` overlay mode. Robocopy exit codes `0..=7` are treated as success.
7. Recycle the configured app pool when enabled.
8. Clean the job workspace on success; on failure, `keep_staging_on_failure` controls cleanup.

Git commands run with `core.longpaths=true` so Windows checkouts can handle long repository paths. Windows Long Paths should still be enabled at OS level for build tools that access the same files.

## Development

### Build

```bash
cargo build
```

### Test

```bash
cargo test
```

### Lint

```bash
cargo clippy
```

### Format

```bash
cargo fmt
```

## Project Structure

```
src/
├── main.rs      # Entry point, tracing setup
├── bot.rs       # Telegram dispatcher setup
├── commands.rs  # Command handlers, callback handlers, branch validation
├── menu.rs      # Inline keyboard builders
├── session.rs   # Session state machine
├── config.rs    # Config loading & validation
├── auth.rs      # Authentication & permission checks
├── git.rs       # Git clone / commit resolve
├── msbuild.rs   # MSBuild FileSystem publish
├── deploy.rs    # Robocopy overlay deploy
├── iis.rs       # IIS app pool recycle
├── backup.rs    # Zip backup of current IIS directory
├── staging.rs   # Build output cleanup
├── storage.rs   # Future persistent storage
├── job.rs       # In-memory job state
├── runner.rs    # Job runner
├── service.rs   # (Phase 7+) Windows service
└── log.rs       # (Phase 7+) Log viewer
```

## Phase Roadmap

| Phase | Status | Description |
|---|---|---|
| 1 | ✅ | Config loading & validation |
| 2 | ✅ | Authentication & permissions |
| 3 | ✅ | Menu & keyboard builders |
| 4 | ✅ | Session state machine & branch validation |
| 5 | ✅ | Git clone & MSBuild |
| 6 | ✅ | IIS deploy & backup |
| 7 | 🟡 | In-memory job queue and runner; persistent storage pending |
| 8 | ⬜ | Windows service, persistent log viewer, rollback |
