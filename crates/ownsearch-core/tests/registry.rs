#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use ownsearch_core::{SPECS, tool_definitions};

#[test]
fn every_spec_builds_one_tool_with_two_schemas() {
    let tools = tool_definitions().expect("tools");
    assert_eq!(tools.len(), SPECS.len());
    for spec in SPECS {
        let tool = tools
            .iter()
            .find(|tool| tool.name == spec.name)
            .unwrap_or_else(|| panic!("{} has no tool", spec.name));
        assert!(
            tool.input_schema.contains_key("properties"),
            "{}",
            spec.name
        );
        assert!(tool.output_schema.is_some(), "{}", spec.name);
        assert_eq!(tool.title.as_deref(), Some(spec.title));
    }
}

#[test]
fn every_tool_has_a_unique_prefixed_name() {
    let mut names: Vec<&str> = SPECS.iter().map(|spec| spec.name).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total);
    for name in names {
        assert!(name.starts_with("ownsearch_"), "{name}");
        assert!(name.len() <= 64, "{name}");
    }
}

#[test]
fn every_tool_annotation_matches_its_read_only_contract() {
    let tools = tool_definitions().expect("tools");
    for tool in tools {
        let spec = SPECS
            .iter()
            .find(|spec| spec.name == tool.name)
            .expect("spec");
        let annotations = tool.annotations.clone().expect("annotations");
        assert_eq!(
            annotations.read_only_hint,
            Some(spec.read_only),
            "{}",
            tool.name
        );
        assert_eq!(
            annotations.destructive_hint,
            Some(spec.destructive),
            "{}",
            tool.name
        );
        assert_eq!(
            annotations.idempotent_hint,
            Some(spec.idempotent),
            "{}",
            tool.name
        );
        assert_eq!(
            annotations.open_world_hint,
            Some(spec.open_world),
            "{}",
            tool.name
        );
    }
}

#[test]
fn the_search_schema_requires_a_query_and_refuses_unknown_fields() {
    let tools = tool_definitions().expect("tools");
    let search = tools
        .iter()
        .find(|tool| tool.name == "ownsearch_search")
        .expect("search tool");
    assert_eq!(
        search.input_schema.get("additionalProperties"),
        Some(&serde_json::Value::Bool(false))
    );
    let required = search
        .input_schema
        .get("required")
        .and_then(|value| value.as_array())
        .expect("required");
    assert!(required.iter().any(|value| value.as_str() == Some("query")));
}

#[test]
fn the_tool_list_stays_inside_its_token_budget() {
    let tools = tool_definitions().expect("tools");
    let bytes = serde_json::to_vec(&tools).expect("serialize").len();
    assert!(bytes < 48_000, "the tool list is {bytes} bytes");
}

#[test]
fn no_tool_description_carries_a_model_directed_instruction() {
    for spec in SPECS {
        let lower = spec.description.to_ascii_lowercase();
        for banned in ["ignore previous", "ignore all", "you must", "system prompt"] {
            assert!(!lower.contains(banned), "{}", spec.name);
        }
    }
}
