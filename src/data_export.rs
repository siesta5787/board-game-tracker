//! Builds a portable export of one user's own data: their collection, every
//! play they logged or are an approved linked player on, and any photos
//! attached to those plays. Mirrors our own schema directly (not BG
//! Catalog's format) — simplest to build completely and correctly, and
//! re-importable into another instance of this app later if needed.

use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;

use crate::AppState;

#[derive(Serialize, Clone)]
struct ExportGame {
    bgg_id: Option<i64>,
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
    notes: Option<String>,
}

#[derive(Serialize)]
struct ExportCollectionItem {
    game_bgg_id: Option<i64>,
    game_name: String,
    status: String,
}

#[derive(Serialize)]
struct ExportPlayer {
    username: Option<String>,
    name: Option<String>,
    score: Option<f64>,
    is_winner: bool,
    link_status: String,
}

#[derive(Serialize)]
struct ExportPlay {
    game_bgg_id: Option<i64>,
    game_name: String,
    play_date: String,
    location_name: Option<String>,
    duration_minutes: Option<i64>,
    notes: Option<String>,
    visibility: String,
    logged_by_username: String,
    players: Vec<ExportPlayer>,
    photos: Vec<String>,
}

#[derive(Serialize)]
struct ExportFile {
    exported_at: String,
    username: String,
    games: Vec<ExportGame>,
    collection: Vec<ExportCollectionItem>,
    plays: Vec<ExportPlay>,
}

#[derive(sqlx::FromRow)]
struct CollectionRow {
    game_id: i64,
    bgg_id: Option<i64>,
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
    notes: Option<String>,
    status: String,
}

#[derive(sqlx::FromRow)]
struct PlayRow {
    game_id: i64,
    bgg_id: Option<i64>,
    game_name: String,
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
    game_notes: Option<String>,
    location_name: Option<String>,
    play_date: String,
    duration_minutes: Option<i64>,
    play_notes: Option<String>,
    visibility: String,
    logged_by_username: String,
}

#[derive(sqlx::FromRow)]
struct PlayPlayerRow {
    player_username: Option<String>,
    player_name: String,
    player_user_id: Option<i64>,
    score: Option<f64>,
    is_winner: bool,
    link_status: String,
}

fn game_row_to_export(r: &CollectionRow) -> ExportGame {
    ExportGame {
        bgg_id: r.bgg_id,
        name: r.name.clone(),
        year_published: r.year_published,
        min_players: r.min_players,
        max_players: r.max_players,
        min_playtime: r.min_playtime,
        max_playtime: r.max_playtime,
        min_age: r.min_age,
        designers: r.designers.clone(),
        artists: r.artists.clone(),
        thumbnail_url: r.thumbnail_url.clone(),
        image_url: r.image_url.clone(),
        average_rating: r.average_rating,
        weight: r.weight,
        is_expansion: r.is_expansion,
        notes: r.notes.clone(),
    }
}

fn play_row_to_game_export(r: &PlayRow) -> ExportGame {
    ExportGame {
        bgg_id: r.bgg_id,
        name: r.game_name.clone(),
        year_published: r.year_published,
        min_players: r.min_players,
        max_players: r.max_players,
        min_playtime: r.min_playtime,
        max_playtime: r.max_playtime,
        min_age: r.min_age,
        designers: r.designers.clone(),
        artists: r.artists.clone(),
        thumbnail_url: r.thumbnail_url.clone(),
        image_url: r.image_url.clone(),
        average_rating: r.average_rating,
        weight: r.weight,
        is_expansion: r.is_expansion,
        notes: r.game_notes.clone(),
    }
}

pub async fn build_export(state: &AppState, user_id: i64) -> Result<Vec<u8>, String> {
    let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| e.to_string())?;

    let my_player_id: Option<i64> = sqlx::query_scalar("SELECT id FROM players WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| e.to_string())?
        .flatten();

    let mut games: HashMap<i64, ExportGame> = HashMap::new();

    // Collection.
    let collection_rows = sqlx::query_as::<_, CollectionRow>(
        "SELECT games.id AS game_id, games.bgg_id, games.name, games.year_published, \
                games.min_players, games.max_players, games.min_playtime, games.max_playtime, \
                games.min_age, games.designers, games.artists, games.thumbnail_url, games.image_url, \
                games.average_rating, games.weight, games.is_expansion, games.notes, game_status.status \
         FROM game_status \
         JOIN games ON games.id = game_status.game_id \
         WHERE game_status.user_id = ? \
         ORDER BY games.name",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let mut collection = Vec::with_capacity(collection_rows.len());
    for r in &collection_rows {
        games
            .entry(r.game_id)
            .or_insert_with(|| game_row_to_export(r));
        collection.push(ExportCollectionItem {
            game_bgg_id: r.bgg_id,
            game_name: r.name.clone(),
            status: r.status.clone(),
        });
    }

    // Plays: ones this user logged, or is an approved linked player on.
    let play_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT plays.id \
         FROM plays \
         LEFT JOIN play_players pp ON pp.play_id = plays.id AND pp.player_id = ? \
         WHERE plays.logged_by_user_id = ? OR pp.link_status = 'approved'",
    )
    .bind(my_player_id)
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let mut plays = Vec::with_capacity(play_ids.len());
    let mut photo_files: Vec<(String, String)> = Vec::new(); // (source path on disk, zip entry path)

    for play_id in play_ids {
        let Some(pr) = sqlx::query_as::<_, PlayRow>(
            "SELECT games.id AS game_id, games.bgg_id, games.name AS game_name, \
                    games.year_published, games.min_players, games.max_players, \
                    games.min_playtime, games.max_playtime, games.min_age, games.designers, \
                    games.artists, games.thumbnail_url, games.image_url, games.average_rating, \
                    games.weight, games.is_expansion, games.notes AS game_notes, \
                    locations.name AS location_name, plays.play_date, plays.duration_minutes, \
                    plays.notes AS play_notes, plays.visibility, users.username AS logged_by_username \
             FROM plays \
             JOIN games ON games.id = plays.game_id \
             LEFT JOIN locations ON locations.id = plays.location_id \
             JOIN users ON users.id = plays.logged_by_user_id \
             WHERE plays.id = ?",
        )
        .bind(play_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| e.to_string())?
        else {
            continue;
        };

        games
            .entry(pr.game_id)
            .or_insert_with(|| play_row_to_game_export(&pr));

        let player_rows = sqlx::query_as::<_, PlayPlayerRow>(
            "SELECT users.username AS player_username, players.name AS player_name, \
                    players.user_id AS player_user_id, play_players.score, \
                    play_players.is_winner, play_players.link_status \
             FROM play_players \
             JOIN players ON players.id = play_players.player_id \
             LEFT JOIN users ON users.id = players.user_id \
             WHERE play_players.play_id = ? \
             ORDER BY play_players.id",
        )
        .bind(play_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| e.to_string())?;

        let players = player_rows
            .into_iter()
            .map(|p| ExportPlayer {
                username: p.player_username,
                name: if p.player_user_id.is_none() {
                    Some(p.player_name)
                } else {
                    None
                },
                score: p.score,
                is_winner: p.is_winner,
                link_status: p.link_status,
            })
            .collect();

        let photo_paths: Vec<String> = sqlx::query_scalar(
            "SELECT file_path FROM play_photos WHERE play_id = ? ORDER BY upload_order",
        )
        .bind(play_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| e.to_string())?;

        let mut photos = Vec::with_capacity(photo_paths.len());
        for path in &photo_paths {
            let filename = path.rsplit('/').next().unwrap_or(path);
            let zip_path = format!("photos/{play_id}/{filename}");
            photo_files.push((path.clone(), zip_path.clone()));
            photos.push(zip_path);
        }

        plays.push(ExportPlay {
            game_bgg_id: pr.bgg_id,
            game_name: pr.game_name.clone(),
            play_date: pr.play_date,
            location_name: pr.location_name,
            duration_minutes: pr.duration_minutes,
            notes: pr.play_notes,
            visibility: pr.visibility,
            logged_by_username: pr.logged_by_username,
            players,
            photos,
        });
    }

    let export = ExportFile {
        exported_at: chrono::Local::now().to_rfc3339(),
        username,
        games: games.into_values().collect(),
        collection,
        plays,
    };

    let json_bytes = serde_json::to_vec_pretty(&export).map_err(|e| e.to_string())?;

    let mut zip_buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        writer
            .start_file("export.json", options)
            .map_err(|e| e.to_string())?;
        writer.write_all(&json_bytes).map_err(|e| e.to_string())?;

        for (source_path, zip_path) in &photo_files {
            let Ok(bytes) = std::fs::read(source_path) else {
                continue;
            };
            writer
                .start_file(zip_path, options)
                .map_err(|e| e.to_string())?;
            writer.write_all(&bytes).map_err(|e| e.to_string())?;
        }

        writer.finish().map_err(|e| e.to_string())?;
    }

    Ok(zip_buf)
}
