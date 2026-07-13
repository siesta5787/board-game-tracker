use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect};
use axum::{Extension, Form};
use serde::Deserialize;

use crate::AppState;
use crate::models::User;
use crate::plays::DISPLAY_NAME_SQL;
use crate::security::{self, CurrentUser};

pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    pub is_active: bool,
    pub totp_enabled: bool,
    pub is_locked: bool,
}

#[derive(Template)]
#[template(path = "admin_users.html")]
struct AdminUsersTemplate {
    title: String,
    username: String,
    current_user_id: i64,
    users: Vec<UserRow>,
    success: Option<String>,
    error: Option<String>,
}

async fn render_users_list(
    state: &AppState,
    current: &User,
    success: Option<String>,
    error: Option<String>,
) -> Html<String> {
    let sql = format!(
        "SELECT id, username, {DISPLAY_NAME_SQL} AS display_name, is_admin, is_active, totp_enabled, \
         (locked_until IS NOT NULL AND locked_until > datetime('now')) AS is_locked \
         FROM users ORDER BY username"
    );
    let rows: Vec<(i64, String, String, bool, bool, bool, bool)> = sqlx::query_as(&sql)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let users = rows
        .into_iter()
        .map(
            |(id, username, display_name, is_admin, is_active, totp_enabled, is_locked)| UserRow {
                id,
                username,
                display_name,
                is_admin,
                is_active,
                totp_enabled,
                is_locked,
            },
        )
        .collect();

    Html(
        AdminUsersTemplate {
            title: "Users".to_string(),
            username: current.username.clone(),
            current_user_id: current.id,
            users,
            success,
            error,
        }
        .render()
        .unwrap(),
    )
}

pub async fn list_users(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    render_users_list(&state, &current, None, None).await
}

pub async fn reset_user(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(user_id): Path<i64>,
) -> impl IntoResponse {
    let target = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let Some(target) = target else {
        return render_users_list(&state, &current, None, Some("User not found.".to_string()))
            .await
            .into_response();
    };

    let new_password = security::generate_temp_password();
    let hash = security::hash_password(&new_password);

    sqlx::query(
        "UPDATE users SET password_hash = ?, must_change_password = 1, totp_secret = NULL, totp_enabled = 0 WHERE id = ?",
    )
    .bind(&hash)
    .bind(user_id)
    .execute(&state.db)
    .await
    .ok();

    let message = format!(
        "{}'s password and two-factor login were reset. New temporary password: {} \u{2014} write this down now, it won't be shown again. They'll be asked to change it and set up two-factor login again the next time they sign in.",
        target.username, new_password
    );

    render_users_list(&state, &current, Some(message), None)
        .await
        .into_response()
}

pub async fn deactivate_user(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(user_id): Path<i64>,
) -> impl IntoResponse {
    if user_id == current.id {
        return render_users_list(
            &state,
            &current,
            None,
            Some("You can't remove your own account.".to_string()),
        )
        .await
        .into_response();
    }

    let target = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let Some(target) = target else {
        return render_users_list(&state, &current, None, Some("User not found.".to_string()))
            .await
            .into_response();
    };

    if target.is_admin {
        let active_admins: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1 AND is_active = 1")
                .fetch_one(&state.db)
                .await
                .unwrap_or(0);
        if active_admins <= 1 {
            return render_users_list(
                &state,
                &current,
                None,
                Some("Can't remove the last remaining admin.".to_string()),
            )
            .await
            .into_response();
        }
    }

    sqlx::query("UPDATE users SET is_active = 0 WHERE id = ?")
        .bind(user_id)
        .execute(&state.db)
        .await
        .ok();

    render_users_list(
        &state,
        &current,
        Some(format!("{} was removed.", target.username)),
        None,
    )
    .await
    .into_response()
}

pub async fn reactivate_user(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(user_id): Path<i64>,
) -> impl IntoResponse {
    let target = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let Some(target) = target else {
        return render_users_list(&state, &current, None, Some("User not found.".to_string()))
            .await
            .into_response();
    };

    sqlx::query("UPDATE users SET is_active = 1 WHERE id = ?")
        .bind(user_id)
        .execute(&state.db)
        .await
        .ok();

    render_users_list(
        &state,
        &current,
        Some(format!("{} was restored.", target.username)),
        None,
    )
    .await
    .into_response()
}

pub async fn unlock_user(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(user_id): Path<i64>,
) -> impl IntoResponse {
    let target = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let Some(target) = target else {
        return render_users_list(&state, &current, None, Some("User not found.".to_string()))
            .await
            .into_response();
    };

    security::reset_failed_login(&state.db, user_id).await;

    render_users_list(
        &state,
        &current,
        Some(format!("{} was unlocked.", target.username)),
        None,
    )
    .await
    .into_response()
}

pub struct SecurityEventRow {
    pub event_type: String,
    pub username: Option<String>,
    pub ip_address: Option<String>,
    pub detail: Option<String>,
    pub created_at: String,
}

pub struct BannedIpRow {
    pub ip_address: String,
    pub banned_until: String,
    pub reason: Option<String>,
}

#[derive(Template)]
#[template(path = "admin_security.html")]
struct AdminSecurityTemplate {
    title: String,
    username: String,
    events: Vec<SecurityEventRow>,
    banned_ips: Vec<BannedIpRow>,
    success: Option<String>,
}

async fn render_security_log(
    state: &AppState,
    current: &User,
    success: Option<String>,
) -> Html<String> {
    let event_rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    )> = sqlx::query_as(
        "SELECT event_type, username, ip_address, detail, created_at \
             FROM security_events ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let events = event_rows
        .into_iter()
        .map(
            |(event_type, username, ip_address, detail, created_at)| SecurityEventRow {
                event_type,
                username,
                ip_address,
                detail,
                created_at,
            },
        )
        .collect();

    let banned_rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT ip_address, banned_until, reason FROM banned_ips \
         WHERE banned_until > datetime('now') ORDER BY banned_until DESC",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let banned_ips = banned_rows
        .into_iter()
        .map(|(ip_address, banned_until, reason)| BannedIpRow {
            ip_address,
            banned_until,
            reason,
        })
        .collect();

    Html(
        AdminSecurityTemplate {
            title: "Security log".to_string(),
            username: current.username.clone(),
            events,
            banned_ips,
            success,
        }
        .render()
        .unwrap(),
    )
}

pub async fn security_log(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    render_security_log(&state, &current, None).await
}

pub async fn unban_ip(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(ip): Path<String>,
) -> impl IntoResponse {
    sqlx::query("DELETE FROM banned_ips WHERE ip_address = ?")
        .bind(&ip)
        .execute(&state.db)
        .await
        .ok();

    render_security_log(&state, &current, Some(format!("{ip} was unbanned."))).await
}

#[derive(Template)]
#[template(path = "admin_new_user.html")]
struct AdminNewUserTemplate {
    title: String,
    username: String,
    error: Option<String>,
}

pub async fn new_user_form(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    Html(
        AdminNewUserTemplate {
            title: "New user".to_string(),
            username: current.username,
            error: None,
        }
        .render()
        .unwrap(),
    )
}

#[derive(Deserialize)]
pub struct NewUserForm {
    username: String,
    temp_password: String,
    is_admin: Option<String>,
    first_name: String,
    last_name: String,
}

pub async fn create_user(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Form(form): Form<NewUserForm>,
) -> impl IntoResponse {
    let render_error = |msg: &str| -> axum::response::Response {
        Html(
            AdminNewUserTemplate {
                title: "New user".to_string(),
                username: current.username.clone(),
                error: Some(msg.to_string()),
            }
            .render()
            .unwrap(),
        )
        .into_response()
    };

    let username = form.username.trim();
    if username.is_empty() {
        return render_error("Username can't be empty.");
    }
    if form.temp_password.len() < security::MIN_PASSWORD_LEN {
        return render_error("Temporary password must be at least 15 characters.");
    }

    let hash = security::hash_password(&form.temp_password);
    let is_admin = form.is_admin.is_some();
    let first_name = form.first_name.trim();
    let last_name = form.last_name.trim();
    let first_name = (!first_name.is_empty()).then_some(first_name);
    let last_name = (!last_name.is_empty()).then_some(last_name);

    let result: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin, must_change_password, first_name, last_name) \
         VALUES (?, ?, ?, 1, ?, ?) RETURNING id",
    )
    .bind(username)
    .bind(&hash)
    .bind(is_admin)
    .bind(first_name)
    .bind(last_name)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(user_id) => {
            let display = security::display_name(username, first_name, last_name);
            security::create_player_for_user(&state.db, user_id, &display).await;
            Redirect::to("/admin/users").into_response()
        }
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            render_error("That username is already taken.")
        }
        Err(_) => render_error("Something went wrong creating the user."),
    }
}
