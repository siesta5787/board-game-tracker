use askama::Template;
use axum::Extension;
use axum::Form;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect};
use serde::Deserialize;

use crate::AppState;
use crate::handlers::plays::PlayFeedRow;
use crate::plays::{DISPLAY_NAME_SQL, INVOLVES_USER, VISIBLE_TO};
use crate::security::{self, CurrentUser};

pub(crate) fn profile_photo_path(user_id: i64) -> String {
    format!("data/profile_photos/{user_id}.jpg")
}

#[derive(Template)]
#[template(path = "user_profile.html")]
struct UserProfileTemplate {
    title: String,
    username: String,
    profile_username: String,
    profile_display_name: String,
    is_own_profile: bool,
    has_photo: bool,
    profile_initial: String,
    first_name: String,
    last_name: String,
    plays: Vec<PlayFeedRow>,
}

pub async fn view_profile(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(profile_username): Path<String>,
) -> impl IntoResponse {
    let subject: Option<(i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, first_name, last_name FROM users WHERE username = ? AND is_active = 1",
    )
    .bind(&profile_username)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((subject_id, first_name, last_name)) = subject else {
        return (axum::http::StatusCode::NOT_FOUND, "User not found").into_response();
    };

    let sql = format!(
        "SELECT plays.id, games.id AS game_id, games.name AS game_name, games.thumbnail_url, plays.play_date, \
                locations.name AS location_name, plays.visibility, users.username AS logged_by_username, \
                {DISPLAY_NAME_SQL} AS logged_by_display_name \
         FROM plays \
         JOIN games ON games.id = plays.game_id \
         LEFT JOIN locations ON locations.id = plays.location_id \
         JOIN users ON users.id = plays.logged_by_user_id \
         WHERE {VISIBLE_TO} AND {INVOLVES_USER} \
         ORDER BY plays.play_date DESC, plays.id DESC"
    );
    let plays = sqlx::query_as::<_, PlayFeedRow>(&sql)
        .bind(current.id)
        .bind(current.id)
        .bind(subject_id)
        .bind(subject_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let has_photo = std::path::Path::new(&profile_photo_path(subject_id)).exists();
    let profile_display_name = security::display_name(
        &profile_username,
        first_name.as_deref(),
        last_name.as_deref(),
    );
    let profile_initial = profile_display_name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());

    Html(
        UserProfileTemplate {
            title: format!("{profile_display_name}'s profile"),
            username: current.username.clone(),
            is_own_profile: current.username == profile_username,
            has_photo,
            profile_initial,
            first_name: first_name.unwrap_or_default(),
            last_name: last_name.unwrap_or_default(),
            profile_display_name,
            profile_username,
            plays,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct UpdateNameForm {
    first_name: String,
    last_name: String,
}

pub async fn update_name(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Form(form): Form<UpdateNameForm>,
) -> impl IntoResponse {
    let first = form.first_name.trim();
    let last = form.last_name.trim();
    let first_opt = (!first.is_empty()).then_some(first);
    let last_opt = (!last.is_empty()).then_some(last);

    sqlx::query("UPDATE users SET first_name = ?, last_name = ? WHERE id = ?")
        .bind(first_opt)
        .bind(last_opt)
        .bind(current.id)
        .execute(&state.db)
        .await
        .ok();

    let new_display_name = security::display_name(&current.username, first_opt, last_opt);
    security::sync_player_name(&state.db, current.id, &new_display_name).await;

    Redirect::to(&format!("/users/{}", current.username)).into_response()
}

/// Serves a user's profile photo. No visibility restriction beyond being
/// logged in at all — profile pages themselves have no privacy setting, so
/// the photo shown on one is exactly as public within the app as the page.
pub async fn serve_profile_photo(
    State(state): State<AppState>,
    Path(username): Path<String>,
) -> impl IntoResponse {
    let user_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE username = ? AND is_active = 1")
            .bind(&username)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some(user_id) = user_id else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };

    match std::fs::read(profile_photo_path(user_id)) {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/jpeg")], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

pub async fn upload_profile_photo(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if let Ok(Some(field)) = multipart.next_field().await {
        if let Ok(bytes) = field.bytes().await {
            if !bytes.is_empty() {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let img = if img.width() > 800 || img.height() > 800 {
                        img.resize(800, 800, image::imageops::FilterType::Lanczos3)
                    } else {
                        img
                    };
                    if std::fs::create_dir_all("data/profile_photos").is_ok() {
                        let _ = img.to_rgb8().save_with_format(
                            profile_photo_path(current.id),
                            image::ImageFormat::Jpeg,
                        );
                    }
                }
            }
        }
    }

    Redirect::to(&format!("/users/{}", current.username)).into_response()
}

pub async fn delete_profile_photo(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    let _ = std::fs::remove_file(profile_photo_path(current.id));
    Redirect::to(&format!("/users/{}", current.username)).into_response()
}
