use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::node::NodeConfig;
use crate::routing::RoutingConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    #[serde(default)]
    pub router: RoutingConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub nodes: Vec<NodeConfig>,
    #[serde(default)]
    pub models: Vec<LocalModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_ms: u64,
}

fn default_request_timeout() -> u64 {
    30_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderConfig {
    Openai {
        api_key: String,
        #[serde(default)]
        base_url: Option<String>,
        models: Vec<String>,
    },
    Anthropic {
        api_key: String,
        #[serde(default)]
        base_url: Option<String>,
        models: Vec<String>,
    },
    Gemini {
        api_key: String,
        models: Vec<String>,
    },
    Ollama {
        endpoint: String,
    },
    Vllm {
        endpoint: String,
        #[serde(default)]
        api_key: Option<String>,
    },
    Sglang {
        endpoint: String,
        #[serde(default)]
        api_key: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelConfig {
    pub id: String,
    pub engine: crate::model::InferenceEngine,
    pub vram_requirement_mb: u64,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            // Bind on all interfaces, port 8080.
            // Env override: GADGETRON_BIND
            bind: "0.0.0.0:8080".to_string(),
            api_key: None,
            request_timeout_ms: 30_000,
        }
    }
}

impl Default for AppConfig {
    /// Built-in defaults used when no gadgetron.toml is present.
    ///
    /// Produces a no-db, no-provider configuration that starts on 0.0.0.0:8080.
    /// Users should run `gadgetron init` to create a proper config file.
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            router: Default::default(),
            providers: Default::default(),
            nodes: vec![],
            models: vec![],
        }
    }
}

impl AppConfig {
    pub fn load(path: &str) -> crate::error::Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            crate::error::GadgetronError::Config(format!(
                "Failed to read config file '{}': {}",
                path, e
            ))
        })?;
        let mut config: AppConfig = toml::from_str(&content).map_err(|e| {
            crate::error::GadgetronError::Config(format!("Failed to parse config: {}", e))
        })?;
        config.resolve_env_vars();
        Ok(config)
    }

    /// Replace ${ENV_VAR} patterns with environment variable values.
    fn resolve_env_vars(&mut self) {
        for provider in self.providers.values_mut() {
            match provider {
                ProviderConfig::Openai { api_key, .. } => {
                    *api_key = Self::expand_env(api_key);
                }
                ProviderConfig::Anthropic { api_key, .. } => {
                    *api_key = Self::expand_env(api_key);
                }
                ProviderConfig::Gemini { api_key, .. } => {
                    *api_key = Self::expand_env(api_key);
                }
                ProviderConfig::Ollama { .. } => {}
                ProviderConfig::Vllm { api_key, .. } => {
                    if let Some(key) = api_key {
                        *key = Self::expand_env(key);
                    }
                }
                ProviderConfig::Sglang { api_key, .. } => {
                    if let Some(key) = api_key {
                        *key = Self::expand_env(key);
                    }
                }
            }
        }
        if let Some(ref key) = self.server.api_key {
            self.server.api_key = Some(Self::expand_env(key));
        }
    }

    fn expand_env(s: &str) -> String {
        if let Some(var_name) = s.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
            std::env::var(var_name).unwrap_or_else(|_| s.to_string())
        } else {
            s.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env() {
        std::env::set_var("GADGETRON_TEST_KEY", "secret123");
        assert_eq!(AppConfig::expand_env("${GADGETRON_TEST_KEY}"), "secret123");
        assert_eq!(AppConfig::expand_env("plain_key"), "plain_key");
        std::env::remove_var("GADGETRON_TEST_KEY");
    }

    #[test]
    fn test_expand_env_missing() {
        assert_eq!(
            AppConfig::expand_env("${NONEXISTENT_VAR}"),
            "${NONEXISTENT_VAR}"
        );
    }
}
