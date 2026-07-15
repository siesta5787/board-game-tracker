mod bgcatalog_import;
mod bgg;
mod data_export;
mod handlers;
mod models;
mod plays;
mod security;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous};
use std::str::FromStr;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_sessions::cookie::time::Duration as CookieDuration;
use tower_sessions::session_store::ExpiredDeletion;
use tower_sessions::{Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
}

/// Mirrors Cargo.toml's `version` field (prefixed with "v" to match the git
/// tag format, e.g. "v0.3.0") — bump Cargo.toml alongside each release tag
/// so this stays in sync. Cargo always sets CARGO_PKG_VERSION itself, so
/// this needs no custom build-time plumbing (unlike two earlier attempts —
/// an env-var passthrough into cross's Docker build container, then a
/// generated file — that both turned out not to reliably reach the
/// compiler inside that container).
pub const APP_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();
    tracing::info!("Board Game Tracker {APP_VERSION} starting");

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://data/boardgames.db".into());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());

    // create_if_missing only creates the database *file*, not its parent
    // directory, so make sure the folders we depend on exist first.
    std::fs::create_dir_all("data/photos").expect("failed to create data directory");
    std::fs::create_dir_all("static").expect("failed to create static directory");

    let connect_options = SqliteConnectOptions::from_str(&database_url)
        .expect("invalid DATABASE_URL")
        .create_if_missing(true)
        .foreign_keys(true)
        // WAL + a busy timeout let one writer and multiple readers coexist
        // without "database is locked" errors when a few people use the
        // app at once on the Pi.
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5));

    let db = SqlitePoolOptions::new()
        .connect_with(connect_options)
        .await
        .expect("failed to connect to database");

    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .expect("failed to run migrations");

    security::bootstrap_admin(&db).await;

    let session_store = SqliteStore::new(db.clone());
    session_store
        .migrate()
        .await
        .expect("failed to run session store migrations");

    // Without this, expired session rows accumulate in the database forever.
    tokio::task::spawn(
        session_store
            .clone()
            .continuously_delete_expired(tokio::time::Duration::from_secs(60 * 60)),
    );

    let insecure_cookies = std::env::var("INSECURE_COOKIES")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    if insecure_cookies {
        tracing::warn!(
            "INSECURE_COOKIES is set - session cookies will be sent over plain HTTP. \
             This is for local LAN testing only and must never be set in production."
        );
    }

    let session_layer = SessionManagerLayer::new(session_store)
        .with_expiry(Expiry::OnInactivity(CookieDuration::days(30)))
        .with_secure(!insecure_cookies);

    let state = AppState { db };

    tokio::task::spawn(handlers::backups::run_scheduled_backups(state.clone()));
    tokio::task::spawn(handlers::backups::run_live_mirror(state.clone()));
    tokio::task::spawn(handlers::system_update::run_scheduled_app_update_check(
        state.clone(),
    ));

    // Reachable without being logged in at all. /auth/verify-2fa belongs here
    // (not under require_session) because at that point the user only has a
    // "pending_2fa_user_id" - they aren't logged in yet by require_session's
    // definition, which checks for "user_id".
    let public_routes = Router::new()
        .route(
            "/login",
            get(handlers::auth::login_form).post(handlers::auth::login),
        )
        .route(
            "/auth/verify-2fa",
            get(handlers::auth::verify_2fa_form).post(handlers::auth::verify_2fa),
        );

    // Reachable with a valid session, even mid-onboarding (forced password
    // change / mandatory 2FA setup) — these routes ARE the onboarding gates,
    // so they can't themselves require onboarding to be complete.
    let onboarding_routes = Router::new()
        .route(
            "/auth/change-password",
            get(handlers::auth::change_password_form).post(handlers::auth::change_password),
        )
        .route(
            "/auth/setup-2fa",
            get(handlers::auth::setup_2fa_form).post(handlers::auth::setup_2fa_verify),
        )
        .route("/logout", post(handlers::auth::logout))
        .layer(from_fn_with_state(state.clone(), security::require_session));

    let admin_routes = Router::new()
        .route("/admin/users", get(handlers::admin::list_users))
        .route(
            "/admin/users/new",
            get(handlers::admin::new_user_form).post(handlers::admin::create_user),
        )
        .route("/admin/users/{id}/reset", post(handlers::admin::reset_user))
        .route(
            "/admin/users/{id}/deactivate",
            post(handlers::admin::deactivate_user),
        )
        .route(
            "/admin/users/{id}/reactivate",
            post(handlers::admin::reactivate_user),
        )
        .route(
            "/admin/users/{id}/unlock",
            post(handlers::admin::unlock_user),
        )
        .route("/admin/security", get(handlers::admin::security_log))
        .route(
            "/admin/security/unban/{ip}",
            post(handlers::admin::unban_ip),
        )
        .route(
            "/admin/games/merge",
            get(handlers::games::merge_games_form).post(handlers::games::merge_games),
        )
        .route("/admin/backups", get(handlers::backups::list_backups))
        .route(
            "/admin/backups/create",
            post(handlers::backups::create_backup),
        )
        .route(
            "/admin/backups/upload",
            post(handlers::backups::upload_backup).layer(DefaultBodyLimit::max(200 * 1024 * 1024)),
        )
        .route(
            "/admin/backups/{filename}/download",
            get(handlers::backups::download_backup),
        )
        .route(
            "/admin/backups/{filename}/delete",
            post(handlers::backups::delete_backup),
        )
        .route(
            "/admin/backups/{filename}/restore",
            post(handlers::backups::restore_backup),
        )
        .route(
            "/admin/backups/schedule",
            post(handlers::backups::save_backup_schedule),
        )
        .route(
            "/admin/backups/format-drive",
            post(handlers::backups::format_drive),
        )
        .route(
            "/admin/update",
            get(handlers::system_update::show_update_page),
        )
        .route(
            "/admin/update/trigger",
            post(handlers::system_update::trigger_update),
        )
        .route(
            "/admin/update/restart",
            post(handlers::system_update::trigger_restart),
        )
        .route(
            "/admin/update/schedule",
            post(handlers::system_update::save_app_update_schedule),
        )
        .route(
            "/admin/system",
            get(handlers::system_maintenance::show_system_page),
        )
        .route(
            "/admin/system/os/check",
            post(handlers::system_maintenance::trigger_os_check),
        )
        .route(
            "/admin/system/os/upgrade",
            post(handlers::system_maintenance::trigger_os_upgrade),
        )
        .route(
            "/admin/system/tailscale/update",
            post(handlers::system_maintenance::trigger_tailscale_update),
        )
        .route(
            "/admin/system/reboot",
            post(handlers::system_maintenance::trigger_reboot),
        )
        .route(
            "/admin/system/schedule",
            post(handlers::system_maintenance::save_schedule),
        )
        .layer(from_fn(security::require_admin));

    // Any fully-onboarded user can import their own BG Catalog history into
    // their own account — not admin-specific, since the import logic always
    // attributes plays to whoever is running it.
    let import_routes = Router::new().route(
        "/import/bgcatalog",
        get(handlers::admin_import::import_form)
            .post(handlers::admin_import::run_import)
            // Export zips (JSON + photos) easily exceed axum's default
            // body-size limit; raise it only for this route.
            .layer(DefaultBodyLimit::max(100 * 1024 * 1024)),
    );

    let export_routes = Router::new().route("/export", get(handlers::export::export_data));

    let collection_routes = Router::new()
        .route("/collection", get(handlers::collection::redirect_to_own))
        .route(
            "/collection/add",
            get(handlers::collection::add_search_form).post(handlers::collection::add_from_bgg),
        )
        .route(
            "/collection/add/manual",
            get(handlers::collection::manual_add_form).post(handlers::collection::create_manual),
        )
        .route(
            "/collection/{game_id}/status/{status}/add",
            post(handlers::collection::add_status),
        )
        .route(
            "/collection/{game_id}/status/{status}/remove",
            post(handlers::collection::remove_status),
        )
        .route(
            "/collection/{username}",
            get(handlers::collection::view_collection),
        );

    let play_routes = Router::new()
        .route(
            "/plays",
            get(handlers::plays::list_plays).post(handlers::plays::create_play),
        )
        .route("/plays/new", get(handlers::plays::new_play_form))
        .route("/plays/{play_id}", get(handlers::plays::view_play))
        .route(
            "/plays/{play_id}/edit",
            get(handlers::plays::edit_play_form).post(handlers::plays::update_play),
        )
        .route(
            "/plays/{play_id}/delete",
            post(handlers::plays::delete_play),
        )
        .route(
            "/plays/{play_id}/photos",
            post(handlers::plays::upload_photos).layer(DefaultBodyLimit::max(30 * 1024 * 1024)),
        )
        .route(
            "/notifications",
            get(handlers::notifications::list_notifications),
        )
        .route(
            "/notifications/{id}/approve",
            post(handlers::notifications::approve),
        )
        .route(
            "/notifications/{id}/decline",
            post(handlers::notifications::decline),
        )
        .route(
            "/notifications/unread-count",
            get(handlers::notifications::unread_count),
        )
        .route("/stats", get(handlers::stats::show_stats))
        .route("/stats/head-to-head", get(handlers::stats::head_to_head))
        .route(
            "/photos/{play_id}/{filename}",
            get(handlers::photos::serve_photo),
        );

    let profile_routes = Router::new()
        .route("/games", get(handlers::games::list_games))
        .route("/games/{game_id}", get(handlers::games::view_game))
        .route("/users/{username}", get(handlers::users::view_profile))
        .route(
            "/users/{username}/photo",
            get(handlers::users::serve_profile_photo),
        )
        .route(
            "/profile/photo",
            post(handlers::users::upload_profile_photo)
                .layer(DefaultBodyLimit::max(10 * 1024 * 1024)),
        )
        .route(
            "/profile/photo/delete",
            post(handlers::users::delete_profile_photo),
        )
        .route("/profile/name", post(handlers::users::update_name))
        .route("/settings", get(handlers::settings::show_settings));

    // Everything else requires a fully onboarded (password changed, 2FA
    // enabled) active user.
    let app_routes = Router::new()
        .route("/", get(handlers::dashboard::home))
        .merge(admin_routes)
        .merge(import_routes)
        .merge(export_routes)
        .merge(collection_routes)
        .merge(play_routes)
        .merge(profile_routes)
        .layer(from_fn_with_state(
            state.clone(),
            security::require_full_auth,
        ));

    let app = Router::new()
        .merge(public_routes)
        .merge(onboarding_routes)
        .merge(app_routes)
        .merge(
            Router::new()
                .nest_service("/static", ServeDir::new("static"))
                .layer(SetResponseHeaderLayer::overriding(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-cache"),
                )),
        )
        // Served at the root path (not /static/sw.js) so its default scope
        // covers the whole app, not just /static/.
        .route_service("/sw.js", ServeFile::new("static/sw.js"))
        .with_state(state)
        .layer(session_layer);

    tracing::info!("listening on {bind_addr}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .unwrap();
}
