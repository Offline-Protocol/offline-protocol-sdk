//! Environment-driven node configuration with conservative defaults.

/// Resolved node configuration.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Stable OfflineID user id of this node (the exchange identity).
    pub user_id: String,
    /// Application id for the protocol.
    pub app_id: String,
    /// Directory for MLS key material and protocol state.
    pub data_dir: String,
    /// HTTP control API bind address. Localhost by default — the control
    /// API is a trusted local surface, never exposed to the network.
    pub bind: String,
    /// HTTP control API port.
    pub port: u16,
    /// Bearer token required on every control-API request. `None` disables
    /// auth (dev mode) with a prominent startup warning.
    pub api_token: Option<String>,
    /// Whether to enable the Internet transport.
    pub internet_enabled: bool,
    /// Internet transport server address (WebSocket/TCP relay).
    pub internet_server: Option<String>,
}

fn parse_port(value: Option<&str>, fallback: u16) -> Result<u16, String> {
    match value {
        None | Some("") => Ok(fallback),
        Some(raw) => raw
            .parse::<u16>()
            .map_err(|_| format!("invalid port: {raw}")),
    }
}

impl NodeConfig {
    /// Loads configuration from the environment.
    pub fn from_env() -> Result<Self, String> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Ok(Self {
            user_id: env("NODE_USER_ID").unwrap_or_else(|| "offline-node".to_string()),
            app_id: env("NODE_APP_ID").unwrap_or_else(|| "capability-exchange".to_string()),
            data_dir: env("NODE_DATA_DIR").unwrap_or_else(|| "./node-data".to_string()),
            bind: env("NODE_BIND").unwrap_or_else(|| "127.0.0.1".to_string()),
            port: parse_port(env("NODE_PORT").as_deref(), 8990)?,
            api_token: env("NODE_API_TOKEN"),
            internet_enabled: env("NODE_INTERNET_ENABLED").as_deref() == Some("true"),
            internet_server: env("NODE_INTERNET_SERVER"),
        })
    }
}
