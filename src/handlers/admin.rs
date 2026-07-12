use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect};
use axum::{Extension, Form};
use serde::Deserialize;

use crate::AppState;
use crate::models::User;
use crate::security::{self, CurrentUser};

pub struct UserRow {
    pub username: String,
    pub is_admin: bool,
    pub is_active: bool,
    pub totp_enabled: bool,
}

#[derive(Template)]
#[template(path = "admin_users.html")]
struct AdminUsersTemplate {
    title: String,
    username: String,
    is_admin: bool,
    users: Vec<UserRow>,
    success: Option<String>,
}

pub async fn list_users(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY username")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let users = rows
        .into_iter()
        .map(|u| UserRow {
            username: u.username,
            is_admin: u.is_admin,
            is_active: u.is_active,
            totp_enabled: u.totp_enabled,
        })
        .collect();

    Html(
        AdminUsersTemplate {
            title: "Users".to_string(),
            username: current.username,
            is_admin: current.is_admin,
            users,
            success: None,
        }
        .render()
        .unwrap(),
    )
}

#[derive(Template)]
#[template(path = "admin_new_user.html")]
struct AdminNewUserTemplate {
    title: String,
    username: String,
    is_admin: bool,
    error: Option<String>,
}

pub async fn new_user_form(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    Html(
        AdminNewUserTemplate {
            title: "New user".to_string(),
            username: current.username,
            is_admin: current.is_admin,
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
                is_admin: current.is_admin,
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

    let result: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin, must_change_password) \
         VALUES (?, ?, ?, 1) RETURNING id",
    )
    .bind(username)
    .bind(&hash)
    .bind(is_admin)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(user_id) => {
            security::create_player_for_user(&state.db, user_id, username).await;
            Redirect::to("/admin/users").into_response()
        }
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            render_error("That username is already taken.")
        }
        Err(_) => render_error("Something went wrong creating the user."),
    }
}
