use crate::types::InstanceStatus;

pub(crate) const SCORE_MAX: f64 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Policy {
    pub cert_min_days: i64,
    pub google_required: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Evidence {
    pub trusted_local: bool,
    pub tls_ok: bool,
    pub certificate_days: Option<i64>,
    pub analytics: bool,
    pub config_ok: Option<bool>,
    pub json_ok: Option<bool>,
    pub html_ok: Option<bool>,
    pub google_ok: Option<bool>,
    pub uptime_month: Option<f64>,
    pub uptime_year: Option<f64>,
    pub search_success: Option<f64>,
    pub search_median_secs: Option<f64>,
    pub initial_success: Option<f64>,
    pub initial_secs: Option<f64>,
    pub google_secs: Option<f64>,
    pub tls_grade: Option<String>,
    pub tls_protocol: Option<String>,
    pub version_age_days: Option<i64>,
    pub engine_count: usize,
    pub category_count: usize,
    pub consecutive_successes: u32,
    pub consecutive_failures: u32,
    pub parser_drift: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Assessment {
    pub score: f64,
    pub hard_gate_passed: bool,
    pub reasons: Vec<String>,
}

pub(crate) fn assess(evidence: &Evidence, policy: Policy) -> Assessment {
    let mut reasons: Vec<String> = Vec::new();
    let mut passed = true;
    if !evidence.trusted_local {
        if !evidence.tls_ok {
            passed = false;
            reasons.push("gate.tls_invalid".to_owned());
        }
        match evidence.certificate_days {
            Some(days) if days >= policy.cert_min_days => {}
            Some(_) => {
                passed = false;
                reasons.push("gate.cert_expiring".to_owned());
            }
            None => {
                passed = false;
                reasons.push("gate.cert_unknown".to_owned());
            }
        }
    }
    if evidence.analytics {
        passed = false;
        reasons.push("gate.analytics".to_owned());
    }
    if evidence.config_ok == Some(false) {
        passed = false;
        reasons.push("gate.config_unparseable".to_owned());
    }
    if evidence.json_ok == Some(false) && evidence.html_ok != Some(true) {
        reasons.push("probe.json_denied".to_owned());
    }
    if policy.google_required && evidence.google_ok != Some(true) {
        passed = false;
        reasons.push("gate.google_missing".to_owned());
    }
    if !passed {
        return Assessment {
            score: 0.0,
            hard_gate_passed: false,
            reasons,
        };
    }
    let mut score = 0.0;
    score += 20.0 * ratio(evidence.uptime_month, 100.0);
    score += 10.0 * ratio(evidence.uptime_year, 100.0);
    score += 20.0 * ratio(evidence.search_success, 100.0);
    score += 15.0 * time_score(evidence.search_median_secs, 0.5, 4.0);
    score += 5.0 * ratio(evidence.initial_success, 100.0);
    score += 5.0 * time_score(evidence.initial_secs, 0.05, 1.5);
    score += 10.0 * google_score(evidence);
    score += tls_score(evidence);
    score += version_score(evidence.version_age_days);
    score += modifier(evidence, &mut reasons);
    let score = score.clamp(0.0, SCORE_MAX);
    if score < 20.0 {
        reasons.push("score.low".to_owned());
    }
    if evidence.consecutive_failures > 0 {
        reasons.push("probe.failed".to_owned());
    }
    if evidence.consecutive_successes >= 2 {
        reasons.push("probe.repeated_success".to_owned());
    }
    Assessment {
        score: (score * 100.0).round() / 100.0,
        hard_gate_passed: true,
        reasons,
    }
}

fn ratio(value: Option<f64>, max: f64) -> f64 {
    match value {
        Some(raw) if raw.is_finite() && max > 0.0 => (raw / max).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

fn time_score(value: Option<f64>, ideal: f64, worst: f64) -> f64 {
    match value {
        Some(raw) if raw.is_finite() && worst > ideal => {
            ((worst - raw.clamp(0.0, worst)) / (worst - ideal)).clamp(0.0, 1.0)
        }
        _ => 0.0,
    }
}

fn google_score(evidence: &Evidence) -> f64 {
    match evidence.google_ok {
        Some(true) => {
            let success = evidence.google_ok.map(|_| 1.0).unwrap_or(0.0);
            0.6 * success + 0.4 * time_score(evidence.google_secs, 0.5, 4.0)
        }
        _ => 0.0,
    }
}

fn tls_score(evidence: &Evidence) -> f64 {
    let grade = match evidence.tls_grade.as_deref() {
        Some(grade) if grade.starts_with('A') => 4.0,
        Some("B") => 2.5,
        Some(_) => 1.0,
        None => 0.0,
    };
    let protocol = match evidence.tls_protocol.as_deref() {
        Some(protocol) if protocol.contains("1.3") => 3.0,
        Some(protocol) if protocol.contains("1.2") => 1.5,
        _ => 0.0,
    };
    let lifetime = match evidence.certificate_days {
        Some(days) if days >= 90 => 3.0,
        Some(days) if days >= 30 => 2.0,
        Some(days) if days >= 7 => 1.0,
        _ => 0.0,
    };
    grade + protocol + lifetime
}

fn version_score(age_days: Option<i64>) -> f64 {
    match age_days {
        Some(days) if days <= 30 => 5.0,
        Some(days) if days <= 90 => 3.5,
        Some(days) if days <= 180 => 2.0,
        Some(days) if days <= 365 => 1.0,
        _ => 0.0,
    }
}

fn modifier(evidence: &Evidence, reasons: &mut Vec<String>) -> f64 {
    let mut total = 0.0;
    total += 2.0 * (evidence.engine_count.min(10) as f64) / 10.0;
    total += 1.0 * (evidence.category_count.min(10) as f64) / 10.0;
    total += 0.5 * f64::from(evidence.consecutive_successes.min(3));
    total -= 0.5 * f64::from(evidence.consecutive_failures.min(3));
    if evidence.parser_drift {
        total -= 2.0;
        reasons.push("parser.drift".to_owned());
    }
    total.clamp(-5.0, 5.0)
}

pub(crate) fn version_age_days(version: &str, now: i64) -> Option<i64> {
    let date = version.split('+').next()?;
    let mut parts = date.split('.');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(2000..=2100).contains(&year) {
        return None;
    }
    let stamp = crate::time::days_from_civil(year, month, day) * 86_400;
    Some(crate::time::days_until(now, stamp))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HysteresisInput {
    pub current: InstanceStatus,
    pub successes: u32,
    pub failures: u32,
    pub promotion_ready: bool,
    pub gate_failed: bool,
    pub rate_limited: bool,
    pub blocked: bool,
    pub denied: bool,
    pub promote_successes: u32,
    pub probation_failures: u32,
    pub quarantine_failures: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Transition {
    pub status: InstanceStatus,
    pub reasons: Vec<String>,
}

pub(crate) fn next_status(input: HysteresisInput) -> Transition {
    let mut reasons: Vec<String> = Vec::new();
    if input.denied {
        reasons.push("policy.denied".to_owned());
        return Transition {
            status: InstanceStatus::Quarantined,
            reasons,
        };
    }
    if input.rate_limited && input.current == InstanceStatus::Quarantined {
        reasons.push("probe.rate_limited".to_owned());
        return Transition {
            status: InstanceStatus::Quarantined,
            reasons,
        };
    }
    if input.rate_limited {
        reasons.push("probe.rate_limited".to_owned());
        reasons.push("cooldown.rate_limited".to_owned());
        return Transition {
            status: input.current,
            reasons,
        };
    }
    if input.blocked {
        reasons.push("probe.blocked".to_owned());
        return Transition {
            status: InstanceStatus::Quarantined,
            reasons,
        };
    }
    if input.current == InstanceStatus::Retired {
        reasons.push("status.retired".to_owned());
        return Transition {
            status: InstanceStatus::Retired,
            reasons,
        };
    }
    if input.gate_failed {
        reasons.push("gate.failed".to_owned());
        let status = if input.failures >= input.quarantine_failures.saturating_mul(2) {
            InstanceStatus::Quarantined
        } else {
            InstanceStatus::Probation
        };
        if status == InstanceStatus::Quarantined {
            reasons.push("hysteresis.quarantine".to_owned());
        }
        return Transition { status, reasons };
    }
    if input.failures >= input.quarantine_failures {
        reasons.push("hysteresis.quarantine".to_owned());
        return Transition {
            status: InstanceStatus::Quarantined,
            reasons,
        };
    }
    if input.failures >= input.probation_failures {
        reasons.push("hysteresis.demote".to_owned());
        return Transition {
            status: InstanceStatus::Probation,
            reasons,
        };
    }
    if input.current == InstanceStatus::Missing && input.failures == 0 {
        reasons.push("hysteresis.recovered".to_owned());
        return Transition {
            status: InstanceStatus::Probation,
            reasons,
        };
    }
    if input.promotion_ready && input.successes >= input.promote_successes {
        reasons.push("hysteresis.promote".to_owned());
        return Transition {
            status: InstanceStatus::Active,
            reasons,
        };
    }
    Transition {
        status: input.current,
        reasons,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FleetCandidate {
    pub instance_id: i64,
    pub score: f64,
    pub network_group: Option<String>,
    pub google_exact: bool,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FleetSelection {
    pub ranks: Vec<(i64, i64)>,
    pub excluded_network: usize,
    pub excluded_google: usize,
    pub short_of_minimum: bool,
}

pub(crate) fn select_fleet(
    candidates: &[FleetCandidate],
    min: usize,
    max: usize,
    per_network: usize,
    google_required: bool,
) -> FleetSelection {
    let mut sorted: Vec<&FleetCandidate> = candidates.iter().collect();
    sorted.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });
    let mut per_network_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut ranks = Vec::new();
    let mut excluded_network = 0;
    let mut excluded_google = 0;
    for candidate in sorted {
        if ranks.len() >= max {
            break;
        }
        if !candidate.pinned && google_required && !candidate.google_exact {
            excluded_google += 1;
            continue;
        }
        if let Some(group) = &candidate.network_group {
            let count = per_network_counts.get(group).copied().unwrap_or(0);
            if !candidate.pinned && count >= per_network {
                excluded_network += 1;
                continue;
            }
            per_network_counts.insert(group.clone(), count.saturating_add(1));
        }
        let rank = i64::try_from(ranks.len()).unwrap_or(0).saturating_add(1);
        ranks.push((candidate.instance_id, rank));
    }
    FleetSelection {
        short_of_minimum: ranks.len() < min,
        excluded_network,
        excluded_google,
        ranks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strong() -> Evidence {
        Evidence {
            trusted_local: false,
            tls_ok: true,
            certificate_days: Some(80),
            analytics: false,
            config_ok: Some(true),
            json_ok: Some(true),
            html_ok: None,
            google_ok: Some(true),
            uptime_month: Some(100.0),
            uptime_year: Some(99.5),
            search_success: Some(100.0),
            search_median_secs: Some(0.6),
            initial_success: Some(100.0),
            initial_secs: Some(0.2),
            google_secs: Some(0.9),
            tls_grade: Some("A+".to_owned()),
            tls_protocol: Some("TLSv1.3".to_owned()),
            version_age_days: Some(4),
            engine_count: 8,
            category_count: 6,
            consecutive_successes: 2,
            consecutive_failures: 0,
            parser_drift: false,
        }
    }

    fn policy() -> Policy {
        Policy {
            cert_min_days: 14,
            google_required: false,
        }
    }

    #[test]
    fn a_strong_instance_scores_high_and_passes_every_gate() {
        let assessment = assess(&strong(), policy());
        assert!(assessment.hard_gate_passed);
        assert!(assessment.score > 80.0, "score {}", assessment.score);
    }

    #[test]
    fn a_certificate_expiring_inside_the_window_cannot_become_active() {
        let mut evidence = strong();
        evidence.certificate_days = Some(5);
        let assessment = assess(&evidence, policy());
        assert!(!assessment.hard_gate_passed);
        assert!(
            assessment
                .reasons
                .contains(&"gate.cert_expiring".to_owned())
        );
    }

    #[test]
    fn analytics_and_a_broken_config_are_hard_gates() {
        let mut evidence = strong();
        evidence.analytics = true;
        evidence.config_ok = Some(false);
        let assessment = assess(&evidence, policy());
        assert!(!assessment.hard_gate_passed);
        assert!(assessment.reasons.contains(&"gate.analytics".to_owned()));
        assert!(
            assessment
                .reasons
                .contains(&"gate.config_unparseable".to_owned())
        );
    }

    #[test]
    fn a_config_probe_with_no_answer_is_not_a_hard_gate() {
        let mut evidence = strong();
        evidence.config_ok = None;
        let assessment = assess(&evidence, policy());
        assert!(
            assessment.hard_gate_passed,
            "a refused or blocked config probe leaves no verdict"
        );
        assert!(
            !assessment
                .reasons
                .contains(&"gate.config_unparseable".to_owned())
        );
    }

    #[test]
    fn google_required_turns_missing_google_into_a_gate() {
        let mut evidence = strong();
        evidence.google_ok = Some(false);
        let strict = assess(
            &evidence,
            Policy {
                cert_min_days: 14,
                google_required: true,
            },
        );
        assert!(!strict.hard_gate_passed);
        let relaxed = assess(&evidence, policy());
        assert!(relaxed.hard_gate_passed);
    }

    #[test]
    fn a_slow_instance_scores_below_a_fast_one() {
        let mut slow = strong();
        slow.search_median_secs = Some(3.9);
        let fast = assess(&strong(), policy());
        let slow = assess(&slow, policy());
        assert!(slow.score < fast.score);
    }

    #[test]
    fn a_version_string_yields_an_age() {
        let now = crate::time::days_from_civil(2026, 10, 26) * 86_400;
        assert_eq!(version_age_days("2026.10.21+dedeb8f7d", now), Some(5));
        assert_eq!(version_age_days("not-a-version", now), None);
    }

    fn candidate_input(successes: u32) -> HysteresisInput {
        HysteresisInput {
            current: InstanceStatus::Candidate,
            successes,
            failures: 0,
            promotion_ready: true,
            gate_failed: false,
            rate_limited: false,
            blocked: false,
            denied: false,
            promote_successes: 2,
            probation_failures: 2,
            quarantine_failures: 4,
        }
    }

    #[test]
    fn a_failing_instance_is_promoted_only_after_repeated_successes() {
        assert_eq!(
            next_status(candidate_input(1)).status,
            InstanceStatus::Candidate
        );
        assert_eq!(
            next_status(candidate_input(2)).status,
            InstanceStatus::Active
        );
    }

    #[test]
    fn a_challenge_page_quarantines_at_once() {
        let transition = next_status(HysteresisInput {
            blocked: true,
            ..candidate_input(5)
        });
        assert_eq!(transition.status, InstanceStatus::Quarantined);
        assert!(transition.reasons.contains(&"probe.blocked".to_owned()));
    }

    #[test]
    fn one_bad_scan_does_not_remove_an_active_instance() {
        let transition = next_status(HysteresisInput {
            current: InstanceStatus::Active,
            successes: 0,
            failures: 1,
            promotion_ready: false,
            gate_failed: false,
            rate_limited: false,
            blocked: false,
            denied: false,
            promote_successes: 2,
            probation_failures: 2,
            quarantine_failures: 4,
        });
        assert_eq!(transition.status, InstanceStatus::Active);
        let second = next_status(HysteresisInput {
            current: InstanceStatus::Active,
            successes: 0,
            failures: 2,
            promotion_ready: false,
            gate_failed: false,
            rate_limited: false,
            blocked: false,
            denied: false,
            promote_successes: 2,
            probation_failures: 2,
            quarantine_failures: 4,
        });
        assert_eq!(second.status, InstanceStatus::Probation);
        assert!(second.reasons.contains(&"hysteresis.demote".to_owned()));
    }

    #[test]
    fn a_missing_instance_recovers_only_after_a_clean_probe() {
        let recovered = next_status(HysteresisInput {
            current: InstanceStatus::Missing,
            successes: 1,
            failures: 0,
            promotion_ready: true,
            ..candidate_input(1)
        });
        assert_eq!(recovered.status, InstanceStatus::Probation);
        assert!(
            recovered
                .reasons
                .contains(&"hysteresis.recovered".to_owned())
        );
        let still_failing = next_status(HysteresisInput {
            current: InstanceStatus::Missing,
            successes: 0,
            failures: 1,
            promotion_ready: false,
            ..candidate_input(1)
        });
        assert_eq!(still_failing.status, InstanceStatus::Missing);
        assert!(
            !still_failing
                .reasons
                .contains(&"hysteresis.recovered".to_owned())
        );
    }

    #[test]
    fn a_rate_limited_instance_keeps_its_status_and_cools_down() {
        let transition = next_status(HysteresisInput {
            current: InstanceStatus::Active,
            successes: 9,
            failures: 0,
            promotion_ready: true,
            gate_failed: false,
            rate_limited: true,
            blocked: false,
            denied: false,
            promote_successes: 2,
            probation_failures: 2,
            quarantine_failures: 4,
        });
        assert_eq!(transition.status, InstanceStatus::Active);
        assert!(
            transition
                .reasons
                .contains(&"cooldown.rate_limited".to_owned())
        );
    }

    #[test]
    fn the_fleet_obeys_size_and_network_caps() {
        let candidates: Vec<FleetCandidate> = (0..40)
            .map(|index| FleetCandidate {
                instance_id: index,
                score: 100.0 - index as f64,
                network_group: Some(format!("net-{}", index % 12)),
                google_exact: index == 0,
                pinned: false,
            })
            .collect();
        let selection = select_fleet(&candidates, 6, 12, 2, false);
        assert_eq!(selection.ranks.len(), 12);
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (instance_id, _) in &selection.ranks {
            let group = format!("net-{}", *instance_id % 12);
            *counts.entry(group).or_insert(0) += 1;
        }
        assert!(counts.values().all(|count| *count <= 2), "{counts:?}");
        assert!(!selection.short_of_minimum);
    }

    #[test]
    fn a_shortlist_reports_that_it_missed_the_minimum() {
        let selection = select_fleet(
            &[FleetCandidate {
                instance_id: 1,
                score: 50.0,
                network_group: None,
                google_exact: false,
                pinned: false,
            }],
            6,
            12,
            2,
            false,
        );
        assert!(selection.short_of_minimum);
    }

    #[test]
    fn a_google_requirement_excludes_every_other_instance() {
        let candidates = vec![
            FleetCandidate {
                instance_id: 1,
                score: 90.0,
                network_group: None,
                google_exact: false,
                pinned: false,
            },
            FleetCandidate {
                instance_id: 2,
                score: 80.0,
                network_group: None,
                google_exact: true,
                pinned: false,
            },
        ];
        let selection = select_fleet(&candidates, 1, 12, 2, true);
        assert_eq!(selection.ranks, vec![(2, 1)]);
        assert_eq!(selection.excluded_google, 1);
    }

    #[test]
    fn a_pinned_instance_survives_the_caps_and_ranks_first() {
        let candidates = vec![
            FleetCandidate {
                instance_id: 1,
                score: 90.0,
                network_group: Some("a".to_owned()),
                google_exact: false,
                pinned: false,
            },
            FleetCandidate {
                instance_id: 2,
                score: 10.0,
                network_group: Some("a".to_owned()),
                google_exact: false,
                pinned: true,
            },
            FleetCandidate {
                instance_id: 3,
                score: 80.0,
                network_group: Some("a".to_owned()),
                google_exact: false,
                pinned: false,
            },
        ];
        let selection = select_fleet(&candidates, 1, 3, 1, false);
        assert_eq!(selection.ranks, vec![(2, 1)]);
        assert_eq!(selection.excluded_network, 2);
    }

    #[test]
    fn a_pinned_instance_survives_a_google_requirement() {
        let candidates = vec![
            FleetCandidate {
                instance_id: 1,
                score: 10.0,
                network_group: None,
                google_exact: false,
                pinned: true,
            },
            FleetCandidate {
                instance_id: 2,
                score: 90.0,
                network_group: None,
                google_exact: true,
                pinned: false,
            },
        ];
        let selection = select_fleet(&candidates, 1, 12, 2, true);
        assert_eq!(selection.ranks, vec![(1, 1), (2, 2)]);
    }
}
