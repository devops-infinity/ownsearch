use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::config::Settings;
use crate::db::{EventRecord, Store};
use crate::error::{Error, Result};
use crate::response::DbCrudOutput;
use crate::types::{AiDbMode, CrudEntity, CrudOperation, InstanceStatus, InstanceType};

const INSTANCE_FIELDS: [&str; 6] = [
    "status",
    "type",
    "scanEnabled",
    "htmlFallbackAllowed",
    "pinned",
    "denied",
];
const OVERRIDE_FIELDS: [&str; 4] = ["scanEnabled", "htmlFallbackAllowed", "pinned", "denied"];

#[derive(Debug, Clone)]
pub struct CrudRequest {
    pub operation: CrudOperation,
    pub entity: CrudEntity,
    pub instance_id: Option<i64>,
    pub url: Option<String>,
    pub fields: BTreeMap<String, Value>,
    pub reason: Option<String>,
    pub dry_run: bool,
    pub confirmation_token: Option<String>,
}

struct Pending {
    fingerprint: String,
    instance_id: i64,
    entity: CrudEntity,
    operation: CrudOperation,
    fields: Value,
    reason: String,
    expires_at: Instant,
    used: bool,
}

struct CrudState {
    pending: BTreeMap<String, Pending>,
    last_apply: Option<Instant>,
    previews: u64,
    applies: u64,
}

pub struct CrudService {
    mode: AiDbMode,
    max_changes: usize,
    confirm_ttl: Duration,
    min_interval: Duration,
    snapshot_path: std::path::PathBuf,
    store: Arc<Store>,
    state: Mutex<CrudState>,
}

impl CrudService {
    pub fn new(settings: &Settings, store: Arc<Store>) -> Self {
        Self {
            mode: settings.ai_db_mode(),
            max_changes: settings.ai_db_max_changes_per_call(),
            confirm_ttl: Duration::from_secs(settings.ai_db_confirm_ttl_secs()),
            min_interval: Duration::from_secs(settings.ai_db_min_interval_secs()),
            snapshot_path: settings.snapshot_path(),
            store,
            state: Mutex::new(CrudState {
                pending: BTreeMap::new(),
                last_apply: None,
                previews: 0,
                applies: 0,
            }),
        }
    }

    pub fn mode(&self) -> AiDbMode {
        self.mode
    }

    pub fn begin(&self, request: CrudRequest) -> Result<DbCrudOutput> {
        match self.begin_inner(request) {
            Ok(output) => Ok(output),
            Err(error) => {
                let _ = self.store.append_event(&EventRecord {
                    kind: "ai_crud_refused".to_owned(),
                    instance_id: None,
                    run_id: None,
                    probe_id: None,
                    old_value: None,
                    new_value: None,
                    reasons: vec![error.code().to_owned()],
                    payload: None,
                });
                Err(error)
            }
        }
    }

    fn begin_inner(&self, request: CrudRequest) -> Result<DbCrudOutput> {
        if self.mode == AiDbMode::ReadOnly {
            return Err(Error::WriteDisabled);
        }
        let validated = self.validate(&request)?;
        let fingerprint = fingerprint(&validated);
        if request.dry_run {
            return self.preview_change(&validated, fingerprint);
        }
        let token = request
            .confirmation_token
            .as_deref()
            .ok_or(Error::ConfirmationExpired)?;
        let digest = token_digest(token);
        let mut state = self.lock()?;
        prune_expired(&mut state);
        if let Some(last) = state.last_apply
            && last.elapsed() < self.min_interval
        {
            drop(state);
            let _ = self.store.append_event(&EventRecord {
                kind: "ai_crud_refused".to_owned(),
                instance_id: Some(validated.instance_id),
                run_id: None,
                probe_id: None,
                old_value: None,
                new_value: None,
                reasons: vec!["ai.rate_limited".to_owned()],
                payload: Some(json!({"operation": validated.operation.as_str()})),
            });
            return Err(Error::PolicyRefused(format!(
                "another database mutation ran less than {} seconds ago",
                self.min_interval.as_secs()
            )));
        }
        let Some(pending) = state.pending.get(&digest) else {
            drop(state);
            return Err(Error::ConfirmationExpired);
        };
        if pending.used || pending.expires_at <= Instant::now() {
            drop(state);
            return Err(Error::ConfirmationExpired);
        }
        if pending.fingerprint != fingerprint
            || pending.instance_id != validated.instance_id
            || pending.entity != validated.entity
            || pending.operation != validated.operation
        {
            drop(state);
            return Err(Error::ConfirmationExpired);
        }
        let instance_id = validated.instance_id;
        let entity = validated.entity;
        let operation = validated.operation;
        let fields = pending.fields.clone();
        let reason = pending.reason.clone();
        state.applies = state.applies.saturating_add(1);
        if let Some(entry) = state.pending.get_mut(&digest) {
            entry.used = true;
        }
        state.last_apply = Some(Instant::now());
        drop(state);
        self.apply(instance_id, entity, operation, &fields, &reason)?;
        let record = self.store.get_instance(instance_id)?;
        if let Err(error) = self.store.append_event(&EventRecord {
            kind: "ai_crud_apply".to_owned(),
            instance_id: Some(instance_id),
            run_id: None,
            probe_id: None,
            old_value: None,
            new_value: Some(operation.as_str().to_owned()),
            reasons: vec!["ai.applied".to_owned()],
            payload: Some(json!({
                "entity": entity.as_str(),
                "fields": fields,
                "reason": reason,
            })),
        }) {
            tracing::warn!(%error, "the apply event could not be written");
        }
        if let Err(error) = self.store.write_snapshot(&self.snapshot_path) {
            tracing::warn!(%error, "the state snapshot could not be written");
        }
        Ok(DbCrudOutput {
            operation: operation.as_str().to_owned(),
            entity: entity.as_str().to_owned(),
            applied: true,
            dry_run: false,
            confirmation_token: None,
            expires_in_secs: None,
            before: None,
            after: None,
            refused_reasons: Vec::new(),
            record: record.as_ref().map(crate::db::instance_to_json),
        })
    }

    fn preview_change(&self, validated: &Validated, fingerprint: String) -> Result<DbCrudOutput> {
        let preview = self.preview(validated)?;
        let token = new_token()?;
        let digest = token_digest(&token);
        let mut state = self.lock()?;
        prune_expired(&mut state);
        if state.pending.len() >= MAX_PENDING_PREVIEWS {
            return Err(Error::PolicyRefused(format!(
                "{} previews are already waiting for a decision; apply or let them expire before asking for another",
                MAX_PENDING_PREVIEWS
            )));
        }
        state.previews = state.previews.saturating_add(1);
        state.pending.insert(
            digest,
            Pending {
                fingerprint,
                instance_id: validated.instance_id,
                entity: validated.entity,
                operation: validated.operation,
                fields: validated.fields.clone(),
                reason: validated.reason.clone(),
                expires_at: Instant::now() + self.confirm_ttl,
                used: false,
            },
        );
        if let Err(error) = self.store.append_event(&EventRecord {
            kind: "ai_crud_preview".to_owned(),
            instance_id: Some(validated.instance_id),
            run_id: None,
            probe_id: None,
            old_value: None,
            new_value: Some(validated.operation.as_str().to_owned()),
            reasons: vec!["ai.preview".to_owned()],
            payload: Some(json!({
                "entity": validated.entity.as_str(),
                "fields": validated.fields.clone(),
                "reason": validated.reason.clone(),
            })),
        }) {
            tracing::warn!(%error, "the preview event could not be written");
        }
        Ok(DbCrudOutput {
            operation: validated.operation.as_str().to_owned(),
            entity: validated.entity.as_str().to_owned(),
            applied: false,
            dry_run: true,
            confirmation_token: Some(token),
            expires_in_secs: Some(self.confirm_ttl.as_secs()),
            before: preview.before,
            after: preview.after,
            refused_reasons: Vec::new(),
            record: None,
        })
    }

    pub fn counters(&self) -> (u64, u64) {
        match self.state.lock() {
            Ok(state) => (state.previews, state.applies),
            Err(poisoned) => {
                let state = poisoned.into_inner();
                (state.previews, state.applies)
            }
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, CrudState>> {
        self.state
            .lock()
            .map_err(|_| Error::State("the crud lock was poisoned".into()))
    }

    fn validate(&self, request: &CrudRequest) -> Result<Validated> {
        let instance_id = match (request.instance_id, &request.url) {
            (Some(id), _) => id,
            (None, Some(url)) => self
                .store
                .find_instance_by_url(url)?
                .map(|row| row.instance_id)
                .ok_or_else(|| Error::InvalidArgs(format!("no instance matches {url}")))?,
            (None, None) => {
                return Err(Error::InvalidArgs(
                    "name the instance with instance_id or url".into(),
                ));
            }
        };
        if self.store.get_instance(instance_id)?.is_none() {
            return Err(Error::InvalidArgs(format!(
                "instance {instance_id} does not exist"
            )));
        }
        match (request.entity, request.operation) {
            (CrudEntity::PolicyOverride, CrudOperation::Create | CrudOperation::Update)
            | (CrudEntity::PolicyOverride, CrudOperation::Delete)
            | (CrudEntity::Instance, CrudOperation::Update)
            | (CrudEntity::Instance, CrudOperation::Retire | CrudOperation::Restore) => {}
            (entity, operation) => {
                return Err(Error::PolicyRefused(format!(
                    "{} does not accept {}",
                    entity.as_str(),
                    operation.as_str()
                )));
            }
        }
        let allowed: &[&str] = match request.entity {
            CrudEntity::Instance => &INSTANCE_FIELDS,
            CrudEntity::PolicyOverride => &OVERRIDE_FIELDS,
        };
        let mut fields = Map::new();
        for (key, value) in &request.fields {
            if !allowed.contains(&key.as_str()) {
                return Err(Error::PolicyRefused(format!(
                    "{key} is not an allowed field for {}",
                    request.entity.as_str()
                )));
            }
            validate_value(key, value)?;
            fields.insert(key.clone(), value.clone());
        }
        if fields.len() > self.max_changes {
            return Err(Error::PolicyRefused(format!(
                "{} fields changes exceed the {}-change cap",
                fields.len(),
                self.max_changes
            )));
        }
        let needs_fields = matches!(
            request.operation,
            CrudOperation::Create | CrudOperation::Update
        );
        if needs_fields && fields.is_empty() {
            return Err(Error::InvalidArgs(
                "name at least one field to change".into(),
            ));
        }
        let reason = request
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|value| value.chars().count() >= 3)
            .map(|value| crate::sanitize::cap(&crate::sanitize::sanitize(value), 200).0)
            .ok_or_else(|| Error::InvalidArgs("give a reason of at least 3 characters".into()))?;
        Ok(Validated {
            instance_id,
            entity: request.entity,
            operation: request.operation,
            fields: Value::Object(fields),
            reason,
        })
    }

    fn preview(&self, validated: &Validated) -> Result<Preview> {
        let instance = self
            .store
            .get_instance(validated.instance_id)?
            .ok_or_else(|| Error::InvalidArgs("instance missing".into()))?;
        let before = match validated.entity {
            CrudEntity::Instance => crate::db::instance_to_json(&instance),
            CrudEntity::PolicyOverride => self
                .store
                .get_override(validated.instance_id)?
                .unwrap_or(Value::Null),
        };
        let after = match validated.operation {
            CrudOperation::Delete => Value::Null,
            CrudOperation::Retire => {
                let mut preview = before.clone();
                if let Some(object) = preview.as_object_mut() {
                    object.insert("status".to_owned(), json!("retired"));
                }
                preview
            }
            CrudOperation::Restore => {
                let mut preview = before.clone();
                if let Some(object) = preview.as_object_mut() {
                    object.insert("status".to_owned(), json!("candidate"));
                }
                preview
            }
            CrudOperation::Create | CrudOperation::Update => {
                let mut preview = if before.is_null() {
                    json!({"instanceId": validated.instance_id})
                } else {
                    before.clone()
                };
                if let (Some(target), Some(source)) =
                    (preview.as_object_mut(), validated.fields.as_object())
                {
                    for (key, value) in source {
                        target.insert(key.clone(), value.clone());
                    }
                }
                preview
            }
        };
        if validated.entity == CrudEntity::Instance
            && let Some(status) = validated.fields.get("status").and_then(Value::as_str)
            && status == "active"
            && instance.hard_gate_passed != Some(true)
        {
            return Err(Error::PolicyRefused(
                "status active needs a passing hard gate; run a scan first".into(),
            ));
        }
        Ok(Preview {
            before: Some(before),
            after: Some(after),
        })
    }

    fn apply(
        &self,
        instance_id: i64,
        entity: CrudEntity,
        operation: CrudOperation,
        fields: &Value,
        reason: &str,
    ) -> Result<()> {
        let reasons = vec![format!("ai.reason:{}", crate::sanitize::cap(reason, 80).0)];
        match operation {
            CrudOperation::Delete => {
                self.store.delete_override(instance_id)?;
            }
            CrudOperation::Retire => {
                self.store
                    .set_status(instance_id, InstanceStatus::Retired, &reasons)?;
            }
            CrudOperation::Restore => {
                let current = self
                    .store
                    .get_instance(instance_id)?
                    .ok_or_else(|| Error::InvalidArgs("instance missing".into()))?;
                if !matches!(
                    current.status,
                    InstanceStatus::Retired | InstanceStatus::Missing
                ) {
                    return Err(Error::PolicyRefused(format!(
                        "restore applies to a retired or missing instance, not {}",
                        current.status.as_str()
                    )));
                }
                self.store
                    .set_status(instance_id, InstanceStatus::Candidate, &reasons)?;
            }
            CrudOperation::Create | CrudOperation::Update => {
                let status = match fields.get("status").and_then(Value::as_str) {
                    Some(raw) => {
                        let status = InstanceStatus::parse(raw)
                            .ok_or_else(|| Error::InvalidArgs(format!("unknown status {raw}")))?;
                        if status == InstanceStatus::Active {
                            let instance = self
                                .store
                                .get_instance(instance_id)?
                                .ok_or_else(|| Error::InvalidArgs("instance missing".into()))?;
                            if instance.hard_gate_passed != Some(true) {
                                return Err(Error::PolicyRefused(
                                    "status active needs a passing hard gate".into(),
                                ));
                            }
                        }
                        Some(status)
                    }
                    None => None,
                };
                let kind = fields
                    .get("type")
                    .and_then(Value::as_str)
                    .map(InstanceType::parse);
                let override_fields: Map<String, Value> = fields
                    .as_object()
                    .map(|object| {
                        object
                            .iter()
                            .filter(|(key, _)| OVERRIDE_FIELDS.contains(&key.as_str()))
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let override_value = (entity == CrudEntity::PolicyOverride
                    || !override_fields.is_empty())
                .then_some(Value::Object(override_fields));
                self.store.apply_policy_change(
                    instance_id,
                    status,
                    kind,
                    override_value.as_ref(),
                    &reasons,
                )?;
            }
        }
        Ok(())
    }
}

struct Validated {
    instance_id: i64,
    entity: CrudEntity,
    operation: CrudOperation,
    fields: Value,
    reason: String,
}

struct Preview {
    before: Option<Value>,
    after: Option<Value>,
}

fn validate_value(key: &str, value: &Value) -> Result<()> {
    let accepted = match key {
        "status" => value
            .as_str()
            .is_some_and(|raw| InstanceStatus::parse(raw).is_some()),
        "type" => value
            .as_str()
            .is_some_and(|raw| matches!(raw, "normal" | "tor" | "unknown")),
        "scanEnabled" | "htmlFallbackAllowed" | "pinned" | "denied" => value.is_boolean(),
        _ => false,
    };
    if accepted {
        Ok(())
    } else {
        Err(Error::InvalidArgs(format!(
            "{key} has an invalid value; check the field type and allowed values"
        )))
    }
}

fn fingerprint(validated: &Validated) -> String {
    let payload = json!({
        "instanceId": validated.instance_id,
        "entity": validated.entity.as_str(),
        "operation": validated.operation.as_str(),
        "fields": validated.fields,
        "reason": validated.reason,
    });
    let mut hasher = Sha256::new();
    hasher.update(payload.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

fn token_digest(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn new_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| Error::State(format!("os random: {error}")))?;
    Ok(hex::encode(bytes))
}

const MAX_PENDING_PREVIEWS: usize = 100;

fn prune_expired(state: &mut CrudState) {
    let now = Instant::now();
    state
        .pending
        .retain(|_, pending| !pending.used && pending.expires_at > now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Overrides;
    use crate::instances::{BootstrapSnapshot, DiscoveredInstance};

    fn store_with_one() -> Arc<Store> {
        let store = Arc::new(Store::open_in_memory().expect("store"));
        let record = DiscoveredInstance {
            url: "https://searx.example/".to_owned(),
            kind: InstanceType::Normal,
            rank: Some(1),
            version: Some("2026.10.21+dedeb8f7d".to_owned()),
            tls_grade: Some("A+".to_owned()),
            tls_protocol: Some("TLSv1.3".to_owned()),
            certificate_not_after: None,
            certificate_days_remaining: Some(80),
            http_grade: Some("A+".to_owned()),
            csp_grade: Some("A+".to_owned()),
            html_grade: Some("C".to_owned()),
            analytics: false,
            ipv6: false,
            uptime_month: Some(100.0),
            uptime_year: Some(99.0),
            search_success: Some(100.0),
            search_median_secs: Some(0.6),
            initial_success: Some(100.0),
            initial_secs: Some(0.2),
            google_success: Some(0.0),
            google_secs: None,
            working_engines: vec!["duckduckgo".to_owned()],
            exact_google_capability: false,
            google_cse_capability: false,
            asn_cidr: None,
        };
        store
            .ensure_source("bootstrap", "bootstrap", "embedded")
            .expect("source");
        store
            .upsert_instance(&record, "bootstrap", None, "2026-10-26T00:00:00.000Z")
            .expect("insert");
        store
    }

    fn settings(mode: AiDbMode, name: &str) -> Settings {
        let dir =
            std::env::temp_dir().join(format!("ownsearch-crud-{name}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut map: BTreeMap<String, String> = [
            ("HOME".to_owned(), "/home/tester".to_owned()),
            ("OWNSEARCH_STATE_DIR".to_owned(), dir.display().to_string()),
            (
                "OWNSEARCH_LEDGER_DIR".to_owned(),
                dir.join("ledger").display().to_string(),
            ),
        ]
        .into_iter()
        .collect();
        map.insert("OWNSEARCH_AI_DB_MODE".to_owned(), mode.as_str().to_owned());
        map.insert(
            "OWNSEARCH_AI_DB_MIN_INTERVAL_SECS".to_owned(),
            "0".to_owned(),
        );
        Settings::load_with(
            Overrides {
                state_dir: Some(dir.clone()),
                ledger_dir: Some(dir.join("ledger")),
                ..Overrides::default()
            },
            |name| map.get(name).cloned(),
        )
        .expect("settings")
    }

    fn request(dry_run: bool, fields: BTreeMap<String, Value>) -> CrudRequest {
        CrudRequest {
            operation: CrudOperation::Update,
            entity: CrudEntity::PolicyOverride,
            instance_id: Some(1),
            url: None,
            fields,
            reason: Some("operator asked for this change".to_owned()),
            dry_run,
            confirmation_token: None,
        }
    }

    #[test]
    fn the_pending_preview_store_refuses_a_call_once_it_is_full() {
        let store = store_with_one();
        let service = CrudService::new(&settings(AiDbMode::ReadWrite, "preview-cap"), store);
        let mut fields = BTreeMap::new();
        fields.insert("pinned".to_owned(), json!(true));
        for _ in 0..MAX_PENDING_PREVIEWS {
            service
                .begin(request(true, fields.clone()))
                .expect("preview inside the cap");
        }
        let error = service
            .begin(request(true, fields))
            .expect_err("the store must refuse past its cap");
        assert_eq!(error.code(), "ownsearch.policy_refused");
    }

    #[test]
    fn read_only_mode_refuses_before_any_work() {
        let store = store_with_one();
        let service = CrudService::new(&settings(AiDbMode::ReadOnly, "read-only"), store);
        let mut fields = BTreeMap::new();
        fields.insert("pinned".to_owned(), json!(true));
        let error = service.begin(request(true, fields)).expect_err("read only");
        assert_eq!(error.code(), "ownsearch.write_disabled");
    }

    #[test]
    fn a_preview_returns_a_token_and_an_apply_changes_exactly_the_previewed_fields() {
        let store = store_with_one();
        let service = CrudService::new(
            &settings(AiDbMode::ReadWrite, "preview"),
            Arc::clone(&store),
        );
        let mut fields = BTreeMap::new();
        fields.insert("htmlFallbackAllowed".to_owned(), json!(true));
        let preview = service
            .begin(request(true, fields.clone()))
            .expect("preview");
        assert!(!preview.applied);
        let token = preview.confirmation_token.expect("token");
        assert!(preview.expires_in_secs.is_some());
        let mut apply = request(false, fields);
        apply.confirmation_token = Some(token.clone());
        let applied = service.begin(apply).expect("apply");
        assert!(applied.applied);
        let row = store.get_instance(1).expect("read").expect("row");
        assert!(row.html_fallback_allowed);
        let mut replay = request(false, BTreeMap::new());
        replay.confirmation_token = Some(token);
        replay.fields = BTreeMap::new();
        replay.operation = CrudOperation::Delete;
        assert!(service.begin(replay).is_err());
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let store = store_with_one();
        let service = CrudService::new(&settings(AiDbMode::ReadWrite, "unknown-field"), store);
        let mut fields = BTreeMap::new();
        fields.insert("url".to_owned(), json!("https://evil.example/"));
        let error = service.begin(request(true, fields)).expect_err("field");
        assert!(error.to_string().contains("url"));
    }

    #[test]
    fn a_missing_reason_is_refused() {
        let store = store_with_one();
        let service = CrudService::new(&settings(AiDbMode::ReadWrite, "missing-reason"), store);
        let mut fields = BTreeMap::new();
        fields.insert("pinned".to_owned(), json!(true));
        let mut request = request(true, fields);
        request.reason = Some("  ".to_owned());
        assert!(service.begin(request).is_err());
    }

    #[test]
    fn a_retired_instance_can_be_restored_but_an_active_one_cannot() {
        let store = store_with_one();
        let service = CrudService::new(
            &settings(AiDbMode::ReadWrite, "retire-restore"),
            Arc::clone(&store),
        );
        let mut retire = request(true, BTreeMap::new());
        retire.operation = CrudOperation::Retire;
        retire.entity = CrudEntity::Instance;
        let preview = service.begin(retire).expect("preview retire");
        let token = preview.confirmation_token.expect("token");
        let mut apply = request(false, BTreeMap::new());
        apply.operation = CrudOperation::Retire;
        apply.entity = CrudEntity::Instance;
        apply.confirmation_token = Some(token);
        service.begin(apply).expect("retire");
        let row = store.get_instance(1).expect("read").expect("row");
        assert_eq!(row.status, InstanceStatus::Retired);
        let mut restore = request(true, BTreeMap::new());
        restore.operation = CrudOperation::Restore;
        restore.entity = CrudEntity::Instance;
        let preview = service.begin(restore).expect("preview restore");
        let token = preview.confirmation_token.expect("token");
        let mut apply = request(false, BTreeMap::new());
        apply.operation = CrudOperation::Restore;
        apply.entity = CrudEntity::Instance;
        apply.confirmation_token = Some(token);
        service.begin(apply).expect("restore");
        let row = store.get_instance(1).expect("read").expect("row");
        assert_eq!(row.status, InstanceStatus::Candidate);
    }

    #[test]
    fn a_bootstrap_snapshot_is_imported_once() {
        let snapshot = BootstrapSnapshot::embedded().expect("bootstrap");
        let store = Store::open_in_memory().expect("store");
        let (added, total) = store
            .import_bootstrap(&snapshot, 1_792_935_594)
            .expect("import");
        assert_eq!(added, 67);
        assert_eq!(total, 67);
        let again = store
            .import_bootstrap(&snapshot, 1_792_935_594)
            .expect("import");
        assert_eq!(again.0, 0);
    }
}
