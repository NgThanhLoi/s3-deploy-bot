use teloxide::types::InlineKeyboardButton;
use teloxide::types::InlineKeyboardMarkup;

use crate::config::Config;

pub fn environment_keyboard(config: &Config) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    if let Some(quick) = &config.quick_deploy {
        if quick.enabled {
            rows.push(vec![InlineKeyboardButton::callback(
                "⚡ Fast deploy",
                "quick:deploy",
            )]);
        }
    }

    rows.extend(config.environments.iter().map(|env| {
        let icon = if env.requires_double_confirm {
            "🔴"
        } else {
            "🟢"
        };
        vec![InlineKeyboardButton::callback(
            format!("{} {}", icon, env.name),
            format!("env:{}", env.key),
        )]
    }));

    rows.push(vec![cancel_button()]);
    InlineKeyboardMarkup::new(rows)
}

pub fn project_keyboard(config: &Config, env_key: &str) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = config
        .projects
        .iter()
        .filter(|p| {
            config
                .deploy_targets
                .iter()
                .any(|dt| dt.project == p.key && dt.environment == env_key)
        })
        .map(|p| {
            vec![InlineKeyboardButton::callback(
                &p.name,
                format!("project:{}", p.key),
            )]
        })
        .collect();

    rows.push(vec![back_button(), cancel_button()]);
    InlineKeyboardMarkup::new(rows)
}

pub fn branch_keyboard(
    main_branch: &str,
    quick_branches: &[String],
    manual_enabled: bool,
) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Main branch always first with ⭐
    rows.push(vec![InlineKeyboardButton::callback(
        format!("⭐ {}", main_branch),
        format!("branch:{}", main_branch),
    )]);

    // Quick branches (excluding main_branch)
    for b in quick_branches {
        if b != main_branch {
            rows.push(vec![InlineKeyboardButton::callback(
                b.to_string(),
                format!("branch:{}", b),
            )]);
        }
    }

    if manual_enabled {
        rows.push(vec![InlineKeyboardButton::callback(
            "✍️ Nhập branch khác",
            "branch:manual",
        )]);
    }

    rows.push(vec![back_button(), cancel_button()]);
    InlineKeyboardMarkup::new(rows)
}

pub fn action_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            "🧱 Chỉ build",
            "action:build",
        )],
        vec![InlineKeyboardButton::callback(
            "🚀 Backup + deploy IIS",
            "action:deploy",
        )],
        vec![back_button(), cancel_button()],
    ])
}

pub fn confirm_keyboard(double: bool) -> InlineKeyboardMarkup {
    if double {
        InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback("✅ Confirm Deploy", "confirm:yes"),
            InlineKeyboardButton::callback("❌ Hủy", "confirm:no"),
        ]])
    } else {
        InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback("✅ Tôi hiểu, tiếp tục", "confirm:yes"),
            InlineKeyboardButton::callback("❌ Hủy", "confirm:no"),
        ]])
    }
}

fn back_button() -> InlineKeyboardButton {
    InlineKeyboardButton::callback("⬅️ Quay lại", "nav:back")
}

fn cancel_button() -> InlineKeyboardButton {
    InlineKeyboardButton::callback("❌ Hủy", "nav:cancel")
}
