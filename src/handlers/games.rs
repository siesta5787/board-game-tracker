use askama::Template;
use axum::Extension;
use axum::Form;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse};
use serde::Deserialize;

use crate::AppState;
use crate::plays::{DISPLAY_NAME_SQL, VISIBLE_TO};
use crate::security::CurrentUser;

#[derive(sqlx::FromRow)]
struct GameRow {
    name: String,
    year_published: Option<i32>,
    min_players: Option<i32>,
    max_players: Option<i32>,
    min_playtime: Option<i32>,
    max_playtime: Option<i32>,
    min_age: Option<i32>,
    designers: Option<String>,
    artists: Option<String>,
    thumbnail_url: Option<String>,
    image_url: Option<String>,
    average_rating: Option<f64>,
    weight: Option<f64>,
    is_expansion: bool,
    base_game_id: Option<i64>,
    base_game_name: Option<String>,
    notes: Option<String>,
}

#[derive(sqlx::FromRow)]
struct RecentPlayRow {
    id: i64,
    play_date: String,
    logged_by_username: String,
    logged_by_display_name: String,
}

#[derive(Template)]
#[template(path = "game_detail.html")]
struct GameDetailTemplate {
    title: String,
    username: String,
    game: GameRow,
    recent_plays: Vec<RecentPlayRow>,
    game_id: i64,
    is_owned: bool,
    is_wishlist: bool,
    is_preordered: bool,
    is_for_sale: bool,
    is_played: bool,
    is_want_to_play: bool,
    is_want_to_trade: bool,
}

#[derive(sqlx::FromRow)]
struct GameListRow {
    id: i64,
    name: String,
    year_published: Option<i32>,
    min_players: Option<i32>,
    max_players: Option<i32>,
    thumbnail_url: Option<String>,
    is_expansion: bool,
    in_my_collection: bool,
}

#[derive(Template)]
#[template(path = "games_list.html")]
struct GamesListTemplate {
    title: String,
    username: String,
    is_admin: bool,
    games: Vec<GameListRow>,
}

pub async fn list_games(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    let games = sqlx::query_as::<_, GameListRow>(
        "SELECT g.id, g.name, g.year_published, g.min_players, g.max_players, \
                g.thumbnail_url, g.is_expansion, \
                EXISTS(SELECT 1 FROM game_status gs WHERE gs.game_id = g.id AND gs.user_id = ?) AS in_my_collection \
         FROM games g \
         ORDER BY g.name",
    )
    .bind(current.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    Html(
        GamesListTemplate {
            title: "Games".to_string(),
            username: current.username,
            is_admin: current.is_admin,
            games,
        }
        .render()
        .unwrap(),
    )
}

pub async fn view_game(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(game_id): Path<i64>,
) -> impl IntoResponse {
    let game = sqlx::query_as::<_, GameRow>(
        "SELECT g.name, g.year_published, g.min_players, g.max_players, \
                g.min_playtime, g.max_playtime, g.min_age, g.designers, g.artists, \
                g.thumbnail_url, g.image_url, g.average_rating, g.weight, g.is_expansion, \
                g.base_game_id, bg.name AS base_game_name, g.notes \
         FROM games g \
         LEFT JOIN games bg ON bg.id = g.base_game_id \
         WHERE g.id = ?",
    )
    .bind(game_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some(game) = game else {
        return (axum::http::StatusCode::NOT_FOUND, "Game not found").into_response();
    };

    let sql = format!(
        "SELECT plays.id, plays.play_date, users.username AS logged_by_username, \
                {DISPLAY_NAME_SQL} AS logged_by_display_name \
         FROM plays \
         JOIN users ON users.id = plays.logged_by_user_id \
         WHERE plays.game_id = ? AND {VISIBLE_TO} \
         ORDER BY plays.play_date DESC, plays.id DESC \
         LIMIT 20"
    );
    let recent_plays = sqlx::query_as::<_, RecentPlayRow>(&sql)
        .bind(game_id)
        .bind(current.id)
        .bind(current.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let my_statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM game_status WHERE game_id = ? AND user_id = ?")
            .bind(game_id)
            .bind(current.id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    Html(
        GameDetailTemplate {
            title: game.name.clone(),
            username: current.username,
            game,
            recent_plays,
            game_id,
            is_owned: my_statuses.iter().any(|s| s == "owned"),
            is_wishlist: my_statuses.iter().any(|s| s == "wishlist"),
            is_preordered: my_statuses.iter().any(|s| s == "preordered"),
            is_for_sale: my_statuses.iter().any(|s| s == "for_sale"),
            is_played: my_statuses.iter().any(|s| s == "played"),
            is_want_to_play: my_statuses.iter().any(|s| s == "want_to_play"),
            is_want_to_trade: my_statuses.iter().any(|s| s == "want_to_trade"),
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

#[derive(sqlx::FromRow, Clone)]
struct GameOption {
    id: i64,
    name: String,
}

#[derive(Template)]
#[template(path = "admin_merge_games.html")]
struct MergeGamesTemplate {
    title: String,
    username: String,
    games: Vec<GameOption>,
    success: Option<String>,
    error: Option<String>,
}

async fn render_merge_form(
    state: &AppState,
    current: &crate::models::User,
    success: Option<String>,
    error: Option<String>,
) -> Html<String> {
    let games: Vec<GameOption> = sqlx::query_as("SELECT id, name FROM games ORDER BY name")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    Html(
        MergeGamesTemplate {
            title: "Merge games".to_string(),
            username: current.username.clone(),
            games,
            success,
            error,
        }
        .render()
        .unwrap(),
    )
}

pub async fn merge_games_form(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    render_merge_form(&state, &current, None, None).await
}

#[derive(Deserialize)]
pub struct MergeGamesForm {
    canonical_id: i64,
    duplicate_id: i64,
}

pub async fn merge_games(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Form(form): Form<MergeGamesForm>,
) -> impl IntoResponse {
    if form.canonical_id == form.duplicate_id {
        return render_merge_form(
            &state,
            &current,
            None,
            Some("Choose two different games to merge.".to_string()),
        )
        .await
        .into_response();
    }

    let names: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM games WHERE id IN (?, ?)")
        .bind(form.canonical_id)
        .bind(form.duplicate_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let canonical_name = names
        .iter()
        .find(|(id, _)| *id == form.canonical_id)
        .map(|(_, n)| n.clone());
    let duplicate_name = names
        .iter()
        .find(|(id, _)| *id == form.duplicate_id)
        .map(|(_, n)| n.clone());
    let (Some(canonical_name), Some(duplicate_name)) = (canonical_name, duplicate_name) else {
        return render_merge_form(&state, &current, None, Some("Game not found.".to_string()))
            .await
            .into_response();
    };

    let Ok(mut tx) = state.db.begin().await else {
        return render_merge_form(
            &state,
            &current,
            None,
            Some("Something went wrong starting the merge.".to_string()),
        )
        .await
        .into_response();
    };

    let plays_moved = sqlx::query("UPDATE plays SET game_id = ? WHERE game_id = ?")
        .bind(form.canonical_id)
        .bind(form.duplicate_id)
        .execute(&mut *tx)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0);

    // Collection statuses: move each one across, but don't blow up if the
    // owner already has that exact status on the canonical game.
    sqlx::query(
        "INSERT OR IGNORE INTO game_status (user_id, game_id, status) \
         SELECT user_id, ?, status FROM game_status WHERE game_id = ?",
    )
    .bind(form.canonical_id)
    .bind(form.duplicate_id)
    .execute(&mut *tx)
    .await
    .ok();
    sqlx::query("DELETE FROM game_status WHERE game_id = ?")
        .bind(form.duplicate_id)
        .execute(&mut *tx)
        .await
        .ok();

    // Expansions used in a specific play: same insert-or-ignore-then-delete
    // approach, since (play_id, expansion_game_id) is also unique.
    sqlx::query(
        "INSERT OR IGNORE INTO play_expansions (play_id, expansion_game_id) \
         SELECT play_id, ? FROM play_expansions WHERE expansion_game_id = ?",
    )
    .bind(form.canonical_id)
    .bind(form.duplicate_id)
    .execute(&mut *tx)
    .await
    .ok();
    sqlx::query("DELETE FROM play_expansions WHERE expansion_game_id = ?")
        .bind(form.duplicate_id)
        .execute(&mut *tx)
        .await
        .ok();

    // Any expansions whose base game was the duplicate now point at the
    // canonical game instead.
    sqlx::query("UPDATE games SET base_game_id = ? WHERE base_game_id = ?")
        .bind(form.canonical_id)
        .bind(form.duplicate_id)
        .execute(&mut *tx)
        .await
        .ok();

    let deleted = sqlx::query("DELETE FROM games WHERE id = ?")
        .bind(form.duplicate_id)
        .execute(&mut *tx)
        .await;

    if deleted.is_err() || tx.commit().await.is_err() {
        return render_merge_form(
            &state,
            &current,
            None,
            Some("Something went wrong merging those games. Nothing was changed.".to_string()),
        )
        .await
        .into_response();
    }

    render_merge_form(
        &state,
        &current,
        Some(format!(
            "Merged \"{duplicate_name}\" into \"{canonical_name}\" ({plays_moved} plays moved). \"{duplicate_name}\" no longer exists."
        )),
        None,
    )
    .await
    .into_response()
}
