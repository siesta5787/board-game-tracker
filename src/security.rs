use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::Extension;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use totp_rs::{Algorithm, Secret, TOTP};
use tower_sessions::Session;

use crate::AppState;
use crate::models::User;

pub const MIN_PASSWORD_LEN: usize = 15;

pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("hashing a non-empty password should not fail")
        .to_string()
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed_hash) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

/// Builds a TOTP object for a given user from a stored (or freshly generated)
/// base32 secret. All accounts share the same algorithm/digits/step, so the
/// secret alone is enough to reconstruct it.
pub fn totp_for_secret(secret_base32: &str, username: &str) -> TOTP {
    let secret = Secret::Encoded(secret_base32.to_string());
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret
            .to_bytes()
            .expect("stored secret should be valid base32"),
        Some("Board Game Tracker".to_string()),
        username.to_string(),
    )
    .expect("fixed TOTP parameters should always be valid")
}

pub fn generate_totp_secret_base32() -> String {
    match Secret::generate_secret().to_encoded() {
        Secret::Encoded(s) => s,
        Secret::Raw(_) => unreachable!("to_encoded() always returns the Encoded variant"),
    }
}

/// One shared struct so handlers can pull the logged-in user out of request
/// extensions with `Extension(CurrentUser(user)): Extension<CurrentUser>`.
#[derive(Clone)]
pub struct CurrentUser(pub User);

async fn load_active_user(state: &AppState, session: &Session) -> Option<User> {
    let user_id: i64 = session.get("user_id").await.ok().flatten()?;
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    if user.is_active { Some(user) } else { None }
}

/// Requires a valid session for a logged-in, active user, but does NOT
/// enforce the forced-password-change / 2FA-setup gates. Used for the
/// setup pages themselves (and logout), which must be reachable in the
/// middle of onboarding.
pub async fn require_session(
    State(state): State<AppState>,
    session: Session,
    mut request: Request,
    next: Next,
) -> Response {
    match load_active_user(&state, &session).await {
        Some(user) => {
            request.extensions_mut().insert(CurrentUser(user));
            next.run(request).await
        }
        None => {
            session.flush().await.ok();
            Redirect::to("/login").into_response()
        }
    }
}

/// Requires a logged-in, active user who has completed the forced password
/// change and mandatory 2FA setup. Use this on every route in the actual app.
pub async fn require_full_auth(
    State(state): State<AppState>,
    session: Session,
    mut request: Request,
    next: Next,
) -> Response {
    match load_active_user(&state, &session).await {
        Some(user) if user.must_change_password => {
            let _ = user;
            Redirect::to("/auth/change-password").into_response()
        }
        Some(user) if !user.totp_enabled => {
            let _ = user;
            Redirect::to("/auth/setup-2fa").into_response()
        }
        Some(user) => {
            request.extensions_mut().insert(CurrentUser(user));
            next.run(request).await
        }
        None => {
            session.flush().await.ok();
            Redirect::to("/login").into_response()
        }
    }
}

/// Must run after `require_full_auth` in the middleware stack so the
/// `CurrentUser` extension is already populated.
pub async fn require_admin(
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    request: Request,
    next: Next,
) -> Response {
    if user.is_admin {
        next.run(request).await
    } else {
        (axum::http::StatusCode::FORBIDDEN, "Admins only").into_response()
    }
}

/// Creates the first admin account from ADMIN_USERNAME / ADMIN_PASSWORD env
/// vars if the users table is empty. There's no self-registration or email
/// in this app, so this is the only way to get a first account onto a fresh
/// install.
pub async fn bootstrap_admin(db: &sqlx::SqlitePool) {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(db)
        .await
        .expect("failed to count users");
    if count > 0 {
        return;
    }

    let username = std::env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let password = std::env::var("ADMIN_PASSWORD")
        .expect("ADMIN_PASSWORD must be set in .env to bootstrap the first admin account");

    let hash = hash_password(&password);
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin, must_change_password) \
         VALUES (?, ?, 1, 1) RETURNING id",
    )
    .bind(&username)
    .bind(&hash)
    .fetch_one(db)
    .await
    .expect("failed to create bootstrap admin");

    create_player_for_user(db, user_id, &username).await;

    tracing::info!("bootstrapped initial admin account: {username}");
}

/// Every registered user needs a matching `players` row (with `user_id` set)
/// so they can be tagged as a participant in a play. Call this any time a
/// new user account is created.
pub async fn create_player_for_user(db: &sqlx::SqlitePool, user_id: i64, username: &str) {
    sqlx::query("INSERT INTO players (user_id, name) VALUES (?, ?)")
        .bind(user_id)
        .bind(username)
        .execute(db)
        .await
        .expect("failed to create player record for new user");
}
