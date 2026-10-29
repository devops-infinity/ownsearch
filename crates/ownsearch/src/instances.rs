use anyhow::Context;
use ownsearch_core::instance_to_json;
use ownsearch_core::{InstanceFilter, InstanceStatus, OperatorAction, Settings, Store};
use serde_json::json;

use crate::cli::{InstancesAction, InstancesArgs};

pub(crate) fn run(args: InstancesArgs) -> anyhow::Result<()> {
    let settings = Settings::load(args.common.overrides()).context("resolving settings")?;
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
            let rows = store.list_instances(&filter)?;
            if as_json {
                let values: Vec<serde_json::Value> = rows.iter().map(instance_to_json).collect();
                println!("{}", serde_json::to_string_pretty(&values)?);
            } else {
                for row in &rows {
                    println!(
                        "{} {} {} score {} rank {} {}",
                        row.instance_id,
                        row.status.as_str(),
                        row.kind.as_str(),
                        row.score
                            .map(|score| format!("{score:.1}"))
                            .unwrap_or_else(|| "-".to_owned()),
                        row.rank
                            .map(|rank| rank.to_string())
                            .unwrap_or_else(|| "-".to_owned()),
                        row.url
                    );
                }
                println!("{} rows", rows.len());
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
                println!("{}", serde_json::to_string_pretty(&instance_fields)?);
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
            json: as_json,
        } => {
            let instance_id = store
                .add_manual_instance(&url, pin, &ownsearch_core::now_rfc3339())
                .with_context(|| format!("adding {url}"))?;
            let row = store
                .get_instance(instance_id)?
                .ok_or_else(|| anyhow::anyhow!("the added instance could not be read back"))?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&instance_to_json(&row))?);
            } else {
                println!("{} {} {}", row.instance_id, row.status.as_str(), row.url);
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
