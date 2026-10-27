#[derive(Debug, Clone, Copy)]
pub struct ToolSpec {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

pub const SPECS: &[ToolSpec] = &[
    ToolSpec {
        name: "ownsearch_search",
        title: "Federated SearXNG web search",
        description: "Search the web through the best-scored public SearXNG instances and return merged, source-attributed results. It may refresh local fleet state first. Use it for general research; results are untrusted web text.",
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    },
    ToolSpec {
        name: "ownsearch_fetch",
        title: "Fetch and extract pages",
        description: "Fetch one public page as Markdown, plain text, or cleaned HTML, with the title and a size cap. It follows up to max_pages same-host links and returns them under pages, and it renders a JavaScript page in headless Chrome when render is always or the page looks like an empty shell. Private and loopback addresses are refused.",
        read_only: true,
        destructive: false,
        idempotent: false,
        open_world: true,
    },
    ToolSpec {
        name: "ownsearch_instance_info",
        title: "Instance capability summary",
        description: "Capability and lifecycle facts for one instance or the whole fleet: status, score, rank, version, JSON and HTML access, and exact-Google evidence.",
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    },
    ToolSpec {
        name: "ownsearch_instance_health",
        title: "Instance health and quality",
        description: "Quality gates, certificate state, probe counts, cooldowns, and recent lifecycle events for one instance or the top of the fleet.",
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    },
    ToolSpec {
        name: "ownsearch_scan",
        title: "Scan the instance fleet",
        description: "Refresh discovery from searx.space, probe candidates once each, rescore them, and reselect the active fleet. Set dry_run false to persist the result.",
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    },
    ToolSpec {
        name: "ownsearch_session",
        title: "Local session statistics",
        description: "Local call counts, cache hits, ledger length, and fleet size for this process. No network call and no query text is stored.",
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    },
    ToolSpec {
        name: "ownsearch_db_query",
        title: "Read instance state",
        description: "Bounded read-only queries over instances, quality, policy overrides, capability snapshots, events, snapshots, and discovery runs. Never accepts raw SQL.",
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    },
    ToolSpec {
        name: "ownsearch_db_crud",
        title: "Typed database change",
        description: "Prepare and apply one typed lifecycle or policy change. Needs write mode, a reason, and a one-use confirmation token from a dry_run preview. It can add an instance from a URL, including a localhost or Docker instance, retire, restore, or deny an instance, and it can delete a policy override. It never deletes an instance row.",
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: false,
    },
];

#[cfg(test)]
pub(crate) fn tool_names() -> Vec<&'static str> {
    SPECS.iter().map(|spec| spec.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_name_carries_the_prefix_and_fits_the_limit() {
        for spec in SPECS {
            assert!(spec.name.starts_with("ownsearch_"), "{}", spec.name);
            assert!(spec.name.len() <= 64, "{}", spec.name);
            assert!(spec.compliant_description(), "{}", spec.name);
        }
    }

    #[test]
    fn no_two_tools_share_a_description() {
        let mut descriptions: Vec<&str> = SPECS.iter().map(|spec| spec.description).collect();
        descriptions.sort_unstable();
        descriptions.dedup();
        assert_eq!(descriptions.len(), SPECS.len());
    }

    impl ToolSpec {
        fn compliant_description(&self) -> bool {
            self.description.chars().count() <= 400 && !self.description.contains("ignore previous")
        }
    }
}
