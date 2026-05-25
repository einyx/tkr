-- Initial schema for jkr-server persistence. Tables here back state
-- that previously lived in `Mutex<HashMap>` / `Mutex<VecDeque>` inside
-- AppState. Designed so each table is independently truncatable for
-- maintenance without breaking other features.

-- sessions: Logto-issued session ID → user identity + timestamps.
-- The `data` JSONB column carries the existing SessionState struct
-- verbatim so the Rust refactor doesn't need a per-field schema
-- migration when SessionState evolves.
CREATE TABLE sessions (
    sid           TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL,
    data          JSONB NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX sessions_user_id_idx ON sessions(user_id);
CREATE INDEX sessions_last_seen_at_idx ON sessions(last_seen_at);

-- receipts_queue: LLM-call receipts awaiting drain to the audit
-- aggregator. Drainers claim rows with `FOR UPDATE SKIP LOCKED` to
-- support multiple instances without coordination. Inserts are
-- append-only; the drainer deletes (not soft-deletes) on success.
CREATE TABLE receipts_queue (
    id            BIGSERIAL PRIMARY KEY,
    enqueued_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    receipt       JSONB NOT NULL
);
CREATE INDEX receipts_queue_enqueued_at_idx ON receipts_queue(enqueued_at);

-- llm_recent: rolling audit log of LLM proxy calls. Replaces the
-- in-memory VecDeque ring. Trim policy: keep newest N (enforced by
-- the writer via DELETE after INSERT). Stored fully so the dashboard
-- table can be reconstructed after restart.
CREATE TABLE llm_recent (
    id            BIGSERIAL PRIMARY KEY,
    ts            BIGINT NOT NULL,                -- unix seconds
    provider      TEXT NOT NULL,
    model         TEXT,
    input_tokens  BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    duration_ms   BIGINT NOT NULL DEFAULT 0,
    status        INTEGER NOT NULL,
    extra         JSONB                            -- forward-compat slot
);
CREATE INDEX llm_recent_ts_idx ON llm_recent(ts DESC);

-- sandbox_recent: rolling audit log of sandbox runs. Same shape as
-- the in-memory ring (SandboxLastRun) + a serial PK so eviction can
-- happen by ID without timestamp ties.
CREATE TABLE sandbox_recent (
    id            BIGSERIAL PRIMARY KEY,
    ts            BIGINT NOT NULL,
    command       TEXT NOT NULL,
    exit_code     INTEGER NOT NULL,
    truncated     BOOLEAN NOT NULL DEFAULT FALSE,
    duration_ms   BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX sandbox_recent_ts_idx ON sandbox_recent(ts DESC);
