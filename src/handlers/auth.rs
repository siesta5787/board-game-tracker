use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect};
use axum::{Extension, Form};
use serde::Deserialize;
use tower_sessions::Session;

use crate::AppState;
use crate::models::User;
use crate::security::{self, CurrentUser};

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    title: String,
    error: Option<String>,
}

pub async fn login_form() -> impl IntoResponse {
    Html(
        LoginTemplate {
            title: "Sign in".to_string(),
            error: None,
        }
        .render()
        .unwrap(),
    )
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

pub async fn login(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    let user =
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ? AND is_active = 1")
            .bind(&form.username)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    let valid = match &user {
        Some(u) => security::verify_password(&form.password, &u.password_hash),
        None => false,
    };

    if !valid {
        return Html(
            LoginTemplate {
                title: "Sign in".to_string(),
                error: Some("Incorrect username or password.".to_string()),
            }
            .render()
            .unwrap(),
        )
        .into_response();
    }

    session.insert("user_id", user.unwrap().id).await.ok();
    Redirect::to("/").into_response()
}

pub async fn logout(session: Session) -> impl IntoResponse {
    session.flush().await.ok();
    Redirect::to("/login")
}

#[derive(Template)]
#[template(path = "change_password.html")]
struct ChangePasswordTemplate {
    title: String,
    error: Option<String>,
}

pub async fn change_password_form(
    Extension(CurrentUser(user)): Extension<CurrentUser>,
) -> impl IntoResponse {
    if !user.must_change_password {
        return Redirect::to("/").into_response();
    }
    Html(
        ChangePasswordTemplate {
            title: "Change password".to_string(),
            error: None,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct ChangePasswordForm {
    current_password: String,
    new_password: String,
    confirm_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Form(form): Form<ChangePasswordForm>,
) -> impl IntoResponse {
    let render_error = |msg: &str| -> axum::response::Response {
        Html(
            ChangePasswordTemplate {
                title: "Change password".to_string(),
                error: Some(msg.to_string()),
            }
            .render()
            .unwrap(),
        )
        .into_response()
    };

    if !security::verify_password(&form.current_password, &user.password_hash) {
        return render_error("Current password is incorrect.");
    }
    if form.new_password.len() < security::MIN_PASSWORD_LEN {
        return render_error("New password must be at least 15 characters.");
    }
    if form.new_password != form.confirm_password {
        return render_error("New passwords don't match.");
    }

    let new_hash = security::hash_password(&form.new_password);
    sqlx::query("UPDATE users SET password_hash = ?, must_change_password = 0 WHERE id = ?")
        .bind(&new_hash)
        .bind(user.id)
        .execute(&state.db)
        .await
        .ok();

    Redirect::to("/").into_response()
}

#[derive(Template)]
#[template(path = "setup_2fa.html")]
struct Setup2faTemplate {
    title: String,
    qr_base64: String,
    secret_base32: String,
    error: Option<String>,
}

pub async fn setup_2fa_form(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
) -> impl IntoResponse {
    if user.totp_enabled {
        return Redirect::to("/").into_response();
    }

    let secret = match user.totp_secret.clone() {
        Some(s) => s,
        None => {
            let s = security::generate_totp_secret_base32();
            sqlx::query("UPDATE users SET totp_secret = ? WHERE id = ?")
                .bind(&s)
                .bind(user.id)
                .execute(&state.db)
                .await
                .ok();
            s
        }
    };

    let totp = security::totp_for_secret(&secret, &user.username);
    let qr_base64 = totp.get_qr_base64().expect("QR generation should not fail");

    Html(
        Setup2faTemplate {
            title: "Set up two-factor login".to_string(),
            qr_base64,
            secret_base32: secret,
            error: None,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct Setup2faForm {
    code: String,
}

pub async fn setup_2fa_verify(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Form(form): Form<Setup2faForm>,
) -> impl IntoResponse {
    let Some(secret) = user.totp_secret.clone() else {
        return Redirect::to("/auth/setup-2fa").into_response();
    };
    let totp = security::totp_for_secret(&secret, &user.username);

    let valid = totp.check_current(&form.code).unwrap_or(false);
    if !valid {
        let qr_base64 = totp.get_qr_base64().expect("QR generation should not fail");
        return Html(
            Setup2faTemplate {
                title: "Set up two-factor login".to_string(),
                qr_base64,
                secret_base32: secret,
                error: Some("That code didn't match. Try again.".to_string()),
            }
            .render()
            .unwrap(),
        )
        .into_response();
    }

    sqlx::query("UPDATE users SET totp_enabled = 1 WHERE id = ?")
        .bind(user.id)
        .execute(&state.db)
        .await
        .ok();

    Redirect::to("/").into_response()
}
