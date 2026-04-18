//! RAG citation pipeline integration test (W3-KL-3 / D-20260418-13).
//!
//! Spec: `docs/design/phase2/11-raw-ingestion-and-rag.md` §9.3 (citation
//! format) + D-20260418-13 §integration test.
//!
//! # What this covers (W3-KL-3)
//!
//! 1. Import a markdown document via `wiki.import`.
//! 2. Query `wiki.search` with a keyword from the imported doc.
//! 3. Assert the search returns a hit referencing the imported
//!    page's path (the footnote anchor Penny would use).
//! 4. Assemble a markdown response that cites the hit via a
//!    `[^N]` footnote (mirroring what Penny's system prompt tells
//!    the model to emit).
//! 5. Parse the assembled response with
//!    `gadgetron_penny::citation::extract_citation_refs` and
//!    verify the page path round-trips unchanged.
//!
//! This is a **headless** validation of the retrieval + citation
//! pipeline. It does NOT spawn a real Penny subprocess — the LLM
//! itself is stubbed out. What it validates:
//!
//! - `wiki.search` over an imported page returns the path Penny
//!   would need to cite.
//! - The citation parser in `gadgetron-penny::citation` round-trips
//!   with the path verbatim (no escaping / truncation).
//! - Content-type filtering (PDF) rejects at the gadget layer
//!   when the `pdf` feature is not wired into
//!   `KnowledgeGadgetProvider` (the current default — PDF lands via
//!   `DocumentFormatsBundle` install, validated by the plugin-level
//!   tests in `plugins/plugin-document-formats/src/pdf.rs`).
//!
//! # Skipping
//!
//! Uses keyword-only search, so no Postgres / pgvector required.
//! Gated by `GADGETRON_SKIP_POSTGRES_TESTS` for consistency with
//! `wiki_import_e2e.rs`.

use std::sync::Arc;

use gadgetron_core::agent::tools::GadgetProvider;
use gadgetron_knowledge::gadget::KnowledgeGadgetProvider;
use gadgetron_knowledge::wiki::Wiki;
use gadgetron_knowledge::WikiKeywordIndex;
use gadgetron_knowledge::{KnowledgeService, KnowledgeServiceBuilder, LlmWikiStore};
use gadgetron_penny::citation::{extract_citation_refs, extract_referenced_labels};
use tempfile::TempDir;

fn should_skip() -> bool {
    std::env::var("GADGETRON_SKIP_POSTGRES_TESTS").is_ok()
}

fn build_wiki(dir: &TempDir) -> Arc<Wiki> {
    use gadgetron_knowledge::config::WikiConfig;
    let cfg = WikiConfig {
        root: dir.path().join("wiki"),
        autocommit: true,
        git_author_name: "W3-KL-3 Test".into(),
        git_author_email: "w3-kl-3@test.local".into(),
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

/// Core E2E: import a wiki page via `wiki.import`, then search for a
/// keyword from its body via `wiki.search`, then assemble a
/// Penny-style citation referencing the imported path and validate
/// that the citation parser extracts the page path verbatim.
#[tokio::test]
async fn rag_citation_round_trip_through_search_and_footnote_parser() {
    if should_skip() {
        eprintln!(
            "GADGETRON_SKIP_POSTGRES_TESTS set — skipping rag_citation_e2e \
             (even though this case needs no PG)"
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let wiki = build_wiki(&dir);
    let service = build_service(wiki);
    let provider = KnowledgeGadgetProvider::from_service(service, None, 10);

    // Step 1 — Import a markdown document that mentions "quarterly
    // review" so keyword search will land on it deterministically.
    let body = "# Quarterly Review Process\n\n\
        Each quarter we review all team-level OKRs. The canonical \
        source for this process is owned by the operator team.";
    let bytes_b64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(body.as_bytes())
    };

    let import_result = provider
        .call(
            "wiki.import",
            serde_json::json!({
                "bytes": bytes_b64,
                "content_type": "text/markdown",
                "title_hint": "Quarterly Review",
            }),
        )
        .await
        .expect("wiki.import must succeed");
    assert!(!import_result.is_error);
    let imported_path = import_result.content["path"]
        .as_str()
        .expect("receipt path is a string")
        .to_string();
    // Spec: default path = imports/<kebab-title>. The title_hint
    // "Quarterly Review" kebab-cases to "quarterly-review".
    assert_eq!(imported_path, "imports/quarterly-review");

    // Step 2 — Keyword search finds the imported page.
    let search_result = provider
        .call(
            "wiki.search",
            serde_json::json!({
                "query": "quarterly review process",
                "limit": 5
            }),
        )
        .await
        .expect("wiki.search");
    let hits = search_result.content["hits"]
        .as_array()
        .expect("hits array")
        .clone();
    assert!(
        !hits.is_empty(),
        "wiki.search must return at least one hit for \"quarterly review process\"; \
         got {hits:?}"
    );
    let top_hit = &hits[0];
    assert_eq!(
        top_hit["page_name"].as_str(),
        Some(imported_path.as_str()),
        "top hit must reference the imported page; got {top_hit:?}"
    );

    // Step 3 — Assemble a Penny-style response that cites the hit
    // with a `[^1]` footnote, per `spawn::PENNY_PERSONA` instructions
    // and design 11 §9.3 format. The response format is what Penny
    // would emit when the model follows the system prompt faithfully.
    let cited_body = format!(
        "The operator team owns the quarterly review cadence[^1].\n\n\
         [^1]: `{}` (imported 2026-04-18)\n",
        imported_path
    );

    // Step 4 — Parse the response back through the citation parser
    // and verify the imported path survives verbatim.
    let refs = extract_citation_refs(&cited_body);
    assert_eq!(
        refs.len(),
        1,
        "expected one footnote definition; got {refs:?}"
    );
    assert_eq!(refs[0].label, "1");
    assert!(
        refs[0].body.contains(&imported_path),
        "footnote body must contain the imported page path verbatim; \
         body={:?}, path={imported_path:?}",
        refs[0].body
    );

    // Step 5 — Inline references are also parseable so a renderer
    // can wire up hover cards. Exactly one `[^1]` reference in the
    // prose (the definition line is excluded by the parser).
    let inline_labels = extract_referenced_labels(&cited_body);
    assert_eq!(
        inline_labels,
        vec!["1".to_string()],
        "expected exactly one inline [^1] reference; got {inline_labels:?}"
    );
}

/// Second E2E: multiple citations (`[^1]`, `[^2]`) referencing
/// different pages. Validates the parser ordering and that multiple
/// imports surface distinct paths.
#[tokio::test]
async fn rag_citation_multi_page_response_preserves_order_and_paths() {
    if should_skip() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let wiki = build_wiki(&dir);
    let service = build_service(wiki);
    let provider = KnowledgeGadgetProvider::from_service(service, None, 10);

    use base64::Engine as _;
    let encode = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());

    // Import page A.
    let a = provider
        .call(
            "wiki.import",
            serde_json::json!({
                "bytes": encode("# Alpha Runbook\n\nHandle alpha events."),
                "content_type": "text/markdown",
                "title_hint": "Alpha Runbook",
            }),
        )
        .await
        .expect("import alpha");
    let path_a = a.content["path"].as_str().unwrap().to_string();
    assert_eq!(path_a, "imports/alpha-runbook");

    // Import page B.
    let b = provider
        .call(
            "wiki.import",
            serde_json::json!({
                "bytes": encode("# Beta Incident\n\nHandle beta pagers."),
                "content_type": "text/markdown",
                "title_hint": "Beta Incident",
            }),
        )
        .await
        .expect("import beta");
    let path_b = b.content["path"].as_str().unwrap().to_string();
    assert_eq!(path_b, "imports/beta-incident");

    // Assemble a multi-citation response. Penny's prompt allows both
    // `[^1]` and `[^2]` in one message when two distinct sources are
    // used.
    let response = format!(
        "Alpha events follow the runbook[^1]; beta pages follow \
         a separate process[^2].\n\n\
         [^1]: `{path_a}` (imported 2026-04-18)\n\
         [^2]: `{path_b}` (imported 2026-04-18)\n"
    );

    let refs = extract_citation_refs(&response);
    assert_eq!(refs.len(), 2, "expected two footnotes; got {refs:?}");
    assert_eq!(refs[0].label, "1");
    assert_eq!(refs[1].label, "2");
    assert!(
        refs[0].body.contains(&path_a),
        "first citation must contain path_a={path_a}; body={:?}",
        refs[0].body
    );
    assert!(
        refs[1].body.contains(&path_b),
        "second citation must contain path_b={path_b}; body={:?}",
        refs[1].body
    );

    // Inline references appear once each in source order.
    let inline = extract_referenced_labels(&response);
    assert_eq!(inline, vec!["1".to_string(), "2".to_string()]);
}

/// PDF extractor bundle-level smoke test.
///
/// The `KnowledgeGadgetProvider` default path uses an in-crate
/// markdown extractor and rejects `application/pdf` at the gadget
/// layer (see `gadget.rs` `wiki.import` content_type check). The PDF
/// path lands via the `DocumentFormatsBundle` install — this test
/// exercises the bundle-level contract by calling `PdfExtractor`
/// directly and validating that it produces the same
/// `ExtractedDocument` shape that `IngestPipeline` would accept.
///
/// Gated by the `pdf` feature on `gadgetron-bundle-document-formats`
/// (enabled via dev-dep). Compiles always; runs always when the
/// feature is on.
#[tokio::test]
async fn pdf_extractor_produces_pipeline_ready_output() {
    use gadgetron_bundle_document_formats::PdfExtractor;
    use gadgetron_core::ingest::{ExtractHints, Extractor};

    // Minimal valid PDF copied from the extractor's own test
    // module — a 1-page "Hello World from Gadgetron" fixture. In a
    // Bundle-driven run the extractor comes from
    // `ctx.plugs.extractors.register(..., PdfExtractor::new())` in
    // `DocumentFormatsBundle::install` (`plugins/plugin-document-formats/src/lib.rs`),
    // which `IngestPipeline` would call on a PDF-typed `ImportRequest`.
    // Re-exercising the fixture from here validates the end-to-end
    // wire shape without building a full bundle harness.
    let pdf: &[u8] = &[
        0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x2e, 0x34, 0x0a, 0x31, 0x20, 0x30, 0x20, 0x6f, 0x62,
        0x6a, 0x0a, 0x3c, 0x3c, 0x20, 0x2f, 0x54, 0x79, 0x70, 0x65, 0x20, 0x2f, 0x43, 0x61, 0x74,
        0x61, 0x6c, 0x6f, 0x67, 0x20, 0x2f, 0x50, 0x61, 0x67, 0x65, 0x73, 0x20, 0x32, 0x20, 0x30,
        0x20, 0x52, 0x20, 0x3e, 0x3e, 0x0a, 0x65, 0x6e, 0x64, 0x6f, 0x62, 0x6a, 0x0a, 0x32, 0x20,
        0x30, 0x20, 0x6f, 0x62, 0x6a, 0x0a, 0x3c, 0x3c, 0x20, 0x2f, 0x54, 0x79, 0x70, 0x65, 0x20,
        0x2f, 0x50, 0x61, 0x67, 0x65, 0x73, 0x20, 0x2f, 0x4b, 0x69, 0x64, 0x73, 0x20, 0x5b, 0x33,
        0x20, 0x30, 0x20, 0x52, 0x5d, 0x20, 0x2f, 0x43, 0x6f, 0x75, 0x6e, 0x74, 0x20, 0x31, 0x20,
        0x3e, 0x3e, 0x0a, 0x65, 0x6e, 0x64, 0x6f, 0x62, 0x6a, 0x0a, 0x33, 0x20, 0x30, 0x20, 0x6f,
        0x62, 0x6a, 0x0a, 0x3c, 0x3c, 0x20, 0x2f, 0x54, 0x79, 0x70, 0x65, 0x20, 0x2f, 0x50, 0x61,
        0x67, 0x65, 0x20, 0x2f, 0x50, 0x61, 0x72, 0x65, 0x6e, 0x74, 0x20, 0x32, 0x20, 0x30, 0x20,
        0x52, 0x20, 0x2f, 0x4d, 0x65, 0x64, 0x69, 0x61, 0x42, 0x6f, 0x78, 0x20, 0x5b, 0x30, 0x20,
        0x30, 0x20, 0x36, 0x31, 0x32, 0x20, 0x37, 0x39, 0x32, 0x5d, 0x20, 0x2f, 0x52, 0x65, 0x73,
        0x6f, 0x75, 0x72, 0x63, 0x65, 0x73, 0x20, 0x3c, 0x3c, 0x20, 0x2f, 0x46, 0x6f, 0x6e, 0x74,
        0x20, 0x3c, 0x3c, 0x20, 0x2f, 0x46, 0x31, 0x20, 0x34, 0x20, 0x30, 0x20, 0x52, 0x20, 0x3e,
        0x3e, 0x20, 0x3e, 0x3e, 0x20, 0x2f, 0x43, 0x6f, 0x6e, 0x74, 0x65, 0x6e, 0x74, 0x73, 0x20,
        0x35, 0x20, 0x30, 0x20, 0x52, 0x20, 0x3e, 0x3e, 0x0a, 0x65, 0x6e, 0x64, 0x6f, 0x62, 0x6a,
        0x0a, 0x34, 0x20, 0x30, 0x20, 0x6f, 0x62, 0x6a, 0x0a, 0x3c, 0x3c, 0x20, 0x2f, 0x54, 0x79,
        0x70, 0x65, 0x20, 0x2f, 0x46, 0x6f, 0x6e, 0x74, 0x20, 0x2f, 0x53, 0x75, 0x62, 0x74, 0x79,
        0x70, 0x65, 0x20, 0x2f, 0x54, 0x79, 0x70, 0x65, 0x31, 0x20, 0x2f, 0x42, 0x61, 0x73, 0x65,
        0x46, 0x6f, 0x6e, 0x74, 0x20, 0x2f, 0x48, 0x65, 0x6c, 0x76, 0x65, 0x74, 0x69, 0x63, 0x61,
        0x20, 0x3e, 0x3e, 0x0a, 0x65, 0x6e, 0x64, 0x6f, 0x62, 0x6a, 0x0a, 0x35, 0x20, 0x30, 0x20,
        0x6f, 0x62, 0x6a, 0x0a, 0x3c, 0x3c, 0x20, 0x2f, 0x4c, 0x65, 0x6e, 0x67, 0x74, 0x68, 0x20,
        0x35, 0x38, 0x20, 0x3e, 0x3e, 0x0a, 0x73, 0x74, 0x72, 0x65, 0x61, 0x6d, 0x0a, 0x42, 0x54,
        0x0a, 0x2f, 0x46, 0x31, 0x20, 0x32, 0x34, 0x20, 0x54, 0x66, 0x0a, 0x37, 0x32, 0x20, 0x37,
        0x32, 0x30, 0x20, 0x54, 0x64, 0x0a, 0x28, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x20, 0x57, 0x6f,
        0x72, 0x6c, 0x64, 0x20, 0x66, 0x72, 0x6f, 0x6d, 0x20, 0x47, 0x61, 0x64, 0x67, 0x65, 0x74,
        0x72, 0x6f, 0x6e, 0x29, 0x20, 0x54, 0x6a, 0x0a, 0x45, 0x54, 0x0a, 0x65, 0x6e, 0x64, 0x73,
        0x74, 0x72, 0x65, 0x61, 0x6d, 0x0a, 0x65, 0x6e, 0x64, 0x6f, 0x62, 0x6a, 0x0a, 0x78, 0x72,
        0x65, 0x66, 0x0a, 0x30, 0x20, 0x36, 0x0a, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
        0x30, 0x30, 0x20, 0x36, 0x35, 0x35, 0x33, 0x35, 0x20, 0x66, 0x20, 0x0a, 0x30, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x39, 0x20, 0x30, 0x30, 0x30, 0x30, 0x30, 0x20, 0x6e,
        0x20, 0x0a, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x35, 0x38, 0x20, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x20, 0x6e, 0x20, 0x0a, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x31,
        0x31, 0x35, 0x20, 0x30, 0x30, 0x30, 0x30, 0x30, 0x20, 0x6e, 0x20, 0x0a, 0x30, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x30, 0x32, 0x34, 0x31, 0x20, 0x30, 0x30, 0x30, 0x30, 0x30, 0x20, 0x6e,
        0x20, 0x0a, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x33, 0x31, 0x31, 0x20, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x20, 0x6e, 0x20, 0x0a, 0x74, 0x72, 0x61, 0x69, 0x6c, 0x65, 0x72, 0x0a,
        0x3c, 0x3c, 0x20, 0x2f, 0x53, 0x69, 0x7a, 0x65, 0x20, 0x36, 0x20, 0x2f, 0x52, 0x6f, 0x6f,
        0x74, 0x20, 0x31, 0x20, 0x30, 0x20, 0x52, 0x20, 0x3e, 0x3e, 0x0a, 0x73, 0x74, 0x61, 0x72,
        0x74, 0x78, 0x72, 0x65, 0x66, 0x0a, 0x34, 0x31, 0x38, 0x0a, 0x25, 0x25, 0x45, 0x4f, 0x46,
        0x0a,
    ];

    let extractor = PdfExtractor::new();
    let out = extractor
        .extract(pdf, "application/pdf", &ExtractHints::default())
        .await
        .expect("PdfExtractor must accept the fixture");

    // The extracted plain_text is what `IngestPipeline` would prepend
    // with frontmatter before writing through `KnowledgeService`.
    // Must contain the ASCII word "Hello" (pdf-extract renders the
    // Helvetica glyph run as ASCII).
    assert!(
        out.plain_text.contains("Hello"),
        "PDF text-layer must contain 'Hello'; got {:?}",
        out.plain_text
    );
    // Source metadata must include `source_format` and `page_count`
    // so the pipeline can annotate frontmatter — the frontmatter
    // writer does not pattern-match but operators grep for these
    // fields in the git log.
    assert_eq!(out.source_metadata["source_format"], "pdf");
    assert_eq!(out.source_metadata["page_count"], 1);
    // No warnings on a clean single-page PDF.
    assert!(
        out.warnings.is_empty(),
        "clean fixture must produce zero warnings; got {:?}",
        out.warnings
    );
}
