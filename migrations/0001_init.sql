-- Board Game Tracker — Database Schema (SQLite)
-- Each table has comments explaining the "why", not just the "what",
-- since this is a learning project.

-- ============================================================
-- USERS & AUTH
-- ============================================================

CREATE TABLE users (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    username        TEXT NOT NULL UNIQUE,
    password_hash   TEXT NOT NULL,          -- bcrypt/argon2 hash, never plain text
    is_admin        INTEGER NOT NULL DEFAULT 0,  -- 0 = standard user, 1 = admin/owner
    is_active       INTEGER NOT NULL DEFAULT 1,  -- soft-delete flag: 0 = removed by admin, blocks login
    must_change_password INTEGER NOT NULL DEFAULT 0,  -- forces a password change before anything else on next login
    totp_secret     TEXT,                   -- set once user enables 2FA
    totp_enabled    INTEGER NOT NULL DEFAULT 0,
    bgg_username    TEXT,                   -- for optional BGG sync
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- ============================================================
-- GAMES
-- ============================================================

CREATE TABLE games (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    bgg_id          INTEGER UNIQUE,          -- BoardGameGeek's ID, used to re-fetch metadata
    name            TEXT NOT NULL,
    year_published  INTEGER,
    min_players     INTEGER,
    max_players     INTEGER,
    min_playtime    INTEGER,                 -- minutes
    max_playtime    INTEGER,
    min_age         INTEGER,
    designers       TEXT,
    artists         TEXT,
    thumbnail_url   TEXT,
    image_url       TEXT,
    average_rating  REAL,
    weight          REAL,                    -- BGG's "complexity" score
    is_expansion    INTEGER NOT NULL DEFAULT 0,
    base_game_id    INTEGER REFERENCES games(id),  -- set if is_expansion = 1
    notes           TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Per-user collection status. A game can be "owned" by one user and
-- "wishlist" for another — this table is what makes that possible.
CREATE TABLE game_status (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL REFERENCES users(id),
    game_id         INTEGER NOT NULL REFERENCES games(id),
    status          TEXT NOT NULL CHECK (status IN
                        ('owned','want_to_buy','wishlist','preordered','for_sale')),
    wishlist_priority INTEGER,
    my_rating       REAL,
    added_date      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, game_id, status)
);

-- ============================================================
-- PLAYERS (people who can appear in a play — may or may not be a registered user)
-- ============================================================

CREATE TABLE players (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER REFERENCES users(id),  -- NULL if this is just a guest name
    name            TEXT NOT NULL,
    bgg_username    TEXT,
    color           TEXT,
    photo_path      TEXT,                   -- profile photo, stored on disk
    is_anonymous    INTEGER NOT NULL DEFAULT 0
);

-- A registered user must map to exactly one player row, or their plays/stats
-- would silently split across two identities.
CREATE UNIQUE INDEX idx_players_user ON players(user_id) WHERE user_id IS NOT NULL;

-- ============================================================
-- LOCATIONS
-- ============================================================

CREATE TABLE locations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT NOT NULL COLLATE NOCASE,
    color           TEXT,
    UNIQUE (name)
);

-- ============================================================
-- PLAYS
-- ============================================================

CREATE TABLE plays (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id         INTEGER NOT NULL REFERENCES games(id),
    location_id     INTEGER REFERENCES locations(id),
    play_date       TEXT NOT NULL,
    duration_minutes INTEGER,
    notes           TEXT,
    visibility      TEXT NOT NULL DEFAULT 'public' CHECK (visibility IN
                        ('public','linked_only','private')),
    logged_by_user_id INTEGER NOT NULL REFERENCES users(id),
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    last_edited_by  INTEGER REFERENCES users(id)  -- any approved linked user can edit, not just the logger
);

-- Join table: which players were in a play, with their score/winner status.
-- link_status only matters when players.user_id is set (a registered user
-- was tagged) — it tracks whether they've approved being linked to this play.
CREATE TABLE play_players (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    play_id         INTEGER NOT NULL REFERENCES plays(id) ON DELETE CASCADE,
    player_id       INTEGER NOT NULL REFERENCES players(id),
    score           REAL,
    is_winner       INTEGER NOT NULL DEFAULT 0,
    seat_order      INTEGER,
    team            TEXT,
    link_status     TEXT NOT NULL DEFAULT 'none' CHECK (link_status IN
                        ('none','pending','approved','declined')),
    UNIQUE (play_id, player_id)
);

-- Photos attached to a play. Capped at 5 per play, enforced in app code.
CREATE TABLE play_photos (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    play_id         INTEGER NOT NULL REFERENCES plays(id) ON DELETE CASCADE,
    file_path       TEXT NOT NULL,
    upload_order    INTEGER NOT NULL DEFAULT 0
);

-- Which expansions were used in a given play, alongside the base game.
-- games.base_game_id (above) links an expansion to its base game in general;
-- this table records which specific expansions came out for one particular play.
CREATE TABLE play_expansions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    play_id         INTEGER NOT NULL REFERENCES plays(id) ON DELETE CASCADE,
    expansion_game_id INTEGER NOT NULL REFERENCES games(id),
    UNIQUE (play_id, expansion_game_id)
);

-- ============================================================
-- NOTIFICATIONS (e.g. "you were tagged in a play, approve the link?")
-- ============================================================

CREATE TABLE notifications (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL REFERENCES users(id),
    type            TEXT NOT NULL,           -- e.g. 'play_link_request'
    play_id         INTEGER REFERENCES plays(id),
    is_read         INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- ============================================================
-- INDEXES (speed up the queries we'll run most often)
-- ============================================================

CREATE INDEX idx_game_status_user ON game_status(user_id);
CREATE INDEX idx_game_status_game ON game_status(game_id);
CREATE INDEX idx_plays_game ON plays(game_id);
CREATE INDEX idx_plays_date ON plays(play_date);
CREATE INDEX idx_plays_logged_by ON plays(logged_by_user_id);
CREATE INDEX idx_play_players_play ON play_players(play_id);
CREATE INDEX idx_play_players_player ON play_players(player_id);
CREATE INDEX idx_notifications_user ON notifications(user_id, is_read);
CREATE INDEX idx_play_expansions_play ON play_expansions(play_id);
