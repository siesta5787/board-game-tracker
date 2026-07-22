use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use axum::{Extension, Form};
use serde::Deserialize;

use crate::AppState;
use crate::bgg::{self, GameDetails, SearchResult};
use crate::security::{self, CurrentUser};
use crate::settings;

const STATUSES: [&str; 7] = [
    "owned",
    "wishlist",
    "preordered",
    "for_sale",
    "played",
    "want_to_play",
    "want_to_trade",
];

pub async fn redirect_to_own(
    Extension(CurrentUser(user)): Extension<CurrentUser>,
) -> impl IntoResponse {
    Redirect::to(&format!("/collection/{}", user.username))
}

#[derive(sqlx::FromRow)]
struct CollectionRow {
    id: i64,
    name: String,
    year_published: Option<i32>,
    min_players: Option<i32>,
    max_players: Option<i32>,
    weight: Option<f64>,
    thumbnail_url: Option<String>,
    statuses: String,
}

pub struct CollectionEntry {
    pub id: i64,
    pub name: String,
    pub year_published: Option<i32>,
    pub min_players: Option<i32>,
    pub max_players: Option<i32>,
    pub weight: Option<String>,
    pub thumbnail_url: Option<String>,
    pub is_owned: bool,
    pub is_wishlist: bool,
    pub is_preordered: bool,
    pub is_for_sale: bool,
    pub is_played: bool,
    pub is_want_to_play: bool,
    pub is_want_to_trade: bool,
}

impl From<CollectionRow> for CollectionEntry {
    fn from(row: CollectionRow) -> Self {
        let statuses: Vec<&str> = row.statuses.split(',').collect();
        CollectionEntry {
            id: row.id,
            name: row.name,
            year_published: row.year_published,
            min_players: row.min_players,
            max_players: row.max_players,
            weight: row.weight.map(|w| format!("{w:.1}")),
            thumbnail_url: row.thumbnail_url,
            is_owned: statuses.contains(&"owned"),
            is_wishlist: statuses.contains(&"wishlist"),
            is_preordered: statuses.contains(&"preordered"),
            is_for_sale: statuses.contains(&"for_sale"),
            is_played: statuses.contains(&"played"),
            is_want_to_play: statuses.contains(&"want_to_play"),
            is_want_to_trade: statuses.contains(&"want_to_trade"),
        }
    }
}

#[derive(Deserialize)]
pub struct CollectionQuery {
    status: Option<String>,
}

#[derive(Template)]
#[template(path = "collection.html")]
struct CollectionTemplate {
    title: String,
    username: String,
    collection_owner: String,
    collection_owner_display: String,
    is_own_collection: bool,
    entries: Vec<CollectionEntry>,
    status_filter: Option<String>,
    statuses: [(&'static str, &'static str); 7],
}

pub async fn view_collection(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(username): Path<String>,
    Query(params): Query<CollectionQuery>,
) -> impl IntoResponse {
    let owner: Option<(i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, first_name, last_name FROM users WHERE username = ? AND is_active = 1",
    )
    .bind(&username)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((owner_id, owner_first, owner_last)) = owner else {
        return (axum::http::StatusCode::NOT_FOUND, "User not found").into_response();
    };
    let collection_owner_display =
        security::display_name(&username, owner_first.as_deref(), owner_last.as_deref());

    let status_filter = params.status.filter(|s| STATUSES.contains(&s.as_str()));

    let rows = if let Some(status) = &status_filter {
        sqlx::query_as::<_, CollectionRow>(
            "SELECT g.id, g.name, g.year_published, g.min_players, g.max_players, g.weight, g.thumbnail_url, \
                    GROUP_CONCAT(gs.status) AS statuses \
             FROM games g \
             JOIN game_status gs ON gs.game_id = g.id \
             WHERE gs.user_id = ? \
               AND g.id IN (SELECT game_id FROM game_status WHERE user_id = ? AND status = ?) \
             GROUP BY g.id \
             ORDER BY g.name",
        )
        .bind(owner_id)
        .bind(owner_id)
        .bind(status)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    } else {
        sqlx::query_as::<_, CollectionRow>(
            "SELECT g.id, g.name, g.year_published, g.min_players, g.max_players, g.weight, g.thumbnail_url, \
                    GROUP_CONCAT(gs.status) AS statuses \
             FROM games g \
             JOIN game_status gs ON gs.game_id = g.id \
             WHERE gs.user_id = ? \
             GROUP BY g.id \
             ORDER BY g.name",
        )
        .bind(owner_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    };

    let entries = rows.into_iter().map(CollectionEntry::from).collect();

    Html(
        CollectionTemplate {
            title: format!("{collection_owner_display}'s collection"),
            username: current.username.clone(),
            collection_owner: username.clone(),
            collection_owner_display,
            is_own_collection: current.username == username,
            entries,
            status_filter,
            statuses: status_options(),
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
}

#[derive(Template)]
#[template(path = "collection_add.html")]
struct CollectionAddTemplate {
    title: String,
    username: String,
    query: String,
    results: Vec<SearchResult>,
    error: Option<String>,
    statuses: [(&'static str, &'static str); 7],
}

pub async fn add_search_form(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    let query = params.q.unwrap_or_default();
    let token = settings::get(&state.db, settings::BGG_API_TOKEN).await;
    let (results, error) = if query.trim().is_empty() {
        (Vec::new(), None)
    } else if token.is_none() {
        (
            Vec::new(),
            Some(
                "BoardGameGeek search isn't set up on this instance yet — an admin needs to add \
                 an API token under Admin console > BGG. You can add this game manually instead."
                    .to_string(),
            ),
        )
    } else {
        match bgg::search(query.trim(), token.as_deref()).await {
            Ok(r) if r.is_empty() => (
                Vec::new(),
                Some("No games found on BoardGameGeek for that search.".to_string()),
            ),
            Ok(r) => (r, None),
            Err(_) => (
                Vec::new(),
                Some(
                    "Couldn't reach BoardGameGeek right now. You can add this game manually instead."
                        .to_string(),
                ),
            ),
        }
    };

    Html(
        CollectionAddTemplate {
            title: "Add a game".to_string(),
            username: current.username,
            query,
            results,
            error,
            statuses: status_options(),
        }
        .render()
        .unwrap(),
    )
}

#[derive(Template)]
#[template(path = "collection_add_preview.html")]
struct CollectionAddPreviewTemplate {
    title: String,
    username: String,
    query: String,
    game: GameDetails,
    statuses: [(&'static str, &'static str); 7],
}

#[derive(Deserialize)]
pub struct PreviewQuery {
    q: Option<String>,
}

/// Shows the same kind of detail (photo, player counts, weight, rating,
/// designers/artists) a game gets once it's actually added — reached by
/// clicking a search result, since BGG's search endpoint itself doesn't
/// return images or any of this, only name/year. Fetching it lazily like
/// this (one extra request, only for the specific game someone's actually
/// considering) avoids firing a full `fetch_game` for every row in a
/// results list just to show a thumbnail.
pub async fn preview_from_bgg(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(bgg_id): Path<i64>,
    Query(params): Query<PreviewQuery>,
) -> impl IntoResponse {
    let token = settings::get(&state.db, settings::BGG_API_TOKEN).await;
    match bgg::fetch_game(bgg_id, token.as_deref()).await {
        Ok(Some(game)) => Html(
            CollectionAddPreviewTemplate {
                title: game.name.clone(),
                username: current.username,
                query: params.q.unwrap_or_default(),
                game,
                statuses: status_options(),
            }
            .render()
            .unwrap(),
        )
        .into_response(),
        _ => (
            axum::http::StatusCode::BAD_GATEWAY,
            "Couldn't fetch that game's details from BoardGameGeek. Try again, or add it manually.",
        )
            .into_response(),
    }
}

fn status_options() -> [(&'static str, &'static str); 7] {
    [
        ("owned", "Owned"),
        ("wishlist", "Wishlist"),
        ("preordered", "Pre-ordered"),
        ("for_sale", "For Sale"),
        ("played", "Played"),
        ("want_to_play", "Want to Play"),
        ("want_to_trade", "Want to Trade"),
    ]
}

#[derive(Deserialize)]
pub struct AddFromBggForm {
    bgg_id: i64,
    status: String,
}

pub async fn add_from_bgg(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Form(form): Form<AddFromBggForm>,
) -> impl IntoResponse {
    if !STATUSES.contains(&form.status.as_str()) {
        return (axum::http::StatusCode::BAD_REQUEST, "Invalid status").into_response();
    }

    let token = settings::get(&state.db, settings::BGG_API_TOKEN).await;
    let details = match bgg::fetch_game(form.bgg_id, token.as_deref()).await {
        Ok(Some(d)) => d,
        _ => {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                "Couldn't fetch that game's details from BoardGameGeek. Try again, or add it manually.",
            )
                .into_response();
        }
    };

    let game_id = match upsert_game_from_bgg(&state, &details).await {
        Ok(id) => id,
        Err(_) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong saving that game.",
            )
                .into_response();
        }
    };

    add_status_row(&state, current.id, game_id, &form.status).await;

    Redirect::to(&format!("/collection/{}", current.username)).into_response()
}

async fn upsert_game_from_bgg(state: &AppState, details: &GameDetails) -> Result<i64, sqlx::Error> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM games WHERE bgg_id = ?")
        .bind(details.bgg_id)
        .fetch_optional(&state.db)
        .await?
    {
        return Ok(id);
    }

    // Someone may have already added this exact game manually (or from a
    // different source) before it was ever looked up on BGG. Names are kept
    // unique across the whole games table, so reuse that row instead of
    // creating a duplicate — and if it was a bare manual entry (no bgg_id
    // yet), upgrade it in place with the real BGG metadata we just fetched.
    let existing_by_name: Option<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, bgg_id FROM games WHERE name = ? COLLATE NOCASE")
            .bind(&details.name)
            .fetch_optional(&state.db)
            .await?;

    if let Some((id, existing_bgg_id)) = existing_by_name {
        if existing_bgg_id.is_none() {
            sqlx::query(
                "UPDATE games SET bgg_id = ?, year_published = ?, min_players = ?, max_players = ?, \
                        min_playtime = ?, max_playtime = ?, min_age = ?, designers = ?, artists = ?, \
                        thumbnail_url = ?, image_url = ?, average_rating = ?, weight = ?, is_expansion = ? \
                 WHERE id = ?",
            )
            .bind(details.bgg_id)
            .bind(details.year_published)
            .bind(details.min_players)
            .bind(details.max_players)
            .bind(details.min_playtime)
            .bind(details.max_playtime)
            .bind(details.min_age)
            .bind(&details.designers)
            .bind(&details.artists)
            .bind(&details.thumbnail_url)
            .bind(&details.image_url)
            .bind(details.average_rating)
            .bind(details.weight)
            .bind(details.is_expansion)
            .bind(id)
            .execute(&state.db)
            .await?;
        }
        return Ok(id);
    }

    let base_game_id: Option<i64> = if let Some(base_bgg_id) = details.base_game_bgg_id {
        sqlx::query_scalar("SELECT id FROM games WHERE bgg_id = ?")
            .bind(base_bgg_id)
            .fetch_optional(&state.db)
            .await?
    } else {
        None
    };

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO games (bgg_id, name, year_published, min_players, max_players, min_playtime, \
                             max_playtime, min_age, designers, artists, thumbnail_url, image_url, \
                             average_rating, weight, is_expansion, base_game_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(details.bgg_id)
    .bind(&details.name)
    .bind(details.year_published)
    .bind(details.min_players)
    .bind(details.max_players)
    .bind(details.min_playtime)
    .bind(details.max_playtime)
    .bind(details.min_age)
    .bind(&details.designers)
    .bind(&details.artists)
    .bind(&details.thumbnail_url)
    .bind(&details.image_url)
    .bind(details.average_rating)
    .bind(details.weight)
    .bind(details.is_expansion)
    .bind(base_game_id)
    .fetch_one(&state.db)
    .await?;

    Ok(id)
}

async fn add_status_row(state: &AppState, user_id: i64, game_id: i64, status: &str) {
    sqlx::query(
        "INSERT INTO game_status (user_id, game_id, status) VALUES (?, ?, ?) \
         ON CONFLICT (user_id, game_id, status) DO NOTHING",
    )
    .bind(user_id)
    .bind(game_id)
    .bind(status)
    .execute(&state.db)
    .await
    .ok();
}

#[derive(Template)]
#[template(path = "collection_add_manual.html")]
struct ManualAddTemplate {
    title: String,
    username: String,
    error: Option<String>,
    existing_game: Option<(i64, String)>,
    statuses: [(&'static str, &'static str); 7],
}

pub async fn manual_add_form(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    Html(
        ManualAddTemplate {
            title: "Add a game manually".to_string(),
            username: current.username,
            error: None,
            existing_game: None,
            statuses: status_options(),
        }
        .render()
        .unwrap(),
    )
}

#[derive(Deserialize)]
pub struct ManualAddForm {
    name: String,
    // Deliberately String, not Option<i32>: an empty form field deserializes
    // to Some("") for an Option<String>, but axum's Form extractor rejects
    // the whole request outright if it's typed Option<i32> and the field is
    // blank (it tries to parse "" as an int and fails before the handler
    // even runs). Parsed manually below instead, where blank/invalid just
    // means "not specified."
    min_players: String,
    max_players: String,
    status: String,
}

pub async fn create_manual(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Form(form): Form<ManualAddForm>,
) -> impl IntoResponse {
    let render_error =
        |msg: &str, existing_game: Option<(i64, String)>| -> axum::response::Response {
            Html(
                ManualAddTemplate {
                    title: "Add a game manually".to_string(),
                    username: current.username.clone(),
                    error: Some(msg.to_string()),
                    existing_game,
                    statuses: status_options(),
                }
                .render()
                .unwrap(),
            )
            .into_response()
        };

    let name = form.name.trim();
    if name.is_empty() {
        return render_error("Game name can't be empty.", None);
    }
    if !STATUSES.contains(&form.status.as_str()) {
        return render_error("Invalid status.", None);
    }

    let existing: Option<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM games WHERE name = ? COLLATE NOCASE")
            .bind(name)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    if let Some((id, existing_name)) = existing {
        return render_error(
            "A game with that name is already in the system.",
            Some((id, existing_name)),
        );
    }

    let min_players: Option<i32> = form.min_players.trim().parse().ok();
    let max_players: Option<i32> = form.max_players.trim().parse().ok();

    let game_id: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO games (bgg_id, name, min_players, max_players) VALUES (NULL, ?, ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(min_players)
    .bind(max_players)
    .fetch_one(&state.db)
    .await;

    let game_id = match game_id {
        Ok(id) => id,
        Err(_) => return render_error("Something went wrong saving that game.", None),
    };

    add_status_row(&state, current.id, game_id, &form.status).await;

    Redirect::to(&format!("/collection/{}", current.username)).into_response()
}

/// Sends the user back wherever the status-toggle form was submitted from
/// (their own collection page, or a game's detail page), falling back to
/// their collection if the browser didn't send a Referer. Only the path is
/// ever taken from the (client-controlled) Referer header, never the scheme
/// or host, so this can't be used to redirect off our own site.
fn redirect_back(headers: &axum::http::HeaderMap, fallback_username: &str) -> Redirect {
    let path = headers
        .get(axum::http::header::REFERER)
        .and_then(|v| v.to_str().ok())
        .and_then(|referer| {
            let after_scheme = referer.splitn(2, "://").nth(1)?;
            let path_start = after_scheme.find('/')?;
            Some(after_scheme[path_start..].to_string())
        });
    match path {
        Some(p) if !p.is_empty() => Redirect::to(&p),
        _ => Redirect::to(&format!("/collection/{fallback_username}")),
    }
}

pub async fn add_status(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path((game_id, status)): Path<(i64, String)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !STATUSES.contains(&status.as_str()) {
        return (axum::http::StatusCode::BAD_REQUEST, "Invalid status").into_response();
    }
    add_status_row(&state, current.id, game_id, &status).await;
    redirect_back(&headers, &current.username).into_response()
}

pub async fn remove_status(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path((game_id, status)): Path<(i64, String)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !STATUSES.contains(&status.as_str()) {
        return (axum::http::StatusCode::BAD_REQUEST, "Invalid status").into_response();
    }
    sqlx::query("DELETE FROM game_status WHERE user_id = ? AND game_id = ? AND status = ?")
        .bind(current.id)
        .bind(game_id)
        .bind(&status)
        .execute(&state.db)
        .await
        .ok();
    redirect_back(&headers, &current.username).into_response()
}
