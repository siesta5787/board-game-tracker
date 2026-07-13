use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use axum::{Extension, Form};
use serde::Deserialize;
use std::collections::HashMap;

use crate::AppState;
use crate::plays::{DISPLAY_NAME_SQL, VISIBLE_OR_TAGGED, VISIBLE_TO};
use crate::security::CurrentUser;

const VISIBILITIES: [&str; 3] = ["public", "linked_only", "private"];
const GUEST_SLOTS: [i32; 4] = [1, 2, 3, 4];

async fn find_or_create_location(state: &AppState, name: &str) -> Option<i64> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM locations WHERE name = ?")
        .bind(name)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
    {
        return Some(id);
    }
    sqlx::query_scalar("INSERT INTO locations (name) VALUES (?) RETURNING id")
        .bind(name)
        .fetch_one(&state.db)
        .await
        .ok()
}

#[derive(sqlx::FromRow)]
pub(crate) struct PlayFeedRow {
    pub(crate) id: i64,
    pub(crate) game_id: i64,
    pub(crate) game_name: String,
    pub(crate) thumbnail_url: Option<String>,
    pub(crate) play_date: String,
    pub(crate) location_name: Option<String>,
    pub(crate) visibility: String,
    pub(crate) logged_by_username: String,
    pub(crate) logged_by_display_name: String,
}

#[derive(Template)]
#[template(path = "plays.html")]
struct PlaysTemplate {
    title: String,
    username: String,
    plays: Vec<PlayFeedRow>,
}

pub async fn list_plays(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    let sql = format!(
        "SELECT plays.id, games.id AS game_id, games.name AS game_name, games.thumbnail_url, plays.play_date, \
                locations.name AS location_name, plays.visibility, users.username AS logged_by_username, \
                {DISPLAY_NAME_SQL} AS logged_by_display_name \
         FROM plays \
         JOIN games ON games.id = plays.game_id \
         LEFT JOIN locations ON locations.id = plays.location_id \
         JOIN users ON users.id = plays.logged_by_user_id \
         WHERE {VISIBLE_TO} \
         ORDER BY plays.play_date DESC, plays.id DESC"
    );
    let plays = sqlx::query_as::<_, PlayFeedRow>(&sql)
        .bind(current.id)
        .bind(current.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    Html(
        PlaysTemplate {
            title: "Plays".to_string(),
            username: current.username,
            plays,
        }
        .render()
        .unwrap(),
    )
}

struct PlayerOption {
    id: i64,
    username: String,
    checked: bool,
    score: Option<f64>,
    is_winner: bool,
}

struct GuestSlot {
    slot: i32,
    name: String,
    score: Option<f64>,
    is_winner: bool,
}

#[derive(Template)]
#[template(path = "play_new.html")]
struct NewPlayTemplate {
    title: String,
    username: String,
    games: Vec<(i64, String)>,
    active_users: Vec<PlayerOption>,
    guest_slots: Vec<GuestSlot>,
    today: String,
    error: Option<String>,
    prefill_game_id: Option<i64>,
    prefill_location: String,
}

async fn games_list(state: &AppState) -> Vec<(i64, String)> {
    sqlx::query_as("SELECT id, name FROM games ORDER BY name")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
}

async fn active_users_list(state: &AppState) -> Vec<(i64, String)> {
    sqlx::query_as("SELECT id, username FROM users WHERE is_active = 1 ORDER BY username")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
}

#[derive(Default)]
struct RosterPrefill {
    users: Vec<(i64, Option<f64>, bool)>,
    guests: Vec<(String, Option<f64>, bool)>,
}

async fn roster_prefill(state: &AppState, play_id: i64) -> RosterPrefill {
    let rows: Vec<(Option<i64>, String, Option<f64>, bool)> = sqlx::query_as(
        "SELECT players.user_id, players.name, play_players.score, play_players.is_winner \
         FROM play_players \
         JOIN players ON players.id = play_players.player_id \
         WHERE play_players.play_id = ? \
         ORDER BY play_players.id",
    )
    .bind(play_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut prefill = RosterPrefill::default();
    for (user_id, name, score, is_winner) in rows {
        match user_id {
            Some(uid) => prefill.users.push((uid, score, is_winner)),
            None => prefill.guests.push((name, score, is_winner)),
        }
    }
    prefill
}

async fn active_users_options(state: &AppState, prefill: &RosterPrefill) -> Vec<PlayerOption> {
    active_users_list(state)
        .await
        .into_iter()
        .map(|(id, username)| {
            let matched = prefill.users.iter().find(|(uid, _, _)| *uid == id);
            PlayerOption {
                id,
                username,
                checked: matched.is_some(),
                score: matched.and_then(|(_, s, _)| *s),
                is_winner: matched.map(|(_, _, w)| *w).unwrap_or(false),
            }
        })
        .collect()
}

fn guest_slots_options(prefill: &RosterPrefill) -> Vec<GuestSlot> {
    GUEST_SLOTS
        .iter()
        .map(|&slot| {
            let idx = (slot - 1) as usize;
            match prefill.guests.get(idx) {
                Some((name, score, is_winner)) => GuestSlot {
                    slot,
                    name: name.clone(),
                    score: *score,
                    is_winner: *is_winner,
                },
                None => GuestSlot {
                    slot,
                    name: String::new(),
                    score: None,
                    is_winner: false,
                },
            }
        })
        .collect()
}

#[derive(Deserialize)]
pub struct NewPlayQuery {
    repeat_from: Option<i64>,
}

pub async fn new_play_form(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Query(params): Query<NewPlayQuery>,
) -> impl IntoResponse {
    let mut prefill_game_id = None;
    let mut prefill_location = String::new();
    let mut prefill = RosterPrefill::default();

    if let Some(play_id) = params.repeat_from {
        let sql = format!(
            "SELECT plays.game_id, locations.name AS location_name \
             FROM plays \
             LEFT JOIN locations ON locations.id = plays.location_id \
             WHERE plays.id = ? AND {VISIBLE_TO}"
        );
        if let Some((game_id, location_name)) = sqlx::query_as::<_, (i64, Option<String>)>(&sql)
            .bind(play_id)
            .bind(current.id)
            .bind(current.id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
        {
            prefill_game_id = Some(game_id);
            prefill_location = location_name.unwrap_or_default();
            prefill = roster_prefill(&state, play_id).await;
        }
    }

    render_new_play(
        &state,
        &current,
        None,
        prefill_game_id,
        prefill_location,
        prefill,
    )
    .await
}

async fn render_new_play(
    state: &AppState,
    current: &crate::models::User,
    error: Option<String>,
    prefill_game_id: Option<i64>,
    prefill_location: String,
    prefill: RosterPrefill,
) -> axum::response::Response {
    Html(
        NewPlayTemplate {
            title: "Log a play".to_string(),
            username: current.username.clone(),
            games: games_list(state).await,
            active_users: active_users_options(state, &prefill).await,
            guest_slots: guest_slots_options(&prefill),
            today: chrono::Local::now().format("%Y-%m-%d").to_string(),
            error,
            prefill_game_id,
            prefill_location,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

pub async fn create_play(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Form(fields): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(game_id) = fields.get("game_id").and_then(|s| s.parse::<i64>().ok()) else {
        return render_new_play(
            &state,
            &current,
            Some("Please choose a game.".to_string()),
            None,
            String::new(),
            RosterPrefill::default(),
        )
        .await;
    };
    let play_date = fields.get("play_date").cloned().unwrap_or_default();
    if play_date.trim().is_empty() {
        return render_new_play(
            &state,
            &current,
            Some("Please choose a date.".to_string()),
            None,
            String::new(),
            RosterPrefill::default(),
        )
        .await;
    }
    let visibility = fields
        .get("visibility")
        .cloned()
        .unwrap_or_else(|| "public".to_string());
    if !VISIBILITIES.contains(&visibility.as_str()) {
        return render_new_play(
            &state,
            &current,
            Some("Invalid visibility.".to_string()),
            None,
            String::new(),
            RosterPrefill::default(),
        )
        .await;
    }

    let location_name = fields
        .get("location")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let location_id = match &location_name {
        Some(name) => find_or_create_location(&state, name).await,
        None => None,
    };
    let duration_minutes: Option<i64> = fields
        .get("duration_minutes")
        .and_then(|s| s.trim().parse().ok());
    let notes = fields
        .get("notes")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let play_id: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO plays (game_id, location_id, play_date, duration_minutes, notes, visibility, logged_by_user_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(game_id)
    .bind(location_id)
    .bind(&play_date)
    .bind(duration_minutes)
    .bind(&notes)
    .bind(&visibility)
    .bind(current.id)
    .fetch_one(&state.db)
    .await;

    let play_id = match play_id {
        Ok(id) => id,
        Err(_) => {
            return render_new_play(
                &state,
                &current,
                Some("Something went wrong saving that play. Check the game is valid.".to_string()),
                None,
                String::new(),
                RosterPrefill::default(),
            )
            .await;
        }
    };

    for (user_id, _username) in active_users_list(&state).await {
        if !fields.contains_key(&format!("include_user_{user_id}")) {
            continue;
        }
        let score: Option<f64> = fields
            .get(&format!("score_user_{user_id}"))
            .and_then(|s| s.trim().parse().ok());
        let is_winner = fields.contains_key(&format!("winner_user_{user_id}"));
        let link_status = if user_id == current.id {
            "approved"
        } else {
            "pending"
        };

        let player_id: Option<i64> = sqlx::query_scalar("SELECT id FROM players WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
        let Some(player_id) = player_id else { continue };

        sqlx::query(
            "INSERT INTO play_players (play_id, player_id, score, is_winner, link_status) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(play_id)
        .bind(player_id)
        .bind(score)
        .bind(is_winner)
        .bind(link_status)
        .execute(&state.db)
        .await
        .ok();

        if link_status == "pending" {
            sqlx::query(
                "INSERT INTO notifications (user_id, type, play_id) VALUES (?, 'play_link_request', ?)",
            )
            .bind(user_id)
            .bind(play_id)
            .execute(&state.db)
            .await
            .ok();
        }
    }

    for i in GUEST_SLOTS {
        let Some(name) = fields
            .get(&format!("guest_name_{i}"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let score: Option<f64> = fields
            .get(&format!("guest_score_{i}"))
            .and_then(|s| s.trim().parse().ok());
        let is_winner = fields.contains_key(&format!("guest_winner_{i}"));

        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM players WHERE user_id IS NULL AND name = ?")
                .bind(&name)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        let player_id = match existing {
            Some(id) => id,
            None => {
                match sqlx::query_scalar::<_, i64>(
                    "INSERT INTO players (user_id, name) VALUES (NULL, ?) RETURNING id",
                )
                .bind(&name)
                .fetch_one(&state.db)
                .await
                {
                    Ok(id) => id,
                    Err(_) => continue,
                }
            }
        };

        sqlx::query(
            "INSERT INTO play_players (play_id, player_id, score, is_winner, link_status) \
             VALUES (?, ?, ?, ?, 'none')",
        )
        .bind(play_id)
        .bind(player_id)
        .bind(score)
        .bind(is_winner)
        .execute(&state.db)
        .await
        .ok();
    }

    Redirect::to(&format!("/plays/{play_id}")).into_response()
}

#[derive(sqlx::FromRow)]
struct PlayDetailRow {
    id: i64,
    game_id: i64,
    game_name: String,
    play_date: String,
    location_name: Option<String>,
    duration_minutes: Option<i64>,
    notes: Option<String>,
    visibility: String,
    logged_by_user_id: i64,
    logged_by_username: String,
    logged_by_display_name: String,
}

#[derive(sqlx::FromRow)]
struct PlayPlayerRow {
    player_user_id: Option<i64>,
    player_username: Option<String>,
    player_name: String,
    score: Option<f64>,
    is_winner: bool,
    link_status: String,
}

struct PhotoView {
    url: String,
}

#[derive(Template)]
#[template(path = "play_detail.html")]
struct PlayDetailTemplate {
    title: String,
    username: String,
    play: PlayDetailRow,
    players: Vec<PlayPlayerRow>,
    photos: Vec<PhotoView>,
    can_edit: bool,
}

pub async fn view_play(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(play_id): Path<i64>,
) -> impl IntoResponse {
    let sql = format!(
        "SELECT plays.id, games.id AS game_id, games.name AS game_name, plays.play_date, locations.name AS location_name, \
                plays.duration_minutes, plays.notes, plays.visibility, plays.logged_by_user_id, \
                users.username AS logged_by_username, {DISPLAY_NAME_SQL} AS logged_by_display_name \
         FROM plays \
         JOIN games ON games.id = plays.game_id \
         LEFT JOIN locations ON locations.id = plays.location_id \
         JOIN users ON users.id = plays.logged_by_user_id \
         WHERE plays.id = ? AND {VISIBLE_OR_TAGGED}"
    );
    let play = sqlx::query_as::<_, PlayDetailRow>(&sql)
        .bind(play_id)
        .bind(current.id)
        .bind(current.id)
        .bind(current.id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let Some(play) = play else {
        return (axum::http::StatusCode::NOT_FOUND, "Play not found").into_response();
    };

    let players = sqlx::query_as::<_, PlayPlayerRow>(
        "SELECT players.user_id AS player_user_id, users.username AS player_username, \
                players.name AS player_name, \
                play_players.score, play_players.is_winner, play_players.link_status \
         FROM play_players \
         JOIN players ON players.id = play_players.player_id \
         LEFT JOIN users ON users.id = players.user_id \
         WHERE play_players.play_id = ? \
         ORDER BY play_players.id",
    )
    .bind(play_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let can_edit = play.logged_by_user_id == current.id
        || players
            .iter()
            .any(|p| p.player_user_id == Some(current.id) && p.link_status == "approved");

    let photos = photo_views_for(&state, play_id).await;

    Html(
        PlayDetailTemplate {
            title: play.game_name.clone(),
            username: current.username,
            play,
            players,
            photos,
            can_edit,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

async fn photo_views_for(state: &AppState, play_id: i64) -> Vec<PhotoView> {
    let paths: Vec<String> = sqlx::query_scalar(
        "SELECT file_path FROM play_photos WHERE play_id = ? ORDER BY upload_order",
    )
    .bind(play_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    paths
        .into_iter()
        .map(|path| {
            let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
            PhotoView {
                url: format!("/photos/{play_id}/{filename}"),
            }
        })
        .collect()
}

const MAX_PHOTOS_PER_PLAY: i64 = 5;

/// Accepts photo uploads for a play. Every image is decoded and re-encoded
/// as JPEG (this both validates it's a real image, not just a renamed file,
/// and strips EXIF/GPS metadata as a side effect — deliberate, since phone
/// photos taken at someone's house shouldn't leak their address) and given
/// a random filename rather than trusting whatever the browser sent.
pub async fn upload_photos(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(play_id): Path<i64>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    let Some((_play, can_edit)) = load_editable_play(&state, current.id, play_id).await else {
        return (axum::http::StatusCode::NOT_FOUND, "Play not found").into_response();
    };
    if !can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "Only the person who logged this play, or an approved linked player, can add photos.",
        )
            .into_response();
    }

    let existing_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM play_photos WHERE play_id = ?")
            .bind(play_id)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let mut next_order = existing_count;

    while let Ok(Some(field)) = multipart.next_field().await {
        if next_order >= MAX_PHOTOS_PER_PLAY {
            break;
        }
        let Ok(bytes) = field.bytes().await else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }

        let Ok(img) = image::load_from_memory(&bytes) else {
            continue;
        };
        let img = if img.width() > 1600 || img.height() > 1600 {
            img.resize(1600, 1600, image::imageops::FilterType::Lanczos3)
        } else {
            img
        };

        let dir = format!("data/photos/{play_id}");
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dest_path = format!("{dir}/{nanos}_{next_order}.jpg");

        if img
            .to_rgb8()
            .save_with_format(&dest_path, image::ImageFormat::Jpeg)
            .is_err()
        {
            continue;
        }

        sqlx::query("INSERT INTO play_photos (play_id, file_path, upload_order) VALUES (?, ?, ?)")
            .bind(play_id)
            .bind(&dest_path)
            .bind(next_order as i32)
            .execute(&state.db)
            .await
            .ok();

        next_order += 1;
    }

    Redirect::to(&format!("/plays/{play_id}")).into_response()
}

#[derive(Template)]
#[template(path = "play_edit.html")]
struct EditPlayTemplate {
    title: String,
    username: String,
    play: PlayDetailRow,
    players: Vec<PlayPlayerRowWithId>,
    available_users: Vec<(i64, String)>,
    guest_slots: [i32; 4],
    games: Vec<(i64, String)>,
    error: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PlayPlayerRowWithId {
    id: i64,
    player_user_id: Option<i64>,
    player_name: String,
    score: Option<f64>,
    is_winner: bool,
}

/// Active users with no play_players row (of any link_status) on this play
/// yet — used both for "add a player" checkboxes and for the "link this
/// guest to an account" dropdowns, since either action results in a new
/// tag for that user and the same (play_id, player_id) uniqueness applies.
async fn available_users_for_play(state: &AppState, play_id: i64) -> Vec<(i64, String)> {
    sqlx::query_as(
        "SELECT users.id, users.username FROM users \
         WHERE users.is_active = 1 \
           AND users.id NOT IN ( \
               SELECT players.user_id FROM play_players \
               JOIN players ON players.id = play_players.player_id \
               WHERE play_players.play_id = ? AND players.user_id IS NOT NULL \
           ) \
         ORDER BY users.username",
    )
    .bind(play_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
}

async fn load_editable_play(
    state: &AppState,
    current_id: i64,
    play_id: i64,
) -> Option<(PlayDetailRow, bool)> {
    let sql = format!(
        "SELECT plays.id, games.id AS game_id, games.name AS game_name, plays.play_date, locations.name AS location_name, \
                plays.duration_minutes, plays.notes, plays.visibility, plays.logged_by_user_id, \
                users.username AS logged_by_username, {DISPLAY_NAME_SQL} AS logged_by_display_name \
         FROM plays \
         JOIN games ON games.id = plays.game_id \
         LEFT JOIN locations ON locations.id = plays.location_id \
         JOIN users ON users.id = plays.logged_by_user_id \
         WHERE plays.id = ? AND {VISIBLE_OR_TAGGED}"
    );
    let play = sqlx::query_as::<_, PlayDetailRow>(&sql)
        .bind(play_id)
        .bind(current_id)
        .bind(current_id)
        .bind(current_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;

    let can_edit = play.logged_by_user_id == current_id || {
        let approved: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM play_players pp JOIN players p ON p.id = pp.player_id \
             WHERE pp.play_id = ? AND p.user_id = ? AND pp.link_status = 'approved'",
        )
        .bind(play_id)
        .bind(current_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        approved.is_some()
    };

    Some((play, can_edit))
}

pub async fn edit_play_form(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(play_id): Path<i64>,
) -> impl IntoResponse {
    let Some((play, can_edit)) = load_editable_play(&state, current.id, play_id).await else {
        return (axum::http::StatusCode::NOT_FOUND, "Play not found").into_response();
    };
    if !can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "Only the person who logged this play, or an approved linked player, can edit it.",
        )
            .into_response();
    }

    let players = sqlx::query_as::<_, PlayPlayerRowWithId>(
        "SELECT play_players.id, players.user_id AS player_user_id, players.name AS player_name, \
                play_players.score, play_players.is_winner \
         FROM play_players \
         JOIN players ON players.id = play_players.player_id \
         WHERE play_players.play_id = ? \
         ORDER BY play_players.id",
    )
    .bind(play_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let available_users = available_users_for_play(&state, play_id).await;

    Html(
        EditPlayTemplate {
            title: format!("Edit {}", play.game_name),
            username: current.username,
            play,
            players,
            available_users,
            guest_slots: GUEST_SLOTS,
            games: games_list(&state).await,
            error: None,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

pub async fn update_play(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(play_id): Path<i64>,
    Form(fields): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let Some((_play, can_edit)) = load_editable_play(&state, current.id, play_id).await else {
        return (axum::http::StatusCode::NOT_FOUND, "Play not found").into_response();
    };
    if !can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "Only the person who logged this play, or an approved linked player, can edit it.",
        )
            .into_response();
    }

    let Some(game_id) = fields.get("game_id").and_then(|s| s.parse::<i64>().ok()) else {
        return (axum::http::StatusCode::BAD_REQUEST, "Invalid game").into_response();
    };
    let play_date = fields.get("play_date").cloned().unwrap_or_default();
    let visibility = fields
        .get("visibility")
        .cloned()
        .unwrap_or_else(|| "public".to_string());
    if play_date.trim().is_empty() || !VISIBILITIES.contains(&visibility.as_str()) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid date or visibility",
        )
            .into_response();
    }
    let location_name = fields
        .get("location")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let location_id = match &location_name {
        Some(name) => find_or_create_location(&state, name).await,
        None => None,
    };
    let duration_minutes: Option<i64> = fields
        .get("duration_minutes")
        .and_then(|s| s.trim().parse().ok());
    let notes = fields
        .get("notes")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    sqlx::query(
        "UPDATE plays SET game_id = ?, location_id = ?, play_date = ?, duration_minutes = ?, \
         notes = ?, visibility = ?, updated_at = datetime('now'), last_edited_by = ? WHERE id = ?",
    )
    .bind(game_id)
    .bind(location_id)
    .bind(&play_date)
    .bind(duration_minutes)
    .bind(&notes)
    .bind(&visibility)
    .bind(current.id)
    .bind(play_id)
    .execute(&state.db)
    .await
    .ok();

    let existing_rows = sqlx::query_as::<_, (i64, Option<i64>)>(
        "SELECT play_players.id, players.user_id FROM play_players \
         JOIN players ON players.id = play_players.player_id \
         WHERE play_players.play_id = ?",
    )
    .bind(play_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut linked_this_request: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut to_remove: Vec<i64> = Vec::new();

    for (pp_id, player_user_id) in &existing_rows {
        let pp_id = *pp_id;
        if fields.contains_key(&format!("remove_pp_{pp_id}")) {
            to_remove.push(pp_id);
            continue;
        }

        // Only guest rows can be re-linked to a registered account — an
        // empty selection means "keep as guest".
        if player_user_id.is_none() {
            if let Some(target_user_id) = fields
                .get(&format!("link_pp_{pp_id}"))
                .and_then(|s| s.parse::<i64>().ok())
            {
                let target_player_id: Option<i64> = sqlx::query_scalar(
                    "SELECT players.id FROM players \
                     JOIN users ON users.id = players.user_id \
                     WHERE players.user_id = ? AND users.is_active = 1",
                )
                .bind(target_user_id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();

                if let Some(target_player_id) = target_player_id {
                    let link_status = if target_user_id == current.id {
                        "approved"
                    } else {
                        "pending"
                    };
                    let updated = sqlx::query(
                        "UPDATE play_players SET player_id = ?, link_status = ? WHERE id = ?",
                    )
                    .bind(target_player_id)
                    .bind(link_status)
                    .bind(pp_id)
                    .execute(&state.db)
                    .await;

                    // A unique-constraint failure means that user already
                    // has a row on this play (e.g. picked in both a link
                    // dropdown and an add-player checkbox) — skip quietly.
                    if updated.is_ok() {
                        linked_this_request.insert(target_user_id);
                        if link_status == "pending" {
                            sqlx::query(
                                "INSERT INTO notifications (user_id, type, play_id) \
                                 VALUES (?, 'play_link_request', ?)",
                            )
                            .bind(target_user_id)
                            .bind(play_id)
                            .execute(&state.db)
                            .await
                            .ok();
                        }
                        continue;
                    }
                }
            }
        }

        let score: Option<f64> = fields
            .get(&format!("score_pp_{pp_id}"))
            .and_then(|s| s.trim().parse().ok());
        let is_winner = fields.contains_key(&format!("winner_pp_{pp_id}"));
        sqlx::query("UPDATE play_players SET score = ?, is_winner = ? WHERE id = ?")
            .bind(score)
            .bind(is_winner)
            .bind(pp_id)
            .execute(&state.db)
            .await
            .ok();
    }

    for pp_id in to_remove {
        sqlx::query("DELETE FROM play_players WHERE id = ? AND play_id = ?")
            .bind(pp_id)
            .bind(play_id)
            .execute(&state.db)
            .await
            .ok();
    }

    // Add newly-tagged registered users.
    for (user_id, _username) in available_users_for_play(&state, play_id).await {
        if linked_this_request.contains(&user_id) {
            continue;
        }
        if !fields.contains_key(&format!("include_user_{user_id}")) {
            continue;
        }
        let score: Option<f64> = fields
            .get(&format!("score_user_{user_id}"))
            .and_then(|s| s.trim().parse().ok());
        let is_winner = fields.contains_key(&format!("winner_user_{user_id}"));
        let link_status = if user_id == current.id {
            "approved"
        } else {
            "pending"
        };

        let player_id: Option<i64> = sqlx::query_scalar("SELECT id FROM players WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
        let Some(player_id) = player_id else { continue };

        let inserted = sqlx::query(
            "INSERT INTO play_players (play_id, player_id, score, is_winner, link_status) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(play_id)
        .bind(player_id)
        .bind(score)
        .bind(is_winner)
        .bind(link_status)
        .execute(&state.db)
        .await;

        if inserted.is_ok() && link_status == "pending" {
            sqlx::query(
                "INSERT INTO notifications (user_id, type, play_id) VALUES (?, 'play_link_request', ?)",
            )
            .bind(user_id)
            .bind(play_id)
            .execute(&state.db)
            .await
            .ok();
        }
    }

    // Add newly-added guests.
    for i in GUEST_SLOTS {
        let Some(name) = fields
            .get(&format!("guest_name_{i}"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let score: Option<f64> = fields
            .get(&format!("guest_score_{i}"))
            .and_then(|s| s.trim().parse().ok());
        let is_winner = fields.contains_key(&format!("guest_winner_{i}"));

        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM players WHERE user_id IS NULL AND name = ?")
                .bind(&name)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        let player_id = match existing {
            Some(id) => id,
            None => {
                match sqlx::query_scalar::<_, i64>(
                    "INSERT INTO players (user_id, name) VALUES (NULL, ?) RETURNING id",
                )
                .bind(&name)
                .fetch_one(&state.db)
                .await
                {
                    Ok(id) => id,
                    Err(_) => continue,
                }
            }
        };

        sqlx::query(
            "INSERT INTO play_players (play_id, player_id, score, is_winner, link_status) \
             VALUES (?, ?, ?, ?, 'none')",
        )
        .bind(play_id)
        .bind(player_id)
        .bind(score)
        .bind(is_winner)
        .execute(&state.db)
        .await
        .ok();
    }

    Redirect::to(&format!("/plays/{play_id}")).into_response()
}

pub async fn delete_play(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(play_id): Path<i64>,
) -> impl IntoResponse {
    let Some((_play, can_edit)) = load_editable_play(&state, current.id, play_id).await else {
        return (axum::http::StatusCode::NOT_FOUND, "Play not found").into_response();
    };
    if !can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "Only the person who logged this play, or an approved linked player, can delete it.",
        )
            .into_response();
    }

    // notifications.play_id has no ON DELETE CASCADE, unlike play_players/
    // play_photos/play_expansions, so it has to be cleared explicitly or the
    // delete below would fail on a foreign key violation.
    sqlx::query("DELETE FROM notifications WHERE play_id = ?")
        .bind(play_id)
        .execute(&state.db)
        .await
        .ok();
    sqlx::query("DELETE FROM plays WHERE id = ?")
        .bind(play_id)
        .execute(&state.db)
        .await
        .ok();

    let _ = std::fs::remove_dir_all(format!("data/photos/{play_id}"));

    Redirect::to("/plays").into_response()
}
