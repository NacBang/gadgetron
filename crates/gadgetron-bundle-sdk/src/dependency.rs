use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::{
    BundleDependencies, BundleId, BundlePackageManifest, BundleSdkError, CapabilityId, LocalId,
    ProvidedCapability, Result,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BundleCandidateState {
    Installed,
    EnabledHealthy,
    EnabledUnhealthy,
}

impl BundleCandidateState {
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::EnabledHealthy | Self::EnabledUnhealthy)
    }

    fn is_available_for_plan(self) -> bool {
        self != Self::EnabledUnhealthy
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleDependencyCandidate {
    pub bundle_id: BundleId,
    pub bundle_version: Version,
    pub package_manifest_sha256: String,
    pub state: BundleCandidateState,
    pub provides: Vec<ProvidedCapability>,
    pub dependencies: BundleDependencies,
}

impl BundleDependencyCandidate {
    pub fn from_manifest(
        manifest: &BundlePackageManifest,
        package_manifest_sha256: impl Into<String>,
        state: BundleCandidateState,
    ) -> Result<Self> {
        manifest.validate_structure()?;
        let package_manifest_sha256 = package_manifest_sha256.into();
        validate_sha256(&package_manifest_sha256)?;
        Ok(Self {
            bundle_id: manifest.bundle.id.clone(),
            bundle_version: manifest.bundle.version.clone(),
            package_manifest_sha256,
            state,
            provides: manifest.capabilities.provides.clone(),
            dependencies: manifest.dependencies.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BundleLifecycleChange {
    None,
    Enable { bundle_id: BundleId },
    Disable { bundle_id: BundleId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DependencyRelation {
    Required,
    Optional,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DependencyBindingState {
    Satisfied,
    Clear,
    Missing,
    Incompatible,
    ProviderNotEnabled,
    Unhealthy,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ResolvedCapabilityProvider {
    pub bundle_id: BundleId,
    pub bundle_version: Version,
    pub capability_version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleDependencyBinding {
    pub consumer_bundle_id: BundleId,
    pub relation: DependencyRelation,
    pub capability: CapabilityId,
    pub version: VersionReq,
    pub feature: LocalId,
    pub reason: String,
    pub state: DependencyBindingState,
    pub blocking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ResolvedCapabilityProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DependencyPlanIssueCode {
    RequiredCycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DependencyPlanIssue {
    pub code: DependencyPlanIssueCode,
    pub bundle_ids: Vec<BundleId>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleDependencyPlan {
    pub desired_enabled: Vec<BundleId>,
    pub enable_order: Vec<BundleId>,
    pub bindings: Vec<BundleDependencyBinding>,
    pub issues: Vec<DependencyPlanIssue>,
}

impl BundleDependencyPlan {
    pub fn is_blocked(&self) -> bool {
        !self.issues.is_empty() || self.bindings.iter().any(|binding| binding.blocking)
    }

    pub fn bindings_for(&self, bundle_id: &BundleId) -> Vec<&BundleDependencyBinding> {
        self.bindings
            .iter()
            .filter(|binding| &binding.consumer_bundle_id == bundle_id)
            .collect()
    }
}

pub fn preview_bundle_dependencies(
    candidates: &[BundleDependencyCandidate],
    change: BundleLifecycleChange,
) -> Result<BundleDependencyPlan> {
    let candidates_by_id = validate_candidates(candidates)?;
    let mut desired_enabled: BTreeSet<BundleId> = candidates
        .iter()
        .filter(|candidate| candidate.state.is_enabled())
        .map(|candidate| candidate.bundle_id.clone())
        .collect();
    match change {
        BundleLifecycleChange::None => {}
        BundleLifecycleChange::Enable { bundle_id } => {
            require_candidate(&candidates_by_id, &bundle_id)?;
            desired_enabled.insert(bundle_id);
        }
        BundleLifecycleChange::Disable { bundle_id } => {
            require_candidate(&candidates_by_id, &bundle_id)?;
            desired_enabled.remove(&bundle_id);
        }
    }
    resolve_bundle_dependencies(candidates, &desired_enabled)
}

pub fn resolve_bundle_dependencies(
    candidates: &[BundleDependencyCandidate],
    desired_enabled: &BTreeSet<BundleId>,
) -> Result<BundleDependencyPlan> {
    let candidates_by_id = validate_candidates(candidates)?;
    for bundle_id in desired_enabled {
        require_candidate(&candidates_by_id, bundle_id)?;
    }

    let mut bindings = Vec::new();
    let mut required_edges: BTreeMap<BundleId, BTreeSet<BundleId>> = desired_enabled
        .iter()
        .cloned()
        .map(|bundle_id| (bundle_id, BTreeSet::new()))
        .collect();

    for consumer_id in desired_enabled {
        let consumer = candidates_by_id
            .get(consumer_id)
            .expect("desired candidates were validated");
        for (relation, declarations) in [
            (
                DependencyRelation::Required,
                &consumer.dependencies.requires,
            ),
            (
                DependencyRelation::Optional,
                &consumer.dependencies.optional,
            ),
            (
                DependencyRelation::Conflict,
                &consumer.dependencies.conflicts,
            ),
        ] {
            for declaration in declarations {
                let binding = resolve_declaration(
                    consumer,
                    relation,
                    declaration,
                    candidates,
                    desired_enabled,
                );
                if relation == DependencyRelation::Required
                    && binding.state == DependencyBindingState::Satisfied
                {
                    let provider = binding
                        .provider
                        .as_ref()
                        .expect("satisfied requirement has a provider");
                    required_edges
                        .entry(provider.bundle_id.clone())
                        .or_default()
                        .insert(consumer.bundle_id.clone());
                }
                bindings.push(binding);
            }
        }
    }
    bindings.sort_by(|left, right| {
        left.consumer_bundle_id
            .cmp(&right.consumer_bundle_id)
            .then_with(|| relation_rank(left.relation).cmp(&relation_rank(right.relation)))
            .then_with(|| left.capability.cmp(&right.capability))
    });

    let (enable_order, cycle) = topological_order(desired_enabled, &required_edges);
    let issues = cycle
        .map(|bundle_ids| {
            vec![DependencyPlanIssue {
                code: DependencyPlanIssueCode::RequiredCycle,
                detail: format!(
                    "required dependency cycle involves {}",
                    bundle_ids
                        .iter()
                        .map(BundleId::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                bundle_ids,
            }]
        })
        .unwrap_or_default();

    Ok(BundleDependencyPlan {
        desired_enabled: desired_enabled.iter().cloned().collect(),
        enable_order,
        bindings,
        issues,
    })
}

fn validate_candidates(
    candidates: &[BundleDependencyCandidate],
) -> Result<BTreeMap<BundleId, &BundleDependencyCandidate>> {
    let mut by_id = BTreeMap::new();
    for candidate in candidates {
        validate_sha256(&candidate.package_manifest_sha256)?;
        if by_id
            .insert(candidate.bundle_id.clone(), candidate)
            .is_some()
        {
            return Err(BundleSdkError::manifest(
                "dependency_candidates",
                format!(
                    "duplicate Bundle candidate {:?}",
                    candidate.bundle_id.as_str()
                ),
            ));
        }
    }
    Ok(by_id)
}

fn require_candidate(
    candidates: &BTreeMap<BundleId, &BundleDependencyCandidate>,
    bundle_id: &BundleId,
) -> Result<()> {
    if candidates.contains_key(bundle_id) {
        Ok(())
    } else {
        Err(BundleSdkError::manifest(
            "desired_enabled",
            format!("references unknown Bundle {:?}", bundle_id.as_str()),
        ))
    }
}

fn resolve_declaration(
    consumer: &BundleDependencyCandidate,
    relation: DependencyRelation,
    declaration: &crate::BundleDependencyDeclaration,
    candidates: &[BundleDependencyCandidate],
    desired_enabled: &BTreeSet<BundleId>,
) -> BundleDependencyBinding {
    let targeted: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.bundle_id != consumer.bundle_id)
        .filter(|candidate| {
            declaration
                .provider_bundle
                .as_ref()
                .map_or(true, |bundle_id| &candidate.bundle_id == bundle_id)
        })
        .flat_map(|candidate| {
            candidate
                .provides
                .iter()
                .filter(|provided| provided.id == declaration.capability)
                .map(move |provided| (candidate, provided))
        })
        .collect();
    let mut compatible: Vec<_> = targeted
        .iter()
        .copied()
        .filter(|(candidate, provided)| {
            declaration.version.matches(&provided.version)
                && declaration
                    .provider_version
                    .as_ref()
                    .map_or(true, |requirement| {
                        requirement.matches(&candidate.bundle_version)
                    })
        })
        .collect();
    compatible.sort_by(provider_order);

    let active: Vec<_> = compatible
        .iter()
        .copied()
        .filter(|(candidate, _)| desired_enabled.contains(&candidate.bundle_id))
        .collect();
    let active_healthy: Vec<_> = active
        .iter()
        .copied()
        .filter(|(candidate, _)| candidate.state.is_available_for_plan())
        .collect();

    let (state, provider, blocking) = match relation {
        DependencyRelation::Conflict => {
            if let Some((candidate, provided)) = active.first().copied() {
                (
                    DependencyBindingState::Conflict,
                    Some(provider_projection(candidate, provided)),
                    true,
                )
            } else {
                (DependencyBindingState::Clear, None, false)
            }
        }
        DependencyRelation::Required | DependencyRelation::Optional => {
            let blocking_relation = relation == DependencyRelation::Required;
            if let Some((candidate, provided)) = active_healthy.first().copied() {
                (
                    DependencyBindingState::Satisfied,
                    Some(provider_projection(candidate, provided)),
                    false,
                )
            } else if let Some((candidate, provided)) = active.first().copied() {
                (
                    DependencyBindingState::Unhealthy,
                    Some(provider_projection(candidate, provided)),
                    blocking_relation,
                )
            } else if let Some((candidate, provided)) = compatible.first().copied() {
                (
                    DependencyBindingState::ProviderNotEnabled,
                    Some(provider_projection(candidate, provided)),
                    blocking_relation,
                )
            } else if targeted.is_empty() {
                (DependencyBindingState::Missing, None, blocking_relation)
            } else {
                (
                    DependencyBindingState::Incompatible,
                    None,
                    blocking_relation,
                )
            }
        }
    };

    BundleDependencyBinding {
        consumer_bundle_id: consumer.bundle_id.clone(),
        relation,
        capability: declaration.capability.clone(),
        version: declaration.version.clone(),
        feature: declaration.feature.clone(),
        reason: declaration.reason.clone(),
        state,
        blocking,
        provider,
    }
}

fn provider_projection(
    candidate: &BundleDependencyCandidate,
    provided: &ProvidedCapability,
) -> ResolvedCapabilityProvider {
    ResolvedCapabilityProvider {
        bundle_id: candidate.bundle_id.clone(),
        bundle_version: candidate.bundle_version.clone(),
        capability_version: provided.version.clone(),
    }
}

fn provider_order(
    left: &(&BundleDependencyCandidate, &ProvidedCapability),
    right: &(&BundleDependencyCandidate, &ProvidedCapability),
) -> Ordering {
    right
        .1
        .version
        .cmp(&left.1.version)
        .then_with(|| left.0.bundle_id.cmp(&right.0.bundle_id))
}

fn relation_rank(relation: DependencyRelation) -> u8 {
    match relation {
        DependencyRelation::Required => 0,
        DependencyRelation::Optional => 1,
        DependencyRelation::Conflict => 2,
    }
}

fn topological_order(
    desired_enabled: &BTreeSet<BundleId>,
    edges: &BTreeMap<BundleId, BTreeSet<BundleId>>,
) -> (Vec<BundleId>, Option<Vec<BundleId>>) {
    let mut indegree: BTreeMap<BundleId, usize> = desired_enabled
        .iter()
        .cloned()
        .map(|bundle_id| (bundle_id, 0))
        .collect();
    for consumers in edges.values() {
        for consumer in consumers {
            *indegree.entry(consumer.clone()).or_default() += 1;
        }
    }
    let mut ready: BTreeSet<BundleId> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(bundle_id, _)| bundle_id.clone())
        .collect();
    let mut order = Vec::with_capacity(desired_enabled.len());
    while let Some(bundle_id) = ready.pop_first() {
        order.push(bundle_id.clone());
        if let Some(consumers) = edges.get(&bundle_id) {
            for consumer in consumers {
                let degree = indegree
                    .get_mut(consumer)
                    .expect("edge consumer belongs to desired set");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(consumer.clone());
                }
            }
        }
    }
    if order.len() == desired_enabled.len() {
        (order, None)
    } else {
        let ordered: BTreeSet<_> = order.iter().cloned().collect();
        let unresolved: BTreeSet<_> = desired_enabled.difference(&ordered).cloned().collect();
        let cycle = unresolved
            .iter()
            .filter(|bundle_id| participates_in_cycle(bundle_id, &unresolved, edges))
            .cloned()
            .collect();
        (Vec::new(), Some(cycle))
    }
}

fn participates_in_cycle(
    start: &BundleId,
    unresolved: &BTreeSet<BundleId>,
    edges: &BTreeMap<BundleId, BTreeSet<BundleId>>,
) -> bool {
    let mut visited = BTreeSet::new();
    let mut pending: Vec<_> = edges
        .get(start)
        .into_iter()
        .flatten()
        .filter(|bundle_id| unresolved.contains(*bundle_id))
        .cloned()
        .collect();
    while let Some(bundle_id) = pending.pop() {
        if &bundle_id == start {
            return true;
        }
        if !visited.insert(bundle_id.clone()) {
            continue;
        }
        pending.extend(
            edges
                .get(&bundle_id)
                .into_iter()
                .flatten()
                .filter(|next| unresolved.contains(*next))
                .cloned(),
        );
    }
    false
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(BundleSdkError::manifest(
            "package_manifest_sha256",
            "must be a 64-character lowercase hexadecimal SHA-256 digest",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BundleDependencyDeclaration;

    fn candidate(
        id: &str,
        state: BundleCandidateState,
        provides: &[(&str, &str)],
        requires: &[(&str, &str)],
        optional: &[(&str, &str)],
        conflicts: &[(&str, &str)],
    ) -> BundleDependencyCandidate {
        let dependency = |(capability, feature): &(&str, &str)| BundleDependencyDeclaration {
            capability: CapabilityId::new(*capability).unwrap(),
            version: VersionReq::parse("^1.0").unwrap(),
            feature: LocalId::new(*feature).unwrap(),
            reason: format!("{feature} dependency"),
            provider_bundle: None,
            provider_version: None,
        };
        BundleDependencyCandidate {
            bundle_id: BundleId::new(id).unwrap(),
            bundle_version: Version::new(1, 0, 0),
            package_manifest_sha256: "a".repeat(64),
            state,
            provides: provides
                .iter()
                .map(|(capability, version)| ProvidedCapability {
                    id: CapabilityId::new(*capability).unwrap(),
                    version: Version::parse(version).unwrap(),
                    description: format!("{capability} provider"),
                })
                .collect(),
            dependencies: BundleDependencies {
                requires: requires.iter().map(dependency).collect(),
                optional: optional.iter().map(dependency).collect(),
                conflicts: conflicts.iter().map(dependency).collect(),
            },
        }
    }

    #[test]
    fn required_edges_order_provider_before_consumer_and_choose_deterministically() {
        let candidates = vec![
            candidate(
                "consumer",
                BundleCandidateState::Installed,
                &[],
                &[("gadgetron.intelligence.context", "assisted-mode")],
                &[],
                &[],
            ),
            candidate(
                "provider-b",
                BundleCandidateState::Installed,
                &[("gadgetron.intelligence.context", "1.2.0")],
                &[],
                &[],
                &[],
            ),
            candidate(
                "provider-a",
                BundleCandidateState::Installed,
                &[("gadgetron.intelligence.context", "1.2.0")],
                &[],
                &[],
                &[],
            ),
        ];
        let desired = candidates
            .iter()
            .map(|item| item.bundle_id.clone())
            .collect();
        let plan = resolve_bundle_dependencies(&candidates, &desired).unwrap();
        assert!(!plan.is_blocked());
        assert_eq!(
            plan.bindings[0]
                .provider
                .as_ref()
                .unwrap()
                .bundle_id
                .as_str(),
            "provider-a"
        );
        assert!(
            plan.enable_order
                .iter()
                .position(|id| id.as_str() == "provider-a")
                < plan
                    .enable_order
                    .iter()
                    .position(|id| id.as_str() == "consumer")
        );
    }

    #[test]
    fn optional_loss_is_nonblocking_but_required_loss_and_conflict_block() {
        let optional_consumer = candidate(
            "travel-planner",
            BundleCandidateState::EnabledHealthy,
            &[],
            &[],
            &[(
                "gadgetron.intelligence.restaurant-context",
                "restaurant-assist",
            )],
            &[],
        );
        let restaurant = candidate(
            "restaurant-research",
            BundleCandidateState::EnabledHealthy,
            &[("gadgetron.intelligence.restaurant-context", "1.0.0")],
            &[],
            &[],
            &[],
        );
        let loss = preview_bundle_dependencies(
            &[optional_consumer.clone(), restaurant.clone()],
            BundleLifecycleChange::Disable {
                bundle_id: restaurant.bundle_id.clone(),
            },
        )
        .unwrap();
        assert!(!loss.is_blocked());
        assert_eq!(
            loss.bindings[0].state,
            DependencyBindingState::ProviderNotEnabled
        );

        let required_consumer = candidate(
            "required-consumer",
            BundleCandidateState::EnabledHealthy,
            &[],
            &[(
                "gadgetron.intelligence.restaurant-context",
                "required-assist",
            )],
            &[],
            &[],
        );
        let blocked = preview_bundle_dependencies(
            &[required_consumer, restaurant.clone()],
            BundleLifecycleChange::Disable {
                bundle_id: restaurant.bundle_id.clone(),
            },
        )
        .unwrap();
        assert!(blocked.is_blocked());

        let conflict = candidate(
            "conflicting-consumer",
            BundleCandidateState::EnabledHealthy,
            &[],
            &[],
            &[],
            &[(
                "gadgetron.intelligence.restaurant-context",
                "exclusive-mode",
            )],
        );
        let conflict_plan =
            preview_bundle_dependencies(&[conflict, restaurant], BundleLifecycleChange::None)
                .unwrap();
        assert!(conflict_plan.is_blocked());
        assert_eq!(
            conflict_plan.bindings[0].state,
            DependencyBindingState::Conflict
        );
    }

    #[test]
    fn required_cycle_has_no_misleading_enable_order() {
        let left = candidate(
            "left",
            BundleCandidateState::EnabledHealthy,
            &[("example.left", "1.0.0")],
            &[("example.right", "right-required")],
            &[],
            &[],
        );
        let right = candidate(
            "right",
            BundleCandidateState::EnabledHealthy,
            &[("example.right", "1.0.0")],
            &[("example.left", "left-required")],
            &[],
            &[],
        );
        let downstream = candidate(
            "downstream",
            BundleCandidateState::EnabledHealthy,
            &[],
            &[("example.left", "left-context")],
            &[],
            &[],
        );
        let plan =
            preview_bundle_dependencies(&[left, right, downstream], BundleLifecycleChange::None)
                .unwrap();
        assert!(plan.is_blocked());
        assert!(plan.enable_order.is_empty());
        assert_eq!(plan.issues[0].code, DependencyPlanIssueCode::RequiredCycle);
        assert_eq!(
            plan.issues[0]
                .bundle_ids
                .iter()
                .map(BundleId::as_str)
                .collect::<Vec<_>>(),
            ["left", "right"]
        );
    }
}
