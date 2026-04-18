//! End-to-end RAW ingestion test.
//!
//! Spec: `docs/design/phase2/11-raw-ingestion-and-rag.md` §13.3 (integration
//! test plan) + D-20260418-11 W3-KL-2 scope item 7.
//!
//! # What this covers
//!
//! 1. `wiki.import` dispatch through `KnowledgeGadgetProvider::call`
//!    reaches the pipeline, writes through `KnowledgeService::write`,
//!    and returns a receipt with the canonical plug id.
//! 2. `wiki.list` subsequently reflects the imported path.
//! 3. `wiki.search` (keyword mode) finds the imported body by a word
//!    from the markdown payload.
//! 4. Frontmatter on the stored page carries `source = "imported"` plus
//!    the `source_bytes_hash` that the caller can use as a citation
//!    anchor. The hash value MUST match `sha256(bytes)` computed locally.
//!
//! # Skipping
//!
//! The test requires a local Postgres with the pgvector extension — the
//! `KnowledgeService` needs at least a canonical store, and the e2e
//! assertion involves reading the page back. When Postgres is
//! unavailable or `GADGETRON_SKIP_POSTGRES_TESTS=1` is set, the test
//! early-returns with an `eprintln!` note so CI matrices that lack
//! pgvector don't block the W3-KL-2 PR.
//!
//! The semantic-index tests in `crates/gadgetron-knowledge/src/gadget.rs`
//! use the same skip pattern (`semantic_pg_available()`). The knowledge
//! service in THIS test uses keyword index only — pgvector is not
//! strictly required — but we reuse the same gate so "no Postgres" is a
//! single switch for the whole suite.

use std::sync::Arc;

use gadgetron_knowledge::gadget::KnowledgeGadgetProvider;
use gadgetron_knowledge::wiki::Wiki;
use gadgetron_knowledge::WikiKeywordIndex;
use gadgetron_knowledge::{KnowledgeService, KnowledgeServiceBuilder, LlmWikiStore};

use gadgetron_core::agent::tools::GadgetProvider;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn should_skip() -> bool {
    std::env::var("GADGETRON_SKIP_POSTGRES_TESTS").is_ok()
}

fn build_wiki(dir: &TempDir) -> Arc<Wiki> {
    use gadgetron_knowledge::config::WikiConfig;
    let cfg = WikiConfig {
        root: dir.path().join("wiki"),
        autocommit: true,
        git_author_name: "W3-KL-2 Test".into(),
        git_author_email: "w3-kl-2@test.local".into(),
        max_page_bytes: 1024 * 1024,
    };
    Arc::new(Wiki::open(cfg).expect("wiki open"))
}

fn build_service(wiki: Arc<Wiki>) -> Arc<KnowledgeService> {
    let store = Arc::new(LlmWikiStore::new(wiki).expect("llm-wiki store"));
    let keyword = Arc::new(WikiKeywordIndex::new().expect("keyword index"));
    KnowledgeServiceBuilder::new()
        .canonical_store(store)
        .add_index(keyword)
        .build()
        .expect("service build")
}

/// Core e2e: base64 markdown → `wiki.import` → `wiki.list` + `wiki.search`
/// + `wiki.get` all reflect the imported page. No Postgres needed — this
/// path runs through the keyword index only.
#[tokio::test]
async fn wiki_import_markdown_end_to_end_without_pg() {
    if should_skip() {
        eprintln!("GADGETRON_SKIP_POSTGRES_TESTS set — skipping wiki_import_e2e (even though this case needs no PG)");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let wiki = build_wiki(&dir);
    let service = build_service(wiki);
    let provider = KnowledgeGadgetProvider::from_service(service, None, 10);

    // Step 1: import the markdown body.
    let body = "# Test\n\nHello world from the integration test.";
    let bytes_b64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(body.as_bytes())
    };

    let result = provider
        .call(
            "wiki.import",
            serde_json::json!({
                "bytes": bytes_b64,
                "content_type": "text/markdown",
            }),
        )
        .await
        .expect("wiki.import must succeed");
    assert!(!result.is_error);
    let imported_path = result.content["path"]
        .as_str()
        .expect("receipt path is a string")
        .to_string();
    assert_eq!(
        imported_path, "imports/test",
        "default path = imports/<kebab-title>"
    );
    assert_eq!(result.content["canonical_plug"], "llm-wiki");

    // Step 2: list includes the imported path.
    let list = provider
        .call("wiki.list", serde_json::json!({}))
        .await
        .expect("wiki.list");
    let pages = list.content["pages"].as_array().expect("pages array");
    let names: Vec<String> = pages
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        names.contains(&imported_path),
        "imports/test missing from wiki.list; got {names:?}"
    );

    // Step 3: keyword search finds the body.
    let search = provider
        .call(
            "wiki.search",
            serde_json::json!({
                "query": "hello",
                "limit": 5
            }),
        )
        .await
        .expect("wiki.search");
    let hits = search.content["hits"].as_array().expect("hits array");
    assert!(
        hits.iter()
            .any(|h| h["page_name"].as_str() == Some(imported_path.as_str())),
        "wiki.search did not return imports/test; hits={hits:?}"
    );

    // Step 4: frontmatter carries source fields. `wiki.get` returns the
    // reassembled markdown body minus frontmatter — but the content_hash
    // must match sha256(bytes).
    let expected_hash = {
        let digest = Sha256::digest(body.as_bytes());
        format!("sha256:{}", hex::encode(digest))
    };
    assert_eq!(
        result.content["content_hash"].as_str(),
        Some(expected_hash.as_str()),
        "ImportReceipt.content_hash must be sha256(RAW bytes)"
    );

    // Fetch the stored page — `wiki.get` returns the body only (the
    // canonical store parses frontmatter off into a separate JSON field).
    // The body must still contain the imported markdown.
    let got = provider
        .call(
            "wiki.get",
            serde_json::json!({
                "name": imported_path,
            }),
        )
        .await
        .expect("wiki.get");
    let stored_body = got.content["content"]
        .as_str()
        .expect("get returns content string");
    assert!(
        stored_body.contains("Hello world"),
        "stored page must still contain the markdown body; got:\n{stored_body}"
    );

    // Inspect the raw on-disk file directly to verify frontmatter
    // composition. `LlmWikiStore` strips frontmatter from the returned
    // `content` field of `wiki.get` — but the Penny citation pipeline
    // reads frontmatter via `KnowledgeDocument.frontmatter`, and we
    // verify the source-tracking fields landed.
    let raw_path = dir.path().join("wiki").join(format!("{imported_path}.md"));
    let raw = std::fs::read_to_string(&raw_path).expect("imported md file exists");
    assert!(
        raw.starts_with("---\n"),
        "imported page must start with frontmatter fence; got:\n{raw}"
    );
    assert!(
        raw.contains("source = \"imported\""),
        "raw page must carry source=imported frontmatter; got:\n{raw}"
    );
    assert!(
        raw.contains(&expected_hash),
        "raw page must carry source_bytes_hash ({expected_hash}); got:\n{raw}"
    );
    assert!(
        raw.contains("source_content_type = \"text/markdown\""),
        "raw page must record source_content_type"
    );
}

/// Sanity check: `wiki.import` rejects non-base64 `bytes`.
#[tokio::test]
async fn wiki_import_rejects_malformed_base64() {
    if should_skip() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let wiki = build_wiki(&dir);
    let service = build_service(wiki);
    let provider = KnowledgeGadgetProvider::from_service(service, None, 10);
    let err = provider
        .call(
            "wiki.import",
            serde_json::json!({
                "bytes": "not base64!",
                "content_type": "text/markdown",
            }),
        )
        .await
        .expect_err("malformed base64 must error");
    use gadgetron_core::agent::tools::GadgetError;
    match err {
        GadgetError::InvalidArgs(msg) => {
            assert!(
                msg.contains("base64"),
                "error should mention base64; got {msg}"
            );
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

/// Sanity check: unsupported content-type (e.g. PDF, which lives in
/// W3-KL-3) is rejected loud and clear.
#[tokio::test]
async fn wiki_import_rejects_unsupported_content_type() {
    if should_skip() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let wiki = build_wiki(&dir);
    let service = build_service(wiki);
    let provider = KnowledgeGadgetProvider::from_service(service, None, 10);
    use base64::Engine as _;
    let body = "%PDF-1.7";
    let encoded = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
    let err = provider
        .call(
            "wiki.import",
            serde_json::json!({
                "bytes": encoded,
                "content_type": "application/pdf",
            }),
        )
        .await
        .expect_err("pdf must be rejected in W3-KL-2");
    use gadgetron_core::agent::tools::GadgetError;
    match err {
        GadgetError::InvalidArgs(msg) => {
            assert!(
                msg.contains("content_type") || msg.contains("supported"),
                "error should mention unsupported content_type; got {msg}"
            );
        }
        other => panic!("wrong variant: {other:?}"),
    }
}
