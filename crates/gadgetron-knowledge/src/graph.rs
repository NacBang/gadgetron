//! Deterministic R2.3 note-to-graph parsing primitives.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::source::{parse_obsidian_note, NoteFormatError};
use crate::wiki::link::parse_links;

pub const GRAPH_SCHEMA_VERSION: i32 = 1;
pub const MAX_GRAPH_NODES: usize = 50_000;
pub const MAX_GRAPH_EDGES: usize = 200_000;

const TYPED_PROPERTIES: &[(&str, &str)] = &[
    ("links_to", "links_to"),
    ("supports", "supports"),
    ("contradicts", "contradicts"),
    ("supersedes", "supersedes"),
    ("applies_to", "applies_to"),
    ("derived_from", "derived_from"),
    ("produced_by", "produced_by"),
    ("outcome_of", "outcome_of"),
    ("bridge_to", "bridge_to"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteRelationSpec {
    pub relation_kind: String,
    pub target_ref: String,
    pub evidence_kind: String,
    pub locator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedGraphNote {
    pub title: String,
    pub properties: BTreeMap<String, serde_json::Value>,
    pub relations: Vec<NoteRelationSpec>,
}

pub fn parse_graph_note(
    raw: &str,
    fallback_title: &str,
) -> Result<ParsedGraphNote, NoteFormatError> {
    let parsed = parse_obsidian_note(raw)?;
    let title = parsed
        .properties
        .get("title")
        .and_then(serde_json::Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(fallback_title)
        .trim()
        .chars()
        .take(512)
        .collect();
    let mut unique = BTreeSet::new();
    let mut relations = Vec::new();

    for link in parse_links(&parsed.body) {
        push_relation(
            &mut unique,
            &mut relations,
            NoteRelationSpec {
                relation_kind: "links_to".to_string(),
                target_ref: link.target,
                evidence_kind: "wikilink".to_string(),
                locator: link.heading,
            },
        );
    }
    for (property, relation_kind) in TYPED_PROPERTIES {
        if let Some(value) = parsed.properties.get(*property) {
            for target_ref in string_values(value) {
                push_relation(
                    &mut unique,
                    &mut relations,
                    NoteRelationSpec {
                        relation_kind: (*relation_kind).to_string(),
                        target_ref,
                        evidence_kind: "yaml_property".to_string(),
                        locator: Some((*property).to_string()),
                    },
                );
            }
        }
    }
    for source_id in parsed
        .properties
        .get("source_ids")
        .into_iter()
        .flat_map(string_values)
    {
        for relation_kind in ["cites", "derived_from"] {
            push_relation(
                &mut unique,
                &mut relations,
                NoteRelationSpec {
                    relation_kind: relation_kind.to_string(),
                    target_ref: source_id.clone(),
                    evidence_kind: "source_registry".to_string(),
                    locator: None,
                },
            );
        }
    }
    relations.sort_by(|left, right| {
        left.relation_kind
            .cmp(&right.relation_kind)
            .then_with(|| left.target_ref.cmp(&right.target_ref))
            .then_with(|| left.evidence_kind.cmp(&right.evidence_kind))
            .then_with(|| left.locator.cmp(&right.locator))
    });
    Ok(ParsedGraphNote {
        title,
        properties: parsed.properties,
        relations,
    })
}

pub fn normalized_title(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn stable_edge_id(
    from_node_id: &str,
    relation_kind: &str,
    target_ref: &str,
    producer_revision: i64,
    evidence_kind: &str,
    locator: Option<&str>,
) -> String {
    let canonical = serde_json::json!({
        "evidence_kind": evidence_kind,
        "from": from_node_id,
        "locator": locator,
        "producer_revision": producer_revision,
        "relation": relation_kind,
        "target_ref": target_ref,
    });
    hex::encode(Sha256::digest(
        serde_json::to_vec(&canonical).expect("canonical graph edge JSON"),
    ))
}

fn string_values(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(value) if !value.trim().is_empty() => {
            vec![relation_target(value)]
        }
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(relation_target)
            .collect(),
        _ => Vec::new(),
    }
}

fn relation_target(value: &str) -> String {
    let value = value.trim();
    let links = parse_links(value);
    if value.starts_with("[[") && value.ends_with("]]") && links.len() == 1 {
        links[0].target.clone()
    } else {
        value.to_string()
    }
}

fn push_relation(
    unique: &mut BTreeSet<(String, String, String, Option<String>)>,
    relations: &mut Vec<NoteRelationSpec>,
    relation: NoteRelationSpec,
) {
    let key = (
        relation.relation_kind.clone(),
        relation.target_ref.clone(),
        relation.evidence_kind.clone(),
        relation.locator.clone(),
    );
    if unique.insert(key) {
        relations.push(relation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_parser_is_deterministic_and_ignores_code_links() {
        let raw = r#"---
title:  GPU   Reset
source_ids: [11111111-1111-1111-1111-111111111111]
supports:
  - "[[22222222-2222-2222-2222-222222222222]]"
contradicts: Cooling claim
derived_from: 33333333-3333-3333-3333-333333333333
---
See [[Cooling claim#Evidence]] and `[[ignored]]`.
```text
[[also ignored]]
```
"#;
        let first = parse_graph_note(raw, "fallback").unwrap();
        let second = parse_graph_note(raw, "fallback").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.title, "GPU   Reset");
        assert_eq!(normalized_title(&first.title), "gpu reset");
        assert_eq!(first.relations.len(), 6);
        assert!(first.relations.iter().any(|relation| {
            relation.relation_kind == "supports"
                && relation.target_ref == "22222222-2222-2222-2222-222222222222"
                && relation.evidence_kind == "yaml_property"
        }));
        assert!(!first
            .relations
            .iter()
            .any(|relation| relation.target_ref.contains("ignored")));
    }

    #[test]
    fn stable_edge_identity_changes_with_canonical_revision() {
        let one = stable_edge_id("note:a", "links_to", "note:b", 1, "wikilink", None);
        let same = stable_edge_id("note:a", "links_to", "note:b", 1, "wikilink", None);
        let next = stable_edge_id("note:a", "links_to", "note:b", 2, "wikilink", None);
        assert_eq!(one, same);
        assert_ne!(one, next);
        assert_eq!(one.len(), 64);
    }
}
