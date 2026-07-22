//! A small client for BoardGameGeek's public XML API.
//!
//! BGG's API is XML (not JSON) and has a history of outages/changes, so this
//! module is written to fail gracefully — a network error or unexpected
//! response just yields an empty result / None, never a panic. The rest of
//! the app must work fully without BGG (manual game entry is always
//! available), per the project brief.

use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use std::time::Duration;

/// Decodes a text node's bytes to UTF-8, then unescapes XML entities
/// (`&amp;`, `&#10;`, etc). `BytesText::decode` only does the former.
fn decode_text(t: &quick_xml::events::BytesText) -> Option<String> {
    let decoded = t.decode().ok()?;
    unescape(&decoded).ok().map(|s| s.into_owned())
}

const USER_AGENT: &str = "board-game-tracker (self-hosted, github.com)";

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub bgg_id: i64,
    pub name: String,
    pub year_published: Option<i32>,
    pub is_expansion: bool,
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GameDetails {
    pub bgg_id: i64,
    pub name: String,
    pub year_published: Option<i32>,
    pub min_players: Option<i32>,
    pub max_players: Option<i32>,
    pub min_playtime: Option<i32>,
    pub max_playtime: Option<i32>,
    pub min_age: Option<i32>,
    pub designers: Option<String>,
    pub artists: Option<String>,
    pub thumbnail_url: Option<String>,
    pub image_url: Option<String>,
    pub average_rating: Option<f64>,
    pub weight: Option<f64>,
    pub description: Option<String>,
    pub is_expansion: bool,
    /// BGG id of the base game, only set when this item is itself an
    /// expansion and BGG told us which game it expands.
    pub base_game_bgg_id: Option<i64>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(10))
        .build()
        .expect("building a basic reqwest client should never fail")
}

/// BGG's XML API requires a Bearer token (an app-registration token, one
/// per self-hosted instance — see `src/settings.rs`). `None` here means no
/// token has been configured yet; callers should check for that case
/// themselves and give a more specific message than a generic HTTP error.
pub async fn search(query: &str, token: Option<&str>) -> Result<Vec<SearchResult>, String> {
    let url = format!(
        "https://boardgamegeek.com/xmlapi2/search?query={}&type=boardgame,boardgameexpansion",
        urlencoding::encode(query)
    );
    let mut request = client().get(&url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("BGG returned HTTP {status}"));
    }
    Ok(parse_search_xml(&body))
}

/// BGG's search endpoint returns no images, only `/thing` does — but `/thing`
/// accepts a comma-separated id list, so a whole page of search results can
/// get their thumbnails in one extra request rather than one per row.
/// BGG hard-caps this at 20 ids per request (confirmed: 21+ returns HTTP 400
/// "Cannot load more than 20 items"), so this cap is load-bearing, not just
/// a politeness margin — raising it breaks the request outright.
const MAX_BATCH_THUMBNAILS: usize = 20;

pub async fn fetch_thumbnails(
    ids: &[i64],
    token: Option<&str>,
) -> Result<Vec<(i64, Option<String>)>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let id_list = ids
        .iter()
        .take(MAX_BATCH_THUMBNAILS)
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let url = format!("https://boardgamegeek.com/xmlapi2/thing?id={id_list}&stats=1");
    let mut request = client().get(&url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("BGG returned HTTP {status}: {body}"));
    }
    Ok(parse_thumbnails_xml(&body))
}

fn parse_thumbnails_xml(xml: &str) -> Vec<(i64, Option<String>)> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut results = Vec::new();

    let mut current_id: Option<i64> = None;
    let mut current_thumbnail: Option<String> = None;
    let mut in_thumbnail = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"item" => {
                    current_id = get_attr(&e, b"id").and_then(|s| s.parse().ok());
                    current_thumbnail = None;
                }
                b"thumbnail" => in_thumbnail = true,
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_thumbnail {
                    current_thumbnail = decode_text(&t);
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"thumbnail" => in_thumbnail = false,
                b"item" => {
                    if let Some(bgg_id) = current_id.take() {
                        results.push((bgg_id, current_thumbnail.take()));
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    results
}

pub async fn fetch_game(bgg_id: i64, token: Option<&str>) -> Result<Option<GameDetails>, String> {
    let url = format!("https://boardgamegeek.com/xmlapi2/thing?id={bgg_id}&stats=1");
    let mut request = client().get(&url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("BGG returned HTTP {status}"));
    }
    let body = response.text().await.map_err(|e| e.to_string())?;
    Ok(parse_thing_xml(&body))
}

fn get_attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn parse_search_xml(xml: &str) -> Vec<SearchResult> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut results = Vec::new();

    let mut current_id: Option<i64> = None;
    let mut current_is_expansion = false;
    let mut current_name: Option<String> = None;
    let mut current_year: Option<i32> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"item" => {
                    current_id = get_attr(&e, b"id").and_then(|s| s.parse().ok());
                    current_is_expansion =
                        get_attr(&e, b"type").as_deref() == Some("boardgameexpansion");
                    current_name = None;
                    current_year = None;
                }
                b"name" => {
                    if get_attr(&e, b"type").as_deref() == Some("primary") {
                        current_name = get_attr(&e, b"value");
                    }
                }
                b"yearpublished" => {
                    current_year = get_attr(&e, b"value").and_then(|s| s.parse().ok());
                }
                _ => {}
            },
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"item" {
                    if let (Some(bgg_id), Some(name)) = (current_id, current_name.take()) {
                        results.push(SearchResult {
                            bgg_id,
                            name,
                            year_published: current_year.take(),
                            is_expansion: current_is_expansion,
                            thumbnail_url: None,
                        });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    results
}

fn parse_thing_xml(xml: &str) -> Option<GameDetails> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut details = GameDetails::default();
    let mut found_item = false;
    let mut designers: Vec<String> = Vec::new();
    let mut artists: Vec<String> = Vec::new();
    let mut in_description = false;
    let mut in_thumbnail = false;
    let mut in_image = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"item" => {
                    found_item = true;
                    details.bgg_id = get_attr(&e, b"id")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    details.is_expansion =
                        get_attr(&e, b"type").as_deref() == Some("boardgameexpansion");
                }
                b"name" => {
                    if get_attr(&e, b"type").as_deref() == Some("primary") {
                        if let Some(v) = get_attr(&e, b"value") {
                            details.name = v;
                        }
                    }
                }
                b"yearpublished" => {
                    details.year_published = get_attr(&e, b"value").and_then(|s| s.parse().ok());
                }
                b"minplayers" => {
                    details.min_players = get_attr(&e, b"value").and_then(|s| s.parse().ok());
                }
                b"maxplayers" => {
                    details.max_players = get_attr(&e, b"value").and_then(|s| s.parse().ok());
                }
                b"minplaytime" => {
                    details.min_playtime = get_attr(&e, b"value").and_then(|s| s.parse().ok());
                }
                b"maxplaytime" => {
                    details.max_playtime = get_attr(&e, b"value").and_then(|s| s.parse().ok());
                }
                b"minage" => {
                    details.min_age = get_attr(&e, b"value").and_then(|s| s.parse().ok());
                }
                b"average" => {
                    details.average_rating = get_attr(&e, b"value").and_then(|s| s.parse().ok());
                }
                b"averageweight" => {
                    details.weight = get_attr(&e, b"value").and_then(|s| s.parse().ok());
                }
                b"link" => {
                    let link_type = get_attr(&e, b"type");
                    let value = get_attr(&e, b"value");
                    match link_type.as_deref() {
                        Some("boardgamedesigner") => {
                            if let Some(v) = value {
                                designers.push(v);
                            }
                        }
                        Some("boardgameartist") => {
                            if let Some(v) = value {
                                artists.push(v);
                            }
                        }
                        Some("boardgameexpansion") => {
                            // "inbound" means this item IS an expansion FOR
                            // the linked game, i.e. that's its base game.
                            if get_attr(&e, b"inbound").as_deref() == Some("true") {
                                details.base_game_bgg_id =
                                    get_attr(&e, b"id").and_then(|s| s.parse().ok());
                            }
                        }
                        _ => {}
                    }
                }
                b"thumbnail" => in_thumbnail = true,
                b"image" => in_image = true,
                b"description" => in_description = true,
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_description {
                    details.description = decode_text(&t);
                } else if in_thumbnail {
                    details.thumbnail_url = decode_text(&t);
                } else if in_image {
                    details.image_url = decode_text(&t);
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"description" => in_description = false,
                b"thumbnail" => in_thumbnail = false,
                b"image" => in_image = false,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    if !found_item {
        return None;
    }
    if !designers.is_empty() {
        details.designers = Some(designers.join(", "));
    }
    if !artists.is_empty() {
        details.artists = Some(artists.join(", "));
    }
    Some(details)
}
