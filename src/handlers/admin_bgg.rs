//! Admin page for configuring this instance's BoardGameGeek API token.
//!
//! BGG's XML API requires an app-registration Bearer token per the project
//! notes — a credential that must never be baked into this (public,
//! forkable) codebase, since every self-hoster needs their own. Stored in
//! the database via `crate::settings`, set here by an admin, never
//! hardcoded anywhere.

use askama::Template;
use axum::Extension;
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use serde::Deserialize;

use crate::AppState;
use crate::security::{self, CurrentUser};
use crate::settings;

#[derive(Template)]
#[template(path = "admin_bgg.html")]
struct AdminBggTemplate {
    title: String,
    username: String,
    message: Option<String>,
    has_token: bool,
}

async fn render_page(
    state: &AppState,
    current_username: &str,
    message: Option<String>,
) -> Html<String> {
    let has_token = settings::get(&state.db, settings::BGG_API_TOKEN)
        .await
        .is_some();

    Html(
        AdminBggTemplate {
            title: "BGG integration".to_string(),
            username: current_username.to_string(),
            message,
            has_token,
        }
        .render()
        .unwrap(),
    )
}

pub async fn show_settings(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    render_page(&state, &current.username, None).await
}

#[derive(Deserialize)]
pub struct SaveTokenForm {
    token: String,
}

pub async fn save_token(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    axum::Form(form): axum::Form<SaveTokenForm>,
) -> impl IntoResponse {
    let token = form.token.trim();
    let message = if token.is_empty() {
        "Token can't be empty.".to_string()
    } else {
        settings::set(&state.db, settings::BGG_API_TOKEN, token).await;
        security::record_security_event(
            &state.db,
            "bgg_token_changed",
            Some(&current.username),
            None,
            None,
        )
        .await;
        "BGG API token saved.".to_string()
    };

    render_page(&state, &current.username, Some(message)).await
}

pub async fn remove_token(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    settings::delete(&state.db, settings::BGG_API_TOKEN).await;
    security::record_security_event(
        &state.db,
        "bgg_token_removed",
        Some(&current.username),
        None,
        None,
    )
    .await;

    render_page(
        &state,
        &current.username,
        Some("BGG API token removed. Search/lookup won't work until a new one is set.".to_string()),
    )
    .await
}
