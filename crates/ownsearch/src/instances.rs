use anyhow::Context;
use ownsearch_core::instance_to_json;
use ownsearch_core::{InstanceFilter, InstanceStatus, Settings, Store};
use serde_json::json;

use crate::cli::{InstancesAction, InstancesArgs};

pub(crate) fn run(args: InstancesArgs) -> anyhow::Result<()> {
    let settings = Settings::load(args.common.overrides()).context("resolving settings")?;
    let store = Store::open(&settings.database_path()).context("opening the state database")?;
    match args.action {
        InstancesAction::List {
            status,
            limit,
            json: as_json,
        } => {
            let filter = InstanceFilter {
                status: match status {
                    Some(raw) => Some(InstanceStatus::parse(&raw).ok_or_else(|| {
                        anyhow::anyhow!(
                            "unknown status {raw}; use candidate, active, probation, missing, quarantined, or retired"
                        )
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
            let policy_row = store.upsert_override(
                instance_id,
                &json!({"denied": true, "scanEnabled": false}),
                &[format!("operator.deny:{reason}")],
            )?;
            store.set_status(
                instance_id,
                InstanceStatus::Quarantined,
                &["policy.denied".to_owned()],
            )?;
            println!("{}", serde_json::to_string_pretty(&policy_row)?);
            Ok(())
        }
        InstancesAction::ResetStatus { instance_id } => {
            ensure_row(&store, instance_id)?;
            store.delete_override(instance_id)?;
            store.set_status(
                instance_id,
                InstanceStatus::Candidate,
                &["operator.reset".to_owned()],
            )?;
            println!("instance {instance_id} is candidate again with no policy override");
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
