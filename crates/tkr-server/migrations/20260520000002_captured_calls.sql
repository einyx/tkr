-- Phase-5 follow-up: persist captured request/response bodies so the
-- dashboard's "captured calls" panel survives restart when
-- TKR_CAPTURE_BODIES=true.
--
-- Kept in its own table (not the `extra JSONB` on llm_recent) for two
-- reasons:
--   1. Body fields are large (~64 KiB each post-truncation); putting
--      them on llm_recent would balloon the row scan cost of the
--      dashboard's token-usage table read.
--   2. Captured bodies are an opt-in audit affordance with a different
--      retention story than the always-on llm_recent ring. Separate
--      tables keep TRUNCATE-and-retune decisions independent.

CREATE TABLE captured_calls (
    id            BIGSERIAL PRIMARY KEY,
    ts            BIGINT NOT NULL,
    provider      TEXT NOT NULL,
    model         TEXT NOT NULL,
    status        INTEGER NOT NULL,
    input_tokens  BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    duration_ms   BIGINT NOT NULL DEFAULT 0,
    streaming     BOOLEAN NOT NULL DEFAULT FALSE,
    request       TEXT NOT NULL,
    response      TEXT NOT NULL
);
CREATE INDEX captured_calls_ts_idx ON captured_calls(ts DESC);
