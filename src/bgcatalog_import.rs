//! One-time importer for BG Catalog (bgcatalog.app) JSON+photo exports.
//!
//! Games are matched to our shared `games` table by `bggId` (BG Catalog
//! always has this for every game, so no fuzzy matching is needed). Every
//! player except the export's own "me" player becomes a guest `players` row
//! — friends are never auto-linked to their real accounts by name, since a
//! wrong guess would be a confusing, hard-to-notice mistake; the "me" player
//! is unambiguous (it's literally the person running the import) so it maps
//! to their real account instead. Imported plays default to `private`
//! visibility, since they were never shared with anyone in the old app.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;

use crate::AppState;

#[derive(Deserialize)]
struct ExportFile {
    games: Vec<RawGame>,
    players: Vec<RawPlayer>,
    locations: Vec<RawLocation>,
    plays: Vec<RawPlay>,
    #[serde(rename = "playersPlays")]
    players_plays: Vec<RawPlayerPlay>,
    #[serde(default)]
    images: Vec<RawImage>,
}

#[derive(Deserialize)]
struct RawGame {
    id: i64,
    name: String,
    #[serde(rename = "bggId")]
    bgg_id: i64,
    #[serde(rename = "yearPublished")]
    year_published: Option<i32>,
    #[serde(rename = "minPlayers")]
    min_players: Option<i32>,
    #[serde(rename = "maxPlayers")]
    max_players: Option<i32>,
    #[serde(rename = "minPlayTime")]
    min_play_time: Option<i32>,
    #[serde(rename = "maxPlayTime")]
    max_play_time: Option<i32>,
    #[serde(rename = "minAge")]
    min_age: Option<i32>,
    #[serde(rename = "urlThumb")]
    url_thumb: Option<String>,
    #[serde(rename = "urlImage")]
    url_image: Option<String>,
    designers: Option<String>,
    artists: Option<String>,
    notes: Option<String>,
    expansion: i32,
    average: Option<f64>,
    // BG Catalog's own export is inconsistently cased here — it's really
    // "averageWeight" everywhere else but this one field is all lowercase.
    #[serde(rename = "averageweight")]
    average_weight: Option<f64>,
}

#[derive(Deserialize)]
struct RawPlayer {
    id: i64,
    name: String,
    me: i32,
}

#[derive(Deserialize)]
struct RawLocation {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
struct RawPlay {
    id: i64,
    #[serde(rename = "playDate")]
    play_date: String,
    #[serde(rename = "gameId")]
    game_id: i64,
    #[serde(rename = "locationId")]
    location_id: Option<i64>,
    notes: Option<String>,
    length: Option<i32>,
}

#[derive(Deserialize)]
struct RawPlayerPlay {
    #[serde(rename = "playId")]
    play_id: i64,
    #[serde(rename = "playerId")]
    player_id: i64,
    score: Option<f64>,
    winner: Option<i32>,
}

#[derive(Deserialize)]
struct RawImage {
    #[serde(rename = "objectId")]
    object_id: i64,
    #[serde(rename = "imageType")]
    image_type: String,
    image: String,
}

#[derive(Default)]
pub struct ImportSummary {
    pub games_created: usize,
    pub games_matched: usize,
    pub games_added_to_collection: usize,
    pub locations_imported: usize,
    pub guest_players_created: usize,
    pub plays_imported: usize,
    pub player_play_records: usize,
    pub photos_copied: usize,
    pub skipped: Vec<String>,
}

/// Strips BG Catalog's auto-appended "#BGCatalog" / "#BGGCatalog" hashtag
/// lines out of a notes field, wherever they appear, and returns `None` if
/// nothing meaningful is left.
fn clean_notes(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let cleaned: Vec<&str> = raw
        .lines()
        .filter(|line| {
            let t = line.trim();
            t != "#BGCatalog" && t != "#BGGCatalog"
        })
        .collect();
    let joined = cleaned.join("\n").trim().to_string();
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

async fn find_or_create_location(state: &AppState, name: &str) -> Option<i64> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
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

async fn find_or_create_guest_player(state: &AppState, name: &str) -> Option<i64> {
    if let Some(id) =
        sqlx::query_scalar::<_, i64>("SELECT id FROM players WHERE user_id IS NULL AND name = ?")
            .bind(name)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
    {
        return Some(id);
    }
    sqlx::query_scalar("INSERT INTO players (user_id, name) VALUES (NULL, ?) RETURNING id")
        .bind(name)
        .fetch_one(&state.db)
        .await
        .ok()
}

async fn upsert_game(state: &AppState, g: &RawGame) -> Option<(i64, bool)> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM games WHERE bgg_id = ?")
        .bind(g.bgg_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
    {
        return Some((id, false));
    }

    let notes = g.notes.as_deref().filter(|s| !s.trim().is_empty());
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO games (bgg_id, name, year_published, min_players, max_players, min_playtime, \
                             max_playtime, min_age, designers, artists, thumbnail_url, image_url, \
                             average_rating, weight, is_expansion, notes) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(g.bgg_id)
    .bind(&g.name)
    .bind(g.year_published)
    .bind(g.min_players)
    .bind(g.max_players)
    .bind(g.min_play_time)
    .bind(g.max_play_time)
    .bind(g.min_age)
    .bind(&g.designers)
    .bind(&g.artists)
    .bind(&g.url_thumb)
    .bind(&g.url_image)
    .bind(g.average)
    .bind(g.average_weight)
    .bind(g.expansion != 0)
    .bind(notes)
    .fetch_one(&state.db)
    .await
    .ok()?;

    Some((id, true))
}

/// Sanitizes a zip entry's filename down to just its base component, so a
/// crafted path (e.g. containing `../`) can never write outside the target
/// photo directory.
fn safe_filename(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("photo.jpg")
        .to_string()
}

pub async fn import_from_zip(
    state: &AppState,
    zip_bytes: Vec<u8>,
    admin_user_id: i64,
) -> Result<ImportSummary, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|e| format!("Not a valid zip file: {e}"))?;

    let json_name = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .find(|name| name.to_lowercase().ends_with(".json"))
        .ok_or("No .json file found in the zip")?;

    let json_bytes = {
        let mut file = archive
            .by_name(&json_name)
            .map_err(|e| format!("Failed to read {json_name}: {e}"))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| format!("Failed to read {json_name}: {e}"))?;
        buf
    };

    let export: ExportFile = serde_json::from_slice(&json_bytes)
        .map_err(|e| format!("Couldn't parse {json_name}: {e}"))?;

    let mut summary = ImportSummary::default();

    let admin_player_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM players WHERE user_id = ?")
            .bind(admin_user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    // Games, matched/created by bggId.
    let mut game_id_map: HashMap<i64, i64> = HashMap::new();
    for g in &export.games {
        match upsert_game(state, g).await {
            Some((new_id, created)) => {
                game_id_map.insert(g.id, new_id);
                if created {
                    summary.games_created += 1;
                } else {
                    summary.games_matched += 1;
                }
            }
            None => summary
                .skipped
                .push(format!("Game \"{}\" couldn't be saved", g.name)),
        }
    }

    // Locations, matched/created by trimmed name.
    let mut location_id_map: HashMap<i64, i64> = HashMap::new();
    for l in &export.locations {
        if let Some(new_id) = find_or_create_location(state, &l.name).await {
            location_id_map.insert(l.id, new_id);
            summary.locations_imported += 1;
        }
    }

    // Players: only ones actually used in a play, mapped to guests except
    // the "me" player, which maps to the importing admin's own account.
    let used_player_ids: HashSet<i64> =
        export.players_plays.iter().map(|pp| pp.player_id).collect();
    let mut player_id_map: HashMap<i64, i64> = HashMap::new();
    for p in &export.players {
        if !used_player_ids.contains(&p.id) {
            continue;
        }
        if p.me != 0 {
            if let Some(aid) = admin_player_id {
                player_id_map.insert(p.id, aid);
                continue;
            }
        }
        let name = if p.name.trim().is_empty() {
            format!("Imported player {}", p.id)
        } else {
            p.name.trim().to_string()
        };
        if let Some(guest_id) = find_or_create_guest_player(state, &name).await {
            player_id_map.insert(p.id, guest_id);
            summary.guest_players_created += 1;
        }
    }

    // Plays, defaulting to private visibility.
    let mut play_id_map: HashMap<i64, i64> = HashMap::new();
    let mut played_game_ids: HashSet<i64> = HashSet::new();
    for pl in &export.plays {
        let Some(&new_game_id) = game_id_map.get(&pl.game_id) else {
            summary
                .skipped
                .push(format!("Play {} references an unknown game", pl.id));
            continue;
        };
        let new_location_id = pl
            .location_id
            .and_then(|old| location_id_map.get(&old).copied());
        let play_date = pl.play_date.split(' ').next().unwrap_or(&pl.play_date);
        let notes = clean_notes(pl.notes.as_deref());

        let inserted: Result<i64, sqlx::Error> = sqlx::query_scalar(
            "INSERT INTO plays (game_id, location_id, play_date, duration_minutes, notes, \
                                 visibility, logged_by_user_id) \
             VALUES (?, ?, ?, ?, ?, 'private', ?) RETURNING id",
        )
        .bind(new_game_id)
        .bind(new_location_id)
        .bind(play_date)
        .bind(pl.length)
        .bind(&notes)
        .bind(admin_user_id)
        .fetch_one(&state.db)
        .await;

        match inserted {
            Ok(new_play_id) => {
                play_id_map.insert(pl.id, new_play_id);
                summary.plays_imported += 1;
                played_game_ids.insert(new_game_id);
            }
            Err(e) => summary.skipped.push(format!("Play {} failed: {e}", pl.id)),
        }
    }

    // A game that shows up in imported play history was very likely owned
    // by the importer, even though BG Catalog's export has no explicit
    // "owned" flag to carry over — mark it Owned in their collection so it
    // isn't just a name in the log-a-play dropdown. INSERT OR IGNORE respects
    // the UNIQUE(user_id, game_id, status) constraint if it's already set.
    for &game_id in &played_game_ids {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO game_status (user_id, game_id, status) VALUES (?, ?, 'owned')",
        )
        .bind(admin_user_id)
        .bind(game_id)
        .execute(&state.db)
        .await;
        if let Ok(r) = result {
            if r.rows_affected() > 0 {
                summary.games_added_to_collection += 1;
            }
        }
    }

    // Player-play records (scores/winners).
    for pp in &export.players_plays {
        let (Some(&new_play_id), Some(&new_player_id)) = (
            play_id_map.get(&pp.play_id),
            player_id_map.get(&pp.player_id),
        ) else {
            continue;
        };
        let link_status = if Some(new_player_id) == admin_player_id {
            "approved"
        } else {
            "none"
        };
        sqlx::query(
            "INSERT INTO play_players (play_id, player_id, score, is_winner, link_status) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(new_play_id)
        .bind(new_player_id)
        .bind(pp.score)
        .bind(pp.winner.unwrap_or(0) != 0)
        .bind(link_status)
        .execute(&state.db)
        .await
        .ok();
        summary.player_play_records += 1;
    }

    // Photos: copy bytes out of the zip into data/photos/{play_id}/.
    for img in &export.images {
        if img.image_type != "play" {
            continue;
        }
        let Some(&new_play_id) = play_id_map.get(&img.object_id) else {
            continue;
        };

        let existing_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM play_photos WHERE play_id = ?")
                .bind(new_play_id)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0);
        if existing_count >= 5 {
            continue;
        }

        let Ok(mut file) = archive.by_name(&img.image) else {
            summary
                .skipped
                .push(format!("Photo {} not found in zip", img.image));
            continue;
        };
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            continue;
        }
        drop(file);

        let filename = safe_filename(&img.image);
        let dir = format!("data/photos/{new_play_id}");
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let dest_path = format!("{dir}/{filename}");
        if std::fs::write(&dest_path, &buf).is_err() {
            continue;
        }

        sqlx::query("INSERT INTO play_photos (play_id, file_path, upload_order) VALUES (?, ?, ?)")
            .bind(new_play_id)
            .bind(&dest_path)
            .bind(existing_count as i32)
            .execute(&state.db)
            .await
            .ok();

        summary.photos_copied += 1;
    }

    Ok(summary)
}
