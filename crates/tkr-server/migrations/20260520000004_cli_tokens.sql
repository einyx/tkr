-- Per-user CLI bearer tokens. Minted via `POST /api/v1/auth/cli-token`
-- (Logto-session-gated, shown once, never retrievable). Used by the
-- `tkr` CLI to authenticate `/api/v1/sandbox/ingest` and (future)
-- other CLI-facing endpoints without sharing the shared-secret env
-- token across machines. Each row stores only a SHA-256 hash of the
-- token — DB compromise does not expose live tokens.
CREATE TABLE IF NOT EXISTS cli_tokens (
    id             BIGSERIAL PRIMARY KEY,
    user_id        TEXT NOT NULL,
    user_email     TEXT NOT NULL DEFAULT '',
    token_sha256   TEXT NOT NULL UNIQUE,
    label          TEXT NOT NULL DEFAULT '',
    created_at     BIGINT NOT NULL,
    last_used_at   BIGINT NOT NULL DEFAULT 0,
    revoked_at     BIGINT NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS cli_tokens_user_idx ON cli_tokens (user_id);
CREATE INDEX IF NOT EXISTS cli_tokens_hash_idx ON cli_tokens (token_sha256);
