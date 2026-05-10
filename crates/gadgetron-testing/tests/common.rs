use gadgetron_testing::{
    harness::{gateway::GatewayHarness, pg::PgHarness},
    mocks::provider::FakeLlmProvider,
};
use std::sync::Arc;

/// Standard E2E fixture: PgHarness + GatewayHarness + (tenant_id, raw_api_key).
///
/// Each scenario that uses `E2EFixture::new()` gets a fresh isolated database
/// and a gateway bound to a random port.
///
/// `content`: the string that `FakeLlmProvider` returns as the chat response.
/// `chunks`:  number of SSE chunks `FakeLlmProvider` emits in streaming mode.
pub struct E2EFixture {
    pub pg: PgHarness,
    pub gw: GatewayHarness,
    pub _tenant_id: uuid::Uuid,
    pub api_key: String,
}

impl E2EFixture {
    pub async fn new(content: &str, chunks: usize) -> Self {
        let pg = PgHarness::new().await;
        let (tenant_id, api_key) = pg.insert_test_tenant().await;
        let provider = Arc::new(FakeLlmProvider::new(
            content,
            chunks,
            vec!["gpt-4o-mini".to_string()],
        ));
        let gw = GatewayHarness::start(provider, &pg).await;
        Self {
            pg,
            gw,
            _tenant_id: tenant_id,
            api_key,
        }
    }

    /// Tear down gateway and drop the test database.
    pub async fn teardown(self) {
        self.gw.shutdown().await;
        self.pg.cleanup().await;
    }
}
