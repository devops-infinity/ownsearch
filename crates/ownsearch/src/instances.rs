use anyhow::Context;
use ownsearch_core::instance_to_json;
use ownsearch_core::{InstanceFilter, InstanceStatus, OperatorAction, Settings, Store};
use serde_json::json;

use crate::cli::{InstancesAction, InstancesArgs};

pub(crate) fn run(args: InstancesArgs, common: &crate::cli::CommonArgs) -> anyhow::Result<()> {
    let settings = Settings::load(common.overrides()).context("resolving settings")?;
    let store = Store::open(&settings.database_path()).context("opening the state database")?;
    store.warm_up()?;
    match args.action {
        InstancesAction::List {
            status,
            limit,
            json: as_json,
        } => {
            let filter = InstanceFilter {
                status: match status {
                    Some(raw) => Some(InstanceStatus::parse(&raw).ok_or_else(|| {
                        let known = InstanceStatus::ALL
                            .iter()
                            .map(|value| value.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        anyhow::anyhow!("unknown status {raw}; use one of {known}")
                    })?),
                    None => None,
                },
                limit: usize::from(limit.clamp(1, 500)),
                ..InstanceFilter::default()
            };
            let limit = filter.limit;
            let total = store.count_instances(&filter)?;
            let rows = store.list_instances(&filter)?;
            let ids: Vec<i64> = rows.iter().map(|row| row.instance_id).collect();
            let yields = store.engine_yield_for(&ids).unwrap_or_default();
            if as_json {
                let values: Vec<serde_json::Value> = rows.iter().map(instance_to_json).collect();
                println!("{}", serde_json::to_string_pretty(&values)?);
            } else {
                for row in &rows {
                    let engine_yield = yields
                        .get(&row.instance_id)
                        .map(|value| format!("{value:.2}"))
                        .unwrap_or_else(|| "-".to_owned());
                    println!(
                        "{} {} {} score {} rank {} yield {} {}",
                        row.instance_id,
                        row.status.as_str(),
                        row.kind.as_str(),
                        row.score
                            .map(|score| format!("{score:.1}"))
                            .unwrap_or_else(|| "-".to_owned()),
                        row.rank
                            .map(|rank| rank.to_string())
                            .unwrap_or_else(|| "-".to_owned()),
                        engine_yield,
                        row.url
                    );
                }
                println!(
                    "{} rows shown of {}; {} hidden by the {} row limit, raise it with --limit",
                    rows.len(),
                    total,
                    total.saturating_sub(rows.len()),
                    limit
                );
            }
            Ok(())
        }
        InstancesAction::Show {
            instance_id,
            url,
            json: as_json,
        } => {
            let row = match (instance_id, url) {
                (Some(id), _) => store.get_instance(id)?,
                (None, Some(url)) => store.find_instance_by_url(&url)?,
                (None, None) => anyhow::bail!("pass an instance id or --url"),
            };
            let Some(row) = row else {
                anyhow::bail!("no instance matched");
            };
            let instance_fields = instance_to_json(&row);
            if as_json {
                let mut shown = instance_fields.clone();
                if let Some(record) = store.auth_for(&[row.instance_id])?.remove(&row.instance_id)
                    && let Some(object) = shown.as_object_mut()
                {
                    let described = match record.kind {
                        ownsearch_core::AuthKind::Header => format!(
                            "header {} from {}",
                            record.header_name.as_deref().unwrap_or("authorization"),
                            record.secret_env
                        ),
                        ownsearch_core::AuthKind::Basic => format!(
                            "basic user {} from {}",
                            record.user_name.as_deref().unwrap_or("-"),
                            record.secret_env
                        ),
                    };
                    object.insert("auth".to_owned(), json!(described));
                }
                println!("{}", serde_json::to_string_pretty(&shown)?);
            } else {
                for (key, field_value) in instance_fields.as_object().into_iter().flatten() {
                    println!("{key}: {field_value}");
                }
                let engines = store.engine_names(row.instance_id)?;
                if !engines.is_empty() {
                    println!("engines: {}", engines.join(", "));
                }
                let categories = store.category_names(row.instance_id)?;
                if !categories.is_empty() {
                    println!("categories: {}", categories.join(", "));
                }
                if let Some(record) = store.auth_for(&[row.instance_id])?.remove(&row.instance_id) {
                    let described = match record.kind {
                        ownsearch_core::AuthKind::Header => format!(
                            "header {} from {}",
                            record.header_name.as_deref().unwrap_or("authorization"),
                            record.secret_env
                        ),
                        ownsearch_core::AuthKind::Basic => format!(
                            "basic user {} from {}",
                            record.user_name.as_deref().unwrap_or("-"),
                            record.secret_env
                        ),
                    };
                    println!("auth: {described}");
                }
            }
            Ok(())
        }
        InstancesAction::Pin { instance_id } => {
            ensure_row(&store, instance_id)?;
            let policy_row = store.upsert_override(
                instance_id,
                &json!({"pinned": true}),
                &["operator.pin".to_owned()],
            )?;
            println!("{}", serde_json::to_string_pretty(&policy_row)?);
            Ok(())
        }
        InstancesAction::Unpin { instance_id } => {
            ensure_row(&store, instance_id)?;
            let policy_row = store.upsert_override(
                instance_id,
                &json!({"pinned": false}),
                &["operator.unpin".to_owned()],
            )?;
            println!("{}", serde_json::to_string_pretty(&policy_row)?);
            Ok(())
        }
        InstancesAction::Deny {
            instance_id,
            reason,
        } => {
            ensure_row(&store, instance_id)?;
            let reason = reason.unwrap_or_else(|| "operator denied".to_owned());
            let (reason, _) = ownsearch_core::cap(&ownsearch_core::sanitize(&reason), 120);
            let status = store.apply_operator_action(
                instance_id,
                OperatorAction::Deny,
                &[format!("operator.deny:{reason}")],
            )?;
            let policy_row = store.get_override(instance_id)?.unwrap_or(json!(null));
            println!("{}", serde_json::to_string_pretty(&policy_row)?);
            if status == InstanceStatus::Quarantined {
                println!(
                    "instance {instance_id} is quarantined and the scanner will not probe it; run `ownsearch instances allow {instance_id}` to undo"
                );
            } else {
                println!(
                    "instance {instance_id} is {}: the denial is recorded and the scanner will not probe it. Run `ownsearch instances set-type {instance_id} normal` to change what this row is, or `ownsearch instances allow {instance_id}` to clear the denial",
                    status.as_str()
                );
            }
            Ok(())
        }
        InstancesAction::Allow {
            instance_id,
            reason,
        } => {
            ensure_row(&store, instance_id)?;
            let reason = reason.unwrap_or_else(|| "operator allowed".to_owned());
            let (reason, _) = ownsearch_core::cap(&ownsearch_core::sanitize(&reason), 120);
            let status = store.apply_operator_action(
                instance_id,
                OperatorAction::Allow,
                &[format!("operator.allow:{reason}")],
            )?;
            let row = store
                .get_instance(instance_id)?
                .ok_or_else(|| anyhow::anyhow!("instance {instance_id} does not exist"))?;
            if row.kind == ownsearch_core::InstanceType::Tor {
                println!(
                    "instance {instance_id} is {}; a Tor instance is never probed. Run `ownsearch instances set-type {instance_id} normal` to change its type",
                    status.as_str()
                );
            } else if status == InstanceStatus::Retired {
                println!(
                    "instance {instance_id} is retired; run `ownsearch instances restore {instance_id}` to return it to service"
                );
            } else {
                println!(
                    "instance {instance_id} is {} and scanning is enabled",
                    status.as_str()
                );
            }
            Ok(())
        }
        InstancesAction::ResetStatus { instance_id } => {
            ensure_row(&store, instance_id)?;
            let status = store.apply_operator_action(
                instance_id,
                OperatorAction::Reset,
                &["operator.reset".to_owned()],
            )?;
            println!(
                "instance {instance_id} is {} again with no policy override",
                status.as_str()
            );
            Ok(())
        }
        InstancesAction::Events {
            instance_id,
            limit,
            json: as_json,
        } => {
            if let Some(instance_id) = instance_id {
                ensure_row(&store, instance_id)?;
            }
            let events = store.events_for(instance_id, usize::from(limit.clamp(1, 500)), 0)?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&events)?);
            } else {
                for event in &events {
                    println!(
                        "{} {} {} {} -> {}",
                        event
                            .get("eventId")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(0),
                        event
                            .get("occurredAt")
                            .and_then(|value| value.as_str())
                            .unwrap_or("-"),
                        event
                            .get("kind")
                            .and_then(|value| value.as_str())
                            .unwrap_or("-"),
                        event
                            .get("oldValue")
                            .and_then(|value| value.as_str())
                            .unwrap_or("-"),
                        event
                            .get("newValue")
                            .and_then(|value| value.as_str())
                            .unwrap_or("-")
                    );
                }
                println!("{} events", events.len());
            }
            Ok(())
        }
        InstancesAction::SetType { instance_id, kind } => {
            ensure_row(&store, instance_id)?;
            let kind = match kind.to_ascii_lowercase().as_str() {
                "normal" => ownsearch_core::InstanceType::Normal,
                "tor" => ownsearch_core::InstanceType::Tor,
                "unknown" => ownsearch_core::InstanceType::Unknown,
                other => anyhow::bail!("unknown type {other}; use normal, tor, or unknown"),
            };
            store.set_type(
                instance_id,
                kind,
                &[format!("operator.set_type:{}", kind.as_str())],
            )?;
            let row = store
                .get_instance(instance_id)?
                .ok_or_else(|| anyhow::anyhow!("instance {instance_id} does not exist"))?;
            println!(
                "instance {instance_id} is {} with type {}",
                row.status.as_str(),
                row.kind.as_str()
            );
            Ok(())
        }
        InstancesAction::Retire {
            instance_id,
            reason,
        } => {
            ensure_row(&store, instance_id)?;
            let reason = reason.unwrap_or_else(|| "operator retired".to_owned());
            let (reason, _) = ownsearch_core::cap(&ownsearch_core::sanitize(&reason), 120);
            store.apply_operator_action(
                instance_id,
                OperatorAction::Retire,
                &[format!("operator.retire:{reason}")],
            )?;
            println!("instance {instance_id} is retired");
            Ok(())
        }
        InstancesAction::Restore { instance_id } => {
            ensure_row(&store, instance_id)?;
            let status = store.apply_operator_action(
                instance_id,
                OperatorAction::Restore,
                &["operator.restore".to_owned()],
            )?;
            println!(
                "instance {instance_id} is {} and scanning is enabled",
                status.as_str()
            );
            Ok(())
        }
        InstancesAction::Add {
            url,
            pin,
            trusted,
            json: as_json,
        } => {
            let instance_id = store
                .add_manual_instance(&url, pin, &ownsearch_core::now_rfc3339())
                .with_context(|| format!("adding {url}"))?;
            if trusted {
                store.upsert_override(
                    instance_id,
                    &json!({"trusted": true}),
                    &["operator.trusted".to_owned()],
                )?;
            }
            let row = store
                .get_instance(instance_id)?
                .ok_or_else(|| anyhow::anyhow!("the added instance could not be read back"))?;
            let decision = ownsearch_core::plain_http_refusal_for_url(
                &row.url,
                settings.allow_private_instances(),
                settings.allow_plain_http_private(),
            );
            if let Some(reason) = &decision {
                store.append_event(&ownsearch_core::EventRecord {
                    kind: "policy_refused".to_owned(),
                    instance_id: Some(instance_id),
                    run_id: None,
                    probe_id: None,
                    old_value: None,
                    new_value: Some(ownsearch_core::PLAIN_HTTP_REFUSED.to_owned()),
                    reasons: vec![ownsearch_core::PLAIN_HTTP_REFUSED.to_owned()],
                    payload: Some(json!({"message": reason})),
                })?;
            }
            if as_json {
                let mut body = instance_to_json(&row);
                if let Some(object) = body.as_object_mut() {
                    object.insert(
                        "transport".to_owned(),
                        match &decision {
                            Some(reason) => json!({
                                "allowed": false,
                                "reasonCode": ownsearch_core::PLAIN_HTTP_REFUSED,
                                "message": reason,
                            }),
                            None => json!({"allowed": true}),
                        },
                    );
                }
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                println!("{} {} {}", row.instance_id, row.status.as_str(), row.url);
                match &decision {
                    Some(reason) => println!(
                        "transport: refused for probe and search with {}: {reason}",
                        ownsearch_core::PLAIN_HTTP_REFUSED
                    ),
                    None => println!("transport: allowed"),
                }
                println!(
                    "next: run `ownsearch scan --instance {instance_id} --force` twice to probe it and promote it, then `ownsearch health --instance {instance_id}` to check it"
                );
            }
            Ok(())
        }
        InstancesAction::SetTrusted { instance_id, off } => {
            ensure_row(&store, instance_id)?;
            let trusted = !off;
            store.upsert_override(
                instance_id,
                &json!({"trusted": trusted}),
                &[if trusted {
                    "operator.trusted"
                } else {
                    "operator.untrusted"
                }
                .to_owned()],
            )?;
            println!(
                "instance {instance_id} is {}trusted",
                if trusted { "" } else { "no longer " }
            );
            Ok(())
        }
        InstancesAction::SetAuth {
            instance_id,
            header,
            basic,
        } => {
            ensure_row(&store, instance_id)?;
            let row = store
                .get_instance(instance_id)?
                .ok_or_else(|| anyhow::anyhow!("instance {instance_id} does not exist"))?;
            let record = match (header, basic) {
                (Some(header), None) => {
                    let (name, env) = header.split_once('=').ok_or_else(|| {
                        anyhow::anyhow!(
                            "--header takes NAME=ENVVAR, for example X-Api-Key=SEARX_TOKEN"
                        )
                    })?;
                    ownsearch_core::AuthRecord {
                        kind: ownsearch_core::AuthKind::Header,
                        header_name: Some(ownsearch_core::validate_header_name(name)?),
                        user_name: None,
                        secret_env: ownsearch_core::validate_env_name(env)?,
                    }
                }
                (None, Some(basic)) => {
                    let (user, env) = basic.split_once(':').ok_or_else(|| {
                        anyhow::anyhow!(
                            "--basic takes USER:ENVVAR, for example ownsearch:SEARX_PASS"
                        )
                    })?;
                    ownsearch_core::AuthRecord {
                        kind: ownsearch_core::AuthKind::Basic,
                        header_name: None,
                        user_name: Some(ownsearch_core::validate_user_name(user)?),
                        secret_env: ownsearch_core::validate_env_name(env)?,
                    }
                }
                _ => anyhow::bail!("pass exactly one of --header or --basic"),
            };
            let auth = ownsearch_core::InstanceAuth::resolve(&row.url, record.clone())?;
            store.set_instance_auth(instance_id, &record)?;
            println!("instance {instance_id} auth: {}", auth.describe());
            Ok(())
        }
        InstancesAction::ClearAuth { instance_id } => {
            ensure_row(&store, instance_id)?;
            let removed = store.clear_instance_auth(instance_id)?;
            println!(
                "instance {instance_id} auth: {}",
                if removed {
                    "cleared"
                } else {
                    "none was stored"
                }
            );
            Ok(())
        }
        InstancesAction::SetUrl { instance_id, url } => {
            ensure_row(&store, instance_id)?;
            let normalized = store.update_instance_url(instance_id, &url)?;
            println!("instance {instance_id} url is now {normalized} and its status is candidate");
            println!(
                "next: run `ownsearch scan --instance {instance_id} --force` twice to probe the new host, then `ownsearch health --instance {instance_id}`"
            );
            Ok(())
        }
        InstancesAction::SetPriority {
            instance_id,
            priority,
        } => {
            ensure_row(&store, instance_id)?;
            if !(-1_000..=1_000).contains(&priority) {
                anyhow::bail!("priority {priority} is outside -1000 to 1000");
            }
            store.set_manual_priority(instance_id, priority)?;
            println!("instance {instance_id} priority is now {priority}");
            Ok(())
        }
        InstancesAction::Engines {
            instance_id,
            json: as_json,
        } => {
            ensure_row(&store, instance_id)?;
            let engines = store.engine_names(instance_id)?;
            let stats = store
                .engine_stats_for(&[instance_id])?
                .get(&instance_id)
                .cloned()
                .unwrap_or_default();
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "instanceId": instance_id,
                        "engines": engines,
                        "stats": stats.iter().map(|stat| json!({
                            "engine": stat.engine,
                            "successes": stat.successes,
                            "failures": stat.failures,
                            "lastSeenAt": stat.last_seen_at,
                        })).collect::<Vec<_>>(),
                    }))?
                );
            } else {
                for engine in &engines {
                    println!("{engine}");
                }
                println!("{} engines enabled", engines.len());
                if engines.is_empty() {
                    println!(
                        "next: run `ownsearch scan --instance {instance_id} --force` to read /config first"
                    );
                }
                for stat in &stats {
                    println!(
                        "{}: {} ok, {} failed, last seen {}",
                        stat.engine, stat.successes, stat.failures, stat.last_seen_at
                    );
                }
            }
            Ok(())
        }
    }
}

fn ensure_row(store: &Store, instance_id: i64) -> anyhow::Result<()> {
    if store.get_instance(instance_id)?.is_none() {
        anyhow::bail!("instance {instance_id} does not exist");
    }
    Ok(())
}
