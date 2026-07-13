use askama::Template;
use axum::Extension;
use axum::extract::State;
use axum::response::{Html, IntoResponse};

use crate::AppState;
use crate::handlers::users::profile_photo_path;
use crate::plays::{INVOLVES_USER, VISIBLE_TO};
use crate::security::{self, CurrentUser};

#[derive(sqlx::FromRow)]
struct RecentPlayRow {
    id: i64,
    game_name: String,
    thumbnail_url: Option<String>,
    play_date: String,
    player_count: i64,
    i_won: bool,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    title: String,
    username: String,
    display_name: String,
    display_initial: String,
    has_photo: bool,
    owned_count: i64,
    total_plays: i64,
    recent_plays: Vec<RecentPlayRow>,
    repeat_play_id: Option<i64>,
    repeat_play_thumbnail: Option<String>,
    wishlist_count: i64,
    want_to_play_count: i64,
    want_to_trade_count: i64,
    win_rate_pct: i64,
    distinct_games: i64,
}

async fn status_count(state: &AppState, user_id: i64, status: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM game_status WHERE user_id = ? AND status = ?")
        .bind(user_id)
        .bind(status)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0)
}

pub async fn home(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
) -> impl IntoResponse {
    let display_name = security::display_name(
        &user.username,
        user.first_name.as_deref(),
        user.last_name.as_deref(),
    );
    let has_photo = std::path::Path::new(&profile_photo_path(user.id)).exists();

    let owned_count = status_count(&state, user.id, "owned").await;
    let wishlist_count = status_count(&state, user.id, "wishlist").await;
    let want_to_play_count = status_count(&state, user.id, "want_to_play").await;
    let want_to_trade_count = status_count(&state, user.id, "want_to_trade").await;

    let recent_sql = format!(
        "SELECT plays.id, games.name AS game_name, games.thumbnail_url, plays.play_date, \
                (SELECT COUNT(*) FROM play_players pp2 WHERE pp2.play_id = plays.id) AS player_count, \
                EXISTS( \
                    SELECT 1 FROM play_players pp3 \
                    JOIN players p3 ON p3.id = pp3.player_id \
                    WHERE pp3.play_id = plays.id AND p3.user_id = ? AND pp3.is_winner = 1 \
                ) AS i_won \
         FROM plays \
         JOIN games ON games.id = plays.game_id \
         WHERE {VISIBLE_TO} AND {INVOLVES_USER} \
         ORDER BY plays.play_date DESC, plays.id DESC \
         LIMIT 8"
    );
    let recent_plays: Vec<RecentPlayRow> = sqlx::query_as(&recent_sql)
        .bind(user.id)
        .bind(user.id)
        .bind(user.id)
        .bind(user.id)
        .bind(user.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let repeat_play_id = recent_plays.first().map(|p| p.id);
    let repeat_play_thumbnail = recent_plays.first().and_then(|p| p.thumbnail_url.clone());

    let total_plays_sql =
        format!("SELECT COUNT(*) FROM plays WHERE {VISIBLE_TO} AND {INVOLVES_USER}");
    let total_plays: i64 = sqlx::query_scalar(&total_plays_sql)
        .bind(user.id)
        .bind(user.id)
        .bind(user.id)
        .bind(user.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let my_player_id: Option<i64> = sqlx::query_scalar("SELECT id FROM players WHERE user_id = ?")
        .bind(user.id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let (win_rate_pct, distinct_games) = if let Some(player_id) = my_player_id {
        let stats_sql = format!(
            "SELECT COUNT(DISTINCT games.id) AS distinct_games, \
                    SUM(CASE WHEN pp.is_winner THEN 1 ELSE 0 END) AS win_count, \
                    COUNT(*) AS play_count \
             FROM play_players pp \
             JOIN players p ON p.id = pp.player_id \
             JOIN plays ON plays.id = pp.play_id \
             JOIN games ON games.id = plays.game_id \
             WHERE p.id = ? AND (pp.link_status = 'approved' OR pp.link_status = 'none') AND {VISIBLE_TO}"
        );
        let row: (i64, i64, i64) = sqlx::query_as(&stats_sql)
            .bind(player_id)
            .bind(user.id)
            .bind(user.id)
            .fetch_one(&state.db)
            .await
            .unwrap_or((0, 0, 0));
        let (distinct_games, win_count, play_count) = row;
        let win_rate_pct = if play_count > 0 {
            (win_count * 100) / play_count
        } else {
            0
        };
        (win_rate_pct, distinct_games)
    } else {
        (0, 0)
    };

    let display_initial = display_name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());

    Html(
        DashboardTemplate {
            title: "Home".to_string(),
            username: user.username,
            display_name,
            display_initial,
            has_photo,
            owned_count,
            total_plays,
            recent_plays,
            repeat_play_id,
            repeat_play_thumbnail,
            wishlist_count,
            want_to_play_count,
            want_to_trade_count,
            win_rate_pct,
            distinct_games,
        }
        .render()
        .unwrap(),
    )
}
