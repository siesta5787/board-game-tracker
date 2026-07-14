//! Lets an admin trigger an app update or restart entirely from the UI.
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
