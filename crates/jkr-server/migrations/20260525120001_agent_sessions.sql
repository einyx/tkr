-- Per-user agent registry: same name -> new version row, old versions retained.
CREATE TABLE agents (
    owner       TEXT   NOT NULL,
    name        TEXT   NOT NULL,
    version     BIGINT NOT NULL,
    manifest    TEXT   NOT NULL,
    created_at  BIGINT NOT NULL,
    PRIMARY KEY (owner, name, version)
);
CREATE INDEX agents_owner_name_idx ON agents (owner, name, version DESC);

-- One row per agent session. Durable state lets a restart reconcile
-- running containers instead of orphaning them.
CREATE TABLE agent_sessions (
    id            TEXT   PRIMARY KEY,
    owner         TEXT   NOT NULL,
    agent_name    TEXT   NOT NULL,
    agent_version BIGINT NOT NULL,
    state         TEXT   NOT NULL,
    container_id  TEXT   NOT NULL DEFAULT '',
    exit_code     INT,
    created_at    BIGINT NOT NULL,
    ended_at      BIGINT
);
CREATE INDEX agent_sessions_owner_idx ON agent_sessions (owner, created_at DESC);

-- Persisted event log enables replay-then-follow and detached runs.
CREATE TABLE agent_session_events (
    session_id  TEXT   NOT NULL REFERENCES agent_sessions(id),
    seq         BIGINT NOT NULL,
    payload     JSONB  NOT NULL,
    created_at  BIGINT NOT NULL,
    PRIMARY KEY (session_id, seq)
);
