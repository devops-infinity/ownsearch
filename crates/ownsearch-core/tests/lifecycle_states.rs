#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use ownsearch_core::{
    InstanceStatus, InstanceType, OperatorAction, ScanUpdate, Store, now_rfc3339,
};

const ACTIONS: [OperatorAction; 5] = [
    OperatorAction::Deny,
    OperatorAction::Allow,
    OperatorAction::Retire,
    OperatorAction::Restore,
    OperatorAction::Reset,
];

fn seed(status: InstanceStatus, scan_enabled: bool, kind: InstanceType) -> (Store, i64) {
    let store = Store::open_in_memory().expect("store");
    let instance_id = store
        .add_manual_instance("https://example.org/", false, &now_rfc3339())
        .expect("instance");
    store
        .apply_scan_update(&ScanUpdate {
            instance_id,
            status,
            previous_status: InstanceStatus::Candidate,
            successes: 0,
            failures: 0,
            score: 50.0,
            rank: None,
            hard_gate_passed: true,
            reasons: vec!["test.seed".to_owned()],
            next_scan_at: Some(now_rfc3339()),
            cooldown_until: None,
            last_success_at: None,
            last_failure_at: None,
            version: None,
            capability_id: None,
            tls_id: None,
        })
        .expect("seed status");
    if !scan_enabled {
        store
            .upsert_override(
                instance_id,
                &serde_json::json!({"scanEnabled": false}),
                &["test.policy".to_owned()],
            )
            .expect("override");
    }
    if kind != InstanceType::Normal {
        store
            .apply_policy_change(
                instance_id,
                None,
                Some(kind),
                None,
                &["test.type".to_owned()],
            )
            .expect("type");
    }
    (store, instance_id)
}

fn scannable(store: &Store, instance_id: i64) -> bool {
    let row = store
        .get_instance(instance_id)
        .expect("row")
        .expect("instance");
    !row.denied
        && row.scan_enabled
        && row.status != InstanceStatus::Retired
        && row.kind != InstanceType::Tor
}

#[test]
fn the_status_registry_has_no_duplicate_spelling() {
    let mut names: Vec<&str> = InstanceStatus::ALL
        .iter()
        .map(|value| value.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), InstanceStatus::ALL.len());
    for status in InstanceStatus::ALL {
        assert_eq!(InstanceStatus::parse(status.as_str()), Some(status));
    }
}

#[test]
fn every_operator_action_leaves_a_state_a_person_can_leave() {
    for kind in [InstanceType::Normal, InstanceType::Tor] {
        for status in InstanceStatus::ALL {
            for denied in [false, true] {
                for scan_enabled in [false, true] {
                    for action in ACTIONS {
                        let (store, instance_id) = seed(status, scan_enabled, kind);
                        if denied {
                            store
                                .apply_operator_action(
                                    instance_id,
                                    OperatorAction::Deny,
                                    &["test.deny".to_owned()],
                                )
                                .expect("deny");
                        }
                        let outcome = store.apply_operator_action(
                            instance_id,
                            action,
                            &["test.action".to_owned()],
                        );
                        if kind == InstanceType::Tor
                            && matches!(action, OperatorAction::Restore | OperatorAction::Reset)
                        {
                            let error = outcome
                                .expect_err("a Tor row must refuse to be restored into service");
                            assert!(
                                error.to_string().contains("set-type"),
                                "the refusal must name the type change: {error}"
                            );
                            continue;
                        }
                        let next = match outcome {
                            Ok(next) => next,
                            Err(error) => panic!(
                                "{action:?} from {status:?} (denied {denied}, scan {scan_enabled}, kind {kind:?}) failed: {error}"
                            ),
                        };
                        let row = store
                            .get_instance(instance_id)
                            .expect("row")
                            .expect("instance");
                        if kind == InstanceType::Tor {
                            let expected = if action == OperatorAction::Retire {
                                InstanceStatus::Retired
                            } else {
                                InstanceStatus::Excluded
                            };
                            assert_eq!(next, expected, "{action:?} on a Tor row");
                        } else {
                            match action {
                                OperatorAction::Deny => {
                                    assert_eq!(next, InstanceStatus::Quarantined);
                                    assert!(row.denied, "deny must record the denial");
                                    assert!(!row.scan_enabled, "deny must stop the scanner");
                                    assert!(row.next_scan_at.is_none());
                                }
                                OperatorAction::Allow => {
                                    assert!(!row.denied, "allow must clear the denial");
                                    assert!(row.scan_enabled, "allow must re-enable scanning");
                                }
                                OperatorAction::Restore => {
                                    assert!(!row.denied, "restore must clear the denial");
                                    assert!(row.scan_enabled, "restore must re-enable scanning");
                                    assert!(
                                        matches!(
                                            next,
                                            InstanceStatus::Candidate
                                                | InstanceStatus::Active
                                                | InstanceStatus::Probation
                                        ),
                                        "restore left {next:?}"
                                    );
                                }
                                OperatorAction::Reset => {
                                    assert_eq!(next, InstanceStatus::Candidate);
                                    assert!(!row.denied);
                                    assert!(row.scan_enabled);
                                }
                                OperatorAction::Retire => {
                                    assert_eq!(next, InstanceStatus::Retired);
                                    assert!(row.next_scan_at.is_none());
                                }
                            }
                        }
                        if !scannable(&store, instance_id) && kind == InstanceType::Normal {
                            store
                                .apply_operator_action(
                                    instance_id,
                                    OperatorAction::Restore,
                                    &["test.reentry".to_owned()],
                                )
                                .expect("restore");
                            assert!(
                                scannable(&store, instance_id),
                                "after {action:?} from {status:?} (denied {denied}, scan {scan_enabled}) the restore control left no next step"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_tor_row_leaves_excluded_only_through_a_type_change() {
    let (store, instance_id) = seed(InstanceStatus::Candidate, true, InstanceType::Tor);
    assert_eq!(
        store
            .get_instance(instance_id)
            .expect("row")
            .expect("instance")
            .status,
        InstanceStatus::Excluded
    );
    store
        .apply_policy_change(
            instance_id,
            None,
            Some(InstanceType::Normal),
            None,
            &["test.type".to_owned()],
        )
        .expect("type change");
    let row = store
        .get_instance(instance_id)
        .expect("row")
        .expect("instance");
    assert_eq!(row.status, InstanceStatus::Candidate);
    assert!(scannable(&store, instance_id));
}

#[test]
fn a_hand_added_instance_goes_unlisted_and_returns_when_announced_again() {
    let store = Store::open_in_memory().expect("store");
    let instance_id = store
        .add_manual_instance("https://manual.example.org/", false, &now_rfc3339())
        .expect("instance");
    store
        .ensure_source("searx_space", "searx.space", "https://searx.space/")
        .expect("source");
    let run_id = store.begin_run("searx_space", "test").expect("run");
    let moved = store
        .mark_missing(&[], &run_id, &now_rfc3339())
        .expect("mark missing");
    assert_eq!(moved, 1);
    assert_eq!(
        store
            .get_instance(instance_id)
            .expect("row")
            .expect("instance")
            .status,
        InstanceStatus::Unlisted,
        "a hand-added instance must not be marked missing"
    );
}

#[test]
fn the_fleet_excludes_an_active_instance_that_failed_its_gate() {
    let (store, instance_id) = seed(InstanceStatus::Active, true, InstanceType::Normal);
    assert_eq!(store.fleet(10, &now_rfc3339()).expect("fleet").len(), 1);
    store
        .apply_scan_update(&ScanUpdate {
            instance_id,
            status: InstanceStatus::Active,
            previous_status: InstanceStatus::Active,
            successes: 0,
            failures: 1,
            score: 50.0,
            rank: Some(1),
            hard_gate_passed: false,
            reasons: vec!["gate.failed".to_owned()],
            next_scan_at: Some(now_rfc3339()),
            cooldown_until: None,
            last_success_at: None,
            last_failure_at: None,
            version: None,
            capability_id: None,
            tls_id: None,
        })
        .expect("gate failure");
    assert!(
        store.fleet(10, &now_rfc3339()).expect("fleet").is_empty(),
        "an instance that failed its hard gate must not answer searches"
    );
}

#[test]
fn making_an_instance_active_requires_a_passing_gate() {
    let (store, instance_id) = seed(InstanceStatus::Candidate, true, InstanceType::Normal);
    store
        .apply_scan_update(&ScanUpdate {
            instance_id,
            status: InstanceStatus::Candidate,
            previous_status: InstanceStatus::Candidate,
            successes: 0,
            failures: 0,
            score: 0.0,
            rank: None,
            hard_gate_passed: false,
            reasons: vec!["gate.failed".to_owned()],
            next_scan_at: Some(now_rfc3339()),
            cooldown_until: None,
            last_success_at: None,
            last_failure_at: None,
            version: None,
            capability_id: None,
            tls_id: None,
        })
        .expect("gate failure");
    assert!(!store.hard_gate_passed(instance_id).expect("gate"));
    let refused = store.set_status(instance_id, InstanceStatus::Active, &["test".to_owned()]);
    assert!(
        refused.is_err(),
        "a failed gate must refuse the active move"
    );

    store
        .apply_scan_update(&ScanUpdate {
            instance_id,
            status: InstanceStatus::Candidate,
            previous_status: InstanceStatus::Candidate,
            successes: 1,
            failures: 0,
            score: 60.0,
            rank: None,
            hard_gate_passed: true,
            reasons: vec!["gate.passed".to_owned()],
            next_scan_at: Some(now_rfc3339()),
            cooldown_until: None,
            last_success_at: None,
            last_failure_at: None,
            version: None,
            capability_id: None,
            tls_id: None,
        })
        .expect("gate pass");
    store
        .set_status(instance_id, InstanceStatus::Active, &["test".to_owned()])
        .expect("a passing gate allows the active move");
    assert_eq!(
        store
            .get_instance(instance_id)
            .expect("row")
            .expect("instance")
            .status,
        InstanceStatus::Active
    );
}

#[test]
fn denying_and_allowing_round_trips_without_losing_the_scan_slot() {
    let (store, instance_id) = seed(InstanceStatus::Active, true, InstanceType::Normal);
    store
        .apply_operator_action(instance_id, OperatorAction::Deny, &["test.deny".to_owned()])
        .expect("deny");
    assert!(!scannable(&store, instance_id));
    let next = store
        .apply_operator_action(
            instance_id,
            OperatorAction::Allow,
            &["test.allow".to_owned()],
        )
        .expect("allow");
    assert_eq!(next, InstanceStatus::Candidate);
    assert!(scannable(&store, instance_id));
}

#[test]
fn restoring_a_candidate_twice_is_a_no_op_not_a_refusal() {
    let (store, instance_id) = seed(InstanceStatus::Candidate, true, InstanceType::Normal);
    let first = store
        .apply_operator_action(instance_id, OperatorAction::Restore, &["test".to_owned()])
        .expect("restore");
    let second = store
        .apply_operator_action(instance_id, OperatorAction::Restore, &["test".to_owned()])
        .expect("a second restore must not refuse");
    assert_eq!(first, InstanceStatus::Candidate);
    assert_eq!(second, InstanceStatus::Candidate);
}
