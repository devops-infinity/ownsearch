CREATE INDEX idx_instances_current_capability
    ON instances(current_capability_id);
CREATE INDEX idx_instances_current_tls
    ON instances(current_tls_observation_id);
CREATE INDEX idx_instance_events_instance_id
    ON instance_events(instance_id, event_id);
CREATE INDEX idx_instance_events_occurred_at
    ON instance_events(occurred_at);
CREATE INDEX idx_probes_started_at
    ON probes(started_at);
CREATE INDEX idx_tls_observations_observed_at
    ON tls_observations(observed_at);
CREATE INDEX idx_capability_snapshots_observed_at
    ON capability_snapshots(observed_at);
CREATE INDEX idx_discovery_runs_started_at
    ON discovery_runs(started_at);
CREATE INDEX idx_fleet_snapshots_generated_at
    ON fleet_snapshots(generated_at);
CREATE INDEX idx_instances_kind
    ON instances(instance_kind, instance_id);

DROP VIEW IF EXISTS active_fleet;
DROP VIEW IF EXISTS scan_due;
