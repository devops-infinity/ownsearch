use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::config::Settings;
use crate::db::{EventRecord, OperatorAction, Store};
use crate::error::{Error, Result};
use crate::response::DbCrudOutput;
use crate::types::{AiDbMode, CrudEntity, CrudOperation, InstanceStatus, InstanceType};

const INSTANCE_FIELDS: [&str; 11] = [
    "url",
    "domain",
    "status",
    "type",
    "priority",
    "scanEnabled",
    "htmlFallbackAllowed",
    "pinned",
    "trusted",
    "denied",
    "auth",
];
pub(crate) const OVERRIDE_FIELDS: [&str; 5] = [
    "scanEnabled",
    "htmlFallbackAllowed",
    "pinned",
    "denied",
    "trusted",
];

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

impl std::fmt::Debug for CrudService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CrudService")
            .field("mode", &self.mode)
            .field("max_changes", &self.max_changes)
            .field("confirm_ttl", &self.confirm_ttl)
            .field("min_interval", &self.min_interval)
            .finish_non_exhaustive()
    }
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
                instance_id: validated
                    .create_url
                    .is_none()
                    .then_some(validated.instance_id),
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
        let (instance_id, applied) = self.apply(
            validated.instance_id,
            validated.create_url.as_deref(),
            entity,
            operation,
            &fields,
            &reason,
        )?;
        let record = self.store.get_instance(instance_id)?;
        if applied {
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
        }
        Ok(DbCrudOutput {
            operation: operation.as_str().to_owned(),
            entity: entity.as_str().to_owned(),
            applied,
            dry_run: false,
            confirmation_token: None,
            expires_in_secs: None,
            before: None,
            after: None,
            refused_reasons: if applied {
                Vec::new()
            } else {
                vec!["no matching policy override; nothing was removed".to_owned()]
            },
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
            instance_id: validated
                .create_url
                .is_none()
                .then_some(validated.instance_id),
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
        let creating = matches!(
            (request.entity, request.operation),
            (CrudEntity::Instance, CrudOperation::Create)
        );
        let (instance_id, create_url) = if creating {
            let url = request
                .url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| Error::InvalidArgs("give the url of the instance to add".into()))?;
            crate::types::normalize_url(url)?;
            (0, Some(url.to_owned()))
        } else {
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
            (instance_id, None)
        };
        match (request.entity, request.operation) {
            (CrudEntity::PolicyOverride, CrudOperation::Create | CrudOperation::Update)
            | (CrudEntity::PolicyOverride, CrudOperation::Delete)
            | (CrudEntity::Instance, CrudOperation::Create | CrudOperation::Update)
            | (CrudEntity::Instance, CrudOperation::Delete)
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
            if key == "auth" {
                parse_auth(value)?;
            }
            fields.insert(key.clone(), value.clone());
        }
        if fields.len() > self.max_changes {
            return Err(Error::PolicyRefused(format!(
                "{} fields changes exceed the {}-change cap",
                fields.len(),
                self.max_changes
            )));
        }
        let needs_fields = match (request.entity, request.operation) {
            (CrudEntity::Instance, CrudOperation::Create) => false,
            (_, CrudOperation::Create | CrudOperation::Update) => true,
            _ => false,
        };
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
            create_url,
            entity: request.entity,
            operation: request.operation,
            fields: Value::Object(fields),
            reason,
        })
    }

    fn preview(&self, validated: &Validated) -> Result<Preview> {
        let creating = validated.create_url.is_some();
        let before = if creating {
            Value::Null
        } else {
            let instance = self
                .store
                .get_instance(validated.instance_id)?
                .ok_or_else(|| Error::InvalidArgs("instance missing".into()))?;
            match validated.entity {
                CrudEntity::Instance => crate::db::instance_to_json(&instance),
                CrudEntity::PolicyOverride => self
                    .store
                    .get_override(validated.instance_id)?
                    .unwrap_or(Value::Null),
            }
        };
        if validated.operation == CrudOperation::Delete
            && validated.entity == CrudEntity::Instance
            && self.store.get_instance(validated.instance_id)?.is_none()
        {
            return Err(Error::InvalidArgs("instance missing".into()));
        }
        let mut after = match validated.operation {
            CrudOperation::Delete => {
                if validated.entity == CrudEntity::PolicyOverride
                    && self.store.get_override(validated.instance_id)?.is_none()
                {
                    return Err(Error::PolicyRefused(
                        "there is no policy override on this instance to remove".into(),
                    ));
                }
                Value::Null
            }
            CrudOperation::Retire => {
                let mut preview = before.clone();
                if let Some(object) = preview.as_object_mut() {
                    object.insert("status".to_owned(), json!(InstanceStatus::Retired.as_str()));
                }
                preview
            }
            CrudOperation::Restore => {
                let current = self
                    .store
                    .get_instance(validated.instance_id)?
                    .ok_or_else(|| Error::InvalidArgs("instance missing".into()))?;
                let target = match current.status {
                    InstanceStatus::Active | InstanceStatus::Probation => current.status,
                    _ => InstanceStatus::Candidate,
                };
                let mut preview = before.clone();
                if let Some(object) = preview.as_object_mut() {
                    object.insert("status".to_owned(), json!(target.as_str()));
                }
                preview
            }
            CrudOperation::Create | CrudOperation::Update => {
                let mut preview = if before.is_null() {
                    match &validated.create_url {
                        Some(url) => json!({"url": url}),
                        None => json!({"instanceId": validated.instance_id}),
                    }
                } else {
                    before.clone()
                };
                if let (Some(target), Some(source)) =
                    (preview.as_object_mut(), validated.fields.as_object())
                {
                    for (key, value) in source {
                        target.insert(key.clone(), value.clone());
                    }
                    if let Some(url) = validated
                        .fields
                        .get("url")
                        .or_else(|| validated.fields.get("domain"))
                        .and_then(Value::as_str)
                    {
                        target.insert(
                            "url".to_owned(),
                            json!(crate::types::normalize_url(url).unwrap_or_default()),
                        );
                    }
                    if let Some(auth) = validated.fields.get("auth") {
                        target.insert("auth".to_owned(), auth_preview(auth));
                    }
                }
                preview
            }
        };
        if validated.entity == CrudEntity::Instance
            && let Some(status) = validated.fields.get("status").and_then(Value::as_str)
            && status == InstanceStatus::Excluded.as_str()
        {
            let tor = !creating
                && self
                    .store
                    .get_instance(validated.instance_id)?
                    .is_some_and(|row| row.kind == InstanceType::Tor);
            if !tor {
                return Err(Error::PolicyRefused(
                    "excluded is set by the scanner for a Tor instance; set the type to tor first, or use deny to stop an instance".into(),
                ));
            }
        }
        if validated.entity == CrudEntity::Instance
            && let Some(denied) = validated.fields.get("denied").and_then(Value::as_bool)
        {
            let current = before
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or(InstanceStatus::Candidate.as_str());
            let target = if denied {
                InstanceStatus::Quarantined.as_str()
            } else if current == InstanceStatus::Quarantined.as_str() {
                InstanceStatus::Candidate.as_str()
            } else {
                current
            };
            if let Some(object) = after.as_object_mut() {
                object.insert("status".to_owned(), json!(target));
            }
        }
        if validated.entity == CrudEntity::Instance
            && let Some(status) = validated.fields.get("status").and_then(Value::as_str)
            && status == InstanceStatus::Active.as_str()
        {
            if creating {
                return Err(Error::PolicyRefused(
                    "a new instance cannot start active; add it, run a scan, and let the scan promote it"
                        .into(),
                ));
            }
            if !self.store.hard_gate_passed(validated.instance_id)? {
                return Err(Error::PolicyRefused(
                    "status active needs a passing hard gate; run a scan first".into(),
                ));
            }
        }
        Ok(Preview {
            before: Some(before),
            after: Some(after),
        })
    }

    fn apply(
        &self,
        instance_id: i64,
        create_url: Option<&str>,
        entity: CrudEntity,
        operation: CrudOperation,
        fields: &Value,
        reason: &str,
    ) -> Result<(i64, bool)> {
        let reasons = vec![format!("ai.reason:{}", crate::sanitize::cap(reason, 80).0)];
        let mut instance_id = instance_id;
        match operation {
            CrudOperation::Delete => match entity {
                CrudEntity::Instance => {
                    self.store.delete_instance(instance_id)?;
                    return Ok((instance_id, true));
                }
                CrudEntity::PolicyOverride => {
                    if !self.store.delete_override(instance_id)? {
                        return Ok((instance_id, false));
                    }
                }
            },
            CrudOperation::Retire => {
                self.store
                    .apply_operator_action(instance_id, OperatorAction::Retire, &reasons)?;
            }
            CrudOperation::Restore => {
                self.store
                    .apply_operator_action(instance_id, OperatorAction::Restore, &reasons)?;
            }
            CrudOperation::Create | CrudOperation::Update => {
                if let Some(url) = create_url {
                    instance_id =
                        self.store
                            .add_manual_instance(url, false, &crate::time::now_rfc3339())?;
                }
                if let Some(url) = fields
                    .get("url")
                    .or_else(|| fields.get("domain"))
                    .and_then(Value::as_str)
                {
                    self.store.update_instance_url(instance_id, url)?;
                }
                if let Some(priority) = fields.get("priority").and_then(Value::as_i64) {
                    self.store.set_manual_priority(instance_id, priority)?;
                }
                if let Some(auth) = fields.get("auth") {
                    match parse_auth(auth)? {
                        Some(record) => self.store.set_instance_auth(instance_id, &record)?,
                        None => {
                            self.store.clear_instance_auth(instance_id)?;
                        }
                    }
                }
                let status = match fields.get("status").and_then(Value::as_str) {
                    Some(raw) => {
                        let status = InstanceStatus::parse(raw)
                            .ok_or_else(|| Error::InvalidArgs(format!("unknown status {raw}")))?;
                        if status == InstanceStatus::Active
                            && !self.store.hard_gate_passed(instance_id)?
                        {
                            return Err(Error::PolicyRefused(
                                "status active needs a passing hard gate".into(),
                            ));
                        }
                        Some(status)
                    }
                    None => None,
                };
                let kind = fields
                    .get("type")
                    .and_then(Value::as_str)
                    .map(InstanceType::parse);
                if status == Some(InstanceStatus::Excluded)
                    && kind != Some(InstanceType::Tor)
                    && !self
                        .store
                        .get_instance(instance_id)?
                        .is_some_and(|row| row.kind == InstanceType::Tor)
                {
                    return Err(Error::PolicyRefused(
                        "excluded is set by the scanner for a Tor instance; set the type to tor first, or use deny to stop an instance".into(),
                    ));
                }
                let override_fields: Map<String, Value> = fields
                    .as_object()
                    .map(|object| {
                        object
                            .iter()
                            .filter(|(key, _)| {
                                OVERRIDE_FIELDS.contains(&key.as_str()) && key.as_str() != "denied"
                            })
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
                match fields.get("denied").and_then(Value::as_bool) {
                    Some(true) => {
                        self.store.apply_operator_action(
                            instance_id,
                            OperatorAction::Deny,
                            &reasons,
                        )?;
                    }
                    Some(false) => {
                        self.store.apply_operator_action(
                            instance_id,
                            OperatorAction::Allow,
                            &reasons,
                        )?;
                    }
                    None => {}
                }
            }
        }
        Ok((instance_id, true))
    }
}

struct Validated {
    instance_id: i64,
    create_url: Option<String>,
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
        "scanEnabled" | "htmlFallbackAllowed" | "pinned" | "denied" | "trusted" => {
            value.is_boolean()
        }
        "url" | "domain" => value
            .as_str()
            .is_some_and(|raw| crate::types::normalize_url(raw).is_ok()),
        "priority" => value
            .as_i64()
            .is_some_and(|priority| (-1_000..=1_000).contains(&priority)),
        "auth" => value.is_null() || value.is_object(),
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

fn parse_auth(value: &Value) -> Result<Option<crate::auth::AuthRecord>> {
    if value.is_null() {
        return Ok(None);
    }
    let object = value
        .as_object()
        .ok_or_else(|| Error::InvalidArgs("auth is an object or null".into()))?;
    if object.get("clear").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    let kind_raw = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidArgs("auth needs kind header or basic".into()))?;
    let kind = crate::auth::AuthKind::parse(kind_raw)
        .ok_or_else(|| Error::InvalidArgs("auth kind is header or basic".into()))?;
    let secret_env = object
        .get("envVar")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidArgs("auth needs envVar".into()))?;
    let secret_env = crate::auth::validate_env_name(secret_env)?;
    let present = std::env::var(&secret_env)
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    if !present {
        return Err(Error::InvalidArgs(format!(
            "the environment variable {secret_env} is unset or empty; export it before this change"
        )));
    }
    let (header_name, user_name) = match kind {
        crate::auth::AuthKind::Header => {
            let name = object
                .get("headerName")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::InvalidArgs("a header auth needs headerName".into()))?;
            (Some(crate::auth::validate_header_name(name)?), None)
        }
        crate::auth::AuthKind::Basic => {
            let user = object
                .get("user")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::InvalidArgs("a basic auth needs user".into()))?;
            (None, Some(crate::auth::validate_user_name(user)?))
        }
    };
    Ok(Some(crate::auth::AuthRecord {
        kind,
        header_name,
        user_name,
        secret_env,
    }))
}

fn auth_preview(value: &Value) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    let Some(object) = value.as_object() else {
        return Value::Null;
    };
    if object.get("clear").and_then(Value::as_bool) == Some(true) {
        return Value::Null;
    }
    let mut shown = Map::new();
    for key in ["kind", "headerName", "user", "envVar"] {
        if let Some(value) = object.get(key) {
            shown.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(shown)
}

fn fingerprint(validated: &Validated) -> String {
    let payload = json!({
        "instanceId": validated.instance_id,
        "createUrl": validated.create_url,
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
    fn an_instance_can_be_added_from_a_url_after_a_preview() {
        let store = store_with_one();
        let service = CrudService::new(
            &settings(AiDbMode::ReadWrite, "create-instance"),
            Arc::clone(&store),
        );
        let mut create = request(true, BTreeMap::new());
        create.operation = CrudOperation::Create;
        create.entity = CrudEntity::Instance;
        create.instance_id = None;
        create.url = Some("http://localhost:8888/".to_owned());
        let preview = service.begin(create).expect("preview create");
        assert!(!preview.applied);
        let after = preview.after.expect("preview body");
        assert_eq!(
            after.get("url").and_then(Value::as_str),
            Some("http://localhost:8888/")
        );
        let token = preview.confirmation_token.expect("token");
        let mut apply = request(false, BTreeMap::new());
        apply.operation = CrudOperation::Create;
        apply.entity = CrudEntity::Instance;
        apply.instance_id = None;
        apply.url = Some("http://localhost:8888/".to_owned());
        apply.confirmation_token = Some(token);
        let applied = service.begin(apply).expect("apply create");
        assert!(applied.applied);
        let record = applied.record.expect("record");
        assert_eq!(
            record.get("url").and_then(Value::as_str),
            Some("http://localhost:8888/")
        );
        let rows = store
            .list_instances(&crate::db::InstanceFilter {
                limit: 50,
                ..crate::db::InstanceFilter::default()
            })
            .expect("list");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn creating_an_instance_without_a_url_is_refused() {
        let store = store_with_one();
        let service = CrudService::new(&settings(AiDbMode::ReadWrite, "create-no-url"), store);
        let mut create = request(true, BTreeMap::new());
        create.operation = CrudOperation::Create;
        create.entity = CrudEntity::Instance;
        create.instance_id = None;
        create.url = None;
        let error = service.begin(create).expect_err("no url");
        assert!(error.to_string().contains("url"));
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
