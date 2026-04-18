//! Cross-crate integration tests: `GadgetRegistry` registering a real
//! `KnowledgeGadgetProvider` and exercising dispatch + allowed-tools
//! filtering end-to-end.
//!
//! This is the first test that proves the registry (Step 10) + the
//! Wiki aggregate (Step 11) + the `KnowledgeGadgetProvider` (Step 11)
//! compose correctly via the stable `GadgetProvider` trait surface
//! from `gadgetron-core` (terminology per ADR-P2A-10).
//!
//! Spec: `00-overview.md §15` Phase 3 Step 14.

use std::sync::Arc;

use gadgetron_core::agent::config::{AgentConfig, GadgetMode};
use gadgetron_core::agent::tools::GadgetError;
use gadgetron_knowledge::config::WikiConfig;
use gadgetron_knowledge::gadget::KnowledgeGadgetProvider;
use gadgetron_knowledge::wiki::Wiki;
use gadgetron_penny::{GadgetRegistry, GadgetRegistryBuilder};
use serde_json::json;
use tempfile::TempDir;

fn fresh_provider() -> (TempDir, KnowledgeGadgetProvider) {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = WikiConfig {
        root: dir.path().join("wiki"),
        autocommit: true,
        git_author_name: "Test".into(),
        git_author_email: "test@test.local".into(),
        max_page_bytes: 1024 * 1024,
    };
    let wiki = Arc::new(Wiki::open(cfg).expect("wiki open"));
    let provider = KnowledgeGadgetProvider::with_components(wiki, None, 10);
    (dir, provider)
}

fn register_knowledge(provider: KnowledgeGadgetProvider) -> GadgetRegistry {
    let mut builder = GadgetRegistryBuilder::new();
    builder
        .register(Arc::new(provider))
        .expect("register knowledge provider");
    // Default AgentConfig permits wiki.write (Auto); other Write Gadgets
    // would be filtered by the L3 gate if present.
    builder.freeze(&AgentConfig::default())
}

#[test]
fn registry_accepts_knowledge_provider() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    assert!(!registry.is_empty(), "registry must have tools registered");
    // Knowledge provider exposes: wiki.{list,get,search,write,delete,rename}.
    // `web.search` is only added when `[knowledge.search]` is configured, so
    // the no-search provider built by `fresh_provider` stops at six.
    assert_eq!(
        registry.len(),
        6,
        "no-search knowledge provider exposes 6 tools (list/get/search/write/delete/rename)"
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
    // With T1 reads always present and every wiki.* T2 tool (write/delete/
    // rename) mapped to the `wiki_write` subcategory (Auto by default), the
    // filtered list should contain all six knowledge tools.
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    let cfg = AgentConfig::default();
    let allowed = registry.build_allowed_tools(&cfg);
    assert!(allowed.contains(&"wiki.list".to_string()));
    assert!(allowed.contains(&"wiki.get".to_string()));
    assert!(allowed.contains(&"wiki.search".to_string()));
    assert!(allowed.contains(&"wiki.write".to_string()));
    assert!(allowed.contains(&"wiki.delete".to_string()));
    assert!(allowed.contains(&"wiki.rename".to_string()));
    assert_eq!(allowed.len(), 6);
}

#[test]
fn build_allowed_tools_with_wiki_write_never_omits_write() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);
    let mut cfg = AgentConfig::default();
    cfg.gadgets.write.wiki_write = GadgetMode::Never;
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
    assert_eq!(
        reg_a.build_allowed_tools(&cfg),
        reg_b.build_allowed_tools(&cfg)
    );
}

#[tokio::test]
async fn dispatch_wiki_write_then_wiki_get_round_trip_via_registry() {
    let (_dir, provider) = fresh_provider();
    let registry = register_knowledge(provider);

    let write = registry
        .dispatch("wiki.write", json!({"name": "home", "content": "hello"}))
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
        GadgetError::UnknownGadget(name) => assert_eq!(name, "nonexistent.tool"),
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
        GadgetError::Denied { reason } => {
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
    let names: Vec<&str> = pages.iter().filter_map(|v| v.as_str()).collect();
    // `fresh_provider` ships the built-in seed pages (README, decisions/README,
    // penny/{conventions,usage}, operators/{getting-started,troubleshooting},
    // runbooks/README) alongside the three pages we just wrote — the list must
    // surface all of them. Assert containment, not equality, to avoid a brittle
    // dependency on the seed page set.
    for expected in ["alpha", "beta", "gamma"] {
        assert!(
            names.contains(&expected),
            "expected {expected:?} in {names:?}"
        );
    }
    assert!(
        pages.len() >= 3,
        "at least our 3 writes must appear ({} pages seen)",
        pages.len()
    );
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
        .dispatch(
            "wiki.search",
            json!({"query": "quarterly", "max_results": 5}),
        )
        .await
        .expect("search");
    let hits = result.content["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["name"], "notes");
}
