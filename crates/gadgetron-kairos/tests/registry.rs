//! Cross-crate integration tests: `McpToolRegistry` registering a real
//! `KnowledgeToolProvider` and exercising dispatch + allowed-tools
//! filtering end-to-end.
//!
//! This is the first test that proves the registry (Step 10) + the
//! Wiki aggregate (Step 11) + the `KnowledgeToolProvider` (Step 11)
//! compose correctly via the stable `McpToolProvider` trait surface
//! from `gadgetron-core`.
//!
//! Spec: `00-overview.md §15` Phase 3 Step 14.

use std::sync::Arc;

use gadgetron_core::agent::config::{AgentConfig, ToolMode};
use gadgetron_core::agent::tools::McpError;
use gadgetron_kairos::{McpToolRegistry, McpToolRegistryBuilder};
use gadgetron_knowledge::config::WikiConfig;
use gadgetron_knowledge::mcp::KnowledgeToolProvider;
use gadgetron_knowledge::wiki::Wiki;
use serde_json::json;
use tempfile::TempDir;

fn fresh_provider() -> (TempDir, KnowledgeToolProvider) {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = WikiConfig {
        root: dir.path().join("wiki"),
        autocommit: true,
        git_author_name: "Test".into(),
        git_author_email: "test@test.local".into(),
        max_page_bytes: 1024 * 1024,
    };
    let wiki = Arc::new(Wiki::open(cfg).expect("wiki open"));
    let provider = KnowledgeToolProvider::with_components(wiki, None, 10);
    (dir, provider)
}

fn register_knowledge(provider: KnowledgeToolProvider) -> McpToolRegistry {
    let mut builder = McpToolRegistryBuilder::new();
    builder
        .register(Arc::new(provider))
        .expect("register knowledge provider");
    builder.freeze()
}

#[test]
fn registry_accepts_knowledge_provider() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    assert!(!registry.is_empty(), "registry must have tools registered");
    assert_eq!(
        registry.len(),
        4,
        "no-search knowledge provider exposes 4 tools (list/get/search/write)"
    );
}

#[test]
fn registry_all_schemas_contains_every_knowledge_tool() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    let names: Vec<&str> = registry
        .all_schemas()
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    for expected in ["wiki.list", "wiki.get", "wiki.search", "wiki.write"] {
        assert!(
            names.contains(&expected),
            "expected {expected:?} in {names:?}"
        );
    }
}

#[test]
fn build_allowed_tools_with_default_config_exposes_read_and_wiki_write() {
    // Default AgentConfig has wiki_write = Auto, other write subcats = Ask.
    // With T1 reads always present and wiki_write in Auto, the filtered
    // list should contain all 4 knowledge tools (wiki.* reads + wiki.write).
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    let cfg = AgentConfig::default();
    let allowed = registry.build_allowed_tools(&cfg);
    assert!(allowed.contains(&"wiki.list".to_string()));
    assert!(allowed.contains(&"wiki.get".to_string()));
    assert!(allowed.contains(&"wiki.search".to_string()));
    assert!(allowed.contains(&"wiki.write".to_string()));
    assert_eq!(allowed.len(), 4);
}

#[test]
fn build_allowed_tools_with_wiki_write_never_omits_write() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    let mut cfg = AgentConfig::default();
    cfg.tools.write.wiki_write = ToolMode::Never;
    let allowed = registry.build_allowed_tools(&cfg);
    assert!(!allowed.contains(&"wiki.write".to_string()));
    assert!(allowed.contains(&"wiki.list".to_string()));
    assert!(allowed.contains(&"wiki.get".to_string()));
    assert!(allowed.contains(&"wiki.search".to_string()));
    assert_eq!(allowed.len(), 3);
}

#[test]
fn build_allowed_tools_output_is_deterministic() {
    // Two freshly-constructed registries over the same tool set must
    // produce identical allowed-tools lists (sorted + deduped per §2.1).
    let (_dir_a, a) = fresh_provider();
    let (_dir_b, b) = fresh_provider();
    let reg_a = register_knowledge(a);
    let reg_b = register_knowledge(b);
    let cfg = AgentConfig::default();
    assert_eq!(reg_a.build_allowed_tools(&cfg), reg_b.build_allowed_tools(&cfg));
}

#[tokio::test]
async fn dispatch_wiki_write_then_wiki_get_round_trip_via_registry() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);

    let write = registry
        .dispatch(
            "wiki.write",
            json!({"name": "home", "content": "hello"}),
        )
        .await
        .expect("write");
    assert!(!write.is_error);
    assert_eq!(write.content["bytes"], 5);

    let get = registry
        .dispatch("wiki.get", json!({"name": "home"}))
        .await
        .expect("get");
    assert_eq!(get.content["content"], "hello");
}

#[tokio::test]
async fn dispatch_unknown_tool_returns_unknown_tool_error() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    let err = registry
        .dispatch("nonexistent.tool", json!({}))
        .await
        .expect_err("unknown");
    match err {
        McpError::UnknownTool(name) => assert_eq!(name, "nonexistent.tool"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_wiki_write_blocked_credential_returns_denied() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    let err = registry
        .dispatch(
            "wiki.write",
            json!({
                "name": "leaked",
                "content": "-----BEGIN RSA PRIVATE KEY-----\nbody\n-----END RSA PRIVATE KEY-----"
            }),
        )
        .await
        .expect_err("must block");
    match err {
        McpError::Denied { reason } => {
            assert!(
                reason.contains("pem_private_key"),
                "reason must identify the pattern: {reason}"
            );
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_wiki_list_reflects_writes_through_registry() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    for name in ["alpha", "beta", "gamma"] {
        registry
            .dispatch(
                "wiki.write",
                json!({"name": name, "content": format!("content of {name}")}),
            )
            .await
            .unwrap();
    }
    let result = registry
        .dispatch("wiki.list", json!({}))
        .await
        .expect("list");
    let pages = result.content["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 3);
    let names: Vec<&str> = pages.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["alpha", "beta", "gamma"]);
}

#[tokio::test]
async fn dispatch_wiki_search_via_registry_returns_hits() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    registry
        .dispatch(
            "wiki.write",
            json!({"name": "notes", "content": "quarterly review with the team"}),
        )
        .await
        .unwrap();
    registry
        .dispatch(
            "wiki.write",
            json!({"name": "grocery", "content": "milk bread eggs"}),
        )
        .await
        .unwrap();
    let result = registry
        .dispatch("wiki.search", json!({"query": "quarterly", "max_results": 5}))
        .await
        .expect("search");
    let hits = result.content["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["name"], "notes");
}
