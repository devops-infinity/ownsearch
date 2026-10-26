pub(crate) mod args;
mod call;
pub(crate) mod specs;

pub use call::call;
pub(crate) use specs::{SPECS, ToolSpec};

use std::sync::Arc;

use rmcp::handler::server::tool::{schema_for_input, schema_for_output};
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::response::{
    DbCrudOutput, DbQueryOutput, FetchOutput, InstanceHealthOutput, InstanceInfoOutput, ScanOutput,
    SearchOutput, SessionOutput,
};
use crate::tools::args::{
    DbCrudArgs, DbQueryArgs, FetchArgs, InstanceHealthArgs, InstanceInfoArgs, ScanArgs, SearchArgs,
    SessionArgs,
};

pub fn definitions() -> Result<Vec<Tool>> {
    SPECS.iter().map(build_tool).collect()
}

fn build_tool(spec: &'static ToolSpec) -> Result<Tool> {
    let schema = match spec.name {
        "ownsearch_search" => input_schema::<SearchArgs>()?,
        "ownsearch_fetch" => input_schema::<FetchArgs>()?,
        "ownsearch_instance_info" => input_schema::<InstanceInfoArgs>()?,
        "ownsearch_instance_health" => input_schema::<InstanceHealthArgs>()?,
        "ownsearch_scan" => input_schema::<ScanArgs>()?,
        "ownsearch_session" => input_schema::<SessionArgs>()?,
        "ownsearch_db_query" => input_schema::<DbQueryArgs>()?,
        "ownsearch_db_crud" => input_schema::<DbCrudArgs>()?,
        other => {
            return Err(Error::Config(format!(
                "tool {other} has no argument schema"
            )));
        }
    };
    let tool = Tool::new(spec.name, spec.description, schema)
        .with_title(spec.title)
        .annotate(
            ToolAnnotations::new()
                .read_only(spec.read_only)
                .destructive(spec.destructive)
                .idempotent(spec.idempotent)
                .open_world(spec.open_world),
        );
    Ok(match spec.name {
        "ownsearch_search" => attach_output_schema::<SearchOutput>(tool),
        "ownsearch_fetch" => attach_output_schema::<FetchOutput>(tool),
        "ownsearch_instance_info" => attach_output_schema::<InstanceInfoOutput>(tool),
        "ownsearch_instance_health" => attach_output_schema::<InstanceHealthOutput>(tool),
        "ownsearch_scan" => attach_output_schema::<ScanOutput>(tool),
        "ownsearch_session" => attach_output_schema::<SessionOutput>(tool),
        "ownsearch_db_query" => attach_output_schema::<DbQueryOutput>(tool),
        "ownsearch_db_crud" => attach_output_schema::<DbCrudOutput>(tool),
        _ => tool,
    })
}

fn attach_output_schema<T: JsonSchema + 'static>(tool: Tool) -> Tool {
    let schema = strip_meta(schema_for_output::<T>());
    tool.with_raw_output_schema(schema)
}

fn strip_meta(schema: Arc<JsonObject>) -> Arc<JsonObject> {
    let mut object = schema.as_ref().clone();
    object.remove("$schema");
    Arc::new(object)
}

fn input_schema<T: JsonSchema + 'static>() -> Result<Arc<JsonObject>> {
    let schema =
        schema_for_input::<T>().map_err(|error| Error::Config(format!("input schema: {error}")))?;
    let mut object = schema.as_ref().clone();
    object.remove("$schema");
    object.insert("additionalProperties".to_owned(), Value::Bool(false));
    Ok(Arc::new(object))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spec_builds_a_tool_with_both_schemas() {
        let tools = definitions().expect("tools");
        assert_eq!(tools.len(), SPECS.len());
        for tool in &tools {
            assert!(tool.input_schema.contains_key("properties"));
            assert!(tool.output_schema.is_some(), "{}", tool.name);
        }
    }

    #[test]
    fn every_input_schema_refuses_unknown_properties() {
        let tools = definitions().expect("tools");
        for tool in &tools {
            assert_eq!(
                tool.input_schema.get("additionalProperties"),
                Some(&Value::Bool(false)),
                "{}",
                tool.name
            );
        }
    }

    #[test]
    fn the_serialized_tool_list_fits_the_budget() {
        let tools = definitions().expect("tools");
        let bytes = serde_json::to_vec(&tools).expect("serialize").len();
        assert!(bytes < 48_000, "the tool list is {bytes} bytes");
    }
}
