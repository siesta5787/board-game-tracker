use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;

use crate::AppState;
use crate::plays::VISIBLE_TO;
use crate::security::CurrentUser;

fn content_type_for(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        // Covers .jpg/.jpeg, and is a reasonable default for anything else —
        // all photos are re-encoded as JPEG on upload (see handlers::plays).
        "image/jpeg"
    }
}

/// Serves a play photo, but only if the requesting user can see that play —
/// this replaces relying on the public `/static` folder, so a private or
/// linked-only play's photos aren't just guessable public URLs.
pub async fn serve_photo(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path((play_id, filename)): Path<(i64, String)>,
) -> impl IntoResponse {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return (StatusCode::BAD_REQUEST, "Invalid filename").into_response();
    }

    let visible_sql = format!("SELECT 1 FROM plays WHERE plays.id = ? AND {VISIBLE_TO}");
    let visible: Option<i64> = sqlx::query_scalar(&visible_sql)
        .bind(play_id)
        .bind(current.id)
        .bind(current.id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    if visible.is_none() {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let expected_path = format!("data/photos/{play_id}/{filename}");
    let registered: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM play_photos WHERE play_id = ? AND file_path = ?")
            .bind(play_id)
            .bind(&expected_path)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    if registered.is_none() {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    match std::fs::read(&expected_path) {
        Ok(bytes) => {
            let headers = [(header::CONTENT_TYPE, content_type_for(&filename))];
            (headers, bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}
