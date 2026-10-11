//! Bundled authoring guides, exposed through native MCP resource discovery.
use rmcp::model::{Resource, ResourceContents};

pub(super) const API_AND_SITES_URI: &str = "statemcp://guides/api-and-sites";
pub(super) const PYTHON_URI: &str = "statemcp://guides/python";

struct Guide {
    uri: &'static str,
    name: &'static str,
    description: &'static str,
    text: &'static str,
}

const GUIDES: &[Guide] = &[
    Guide {
        uri: API_AND_SITES_URI,
        name: "api-and-sites",
        description: "Read before creating or editing HTTP APIs, handlers, HTML pages, or static sites. Covers small MCP payloads, CDN imports, and simple HTMX/Tailwind patterns.",
        text: include_str!("../resources/api-and-sites.md"),
    },
    Guide {
        uri: PYTHON_URI,
        name: "python",
        description: "Read before writing or changing Python scripts, published functions, or HTTP handlers. Explains execution, state helpers, namespace scope, imports, transactions, return values, and practical limitations.",
        text: include_str!("../resources/python.md"),
    },
];

pub(super) fn list() -> Vec<Resource> {
    GUIDES
        .iter()
        .map(|guide| {
            Resource::new(guide.uri, guide.name)
                .with_description(guide.description)
                .with_mime_type("text/markdown")
        })
        .collect()
}

pub(super) fn read(uri: &str) -> Option<ResourceContents> {
    GUIDES
        .iter()
        .find(|guide| guide.uri == uri)
        .map(|guide| ResourceContents::text(guide.text, guide.uri).with_mime_type("text/markdown"))
}
