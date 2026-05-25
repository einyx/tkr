-- Per-user scoping for the captured-bodies panel. Before this, every
-- proxied call landed in one global ring readable by anyone hitting
-- GET /api/v1/llm/captured. `owner` carries the jkr user_id resolved
-- from the caller's CLI token (x-jkr-token header → cli_tokens lookup);
-- the dashboard now filters reads to the logged-in session's user.
--
-- Empty string is the "unattributed" bucket: calls proxied without a
-- jkr token (e.g. a raw curl, or a sandbox launched before `jkr login`).
-- Legacy rows captured before this migration default to '' and are
-- therefore visible only via the unattributed view, never leaked into a
-- real user's scope.
ALTER TABLE captured_calls ADD COLUMN owner TEXT NOT NULL DEFAULT '';

-- The dashboard read is `WHERE owner = $1 ORDER BY id DESC LIMIT $2`,
-- so a composite (owner, id) index serves it without a separate sort.
CREATE INDEX captured_calls_owner_id_idx ON captured_calls (owner, id DESC);
