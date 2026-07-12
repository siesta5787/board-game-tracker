use askama::Template;
use axum::Extension;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse};
use serde::Deserialize;

use crate::AppState;
use crate::plays::VISIBLE_TO;
use crate::security::CurrentUser;

/// A play only counts toward a player's stats if it's a guest link (always
/// counts) or a registered user's *approved* link — pending/declined links
/// don't count, and this stays consistent with what shows in their feed.
const COUNTS_TOWARD_STATS: &str = "(pp.link_status = 'approved' OR pp.link_status = 'none')";

async fn player_id_for_user(state: &AppState, user_id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT id FROM players WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}

#[derive(sqlx::FromRow)]
struct TopGameRow {
    name: String,
    play_count: i64,
}

#[derive(sqlx::FromRow)]
struct LeaderboardRow {
    player_name: String,
    play_count: i64,
    win_count: i64,
}

struct LeaderboardEntry {
    player_name: String,
    play_count: i64,
    win_count: i64,
    win_rate_pct: i64,
}

#[derive(sqlx::FromRow)]
struct ScoreByCountRow {
    player_count: i64,
    avg_score: f64,
    play_count: i64,
}

struct ScoreByCountEntry {
    player_count: i64,
    avg_score: String,
    play_count: i64,
}

#[derive(sqlx::FromRow)]
struct MonthRow {
    month: String,
    play_count: i64,
    win_count: i64,
}

struct MonthEntry {
    label: String,
    play_count: i64,
    win_count: i64,
    bar_pct: i64,
}

#[derive(sqlx::FromRow, Default)]
struct MyTotalsRow {
    total_plays: i64,
    distinct_games: i64,
    win_count: i64,
}

#[derive(Template)]
#[template(path = "stats.html")]
struct StatsTemplate {
    title: String,
    username: String,
    is_admin: bool,
    total_plays: i64,
    distinct_games: i64,
    win_rate_pct: i64,
    top_games: Vec<TopGameRow>,
    leaderboard: Vec<LeaderboardEntry>,
    score_by_count: Vec<ScoreByCountEntry>,
    months: Vec<MonthEntry>,
}

pub async fn show_stats(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    let Some(my_player_id) = player_id_for_user(&state, current.id).await else {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "No player record found for your account.",
        )
            .into_response();
    };

    let totals_sql = format!(
        "SELECT COUNT(*) AS total_plays, COUNT(DISTINCT games.id) AS distinct_games, \
                SUM(CASE WHEN pp.is_winner THEN 1 ELSE 0 END) AS win_count \
         FROM play_players pp \
         JOIN players p ON p.id = pp.player_id \
         JOIN plays ON plays.id = pp.play_id \
         JOIN games ON games.id = plays.game_id \
         WHERE p.id = ? AND {COUNTS_TOWARD_STATS} AND {VISIBLE_TO}"
    );
    let totals = sqlx::query_as::<_, MyTotalsRow>(&totals_sql)
        .bind(my_player_id)
        .bind(current.id)
        .bind(current.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or_default();
    let win_rate_pct = if totals.total_plays > 0 {
        (totals.win_count * 100) / totals.total_plays
    } else {
        0
    };

    let top_games_sql = format!(
        "SELECT games.name, COUNT(*) AS play_count \
         FROM play_players pp \
         JOIN players p ON p.id = pp.player_id \
         JOIN plays ON plays.id = pp.play_id \
         JOIN games ON games.id = plays.game_id \
         WHERE p.id = ? AND {COUNTS_TOWARD_STATS} AND {VISIBLE_TO} \
         GROUP BY games.id \
         ORDER BY play_count DESC, games.name \
         LIMIT 10"
    );
    let top_games = sqlx::query_as::<_, TopGameRow>(&top_games_sql)
        .bind(my_player_id)
        .bind(current.id)
        .bind(current.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let leaderboard_sql = format!(
        "SELECT p.name AS player_name, COUNT(*) AS play_count, \
                SUM(CASE WHEN pp.is_winner THEN 1 ELSE 0 END) AS win_count \
         FROM play_players pp \
         JOIN players p ON p.id = pp.player_id \
         JOIN plays ON plays.id = pp.play_id \
         WHERE {COUNTS_TOWARD_STATS} AND {VISIBLE_TO} \
         GROUP BY p.id \
         ORDER BY (CAST(win_count AS REAL) / play_count) DESC, play_count DESC"
    );
    let leaderboard_rows = sqlx::query_as::<_, LeaderboardRow>(&leaderboard_sql)
        .bind(current.id)
        .bind(current.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let leaderboard = leaderboard_rows
        .into_iter()
        .map(|r| {
            let win_rate_pct = if r.play_count > 0 {
                (r.win_count * 100) / r.play_count
            } else {
                0
            };
            LeaderboardEntry {
                player_name: r.player_name,
                play_count: r.play_count,
                win_count: r.win_count,
                win_rate_pct,
            }
        })
        .collect();

    let score_by_count_sql = format!(
        "WITH my_plays AS ( \
            SELECT pp.play_id, pp.score, \
                   (SELECT COUNT(*) FROM play_players pp2 WHERE pp2.play_id = pp.play_id) AS player_count \
            FROM play_players pp \
            JOIN players p ON p.id = pp.player_id \
            JOIN plays ON plays.id = pp.play_id \
            WHERE p.id = ? AND {COUNTS_TOWARD_STATS} AND {VISIBLE_TO} AND pp.score IS NOT NULL \
         ) \
         SELECT player_count, AVG(score) AS avg_score, COUNT(*) AS play_count \
         FROM my_plays \
         GROUP BY player_count \
         ORDER BY player_count"
    );
    let score_by_count: Vec<ScoreByCountEntry> =
        sqlx::query_as::<_, ScoreByCountRow>(&score_by_count_sql)
            .bind(my_player_id)
            .bind(current.id)
            .bind(current.id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| ScoreByCountEntry {
                player_count: r.player_count,
                avg_score: format!("{:.1}", r.avg_score),
                play_count: r.play_count,
            })
            .collect();

    let months_sql = format!(
        "SELECT strftime('%Y-%m', plays.play_date) AS month, COUNT(*) AS play_count, \
                SUM(CASE WHEN pp.is_winner THEN 1 ELSE 0 END) AS win_count \
         FROM play_players pp \
         JOIN players p ON p.id = pp.player_id \
         JOIN plays ON plays.id = pp.play_id \
         WHERE p.id = ? AND {COUNTS_TOWARD_STATS} AND {VISIBLE_TO} \
         GROUP BY month \
         ORDER BY month DESC \
         LIMIT 12"
    );
    let mut month_rows = sqlx::query_as::<_, MonthRow>(&months_sql)
        .bind(my_player_id)
        .bind(current.id)
        .bind(current.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    month_rows.reverse();
    let max_plays = month_rows
        .iter()
        .map(|m| m.play_count)
        .max()
        .unwrap_or(1)
        .max(1);
    let months = month_rows
        .into_iter()
        .map(|m| {
            let label = chrono::NaiveDate::parse_from_str(&format!("{}-01", m.month), "%Y-%m-%d")
                .map(|d| d.format("%b %Y").to_string())
                .unwrap_or(m.month);
            MonthEntry {
                label,
                play_count: m.play_count,
                win_count: m.win_count,
                bar_pct: (m.play_count * 100) / max_plays,
            }
        })
        .collect();

    Html(
        StatsTemplate {
            title: "Stats".to_string(),
            username: current.username,
            is_admin: current.is_admin,
            total_plays: totals.total_plays,
            distinct_games: totals.distinct_games,
            win_rate_pct,
            top_games,
            leaderboard,
            score_by_count,
            months,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct HeadToHeadQuery {
    player_a: Option<i64>,
    player_b: Option<i64>,
}

#[derive(sqlx::FromRow, Clone)]
struct PlayerOption {
    id: i64,
    name: String,
}

#[derive(sqlx::FromRow)]
struct MatchupRow {
    play_date: String,
    game_name: String,
    score_a: Option<f64>,
    winner_a: bool,
    score_b: Option<f64>,
    winner_b: bool,
}

#[derive(Template)]
#[template(path = "stats_head_to_head.html")]
struct HeadToHeadTemplate {
    title: String,
    username: String,
    is_admin: bool,
    players: Vec<PlayerOption>,
    player_a: Option<i64>,
    player_b: Option<i64>,
    matchups: Vec<MatchupRow>,
    wins_a: i64,
    wins_b: i64,
    ties: i64,
    name_a: String,
    name_b: String,
}

pub async fn head_to_head(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Query(params): Query<HeadToHeadQuery>,
) -> impl IntoResponse {
    let players: Vec<PlayerOption> = sqlx::query_as("SELECT id, name FROM players ORDER BY name")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let mut matchups = Vec::new();
    let mut wins_a = 0i64;
    let mut wins_b = 0i64;
    let mut ties = 0i64;
    let mut name_a = String::new();
    let mut name_b = String::new();

    if let (Some(a), Some(b)) = (params.player_a, params.player_b) {
        if a != b {
            name_a = players
                .iter()
                .find(|p| p.id == a)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            name_b = players
                .iter()
                .find(|p| p.id == b)
                .map(|p| p.name.clone())
                .unwrap_or_default();

            let sql = format!(
                "SELECT plays.play_date, games.name AS game_name, \
                        ppa.score AS score_a, ppa.is_winner AS winner_a, \
                        ppb.score AS score_b, ppb.is_winner AS winner_b \
                 FROM plays \
                 JOIN games ON games.id = plays.game_id \
                 JOIN play_players ppa ON ppa.play_id = plays.id AND ppa.player_id = ? \
                 JOIN play_players ppb ON ppb.play_id = plays.id AND ppb.player_id = ? \
                 WHERE {VISIBLE_TO} \
                   AND ppa.link_status IN ('approved', 'none') \
                   AND ppb.link_status IN ('approved', 'none') \
                 ORDER BY plays.play_date DESC"
            );
            matchups = sqlx::query_as::<_, MatchupRow>(&sql)
                .bind(a)
                .bind(b)
                .bind(current.id)
                .bind(current.id)
                .fetch_all(&state.db)
                .await
                .unwrap_or_default();

            for m in &matchups {
                match (m.winner_a, m.winner_b) {
                    (true, true) | (false, false) => ties += 1,
                    (true, false) => wins_a += 1,
                    (false, true) => wins_b += 1,
                }
            }
        }
    }

    Html(
        HeadToHeadTemplate {
            title: "Head-to-head".to_string(),
            username: current.username,
            is_admin: current.is_admin,
            players,
            player_a: params.player_a,
            player_b: params.player_b,
            matchups,
            wins_a,
            wins_b,
            ties,
            name_a,
            name_b,
        }
        .render()
        .unwrap(),
    )
}
