-- Generic instance-wide key/value settings, for things an admin configures
-- per self-hosted install and that must never live in the (public,
-- forkable) codebase itself — e.g. third-party API credentials like a BGG
-- application token. Deliberately just two columns; add rows, not tables,
-- for future instance-level settings unless one outgrows a plain string.
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
