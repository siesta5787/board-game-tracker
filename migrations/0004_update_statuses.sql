-- Adds Played / Want to Play / Want to Trade, and removes Want to Buy
-- (redundant with Wishlist). SQLite can't ALTER a CHECK constraint in
-- place, so the table is recreated. Any pre-existing 'want_to_buy' rows are
-- folded into 'wishlist'; INSERT OR IGNORE drops the rare case where a user
-- already has both set for the same game (UNIQUE(user_id, game_id, status)).
CREATE TABLE game_status_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL REFERENCES users(id),
    game_id         INTEGER NOT NULL REFERENCES games(id),
    status          TEXT NOT NULL CHECK (status IN
                        ('owned','wishlist','preordered','for_sale','played','want_to_play','want_to_trade')),
    wishlist_priority INTEGER,
    my_rating       REAL,
    added_date      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, game_id, status)
);

INSERT OR IGNORE INTO game_status_new (user_id, game_id, status, wishlist_priority, my_rating, added_date)
SELECT user_id, game_id,
       CASE WHEN status = 'want_to_buy' THEN 'wishlist' ELSE status END,
       wishlist_priority, my_rating, added_date
FROM game_status;

DROP TABLE game_status;
ALTER TABLE game_status_new RENAME TO game_status;

CREATE INDEX idx_game_status_user ON game_status(user_id);
CREATE INDEX idx_game_status_game ON game_status(game_id);
