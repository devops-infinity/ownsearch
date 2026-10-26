use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::{
    CrudEntity, CrudOperation, InstanceStatus, InstanceType, Preset, QueryEntity, SafeSearch,
    SearchMode, TextMode, TimeRange, normalize_url,
};

pub(crate) const MAX_QUERY_CHARS: usize = 2_048;
pub(crate) const MAX_LIST_ITEMS: usize = 16;
pub(crate) const MAX_RESULT_LIMIT: u16 = 100;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchArgs {
    pub query: String,
    #[serde(default)]
    pub categories: Option<Vec<String>>,
    #[serde(default)]
    pub engines: Option<Vec<String>>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub time_range: Option<TimeRange>,
    #[serde(default)]
    pub safesearch: Option<SafeSearch>,
    #[serde(default)]
    pub limit: Option<u16>,
    #[serde(default)]
    pub fields: Option<Preset>,
    #[serde(default)]
    pub mode: Option<SearchMode>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub result_text: Option<TextMode>,
}

impl SearchArgs {
    pub(crate) fn validate(self) -> Result<ValidatedSearch> {
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return Err(Error::InvalidArgs("query is empty".into()));
        }
        if query.chars().count() > MAX_QUERY_CHARS {
            return Err(Error::InvalidArgs(format!(
                "query is longer than {MAX_QUERY_CHARS} characters"
            )));
        }
        let categories = clean_list(self.categories, "categories")?;
        let engines = clean_list(self.engines, "engines")?;
        if let Some(language) = &self.language
            && !is_language_code(language)
        {
            return Err(Error::InvalidArgs(format!(
                "language {language:?} is not a language code such as en or pt-BR"
            )));
        }
        let page = self.page.unwrap_or(1);
        if page == 0 {
            return Err(Error::InvalidArgs("page starts at 1".into()));
        }
        if let Some(limit) = self.limit
            && !(1..=MAX_RESULT_LIMIT).contains(&limit)
        {
            return Err(Error::InvalidArgs(format!(
                "limit {limit} is outside 1..{MAX_RESULT_LIMIT}"
            )));
        }
        if let Some(timeout) = self.timeout_secs
            && !(1..=120).contains(&timeout)
        {
            return Err(Error::InvalidArgs(format!(
                "timeout_secs {timeout} is outside 1..120"
            )));
        }
        Ok(ValidatedSearch {
            query,
            categories,
            engines,
            language: self.language.map(|value| value.trim().to_owned()),
            page,
            time_range: self.time_range,
            safe_search: self.safesearch.unwrap_or_default(),
            limit: self.limit.map(usize::from),
            preset: self.fields,
            mode: self.mode.unwrap_or_default(),
            timeout_secs: self.timeout_secs,
            text_mode: self.result_text.unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedSearch {
    pub query: String,
    pub categories: Vec<String>,
    pub engines: Vec<String>,
    pub language: Option<String>,
    pub page: u32,
    pub time_range: Option<TimeRange>,
    pub safe_search: SafeSearch,
    pub limit: Option<usize>,
    pub preset: Option<Preset>,
    pub mode: SearchMode,
    pub timeout_secs: Option<u64>,
    pub text_mode: TextMode,
}

fn clean_list(values: Option<Vec<String>>, field: &str) -> Result<Vec<String>> {
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    if values.len() > MAX_LIST_ITEMS {
        return Err(Error::InvalidArgs(format!(
            "{field} holds {} items, above {MAX_LIST_ITEMS}",
            values.len()
        )));
    }
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim().to_owned();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > 64 || trimmed.chars().any(char::is_control) {
            return Err(Error::InvalidArgs(format!(
                "{field} holds an invalid value"
            )));
        }
        if !out.iter().any(|existing| existing == &trimmed) {
            out.push(trimmed);
        }
    }
    Ok(out)
}

fn is_language_code(raw: &str) -> bool {
    let trimmed = raw.trim();
    let mut parts = trimmed.split('-');
    let base = parts.next().unwrap_or_default();
    let region = parts.next();
    let base_ok = (2..=3).contains(&base.chars().count())
        && base
            .chars()
            .all(|character| character.is_ascii_alphabetic());
    let region_ok = match region {
        None => true,
        Some(region) => {
            (2..=3).contains(&region.chars().count())
                && region
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        }
    };
    base_ok && region_ok && parts.next().is_none()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FetchArgs {
    pub url: String,
    #[serde(default)]
    pub format: Option<crate::types::FetchFormat>,
    #[serde(default)]
    pub max_chars: Option<u32>,
    #[serde(default)]
    pub max_pages: Option<u8>,
    #[serde(default)]
    pub same_host_only: Option<bool>,
    #[serde(default)]
    pub render: Option<crate::types::RenderMode>,
}

impl FetchArgs {
    pub(crate) fn validate(self) -> Result<crate::fetch::FetchRequest> {
        let url = self.url.trim().to_owned();
        normalize_url(&url)?;
        let max_chars = usize::try_from(self.max_chars.unwrap_or(20_000)).unwrap_or(20_000);
        if !(1_000..=200_000).contains(&max_chars) {
            return Err(Error::InvalidArgs(format!(
                "max_chars {max_chars} is outside 1000..200000"
            )));
        }
        let max_pages = usize::from(self.max_pages.unwrap_or(3));
        if !(1..=8).contains(&max_pages) {
            return Err(Error::InvalidArgs(format!(
                "max_pages {max_pages} is outside 1..8"
            )));
        }
        Ok(crate::fetch::FetchRequest {
            url,
            format: self.format.unwrap_or_default(),
            max_chars,
            max_pages,
            same_host_only: self.same_host_only.unwrap_or(true),
            render: self.render.unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstanceInfoArgs {
    #[serde(default)]
    pub instance_id: Option<i64>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub status: Option<InstanceStatus>,
    #[serde(default)]
    pub limit: Option<u16>,
    #[serde(default)]
    pub include_capabilities: Option<bool>,
}

impl InstanceInfoArgs {
    pub(crate) fn limit(&self) -> usize {
        usize::from(self.limit.unwrap_or(25).clamp(1, 200))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.instance_id.is_some() && self.url.is_some() {
            return Err(Error::InvalidArgs(
                "pass instance_id or url, not both".into(),
            ));
        }
        if let Some(url) = &self.url {
            normalize_url(url)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstanceHealthArgs {
    #[serde(default)]
    pub instance_id: Option<i64>,
    #[serde(default)]
    pub limit: Option<u16>,
    #[serde(default)]
    pub include_events: Option<bool>,
}

impl InstanceHealthArgs {
    pub(crate) fn limit(&self) -> usize {
        usize::from(self.limit.unwrap_or(10).clamp(1, 100))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScanArgs {
    #[serde(default)]
    pub dry_run: Option<bool>,
    #[serde(default)]
    pub instance_id: Option<i64>,
    #[serde(default)]
    pub limit: Option<u16>,
    #[serde(default)]
    pub force_discovery: Option<bool>,
    #[serde(default)]
    pub probe_search: Option<bool>,
    #[serde(default)]
    pub probe_google: Option<bool>,
    #[serde(default)]
    pub force: Option<bool>,
}

impl ScanArgs {
    pub(crate) fn options(&self) -> crate::scanner::ScanOptions {
        crate::scanner::ScanOptions {
            dry_run: self.dry_run.unwrap_or(true),
            instance_id: self.instance_id,
            limit: usize::from(self.limit.unwrap_or(200).clamp(1, 500)),
            force_discovery: self.force_discovery.unwrap_or(false),
            probe_search: self.probe_search.unwrap_or(true),
            probe_google: self.probe_google.unwrap_or(true),
            force: self.force.unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionArgs {
    #[serde(default)]
    pub include_tools: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DbQueryArgs {
    pub entity: QueryEntity,
    #[serde(default)]
    pub instance_id: Option<i64>,
    #[serde(default)]
    pub status: Option<InstanceStatus>,
    #[serde(default, rename = "type")]
    pub kind: Option<InstanceType>,
    #[serde(default)]
    pub min_score: Option<f64>,
    #[serde(default)]
    pub limit: Option<u16>,
    #[serde(default)]
    pub offset: Option<u32>,
}

impl DbQueryArgs {
    pub(crate) fn filter(&self) -> Result<crate::db::InstanceFilter> {
        if let Some(score) = self.min_score
            && (!score.is_finite() || !(0.0..=100.0).contains(&score))
        {
            return Err(Error::InvalidArgs(format!(
                "min_score {score} is outside 0..100"
            )));
        }
        let broad = self.status.is_some() || self.kind.is_some() || self.min_score.is_some();
        if broad && !matches!(self.entity, QueryEntity::Instances) {
            return Err(Error::InvalidArgs(format!(
                "status, type, and min_score only apply to entity=instances, not {}",
                self.entity.as_str()
            )));
        }
        if self.instance_id.is_some()
            && matches!(self.entity, QueryEntity::Snapshots | QueryEntity::Runs)
        {
            return Err(Error::InvalidArgs(format!(
                "instance_id does not apply to entity={}, which belongs to no single instance",
                self.entity.as_str()
            )));
        }

        Ok(crate::db::InstanceFilter {
            status: self.status,
            kind: self.kind,
            min_score: self.min_score,
            instance_id: self.instance_id,
            ids: Vec::new(),
            url: None,
            limit: usize::from(self.limit.unwrap_or(25).clamp(1, 200)),
            offset: usize::try_from(self.offset.unwrap_or(0)).unwrap_or(0),
        })
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DbCrudArgs {
    pub operation: CrudOperation,
    pub entity: CrudEntity,
    #[serde(default)]
    pub instance_id: Option<i64>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub fields: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub dry_run: Option<bool>,
    #[serde(default)]
    pub confirmation_token: Option<String>,
}

impl DbCrudArgs {
    pub(crate) fn request(self) -> Result<crate::crud::CrudRequest> {
        if self.instance_id.is_none() && self.url.is_none() {
            return Err(Error::InvalidArgs(
                "name the instance with instance_id or url".into(),
            ));
        }
        if let Some(url) = &self.url {
            normalize_url(url)?;
        }
        Ok(crate::crud::CrudRequest {
            operation: self.operation,
            entity: self.entity,
            instance_id: self.instance_id,
            url: self.url,
            fields: self.fields.unwrap_or_default(),
            reason: self.reason,
            dry_run: self.dry_run.unwrap_or(true),
            confirmation_token: self.confirmation_token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search_args(query: &str) -> SearchArgs {
        SearchArgs {
            query: query.to_owned(),
            categories: None,
            engines: None,
            language: None,
            page: None,
            time_range: None,
            safesearch: None,
            limit: None,
            fields: None,
            mode: None,
            timeout_secs: None,
            result_text: None,
        }
    }

    #[test]
    fn a_blank_query_is_refused() {
        assert!(search_args("   ").validate().is_err());
    }

    #[test]
    fn the_defaults_match_the_contract() {
        let validated = search_args("rust").validate().expect("valid");
        assert_eq!(validated.limit, None);
        assert_eq!(validated.page, 1);
        assert_eq!(validated.preset, None);
        assert_eq!(validated.mode, SearchMode::Balanced);
        assert_eq!(validated.safe_search, SafeSearch::Moderate);
    }

    #[test]
    fn an_out_of_range_limit_is_refused() {
        let mut args = search_args("rust");
        args.limit = Some(0);
        assert!(args.validate().is_err());
        let mut args = search_args("rust");
        args.limit = Some(500);
        assert!(args.validate().is_err());
    }

    #[test]
    fn a_bad_language_code_is_refused() {
        let mut args = search_args("rust");
        args.language = Some("english please".to_owned());
        assert!(args.validate().is_err());
        let mut args = search_args("rust");
        args.language = Some("pt-BR".to_owned());
        assert!(args.validate().is_ok());
    }

    #[test]
    fn the_scan_defaults_to_a_dry_run() {
        let args = ScanArgs {
            dry_run: None,
            instance_id: None,
            limit: None,
            force_discovery: None,
            probe_search: None,
            probe_google: None,
            force: None,
        };
        assert!(args.options().dry_run);
        assert!(!args.options().force_discovery);
    }

    #[test]
    fn a_crud_request_defaults_to_a_dry_run() {
        let args = DbCrudArgs {
            operation: CrudOperation::Update,
            entity: CrudEntity::PolicyOverride,
            instance_id: Some(1),
            url: None,
            fields: None,
            reason: Some("needed by the operator".to_owned()),
            dry_run: None,
            confirmation_token: None,
        };
        let request = args.request().expect("request");
        assert!(request.dry_run);
    }
}
