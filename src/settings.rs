//! Generic key/value store for instance-wide settings an admin configures
//! per self-hosted install — deliberately in the database, not the
//! codebase, so a credential like a BGG API token never has to be
//! hardcoded (and shared with, or overwritten by, everyone else who forks
//! or self-hosts this app). See `migrations/0006_settings.sql`.

use sqlx::SqlitePool;

pub const BGG_API_TOKEN: &str = "bgg_api_token";

pub async fn get(db: &SqlitePool, key: &str) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

pub async fn set(db: &SqlitePool, key: &str, value: &str) {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(db)
    .await
    .ok();
}

pub async fn delete(db: &SqlitePool, key: &str) {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(key)
        .execute(db)
        .await
        .ok();
}
