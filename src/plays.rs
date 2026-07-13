//! Shared play-related domain logic used by more than one handler.
//!
//! `VISIBLE_TO` in particular exists so there is exactly one place that
//! decides whether a play is visible to a given viewer. Every query that
//! lists or fetches plays (the feed, a single play's detail page, and later
//! any stats) must reuse this fragment rather than re-implementing the
//! Public/Linked-only/Private rule — that's the only way to guarantee a
//! private or linked-only play never leaks through a screen that forgot to
//! filter.

/// Boolean SQL expression deciding whether a row in `plays` is visible to a
/// given viewer. Has two `?` placeholders — bind the viewer's user id to
/// BOTH of them, in order.
///
/// Rules: public plays are visible to everyone; a play is always visible to
/// whoever logged it; a linked-only play is additionally visible to anyone
/// with an *approved* link on it. A private play is visible only to its
/// logger, even if other people are tagged in it.
pub const VISIBLE_TO: &str = "(
    plays.visibility = 'public'
    OR plays.logged_by_user_id = ?
    OR (plays.visibility = 'linked_only' AND EXISTS (
        SELECT 1 FROM play_players pp
        JOIN players p ON p.id = pp.player_id
        WHERE pp.play_id = plays.id AND p.user_id = ? AND pp.link_status = 'approved'
    ))
)";

/// Like `VISIBLE_TO`, but also true if the viewer has a pending OR approved
/// tag on the play — used only for the single-play detail page, so someone
/// asked to approve a link can actually open the play to review it even if
/// its visibility would otherwise hide it from them. Takes THREE `?`
/// placeholders; bind the viewer's user id to all three, in order.
pub const VISIBLE_OR_TAGGED: &str = "(
    (
        plays.visibility = 'public'
        OR plays.logged_by_user_id = ?
        OR (plays.visibility = 'linked_only' AND EXISTS (
            SELECT 1 FROM play_players pp
            JOIN players p ON p.id = pp.player_id
            WHERE pp.play_id = plays.id AND p.user_id = ? AND pp.link_status = 'approved'
        ))
    )
    OR EXISTS (
        SELECT 1 FROM play_players pp2
        JOIN players p2 ON p2.id = pp2.player_id
        WHERE pp2.play_id = plays.id AND p2.user_id = ? AND pp2.link_status IN ('pending', 'approved')
    )
)";

/// True if `subject_user_id` (bind the same id to both `?` placeholders, in
/// order) is either the play's logger or an *approved* linked player on it.
/// Combine with `VISIBLE_TO` (bound with the viewer's id) to build a "plays
/// involving user Y that viewer X can see" query for profile pages — guests
/// never match this since it's keyed on `players.user_id`.
pub const INVOLVES_USER: &str = "(
    plays.logged_by_user_id = ?
    OR EXISTS (
        SELECT 1 FROM play_players pp3
        JOIN players p3 ON p3.id = pp3.player_id
        WHERE pp3.play_id = plays.id AND p3.user_id = ? AND pp3.link_status = 'approved'
    )
)";

/// SQL expression computing a person's display name from a `users` row
/// already joined into the query under the alias `users`: "First Last" if
/// either is set, otherwise their username. No `?` placeholders — usernames
/// stay the login/URL identifier, this is purely for what's shown on screen.
pub const DISPLAY_NAME_SQL: &str = "COALESCE(NULLIF(TRIM(COALESCE(users.first_name, '') || ' ' || COALESCE(users.last_name, '')), ''), users.username)";
