-- Prevents the same game from being added twice under the same name (case-
-- insensitive), whether via BGG search or manual entry. Safe to add now that
-- the two known pre-existing duplicates (Everdell, Wingspan) have been
-- merged via the admin merge tool.
CREATE UNIQUE INDEX idx_games_name_unique ON games (name COLLATE NOCASE);
