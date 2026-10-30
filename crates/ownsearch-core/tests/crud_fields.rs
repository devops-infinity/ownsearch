#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::sync::Arc;

use ownsearch_core::{InstanceStatus, ToolContext};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn add_instance(ctx: &Arc<ToolContext>, url: &str) -> i64 {
    ctx.store
        .add_manual_instance(url, false, &ownsearch_core::now_rfc3339())
        .expect("add")
}

async fn crud(ctx: &Arc<ToolContext>, args: Value) -> Result<Value, Value> {
    let map = args.as_object().cloned().unwrap_or_default();
    let result = ownsearch_core::call_tool(
        ctx,
        "ownsearch_db_crud",
        Some(&map),
        &CancellationToken::new(),
    )
    .await;
    if result.is_error == Some(true) {
        let body = result
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|text| text.text.clone())
            .unwrap_or_default();
        return Err(serde_json::from_str(&body).unwrap_or(Value::Null));
    }
    Ok(result.structured_content.unwrap_or(Value::Null))
}

async fn apply(ctx: &Arc<ToolContext>, operation: &str, fields: Value) -> Result<Value, Value> {
    let preview = crud(
        ctx,
        json!({
            "operation": operation,
            "entity": "instance",
            "instance_id": 1,
            "fields": fields,
            "reason": "operator asked for the change",
            "dry_run": true,
        }),
    )
    .await?;
    let token = preview
        .get("confirmationToken")
        .and_then(Value::as_str)
        .expect("token")
        .to_owned();
    crud(
        ctx,
        json!({
            "operation": operation,
            "entity": "instance",
            "instance_id": 1,
            "fields": fields,
            "reason": "operator asked for the change",
            "dry_run": false,
            "confirmation_token": token,
        }),
    )
    .await
}

#[tokio::test]
async fn every_instance_field_is_updatable_through_the_tool() {
    let dir = common::temp_dir("crud-tool-fields");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = add_instance(&ctx, "https://old.example/");
    assert_eq!(id, 1);
    ctx.store
        .apply_scan_update(&ownsearch_core::ScanUpdate {
            instance_id: id,
            status: InstanceStatus::Active,
            previous_status: InstanceStatus::Candidate,
            successes: 2,
            failures: 0,
            score: 70.0,
            rank: None,
            hard_gate_passed: true,
            reasons: vec!["probe.json_ok".to_owned()],
            next_scan_at: None,
            cooldown_until: None,
            last_success_at: None,
            last_failure_at: None,
            version: None,
            capability_id: None,
            tls_id: None,
        })
        .expect("update");

    let preview = crud(
        &ctx,
        json!({
            "operation": "update",
            "entity": "instance",
            "instance_id": id,
            "fields": {"url": "https://new.example/"},
            "reason": "moving to the production domain",
            "dry_run": true,
        }),
    )
    .await
    .expect("preview");
    let after = preview.get("after").expect("after");
    assert_eq!(
        after.get("url").and_then(Value::as_str),
        Some("https://new.example/")
    );

    let applied = apply(&ctx, "update", json!({"url": "https://new.example/"}))
        .await
        .expect("apply");
    assert_eq!(applied.get("applied").and_then(Value::as_bool), Some(true));
    let row = ctx.store.get_instance(id).expect("row").expect("row");
    assert_eq!(row.url, "https://new.example/");
    assert_eq!(row.status, InstanceStatus::Candidate);
    assert_eq!(row.score, None);

    apply(
        &ctx,
        "update",
        json!({
            "priority": 7,
            "trusted": true,
            "pinned": true,
            "scanEnabled": false,
            "htmlFallbackAllowed": true,
        }),
    )
    .await
    .expect("apply policy fields");
    let row = ctx.store.get_instance(id).expect("row").expect("row");
    assert_eq!(row.manual_priority, 7);
    assert!(row.trusted);
    assert!(row.pinned);
    assert!(!row.scan_enabled);
    assert!(row.html_fallback_allowed);

    apply(&ctx, "update", json!({"status": "retired"}))
        .await
        .expect("status");
    assert_eq!(
        ctx.store
            .get_instance(id)
            .expect("row")
            .expect("row")
            .status,
        InstanceStatus::Retired
    );
    apply(&ctx, "restore", json!({})).await.expect("restore");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_domain_alias_reaches_the_row_and_a_missing_secret_is_refused() {
    let dir = common::temp_dir("crud-tool-alias");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = add_instance(&ctx, "https://first.example/");
    assert_eq!(id, 1);
    apply(
        &ctx,
        "update",
        json!({"domain": "https://second.example/", "priority": -3}),
    )
    .await
    .expect("apply");
    let row = ctx.store.get_instance(id).expect("row").expect("row");
    assert_eq!(row.url, "https://second.example/");
    assert_eq!(row.manual_priority, -3);

    let error = apply(
        &ctx,
        "update",
        json!({"auth": {"kind": "header", "headerName": "x-api-key", "envVar": "OWNSEARCH_TEST_ABSENT"}}),
    )
    .await
    .expect_err("the missing variable is refused");
    assert!(
        error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("OWNSEARCH_TEST_ABSENT")),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn deleting_an_instance_removes_its_rows() {
    let dir = common::temp_dir("crud-tool-delete");
    let settings = common::settings(&dir);
    let ctx = Arc::new(ToolContext::new(settings).expect("context"));
    let id = add_instance(&ctx, "https://gone.example/");
    assert_eq!(id, 1);
    let preview = crud(
        &ctx,
        json!({
            "operation": "delete",
            "entity": "instance",
            "instance_id": id,
            "reason": "the domain is retired",
            "dry_run": true,
        }),
    )
    .await
    .expect("preview");
    assert!(preview.get("after").is_some_and(Value::is_null));
    let token = preview
        .get("confirmationToken")
        .and_then(Value::as_str)
        .expect("token")
        .to_owned();
    let applied = crud(
        &ctx,
        json!({
            "operation": "delete",
            "entity": "instance",
            "instance_id": id,
            "reason": "the domain is retired",
            "dry_run": false,
            "confirmation_token": token,
        }),
    )
    .await
    .expect("apply");
    assert_eq!(applied.get("applied").and_then(Value::as_bool), Some(true));
    assert!(ctx.store.get_instance(id).expect("row").is_none());
    assert_eq!(
        ctx.store
            .count_instances(&Default::default())
            .expect("count"),
        0
    );
    let events = ctx.store.events_for(None, 10, 0).expect("events");
    assert!(
        events
            .iter()
            .all(|event| event.get("instanceId").and_then(Value::as_i64) != Some(id)),
        "the deleted instance events must be gone"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
