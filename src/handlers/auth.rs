use askama::Template;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect};
use axum::{Extension, Form};
use serde::Deserialize;
use std::net::SocketAddr;
use tower_sessions::Session;

use crate::AppState;
use crate::models::User;
use crate::security::{self, CurrentUser};

const BANNED_MESSAGE: &str =
    "Too many failed attempts from your network. Try again in about an hour.";
const LOCKED_MESSAGE: &str = "This account is temporarily locked after too many failed attempts. Try again in about 15 minutes.";

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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    let ip = security::client_ip(&headers, addr);

    if security::is_ip_banned(&state.db, &ip).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Html(
                LoginTemplate {
                    title: "Sign in".to_string(),
                    error: Some(BANNED_MESSAGE.to_string()),
                }
                .render()
                .unwrap(),
            ),
        )
            .into_response();
    }

    let user =
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ? AND is_active = 1")
            .bind(&form.username)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    if let Some(u) = &user {
        if security::is_account_locked(&state.db, u.id).await {
            security::record_security_event(
                &state.db,
                "login_failed",
                Some(&form.username),
                Some(&ip),
                Some("account locked"),
            )
            .await;
            security::check_and_ban_ip_if_needed(&state.db, &ip).await;
            return Html(
                LoginTemplate {
                    title: "Sign in".to_string(),
                    error: Some(LOCKED_MESSAGE.to_string()),
                }
                .render()
                .unwrap(),
            )
            .into_response();
        }
    }

    let valid = match &user {
        Some(u) => security::verify_password(&form.password, &u.password_hash),
        None => false,
    };

    if !valid {
        if let Some(u) = &user {
            security::record_failed_login(&state.db, u.id).await;
        }
        security::record_security_event(
            &state.db,
            "login_failed",
            Some(&form.username),
            Some(&ip),
            None,
        )
        .await;
        security::check_and_ban_ip_if_needed(&state.db, &ip).await;

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

    let user = user.unwrap();
    security::reset_failed_login(&state.db, user.id).await;

    if user.totp_enabled {
        // Password is correct, but 2FA is enrolled - hold the login in a
        // "pending" state (deliberately NOT "user_id") until they also pass
        // a fresh TOTP code, so every login requires both factors. The
        // "login_success" event is logged once verify_2fa also passes.
        session.insert("pending_2fa_user_id", user.id).await.ok();
        Redirect::to("/auth/verify-2fa").into_response()
    } else {
        // Not yet enrolled in 2FA - password alone is the full login so this
        // IS the success event; require_full_auth will route them through
        // mandatory setup next.
        security::record_security_event(
            &state.db,
            "login_success",
            Some(&user.username),
            Some(&ip),
            None,
        )
        .await;
        session.insert("user_id", user.id).await.ok();
        Redirect::to("/").into_response()
    }
}

#[derive(Template)]
#[template(path = "verify_2fa.html")]
struct Verify2faTemplate {
    title: String,
    error: Option<String>,
}

pub async fn verify_2fa_form(session: Session) -> impl IntoResponse {
    let pending: Option<i64> = session.get("pending_2fa_user_id").await.ok().flatten();
    if pending.is_none() {
        return Redirect::to("/login").into_response();
    }
    Html(
        Verify2faTemplate {
            title: "Verify your code".to_string(),
            error: None,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct Verify2faForm {
    code: String,
}

pub async fn verify_2fa(
    State(state): State<AppState>,
    session: Session,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<Verify2faForm>,
) -> impl IntoResponse {
    let ip = security::client_ip(&headers, addr);

    if security::is_ip_banned(&state.db, &ip).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Html(
                Verify2faTemplate {
                    title: "Verify your code".to_string(),
                    error: Some(BANNED_MESSAGE.to_string()),
                }
                .render()
                .unwrap(),
            ),
        )
            .into_response();
    }

    let Some(user_id): Option<i64> = session.get("pending_2fa_user_id").await.ok().flatten() else {
        return Redirect::to("/login").into_response();
    };

    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ? AND is_active = 1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let Some(user) = user else {
        session.flush().await.ok();
        return Redirect::to("/login").into_response();
    };

    if security::is_account_locked(&state.db, user.id).await {
        session.flush().await.ok();
        return Html(
            LoginTemplate {
                title: "Sign in".to_string(),
                error: Some(LOCKED_MESSAGE.to_string()),
            }
            .render()
            .unwrap(),
        )
        .into_response();
    }

    let Some(secret) = user.totp_secret.clone() else {
        session.flush().await.ok();
        return Redirect::to("/login").into_response();
    };

    let totp = security::totp_for_secret(&secret, &user.username);
    // Copying a code from a phone's authenticator app commonly brings along
    // a trailing space or newline, which would otherwise fail verification
    // even though the digits are correct.
    let valid = totp.check_current(form.code.trim()).unwrap_or(false);
    if !valid {
        security::record_failed_login(&state.db, user.id).await;
        security::record_security_event(
            &state.db,
            "totp_failed",
            Some(&user.username),
            Some(&ip),
            None,
        )
        .await;
        security::check_and_ban_ip_if_needed(&state.db, &ip).await;

        return Html(
            Verify2faTemplate {
                title: "Verify your code".to_string(),
                error: Some("That code didn't match. Try again.".to_string()),
            }
            .render()
            .unwrap(),
        )
        .into_response();
    }

    security::reset_failed_login(&state.db, user.id).await;
    security::record_security_event(
        &state.db,
        "login_success",
        Some(&user.username),
        Some(&ip),
        None,
    )
    .await;

    session.remove::<i64>("pending_2fa_user_id").await.ok();
    session.insert("user_id", user.id).await.ok();
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

    let valid = totp.check_current(form.code.trim()).unwrap_or(false);
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
