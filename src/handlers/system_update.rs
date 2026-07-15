//! Lets an admin trigger an app update or restart entirely from the UI, and
//! optionally schedule automatic update checks.
//!
//! This process itself never touches its own binary or calls anything
//! privileged — it only ever writes one word ("update" or "restart") into
//! `data/update_requested`, a file inside its own already-writable data
//! directory. A completely separate, root-owned systemd path unit
//! (installed by deploy/install.sh, living outside this app's install
//! directory entirely) watches for that file and does the actual privileged
//! work. Even a fully compromised app process can only ever *request* one of
//! two fixed actions — it can never reach or modify the privileged side.

use askama::Template;
use axum::Extension;
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use serde::Deserialize;
use std::time::Duration;

use crate::AppState;
use crate::models::User;
use crate::security::{self, CurrentUser};

const FLAG_FILE: &str = "data/update_requested";
const REPO_API_URL: &str =
    "https://api.github.com/repos/siesta5787/board-game-tracker/releases/latest";
const SCHEDULE_FILE: &str = "data/app_update_schedule.conf";
const LAST_CHECK_FILE: &str = "data/app_update_last_check";

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

async fn latest_release_tag() -> Option<String> {
    let client = reqwest::Client::builder()
        .user_agent("board-game-tracker (self-hosted, github.com)")
        .timeout(Duration::from_secs(8))
        .build()
        .ok()?;
    let response = client.get(REPO_API_URL).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .json::<LatestRelease>()
        .await
        .ok()
        .map(|r| r.tag_name)
}

#[derive(Clone)]
struct AppUpdateScheduleConfig {
    check_interval_hours: String,
    auto_install_enabled: bool,
}

impl AppUpdateScheduleConfig {
    fn defaults() -> Self {
        AppUpdateScheduleConfig {
            check_interval_hours: "24".to_string(),
            auto_install_enabled: false,
        }
    }

    async fn load() -> Self {
        let Ok(contents) = tokio::fs::read_to_string(SCHEDULE_FILE).await else {
            return Self::defaults();
        };
        let mut config = Self::defaults();
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().to_string();
            match key.trim() {
                "CHECK_INTERVAL_HOURS" => config.check_interval_hours = value,
                "AUTO_INSTALL_ENABLED" => config.auto_install_enabled = value == "true",
                _ => {}
            }
        }
        config
    }

    fn to_file_contents(&self) -> String {
        format!(
            "CHECK_INTERVAL_HOURS={}\nAUTO_INSTALL_ENABLED={}\n",
            self.check_interval_hours, self.auto_install_enabled,
        )
    }
}

#[derive(Template)]
#[template(path = "admin_update.html")]
struct UpdateTemplate {
    title: String,
    username: String,
    current_version: String,
    latest_version: Option<String>,
    update_available: bool,
    check_failed: bool,
    message: Option<String>,
    watcher_warning: Option<String>,
    schedule: AppUpdateScheduleConfig,
}

async fn render_update_page(current: &User, message: Option<String>) -> Html<String> {
    let current_version = crate::APP_VERSION.to_string();
    let latest_version = latest_release_tag().await;
    let check_failed = latest_version.is_none();
    let update_available = latest_version
        .as_deref()
        .is_some_and(|latest| latest != current_version);

    Html(
        UpdateTemplate {
            title: "Software update".to_string(),
            username: current.username.clone(),
            current_version,
            latest_version,
            update_available,
            check_failed,
            message,
            watcher_warning: security::watcher_version_warning().await,
            schedule: AppUpdateScheduleConfig::load().await,
        }
        .render()
        .unwrap(),
    )
}

pub async fn show_update_page(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    render_update_page(&current, None).await
}

async fn request_action(
    state: &AppState,
    current: &User,
    action: &str,
    event_type: &str,
    started_message: &str,
) -> Html<String> {
    let message = if tokio::fs::write(FLAG_FILE, action).await.is_ok() {
        security::record_security_event(&state.db, event_type, Some(&current.username), None, None)
            .await;
        started_message.to_string()
    } else {
        "Couldn't write the request file — the update watcher may not be set up on this install."
            .to_string()
    };
    render_update_page(current, Some(message)).await
}

pub async fn trigger_update(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    request_action(
        &state,
        &current,
        "update",
        "system_update_triggered",
        "Update started. The app will download the new version and restart automatically — \
         this can take about a minute. Refresh this page shortly.",
    )
    .await
}

pub async fn trigger_restart(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    request_action(
        &state,
        &current,
        "restart",
        "system_restart_triggered",
        "Restarting now. Refresh this page in a few seconds.",
    )
    .await
}

#[derive(Deserialize)]
pub struct AppUpdateScheduleForm {
    check_interval_hours: String,
    auto_install_enabled: Option<String>,
}

const VALID_INTERVALS: [&str; 4] = ["1", "6", "12", "24"];

pub async fn save_app_update_schedule(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    axum::Form(form): axum::Form<AppUpdateScheduleForm>,
) -> impl IntoResponse {
    let check_interval_hours = if VALID_INTERVALS.contains(&form.check_interval_hours.as_str()) {
        form.check_interval_hours
    } else {
        "24".to_string()
    };
    let config = AppUpdateScheduleConfig {
        check_interval_hours,
        auto_install_enabled: form.auto_install_enabled.is_some(),
    };

    let message = if tokio::fs::write(SCHEDULE_FILE, config.to_file_contents())
        .await
        .is_ok()
    {
        security::record_security_event(
            &state.db,
            "app_update_schedule_changed",
            Some(&current.username),
            None,
            None,
        )
        .await;
        "Update-check schedule saved.".to_string()
    } else {
        "Couldn't save the schedule.".to_string()
    };

    render_update_page(&current, Some(message)).await
}

/// Background task: checks for a new app release on the configured
/// interval and, if auto-install is enabled and a new version is found,
/// triggers the same "update" flag-file request the manual button uses.
/// Only ever writes the word "update" — an action every version of the
/// watcher has understood since it was first introduced — so unlike the
/// OS-update/format-drive actions, this one has no watcher-version-skew
/// concern to worry about.
pub async fn run_scheduled_app_update_check(_state: AppState) {
    let mut interval = tokio::time::interval(Duration::from_secs(15 * 60));
    loop {
        interval.tick().await;

        let schedule = AppUpdateScheduleConfig::load().await;
        if !schedule.auto_install_enabled {
            continue;
        }
        if !is_due(&schedule).await {
            continue;
        }

        let now = chrono::Local::now().to_rfc3339();
        if tokio::fs::write(LAST_CHECK_FILE, &now).await.is_err() {
            tracing::warn!("couldn't write app-update last-check marker, skipping this cycle");
            continue;
        }

        let Some(latest) = latest_release_tag().await else {
            continue;
        };
        if latest != crate::APP_VERSION {
            tracing::info!("auto-update: new version {latest} found, triggering update");
            let _ = tokio::fs::write(FLAG_FILE, "update").await;
        }
    }
}

async fn is_due(schedule: &AppUpdateScheduleConfig) -> bool {
    let Ok(hours) = schedule.check_interval_hours.trim().parse::<i64>() else {
        return false;
    };
    let Ok(last_check_str) = tokio::fs::read_to_string(LAST_CHECK_FILE).await else {
        return true;
    };
    let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(last_check_str.trim()) else {
        return true;
    };
    let elapsed = chrono::Local::now().signed_duration_since(last_check);
    elapsed >= chrono::Duration::hours(hours)
}
