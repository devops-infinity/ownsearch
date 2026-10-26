CREATE TABLE application_metadata (
    metadata_key    TEXT PRIMARY KEY,
    metadata_value  TEXT NOT NULL
) STRICT;

CREATE TABLE discovery_sources (
    source_id    TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    name         TEXT NOT NULL,
    endpoint     TEXT NOT NULL,
    created_at   TEXT NOT NULL
) STRICT;

CREATE TABLE discovery_runs (
    run_id                    TEXT PRIMARY KEY,
    discovery_source_id       TEXT NOT NULL REFERENCES discovery_sources(source_id) ON DELETE RESTRICT,
    started_at                TEXT NOT NULL,
    completed_at              TEXT,
    outcome                   TEXT NOT NULL,
    source_published_at       TEXT,
    source_digest_sha256      BLOB,
    discovered_count          INTEGER NOT NULL DEFAULT 0,
    changed_count             INTEGER NOT NULL DEFAULT 0,
    missing_count             INTEGER NOT NULL DEFAULT 0,
    scanner_version           TEXT NOT NULL,
    error_code                TEXT,
    error_message             TEXT
) STRICT;

CREATE TABLE instances (
    instance_id                    INTEGER PRIMARY KEY,
    instance_slug                  TEXT NOT NULL UNIQUE,
    normalized_url                 TEXT NOT NULL UNIQUE,
    url_digest_sha256              BLOB NOT NULL UNIQUE,
    host                           TEXT NOT NULL,
    instance_kind                  TEXT NOT NULL,
    lifecycle_status               TEXT NOT NULL,
    discovery_source_id            TEXT NOT NULL REFERENCES discovery_sources(source_id) ON DELETE RESTRICT,
    discovered_at                  TEXT NOT NULL,
    last_announced_at              TEXT,
    last_success_at                TEXT,
    last_failure_at                TEXT,
    next_scan_at                   TEXT,
    consecutive_successes          INTEGER NOT NULL DEFAULT 0,
    consecutive_failures           INTEGER NOT NULL DEFAULT 0,
    cooldown_until                 TEXT,
    searxng_version                TEXT,
    searxng_version_observed_at    TEXT,
    is_scan_enabled                INTEGER NOT NULL DEFAULT 1,
    is_html_fallback_allowed       INTEGER NOT NULL DEFAULT 0,
    discovery_rank                 INTEGER,
    network_group                  TEXT,
    discovery_metrics_json         TEXT,
    current_discovery_run_id       TEXT REFERENCES discovery_runs(run_id) ON DELETE SET NULL,
    current_capability_id          INTEGER REFERENCES capability_snapshots(capability_id) ON DELETE SET NULL,
    current_tls_observation_id     INTEGER REFERENCES tls_observations(tls_observation_id) ON DELETE SET NULL,
    updated_at                     TEXT NOT NULL
) STRICT;

CREATE TABLE capability_snapshots (
    capability_id             INTEGER PRIMARY KEY,
    instance_id               INTEGER NOT NULL REFERENCES instances(instance_id) ON DELETE RESTRICT,
    observed_at               TEXT NOT NULL,
    discovery_run_id          TEXT REFERENCES discovery_runs(run_id) ON DELETE SET NULL,
    outcome                   TEXT NOT NULL,
    http_status               INTEGER,
    duration_ms               INTEGER,
    response_digest_sha256    BLOB,
    instance_name             TEXT,
    categories_json           TEXT NOT NULL,
    engines_json              TEXT NOT NULL,
    locales_json              TEXT NOT NULL,
    plugins_json              TEXT NOT NULL,
    default_safe_search       INTEGER,
    is_json_enabled           INTEGER NOT NULL,
    is_csv_enabled            INTEGER NOT NULL,
    is_rss_enabled            INTEGER NOT NULL,
    is_html_enabled           INTEGER NOT NULL,
    is_google_exact_seen      INTEGER NOT NULL DEFAULT 0,
    is_google_cse_seen        INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE engines (
    engine_id    INTEGER PRIMARY KEY,
    engine_name  TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE categories (
    category_id    INTEGER PRIMARY KEY,
    category_name  TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE capability_engines (
    capability_id    INTEGER NOT NULL REFERENCES capability_snapshots(capability_id) ON DELETE CASCADE,
    engine_id        INTEGER NOT NULL REFERENCES engines(engine_id) ON DELETE RESTRICT,
    PRIMARY KEY (capability_id, engine_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE capability_categories (
    capability_id    INTEGER NOT NULL REFERENCES capability_snapshots(capability_id) ON DELETE CASCADE,
    category_id      INTEGER NOT NULL REFERENCES categories(category_id) ON DELETE RESTRICT,
    PRIMARY KEY (capability_id, category_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE probes (
    probe_id                 INTEGER PRIMARY KEY,
    instance_id              INTEGER NOT NULL REFERENCES instances(instance_id) ON DELETE RESTRICT,
    discovery_run_id         TEXT REFERENCES discovery_runs(run_id) ON DELETE SET NULL,
    probe_kind               TEXT NOT NULL,
    outcome                  TEXT NOT NULL,
    attempt                  INTEGER NOT NULL,
    started_at               TEXT NOT NULL,
    completed_at             TEXT,
    duration_ms              INTEGER,
    http_status              INTEGER,
    content_type             TEXT,
    result_count             INTEGER,
    response_bytes           INTEGER,
    response_digest_sha256   BLOB,
    error_code               TEXT,
    error_message            TEXT
) STRICT;

CREATE TABLE tls_observations (
    tls_observation_id      INTEGER PRIMARY KEY,
    instance_id             INTEGER NOT NULL REFERENCES instances(instance_id) ON DELETE RESTRICT,
    probe_id                INTEGER NOT NULL UNIQUE REFERENCES probes(probe_id) ON DELETE RESTRICT,
    observed_at             TEXT NOT NULL,
    protocol                TEXT,
    grade_source            TEXT,
    grade                   TEXT,
    issuer                  TEXT,
    not_before              TEXT,
    not_after               TEXT,
    fingerprint_sha256      BLOB,
    is_hostname_verified    INTEGER,
    is_chain_valid          INTEGER,
    error_code              TEXT,
    error_message           TEXT
) STRICT;

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

CREATE TABLE instance_quality (
    instance_id             INTEGER PRIMARY KEY REFERENCES instances(instance_id) ON DELETE RESTRICT,
    score_model_version     TEXT NOT NULL,
    score                   REAL NOT NULL,
    fleet_rank              INTEGER,
    is_hard_gate_passed     INTEGER NOT NULL,
    reason_codes_json       TEXT NOT NULL,
    computed_at             TEXT NOT NULL
) STRICT;

CREATE TABLE instance_events (
    event_id             INTEGER PRIMARY KEY,
    occurred_at          TEXT NOT NULL,
    event_kind           TEXT NOT NULL,
    instance_id          INTEGER REFERENCES instances(instance_id) ON DELETE RESTRICT,
    discovery_run_id     TEXT REFERENCES discovery_runs(run_id) ON DELETE SET NULL,
    probe_id             INTEGER REFERENCES probes(probe_id) ON DELETE SET NULL,
    previous_value       TEXT,
    current_value        TEXT,
    reason_codes_json    TEXT NOT NULL,
    payload_json         TEXT
) STRICT;

CREATE TABLE fleet_snapshots (
    snapshot_id             INTEGER PRIMARY KEY,
    generated_at            TEXT NOT NULL,
    snapshot_kind           TEXT NOT NULL,
    payload_format          TEXT NOT NULL,
    instance_count          INTEGER NOT NULL,
    byte_size               INTEGER NOT NULL,
    payload_digest_sha256   BLOB NOT NULL,
    payload_json            TEXT NOT NULL
) STRICT;

CREATE TABLE instance_policy_overrides (
    instance_id                INTEGER PRIMARY KEY REFERENCES instances(instance_id) ON DELETE RESTRICT,
    is_scan_enabled            INTEGER,
    is_html_fallback_allowed   INTEGER,
    is_pinned                  INTEGER NOT NULL DEFAULT 0,
    is_denied                  INTEGER NOT NULL DEFAULT 0,
    reason_codes_json          TEXT NOT NULL,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL
) STRICT;

CREATE TABLE maintenance_runs (
    maintenance_id    INTEGER PRIMARY KEY,
    maintenance_kind  TEXT NOT NULL,
    started_at        TEXT NOT NULL,
    completed_at      TEXT,
    outcome           TEXT NOT NULL,
    detail_json       TEXT NOT NULL
) STRICT;

CREATE INDEX idx_discovery_runs_source_started
    ON discovery_runs(discovery_source_id, started_at);
CREATE INDEX idx_instances_lifecycle_status
    ON instances(lifecycle_status, instance_id);
CREATE INDEX idx_instance_quality_rank
    ON instance_quality(fleet_rank, score);
CREATE INDEX idx_instances_discovery_source
    ON instances(discovery_source_id, instance_id);
CREATE INDEX idx_instances_next_scan
    ON instances(next_scan_at) WHERE is_scan_enabled <> 0;
CREATE INDEX idx_instances_last_announced
    ON instances(last_announced_at);
CREATE INDEX idx_instances_current_run
    ON instances(current_discovery_run_id);
CREATE INDEX idx_capability_snapshots_run
    ON capability_snapshots(discovery_run_id, instance_id);
CREATE INDEX idx_instance_events_run
    ON instance_events(discovery_run_id, occurred_at);
CREATE INDEX idx_instance_events_probe
    ON instance_events(probe_id);
CREATE INDEX idx_capability_snapshots_instance_time
    ON capability_snapshots(instance_id, observed_at);
CREATE INDEX idx_capability_engines_engine
    ON capability_engines(engine_id, capability_id);
CREATE INDEX idx_capability_categories_category
    ON capability_categories(category_id, capability_id);
CREATE INDEX idx_probes_instance_kind_time
    ON probes(instance_id, probe_kind, started_at);
CREATE INDEX idx_probes_discovery_run
    ON probes(discovery_run_id, instance_id);
CREATE INDEX idx_tls_observations_instance_time
    ON tls_observations(instance_id, observed_at);
CREATE INDEX idx_hourly_probe_metrics_window
    ON hourly_probe_metrics(window_start_utc, instance_id, probe_kind);
CREATE INDEX idx_instance_events_instance_time
    ON instance_events(instance_id, occurred_at, event_id);
CREATE INDEX idx_instance_events_kind_time
    ON instance_events(event_kind, occurred_at, event_id);

CREATE VIEW active_fleet AS
SELECT i.instance_id,
       i.normalized_url,
       i.instance_kind,
       i.searxng_version,
       q.score,
       q.fleet_rank,
       c.is_json_enabled,
       c.is_html_enabled,
       c.is_google_exact_seen,
       c.is_google_cse_seen
  FROM instances AS i
  JOIN instance_quality AS q
    ON q.instance_id = i.instance_id
  JOIN capability_snapshots AS c
    ON c.capability_id = i.current_capability_id
  LEFT JOIN instance_policy_overrides AS o
    ON o.instance_id = i.instance_id
 WHERE i.lifecycle_status = 'active'
   AND i.is_scan_enabled <> 0
   AND i.instance_kind <> 'tor'
   AND COALESCE(o.is_denied, 0) = 0
 ORDER BY q.fleet_rank, q.score DESC;

CREATE VIEW scan_due AS
SELECT i.instance_id,
       i.normalized_url,
       i.next_scan_at,
       i.lifecycle_status
  FROM instances AS i
 WHERE i.is_scan_enabled <> 0
   AND i.next_scan_at IS NOT NULL
   AND i.lifecycle_status NOT IN ('retired')
   AND (i.cooldown_until IS NULL OR i.cooldown_until <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
   AND NOT EXISTS (
     SELECT 1 FROM instance_policy_overrides AS o
      WHERE o.instance_id = i.instance_id AND o.is_denied <> 0)
 ORDER BY i.next_scan_at, i.instance_id;
