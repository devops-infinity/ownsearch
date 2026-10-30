ALTER TABLE instances ADD COLUMN json_throttled_until TEXT;

ALTER TABLE instances ADD COLUMN manual_priority INTEGER NOT NULL DEFAULT 0;

ALTER TABLE instance_policy_overrides ADD COLUMN is_trusted INTEGER NOT NULL DEFAULT 0;

CREATE TABLE engine_stats (
    instance_id    INTEGER NOT NULL REFERENCES instances(instance_id) ON DELETE RESTRICT,
    engine_name    TEXT NOT NULL,
    success_count  INTEGER NOT NULL DEFAULT 0,
    failure_count  INTEGER NOT NULL DEFAULT 0,
    last_seen_at   TEXT NOT NULL,
    PRIMARY KEY (instance_id, engine_name)
) STRICT;

CREATE INDEX idx_engine_stats_instance
    ON engine_stats(instance_id, engine_name);

CREATE TABLE instance_auth (
    instance_id  INTEGER PRIMARY KEY REFERENCES instances(instance_id) ON DELETE RESTRICT,
    auth_kind    TEXT NOT NULL,
    header_name  TEXT,
    user_name    TEXT,
    secret_env   TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
) STRICT;
