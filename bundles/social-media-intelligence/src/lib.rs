//! Independent Social Media Intelligence Bundle runtime.

use async_trait::async_trait;
use gadgetron_bundle_runtime::{
    broker_host_error, gadget_host_error, BundleGadgetHandler, SharedBundleBroker,
};
use gadgetron_bundle_sdk::{
    BrokerResource, BrokerResourceReadiness, GadgetInvocation, GadgetResult, HealthReport,
    HealthStatus, HostResponse, LocalId,
};

mod social;

pub struct SocialMediaIntelligenceHandler;

#[async_trait]
impl BundleGadgetHandler for SocialMediaIntelligenceHandler {
    async fn health(&self, broker: &SharedBundleBroker) -> HealthReport {
        for table in [
            "social_posts",
            "social_conversations",
            "social_signals",
            "social_briefings",
            "social_response_drafts",
        ] {
            for permission in ["social-read", "social-write"] {
                let resource =
                    BrokerResource::database_table(table).expect("static table resource is valid");
                match broker
                    .lock()
                    .await
                    .probe(
                        LocalId::new(permission).expect("static permission is valid"),
                        resource,
                    )
                    .await
                {
                    Ok(result) if result.readiness == BrokerResourceReadiness::Ready => {}
                    Ok(result) => {
                        return HealthReport::with_message(
                            HealthStatus::Degraded,
                            result
                                .message
                                .unwrap_or_else(|| format!("{table} is unavailable")),
                        );
                    }
                    Err(error) => {
                        return HealthReport::with_message(
                            HealthStatus::Degraded,
                            format!("{table} probe failed: {}", error.public_message()),
                        );
                    }
                }
            }
        }
        let collection = BrokerResource::knowledge_collection()
            .expect("static knowledge collection resource is valid");
        match broker
            .lock()
            .await
            .probe(
                LocalId::new("social-collections").expect("static permission is valid"),
                collection,
            )
            .await
        {
            Ok(result) if result.readiness == BrokerResourceReadiness::Ready => {
                HealthReport::with_message(
                    HealthStatus::Healthy,
                    "Social watchlists and purgeable intelligence storage are ready",
                )
            }
            Ok(result) => HealthReport::with_message(
                HealthStatus::Degraded,
                result
                    .message
                    .unwrap_or_else(|| "Social watchlist collection is unavailable".into()),
            ),
            Err(error) => HealthReport::with_message(
                HealthStatus::Degraded,
                format!("Social watchlist probe failed: {}", error.public_message()),
            ),
        }
    }

    async fn invoke(
        &self,
        invocation: GadgetInvocation,
        broker: &SharedBundleBroker,
    ) -> HostResponse {
        social::invoke(invocation, broker).await
    }
}

pub(crate) fn host_error(code: &str, message: &str) -> HostResponse {
    gadget_host_error(code, message)
}

pub(crate) fn broker_error(error: gadgetron_bundle_runtime::BrokerClientError) -> HostResponse {
    broker_host_error(error)
}

pub(crate) fn gadget_result(value: serde_json::Value) -> HostResponse {
    HostResponse::GadgetResult(GadgetResult::new(value))
}
