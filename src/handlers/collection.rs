use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use axum::{Extension, Form};
use serde::Deserialize;

use crate::AppState;
use crate::bgg::{self, GameDetails, SearchResult};
use crate::security::CurrentUser;

const STATUSES: [&str; 5] = ["owned", "want_to_buy", "wishlist", "preordered", "for_sale"];

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
    pub is_want_to_buy: bool,
    pub is_wishlist: bool,
    pub is_preordered: bool,
    pub is_for_sale: bool,
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
            is_want_to_buy: statuses.contains(&"want_to_buy"),
            is_wishlist: statuses.contains(&"wishlist"),
            is_preordered: statuses.contains(&"preordered"),
            is_for_sale: statuses.contains(&"for_sale"),
        }
    }
}

#[derive(Template)]
#[template(path = "collection.html")]
struct CollectionTemplate {
    title: String,
    username: String,
    is_admin: bool,
    collection_owner: String,
    is_own_collection: bool,
    entries: Vec<CollectionEntry>,
}

pub async fn view_collection(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(username): Path<String>,
) -> impl IntoResponse {
    let owner_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE username = ? AND is_active = 1")
            .bind(&username)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    let Some(owner_id) = owner_id else {
        return (axum::http::StatusCode::NOT_FOUND, "User not found").into_response();
    };

    let rows = sqlx::query_as::<_, CollectionRow>(
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
    .unwrap_or_default();

    let entries = rows.into_iter().map(CollectionEntry::from).collect();

    Html(
        CollectionTemplate {
            title: format!("{username}'s collection"),
            username: current.username.clone(),
            is_admin: current.is_admin,
            collection_owner: username.clone(),
            is_own_collection: current.username == username,
            entries,
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
    is_admin: bool,
    query: String,
    results: Vec<SearchResult>,
    error: Option<String>,
    statuses: [(&'static str, &'static str); 5],
}

pub async fn add_search_form(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    let query = params.q.unwrap_or_default();
    let (results, error) = if query.trim().is_empty() {
        (Vec::new(), None)
    } else {
        match bgg::search(query.trim()).await {
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
            is_admin: current.is_admin,
            query,
            results,
            error,
            statuses: status_options(),
        }
        .render()
        .unwrap(),
    )
}

fn status_options() -> [(&'static str, &'static str); 5] {
    [
        ("owned", "Owned"),
        ("want_to_buy", "Want to Buy"),
        ("wishlist", "Wishlist"),
        ("preordered", "Pre-ordered"),
        ("for_sale", "For Sale"),
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

    let details = match bgg::fetch_game(form.bgg_id).await {
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
    is_admin: bool,
    error: Option<String>,
    statuses: [(&'static str, &'static str); 5],
}

pub async fn manual_add_form(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    Html(
        ManualAddTemplate {
            title: "Add a game manually".to_string(),
            username: current.username,
            is_admin: current.is_admin,
            error: None,
            statuses: status_options(),
        }
        .render()
        .unwrap(),
    )
}

#[derive(Deserialize)]
pub struct ManualAddForm {
    name: String,
    min_players: Option<i32>,
    max_players: Option<i32>,
    status: String,
}

pub async fn create_manual(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Form(form): Form<ManualAddForm>,
) -> impl IntoResponse {
    let render_error = |msg: &str| -> axum::response::Response {
        Html(
            ManualAddTemplate {
                title: "Add a game manually".to_string(),
                username: current.username.clone(),
                is_admin: current.is_admin,
                error: Some(msg.to_string()),
                statuses: status_options(),
            }
            .render()
            .unwrap(),
        )
        .into_response()
    };

    let name = form.name.trim();
    if name.is_empty() {
        return render_error("Game name can't be empty.");
    }
    if !STATUSES.contains(&form.status.as_str()) {
        return render_error("Invalid status.");
    }

    let game_id: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO games (bgg_id, name, min_players, max_players) VALUES (NULL, ?, ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(form.min_players)
    .bind(form.max_players)
    .fetch_one(&state.db)
    .await;

    let game_id = match game_id {
        Ok(id) => id,
        Err(_) => return render_error("Something went wrong saving that game."),
    };

    add_status_row(&state, current.id, game_id, &form.status).await;

    Redirect::to(&format!("/collection/{}", current.username)).into_response()
}

pub async fn add_status(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path((game_id, status)): Path<(i64, String)>,
) -> impl IntoResponse {
    if !STATUSES.contains(&status.as_str()) {
        return (axum::http::StatusCode::BAD_REQUEST, "Invalid status").into_response();
    }
    add_status_row(&state, current.id, game_id, &status).await;
    Redirect::to(&format!("/collection/{}", current.username)).into_response()
}

pub async fn remove_status(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path((game_id, status)): Path<(i64, String)>,
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
    Redirect::to(&format!("/collection/{}", current.username)).into_response()
}
