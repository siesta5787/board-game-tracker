use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::Extension;
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use totp_rs::{Algorithm, Secret, TOTP};
use tower_sessions::Session;

use crate::AppState;
use crate::models::User;

pub const MIN_PASSWORD_LEN: usize = 15;

/// Failed *password or TOTP* attempts against one account before it locks.
pub const MAX_FAILED_ATTEMPTS: i64 = 5;
/// How long a locked account stays locked before auto-unlocking.
pub const LOCKOUT_MINUTES: i64 = 15;
/// Failed attempts from one IP (across any account) within the window below
/// before that IP is banned from reaching the login page at all.
pub const MAX_FAILED_PER_IP: i64 = 15;
pub const IP_FAILURE_WINDOW_MINUTES: i64 = 15;
/// How long an IP ban lasts before auto-expiring.
pub const IP_BAN_HOURS: i64 = 1;

/// Best-effort client IP: prefers X-Forwarded-For (set by a reverse proxy —
/// relevant if Tailscale Funnel ever terminates in front of us that way),
/// falling back to the TCP peer address.
pub fn client_ip(headers: &HeaderMap, addr: SocketAddr) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| addr.ip().to_string())
}

pub async fn record_security_event(
    db: &SqlitePool,
    event_type: &str,
    username: Option<&str>,
    ip: Option<&str>,
    detail: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO security_events (event_type, username, ip_address, detail) VALUES (?, ?, ?, ?)",
    )
    .bind(event_type)
    .bind(username)
    .bind(ip)
    .bind(detail)
    .execute(db)
    .await
    .ok();
}

pub async fn is_ip_banned(db: &SqlitePool, ip: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM banned_ips WHERE ip_address = ? AND banned_until > datetime('now'))",
    )
    .bind(ip)
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

/// Counts recent failed attempts from this IP and bans it if over threshold.
/// Call after recording a failed-login-type security event.
pub async fn check_and_ban_ip_if_needed(db: &SqlitePool, ip: &str) {
    let recent_failures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM security_events \
         WHERE ip_address = ? AND event_type IN ('login_failed', 'totp_failed') \
         AND created_at > datetime('now', ?)",
    )
    .bind(ip)
    .bind(format!("-{IP_FAILURE_WINDOW_MINUTES} minutes"))
    .fetch_one(db)
    .await
    .unwrap_or(0);

    if recent_failures >= MAX_FAILED_PER_IP {
        sqlx::query(
            "INSERT INTO banned_ips (ip_address, banned_until, reason) VALUES (?, datetime('now', ?), ?) \
             ON CONFLICT(ip_address) DO UPDATE SET banned_until = excluded.banned_until, reason = excluded.reason",
        )
        .bind(ip)
        .bind(format!("+{IP_BAN_HOURS} hours"))
        .bind(format!(
            "{recent_failures} failed login attempts within {IP_FAILURE_WINDOW_MINUTES} minutes"
        ))
        .execute(db)
        .await
        .ok();

        record_security_event(
            db,
            "ip_banned",
            None,
            Some(ip),
            Some(&format!("{recent_failures} failed attempts")),
        )
        .await;
    }
}

/// True if this account is currently locked out (and the lock hasn't expired).
pub async fn is_account_locked(db: &SqlitePool, user_id: i64) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT locked_until IS NOT NULL AND locked_until > datetime('now') FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// Records one failed password/TOTP attempt against a known account, locking
/// it once MAX_FAILED_ATTEMPTS is reached. Deliberately does nothing if the
/// account is already locked, so repeated hammering can't extend the lockout
/// and lock a legitimate user out indefinitely.
pub async fn record_failed_login(db: &SqlitePool, user_id: i64) {
    if is_account_locked(db, user_id).await {
        return;
    }

    sqlx::query("UPDATE users SET failed_login_attempts = failed_login_attempts + 1 WHERE id = ?")
        .bind(user_id)
        .execute(db)
        .await
        .ok();

    let attempts: i64 = sqlx::query_scalar("SELECT failed_login_attempts FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(db)
        .await
        .unwrap_or(0);

    if attempts >= MAX_FAILED_ATTEMPTS {
        sqlx::query("UPDATE users SET locked_until = datetime('now', ?) WHERE id = ?")
            .bind(format!("+{LOCKOUT_MINUTES} minutes"))
            .bind(user_id)
            .execute(db)
            .await
            .ok();
    }
}

pub async fn reset_failed_login(db: &SqlitePool, user_id: i64) {
    sqlx::query("UPDATE users SET failed_login_attempts = 0, locked_until = NULL WHERE id = ?")
        .bind(user_id)
        .execute(db)
        .await
        .ok();
}

pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("hashing a non-empty password should not fail")
        .to_string()
}

/// Generates a random temporary password, well above MIN_PASSWORD_LEN, for
/// admin-initiated resets (the admin doesn't get to pick this one).
pub fn generate_temp_password() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = OsRng;
    (0..20)
        .map(|_| CHARS[(rng.next_u32() as usize) % CHARS.len()] as char)
        .collect()
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

/// The name shown for a person everywhere except login/URLs: "First Last" if
/// either is set, otherwise their (unique, login-only) username. Usernames
/// exist so two friends who happen to share a real name can both have
/// accounts; this is what people actually see.
pub fn display_name(username: &str, first_name: Option<&str>, last_name: Option<&str>) -> String {
    let first = first_name.map(str::trim).filter(|s| !s.is_empty());
    let last = last_name.map(str::trim).filter(|s| !s.is_empty());
    match (first, last) {
        (Some(f), Some(l)) => format!("{f} {l}"),
        (Some(f), None) => f.to_string(),
        (None, Some(l)) => l.to_string(),
        (None, None) => username.to_string(),
    }
}

/// Every play/stats display (roster, leaderboard, head-to-head) reads a
/// person's name from `players.name`, not `users`, so a registered user's
/// display name has to be mirrored there any time it changes.
pub async fn sync_player_name(db: &sqlx::SqlitePool, user_id: i64, name: &str) {
    sqlx::query("UPDATE players SET name = ? WHERE user_id = ?")
        .bind(name)
        .bind(user_id)
        .execute(db)
        .await
        .ok();
}
