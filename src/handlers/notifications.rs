use askama::Template;
use axum::Extension;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect};

use crate::AppState;
use crate::security::CurrentUser;

#[derive(sqlx::FromRow)]
struct NotificationRow {
    id: i64,
    play_id: i64,
    game_name: String,
    play_date: String,
    logged_by_username: String,
}

#[derive(Template)]
#[template(path = "notifications.html")]
struct NotificationsTemplate {
    title: String,
    username: String,
    is_admin: bool,
    notifications: Vec<NotificationRow>,
}

pub async fn list_notifications(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    let notifications = sqlx::query_as::<_, NotificationRow>(
        "SELECT notifications.id, plays.id AS play_id, games.name AS game_name, plays.play_date, \
                users.username AS logged_by_username \
         FROM notifications \
         JOIN plays ON plays.id = notifications.play_id \
         JOIN games ON games.id = plays.game_id \
         JOIN users ON users.id = plays.logged_by_user_id \
         WHERE notifications.user_id = ? AND notifications.type = 'play_link_request' AND notifications.is_read = 0 \
         ORDER BY notifications.created_at DESC",
    )
    .bind(current.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    Html(
        NotificationsTemplate {
            title: "Notifications".to_string(),
            username: current.username,
            is_admin: current.is_admin,
            notifications,
        }
        .render()
        .unwrap(),
    )
}

async fn respond(state: &AppState, user_id: i64, notification_id: i64, new_status: &str) {
    let play_id: Option<i64> =
        sqlx::query_scalar("SELECT play_id FROM notifications WHERE id = ? AND user_id = ?")
            .bind(notification_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some(play_id) = play_id else { return };

    let player_id: Option<i64> = sqlx::query_scalar("SELECT id FROM players WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let Some(player_id) = player_id else { return };

    sqlx::query("UPDATE play_players SET link_status = ? WHERE play_id = ? AND player_id = ?")
        .bind(new_status)
        .bind(play_id)
        .bind(player_id)
        .execute(&state.db)
        .await
        .ok();

    sqlx::query("UPDATE notifications SET is_read = 1 WHERE id = ?")
        .bind(notification_id)
        .execute(&state.db)
        .await
        .ok();
}

pub async fn approve(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(notification_id): Path<i64>,
) -> impl IntoResponse {
    respond(&state, current.id, notification_id, "approved").await;
    Redirect::to("/notifications")
}

pub async fn decline(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(notification_id): Path<i64>,
) -> impl IntoResponse {
    respond(&state, current.id, notification_id, "declined").await;
    Redirect::to("/notifications")
}
