CREATE TABLE hourly_probe_metrics (
    instance_id          INTEGER NOT NULL REFERENCES instances(instance_id) ON DELETE RESTRICT,
    probe_kind           TEXT NOT NULL,
    window_start_utc     TEXT NOT NULL,
    attempt_count        INTEGER NOT NULL,
    success_count        INTEGER NOT NULL,
    failure_count        INTEGER NOT NULL,
    success_rate         REAL NOT NULL,
    duration_last_ms     INTEGER,
    duration_min_ms      INTEGER,
    duration_max_ms      INTEGER,
    total_result_count   INTEGER NOT NULL DEFAULT 0,
    response_bytes       INTEGER NOT NULL DEFAULT 0,
    updated_at           TEXT NOT NULL,
    PRIMARY KEY (instance_id, probe_kind, window_start_utc)
) STRICT;

CREATE INDEX idx_hourly_probe_metrics_window_start
    ON hourly_probe_metrics(window_start_utc);
