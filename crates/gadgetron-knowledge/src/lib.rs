//! gadgetron-knowledge — knowledge layer for the Gadgetron AI assistant.
//!
//! Provides: wiki store (markdown + git), web search proxy (SearXNG),
//! and the knowledge Gadget provider. Consumers: `gadgetron-penny`
//! (Claude Code subprocess) and `gadgetron-cli` (`gadget serve`).

pub mod candidate;
pub mod config;
mod conversation_source;
pub mod embedding;
pub mod error;
pub mod gadget;
pub mod graph;
pub mod ingest;
pub mod keyword_index;
pub mod llm_wiki;
pub mod maintenance;
pub mod ontology;
mod reviewed_vault;
pub mod search;
pub mod semantic;
pub mod semantic_index;
pub mod service;
pub mod source;
pub mod vault;
pub mod wiki;

pub use embedding::{EmbeddingError, EmbeddingProvider, OpenAiCompatEmbedding};
pub use error::{SearchError, WikiError};
pub use gadget::KnowledgeGadgetProvider;
pub use gadgetron_core::error::{KnowledgeErrorKind, WikiErrorKind};
pub use ingest::{ImportReceipt, ImportRequest, IngestPipeline};
pub use keyword_index::WikiKeywordIndex;
pub use llm_wiki::LlmWikiStore;
pub use maintenance::{
    audit_wiki, run_reindex, MaintenanceError, ReindexAction, ReindexActionKind, ReindexMode,
    ReindexOptions, ReindexReport, StalePage, WikiAuditReport,
};
pub use ontology::{
    OntologyActivationAction, OntologyActivationCommand, OntologyActivationEvent,
    OntologyActivationReceipt, OntologyKernel, OntologyMappingCommand, OntologyMappingDisposition,
    OntologyMappingEvent, OntologyPackageRegistration, OntologyRegistrationReceipt,
    OntologyRegistry, OntologyRegistryEntry, OntologyRegistryError, OntologyRevision,
    OntologySchemaRegistration,
};
pub use semantic_index::SemanticPgVectorIndex;
pub use service::{KnowledgeService, KnowledgeServiceBuilder};
