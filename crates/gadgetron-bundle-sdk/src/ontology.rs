use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BundleSdkError, Result};

const MAX_TYPES: usize = 512;
const MAX_RELATIONS: usize = 1_024;
const MAX_ALIASES: usize = 32;
const MAX_MIGRATIONS: usize = 32;
const MAX_MAPPINGS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyNodeFamily {
    Source,
    Claim,
    Entity,
    Event,
    Procedure,
    Decision,
    Lesson,
    Insight,
    Action,
    Outcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyRelationFamily {
    Cites,
    DerivedFrom,
    Supports,
    Contradicts,
    Supersedes,
    AppliesTo,
    ProducedBy,
    OutcomeOf,
    BridgeTo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainOntology {
    pub format_version: u32,
    pub types: Vec<OntologyType>,
    pub relations: Vec<OntologyRelation>,
    #[serde(default)]
    pub migrations: Vec<OntologyMigration>,
    #[serde(default)]
    pub legacy_adapter: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OntologyType {
    pub id: String,
    pub label: String,
    pub family: OntologyNodeFamily,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OntologyRelation {
    pub id: String,
    pub label: String,
    pub family: OntologyRelationFamily,
    #[serde(default)]
    pub source_types: Vec<String>,
    #[serde(default)]
    pub target_types: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OntologyMigration {
    pub from_version: u32,
    pub mappings: Vec<OntologyTypeMigration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyMigrationKind {
    Rename,
    Merge,
    Split,
    Deprecate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OntologyTypeMigration {
    pub kind: OntologyMigrationKind,
    pub from: String,
    #[serde(default)]
    pub to: Vec<String>,
}

impl DomainOntology {
    pub fn parse_json(source: &[u8], schema_version: u32) -> Result<Self> {
        let value: Value = serde_json::from_slice(source).map_err(|error| {
            BundleSdkError::manifest("domain_schema", format!("JSON parse failed: {error}"))
        })?;
        let ontology = if value.get("format_version").is_some() {
            serde_json::from_value(value).map_err(|error| {
                BundleSdkError::manifest(
                    "domain_schema",
                    format!("ontology contract parse failed: {error}"),
                )
            })?
        } else {
            adapt_legacy_schema(&value)?
        };
        ontology.validate(schema_version)?;
        Ok(ontology)
    }

    pub fn validate(&self, schema_version: u32) -> Result<()> {
        if self.format_version != 1 {
            return Err(BundleSdkError::manifest(
                "domain_schema.format_version",
                "only ontology format version 1 is supported",
            ));
        }
        if self.types.is_empty() || self.types.len() > MAX_TYPES {
            return Err(BundleSdkError::manifest(
                "domain_schema.types",
                format!("must contain 1-{MAX_TYPES} types"),
            ));
        }
        if self.relations.len() > MAX_RELATIONS {
            return Err(BundleSdkError::manifest(
                "domain_schema.relations",
                format!("must contain at most {MAX_RELATIONS} relations"),
            ));
        }

        let mut type_ids = BTreeSet::new();
        for (index, item) in self.types.iter().enumerate() {
            validate_item_id(&format!("domain_schema.types[{index}].id"), &item.id)?;
            validate_label(&format!("domain_schema.types[{index}].label"), &item.label)?;
            validate_aliases(
                &format!("domain_schema.types[{index}].aliases"),
                &item.aliases,
            )?;
            if !type_ids.insert(item.id.as_str()) {
                return Err(BundleSdkError::manifest(
                    "domain_schema.types",
                    format!("duplicate type id {:?}", item.id),
                ));
            }
        }

        let mut relation_ids = BTreeSet::new();
        for (index, item) in self.relations.iter().enumerate() {
            validate_item_id(&format!("domain_schema.relations[{index}].id"), &item.id)?;
            validate_label(
                &format!("domain_schema.relations[{index}].label"),
                &item.label,
            )?;
            validate_aliases(
                &format!("domain_schema.relations[{index}].aliases"),
                &item.aliases,
            )?;
            if !relation_ids.insert(item.id.as_str()) {
                return Err(BundleSdkError::manifest(
                    "domain_schema.relations",
                    format!("duplicate relation id {:?}", item.id),
                ));
            }
            validate_type_refs(
                &format!("domain_schema.relations[{index}].source_types"),
                &item.source_types,
                &type_ids,
            )?;
            validate_type_refs(
                &format!("domain_schema.relations[{index}].target_types"),
                &item.target_types,
                &type_ids,
            )?;
        }

        if self.migrations.len() > MAX_MIGRATIONS {
            return Err(BundleSdkError::manifest(
                "domain_schema.migrations",
                format!("must contain at most {MAX_MIGRATIONS} migrations"),
            ));
        }
        let mut from_versions = BTreeSet::new();
        for (index, migration) in self.migrations.iter().enumerate() {
            if migration.from_version == 0 || migration.from_version >= schema_version {
                return Err(BundleSdkError::manifest(
                    format!("domain_schema.migrations[{index}].from_version"),
                    "must reference an earlier positive schema version",
                ));
            }
            if !from_versions.insert(migration.from_version) {
                return Err(BundleSdkError::manifest(
                    "domain_schema.migrations",
                    format!(
                        "duplicate migration from version {}",
                        migration.from_version
                    ),
                ));
            }
            if migration.mappings.len() > MAX_MAPPINGS {
                return Err(BundleSdkError::manifest(
                    format!("domain_schema.migrations[{index}].mappings"),
                    format!("must contain at most {MAX_MAPPINGS} mappings"),
                ));
            }
            let mut sources = BTreeSet::new();
            for (mapping_index, mapping) in migration.mappings.iter().enumerate() {
                validate_item_id(
                    &format!("domain_schema.migrations[{index}].mappings[{mapping_index}].from"),
                    &mapping.from,
                )?;
                if !sources.insert(mapping.from.as_str()) {
                    return Err(BundleSdkError::manifest(
                        format!("domain_schema.migrations[{index}].mappings"),
                        format!("duplicate source type {:?}", mapping.from),
                    ));
                }
                let valid_count = match mapping.kind {
                    OntologyMigrationKind::Rename | OntologyMigrationKind::Merge => {
                        mapping.to.len() == 1
                    }
                    OntologyMigrationKind::Split => mapping.to.len() >= 2,
                    OntologyMigrationKind::Deprecate => mapping.to.is_empty(),
                };
                if !valid_count {
                    return Err(BundleSdkError::manifest(
                        format!("domain_schema.migrations[{index}].mappings[{mapping_index}].to"),
                        "target count does not match the migration kind",
                    ));
                }
                validate_type_refs(
                    &format!("domain_schema.migrations[{index}].mappings[{mapping_index}].to"),
                    &mapping.to,
                    &type_ids,
                )?;
            }
        }
        Ok(())
    }

    pub fn type_by_id(&self, id: &str) -> Option<&OntologyType> {
        self.types.iter().find(|item| item.id == id)
    }
}

fn adapt_legacy_schema(value: &Value) -> Result<DomainOntology> {
    let entities = legacy_const_strings(value, "entities")?;
    let relations = legacy_const_strings(value, "relations")?;
    Ok(DomainOntology {
        format_version: 1,
        types: entities
            .into_iter()
            .map(|id| OntologyType {
                label: humanize(&id),
                family: legacy_node_family(&id),
                id,
                aliases: Vec::new(),
                deprecated: false,
            })
            .collect(),
        relations: relations
            .into_iter()
            .map(|id| OntologyRelation {
                label: humanize(&id),
                family: legacy_relation_family(&id),
                id,
                source_types: Vec::new(),
                target_types: Vec::new(),
                aliases: Vec::new(),
                deprecated: false,
            })
            .collect(),
        migrations: Vec::new(),
        legacy_adapter: true,
    })
}

fn legacy_const_strings(value: &Value, field: &str) -> Result<Vec<String>> {
    value
        .get("properties")
        .and_then(|properties| properties.get(field))
        .and_then(|definition| definition.get("const"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            BundleSdkError::manifest(
                format!("domain_schema.properties.{field}.const"),
                "legacy schema requires an array of strings",
            )
        })?
        .iter()
        .map(|item| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                BundleSdkError::manifest(
                    format!("domain_schema.properties.{field}.const"),
                    "legacy schema entries must be strings",
                )
            })
        })
        .collect()
}

fn legacy_node_family(id: &str) -> OntologyNodeFamily {
    let normalized = id.to_ascii_lowercase();
    [
        ("source", OntologyNodeFamily::Source),
        ("claim", OntologyNodeFamily::Claim),
        ("event", OntologyNodeFamily::Event),
        ("procedure", OntologyNodeFamily::Procedure),
        ("decision", OntologyNodeFamily::Decision),
        ("lesson", OntologyNodeFamily::Lesson),
        ("insight", OntologyNodeFamily::Insight),
        ("action", OntologyNodeFamily::Action),
        ("outcome", OntologyNodeFamily::Outcome),
    ]
    .into_iter()
    .find_map(|(suffix, family)| normalized.ends_with(suffix).then_some(family))
    .unwrap_or(OntologyNodeFamily::Entity)
}

fn legacy_relation_family(id: &str) -> OntologyRelationFamily {
    match id {
        "cites" => OntologyRelationFamily::Cites,
        "derived_from" => OntologyRelationFamily::DerivedFrom,
        "supersedes" => OntologyRelationFamily::Supersedes,
        "produced_by" => OntologyRelationFamily::ProducedBy,
        "outcome_of" => OntologyRelationFamily::OutcomeOf,
        "bridge_to" => OntologyRelationFamily::BridgeTo,
        value if value.starts_with("supports") => OntologyRelationFamily::Supports,
        value if value.starts_with("contradicts") => OntologyRelationFamily::Contradicts,
        _ => OntologyRelationFamily::AppliesTo,
    }
}

fn validate_item_id(field: &str, value: &str) -> Result<()> {
    let valid = (1..=128).contains(&value.len())
        && value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        });
    if valid {
        Ok(())
    } else {
        Err(BundleSdkError::manifest(
            field,
            "must be a 1-128 byte identifier beginning with an ASCII letter",
        ))
    }
}

fn validate_label(field: &str, value: &str) -> Result<()> {
    if !value.trim().is_empty() && value.len() <= 120 {
        Ok(())
    } else {
        Err(BundleSdkError::manifest(
            field,
            "must contain 1-120 UTF-8 bytes",
        ))
    }
}

fn validate_aliases(field: &str, aliases: &[String]) -> Result<()> {
    if aliases.len() > MAX_ALIASES {
        return Err(BundleSdkError::manifest(
            field,
            format!("must contain at most {MAX_ALIASES} aliases"),
        ));
    }
    let mut unique = BTreeSet::new();
    for alias in aliases {
        validate_label(field, alias)?;
        if !unique.insert(alias.as_str()) {
            return Err(BundleSdkError::manifest(
                field,
                "contains a duplicate alias",
            ));
        }
    }
    Ok(())
}

fn validate_type_refs(field: &str, values: &[String], type_ids: &BTreeSet<&str>) -> Result<()> {
    let mut unique = BTreeSet::new();
    for value in values {
        if !type_ids.contains(value.as_str()) {
            return Err(BundleSdkError::manifest(
                field,
                format!("references unknown type {value:?}"),
            ));
        }
        if !unique.insert(value.as_str()) {
            return Err(BundleSdkError::manifest(field, "contains a duplicate type"));
        }
    }
    Ok(())
}

fn humanize(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 4);
    let mut previous_lower = false;
    for character in value.chars() {
        if matches!(character, '_' | '-' | '.') {
            if !output.ends_with(' ') {
                output.push(' ');
            }
            previous_lower = false;
            continue;
        }
        if character.is_ascii_uppercase() && previous_lower {
            output.push(' ');
        }
        output.push(character);
        previous_lower = character.is_ascii_lowercase();
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_const_schema_is_normalized_without_inventing_domain_merges() {
        let source = br#"{
          "properties": {
            "entities": {"const": ["Branch", "VisitOutcome"]},
            "relations": {"const": ["serves", "outcome_of"]}
          }
        }"#;
        let ontology = DomainOntology::parse_json(source, 1).unwrap();
        assert!(ontology.legacy_adapter);
        assert_eq!(ontology.types[0].family, OntologyNodeFamily::Entity);
        assert_eq!(ontology.types[1].family, OntologyNodeFamily::Outcome);
        assert_eq!(
            ontology.relations[0].family,
            OntologyRelationFamily::AppliesTo
        );
        assert_eq!(
            ontology.relations[1].family,
            OntologyRelationFamily::OutcomeOf
        );
    }

    #[test]
    fn explicit_schema_validates_backward_mapping_targets() {
        let source = br#"{
          "format_version": 1,
          "types": [
            {"id":"Place","label":"Place","family":"entity"},
            {"id":"Branch","label":"Branch","family":"entity"},
            {"id":"VisitOutcome","label":"Visit outcome","family":"outcome"}
          ],
          "relations": [{
            "id":"outcome_of","label":"Outcome of","family":"outcome_of",
            "source_types":["VisitOutcome"],"target_types":["Branch"]
          }],
          "migrations": [{
            "from_version":1,
            "mappings":[{"kind":"rename","from":"Restaurant","to":["Place"]}]
          }]
        }"#;
        let ontology = DomainOntology::parse_json(source, 2).unwrap();
        assert_eq!(
            ontology.type_by_id("Place").unwrap().family,
            OntologyNodeFamily::Entity
        );

        let invalid = String::from_utf8(source.to_vec())
            .unwrap()
            .replace("\"Place\"]}", "\"Missing\"]}");
        assert!(DomainOntology::parse_json(invalid.as_bytes(), 2).is_err());
    }

    #[test]
    fn unsupported_format_and_duplicate_types_fail_closed() {
        let unsupported = br#"{
          "format_version":2,
          "types":[{"id":"Place","label":"Place","family":"entity"}],
          "relations":[]
        }"#;
        assert!(DomainOntology::parse_json(unsupported, 1).is_err());

        let duplicate = br#"{
          "format_version":1,
          "types":[
            {"id":"Place","label":"Place","family":"entity"},
            {"id":"Place","label":"Other","family":"entity"}
          ],
          "relations":[]
        }"#;
        assert!(DomainOntology::parse_json(duplicate, 1).is_err());
    }
}
