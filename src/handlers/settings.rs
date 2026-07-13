use askama::Template;
use axum::Extension;
use axum::extract::State;
use axum::response::{Html, IntoResponse};

use crate::AppState;
use crate::security::CurrentUser;

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    title: String,
    username: String,
    is_admin: bool,
    unread_notifications: i64,
}

pub async fn show_settings(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    let unread_notifications: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications \
         WHERE user_id = ? AND type = 'play_link_request' AND is_read = 0",
    )
    .bind(current.id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    Html(
        SettingsTemplate {
            title: "Settings".to_string(),
            username: current.username,
            is_admin: current.is_admin,
            unread_notifications,
        }
        .render()
        .unwrap(),
    )
}
