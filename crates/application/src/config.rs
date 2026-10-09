//! # Config loading (F-008 · #70)
//!
//! Typed config struct deserialised from environment variables.
//!
//! ## Fail-fast contract
//!
//! [`Settings::load`] collects **all** missing/invalid keys before returning
//! an error. A missing key never silently falls back to a default that could
//! mask a misconfiguration.
//!
//! ## Safety-critical validation
//!
//! A disabled kill switch outside `local` or `test` environments is a startup
//! error. The service refuses to start rather than running without the ability
//! to halt money movement (Q35).
//!
//! ## Secrets redaction
//!
//! [`Settings::redacted_display`] logs every key with secrets replaced by
//! `[REDACTED]`. This is used at startup; raw values are never logged.

use std::fmt;

use serde::Deserialize;

// ─────────────────────────────────────────────────────────────────────────────
// Sub-configs
// ─────────────────────────────────────────────────────────────────────────────

/// Postgres connection configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// Full connection URL, e.g. `postgres://user:pass@host:5432/db`.
    pub url: String,
    /// Maximum number of connections in the pool.
    #[serde(default = "DatabaseConfig::default_max_connections")]
    pub max_connections: u32,
    /// Connection acquisition timeout in seconds.
    #[serde(default = "DatabaseConfig::default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
}

impl DatabaseConfig {
    fn default_max_connections() -> u32 {
        10
    }
    fn default_connect_timeout_secs() -> u64 {
        5
    }
}

/// HTTP server configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Bind address, e.g. `0.0.0.0`.
    #[serde(default = "ServerConfig::default_host")]
    pub host: String,
    /// Port to listen on.
    #[serde(default = "ServerConfig::default_port")]
    pub port: u16,
    /// Graceful shutdown drain timeout in seconds (F-009).
    #[serde(default = "ServerConfig::default_shutdown_timeout_secs")]
    pub shutdown_timeout_secs: u64,
}

impl ServerConfig {
    fn default_host() -> String {
        "0.0.0.0".to_string()
    }
    fn default_port() -> u16 {
        8080
    }
    fn default_shutdown_timeout_secs() -> u64 {
        30
    }

    /// Full socket address string.
    #[must_use]
    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Kill-switch configuration (Q35).
///
/// The kill switch must be explicitly enabled. A disabled kill switch is a
/// startup error outside local/test environments.
///
/// The switch's *state* is not configuration — it is a row in Postgres, read on
/// every money path. The distinction matters: config says the mechanism is
/// present and reachable, the row says whether it is currently thrown, and
/// conflating them would mean restarting the process to resume trading. It also
/// means the switch and the money it halts live in one transactional system, so
/// there is no second datastore to fall out of sync with the ledger.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KillSwitchConfig {
    /// Whether the kill switch mechanism is active and reachable.
    /// Must be `true` in staging and production.
    #[serde(default)]
    pub enabled: bool,
    /// The `platform_settings` key that holds the switch state.
    #[serde(default = "KillSwitchConfig::default_state_key")]
    pub state_key: String,
}

impl KillSwitchConfig {
    fn default_state_key() -> String {
        "kill_switch".to_string()
    }
}

/// Observability configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityConfig {
    /// Log level filter, e.g. `"info"`, `"debug,sqlx=warn"`.
    #[serde(default = "ObservabilityConfig::default_log_level")]
    pub log_level: String,
    /// Emit structured JSON logs (true for staging/prod; false for local dev).
    #[serde(default = "ObservabilityConfig::default_json_logs")]
    pub json_logs: bool,
}

impl ObservabilityConfig {
    fn default_log_level() -> String {
        "info".to_string()
    }
    fn default_json_logs() -> bool {
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Environment enum
// ─────────────────────────────────────────────────────────────────────────────

/// Deployment environment.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    /// Local developer machine — relaxed safety rules.
    Local,
    /// Automated test runs — relaxed safety rules.
    Test,
    /// Staging environment — production rules apply.
    Staging,
    /// Production — full safety rules.
    Production,
}

impl Environment {
    /// Returns `true` for environments where safety rules are relaxed.
    #[must_use]
    pub fn is_relaxed(&self) -> bool {
        matches!(self, Self::Local | Self::Test)
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => write!(f, "local"),
            Self::Test => write!(f, "test"),
            Self::Staging => write!(f, "staging"),
            Self::Production => write!(f, "production"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation error
// ─────────────────────────────────────────────────────────────────────────────

/// All validation failures collected during [`Settings::load`].
#[derive(Debug)]
pub struct ConfigError {
    /// Every missing or invalid key, collected before returning.
    pub errors: Vec<String>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Configuration errors ({} found):", self.errors.len())?;
        for e in &self.errors {
            writeln!(f, "  • {e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

// ─────────────────────────────────────────────────────────────────────────────
// Root settings
// ─────────────────────────────────────────────────────────────────────────────

/// The complete, validated application configuration.
///
/// Loaded once at startup from environment variables. Stored in an `Arc`
/// and shared read-only across the application.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Deployment environment.
    pub environment: Environment,
    /// Postgres configuration.
    pub database: DatabaseConfig,
    /// HTTP server configuration.
    pub server: ServerConfig,
    /// Kill-switch configuration (Q35).
    pub kill_switch: KillSwitchConfig,
    /// Observability configuration.
    pub observability: ObservabilityConfig,
}

impl Settings {
    /// Load and validate settings from environment variables.
    ///
    /// Collects **all** missing / invalid keys before returning an error,
    /// naming every problem at once (not just the first).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] listing every validation failure.
    pub fn load() -> Result<Self, ConfigError> {
        // Load .env file if present (dev convenience; ignored in prod).
        let _ = dotenvy::dotenv();

        let mut errors: Vec<String> = Vec::new();

        // Pull required env vars individually so we can report them all.
        // The closure returns `()` because `map_err` is discarding the cause
        // and the real error has already been recorded in `errors`.
        macro_rules! require_env {
            ($key:literal) => {
                std::env::var($key).map_err(|_| {
                    errors.push(format!("missing required env var: {}", $key));
                })
            };
        }

        let environment_str = require_env!("APP_ENVIRONMENT").unwrap_or_default();
        let database_url = require_env!("DATABASE_URL").unwrap_or_default();

        // Parse environment
        let environment: Option<Environment> = if environment_str.is_empty() {
            None
        } else {
            match environment_str.as_str() {
                "local" => Some(Environment::Local),
                "test" => Some(Environment::Test),
                "staging" => Some(Environment::Staging),
                "production" => Some(Environment::Production),
                other => {
                    errors.push(format!(
                        "APP_ENVIRONMENT must be local|test|staging|production, got: {other}"
                    ));
                    None
                }
            }
        };

        // Optional vars with defaults
        let server_host =
            std::env::var("SERVER_HOST").unwrap_or_else(|_| ServerConfig::default_host());
        let server_port: u16 = std::env::var("SERVER_PORT")
            .unwrap_or_else(|_| "8080".to_string())
            .parse()
            .unwrap_or_else(|_| {
                errors.push("SERVER_PORT must be a valid port number (1–65535)".to_string());
                8080
            });
        let shutdown_timeout: u64 = std::env::var("SERVER_SHUTDOWN_TIMEOUT_SECS")
            .unwrap_or_else(|_| "30".to_string())
            .parse()
            .unwrap_or_else(|_| {
                errors.push("SERVER_SHUTDOWN_TIMEOUT_SECS must be a positive integer".to_string());
                30
            });
        let db_max_conn: u32 = std::env::var("DATABASE_MAX_CONNECTIONS")
            .unwrap_or_else(|_| "10".to_string())
            .parse()
            .unwrap_or_else(|_| {
                errors.push("DATABASE_MAX_CONNECTIONS must be a positive integer".to_string());
                10
            });
        let kill_switch_enabled: bool = std::env::var("KILL_SWITCH_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or_else(|_| {
                errors.push("KILL_SWITCH_ENABLED must be true or false".to_string());
                false
            });
        let kill_switch_state_key = std::env::var("KILL_SWITCH_STATE_KEY")
            .unwrap_or_else(|_| KillSwitchConfig::default_state_key());
        let log_level =
            std::env::var("LOG_LEVEL").unwrap_or_else(|_| ObservabilityConfig::default_log_level());
        let json_logs: bool = std::env::var("JSON_LOGS")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);

        // Safety-critical validation: kill switch must be enabled outside local/test
        if let Some(ref env) = environment {
            if !env.is_relaxed() && !kill_switch_enabled {
                errors.push(format!(
                    "KILL_SWITCH_ENABLED must be true in {env} environment (Q35: \
                     a disabled kill switch cannot halt money movement)"
                ));
            }
        }

        if !errors.is_empty() {
            return Err(ConfigError { errors });
        }

        // At this point all required fields are present
        let environment = environment.unwrap_or(Environment::Local);

        Ok(Self {
            environment,
            database: DatabaseConfig {
                url: database_url,
                max_connections: db_max_conn,
                connect_timeout_secs: 5,
            },
            server: ServerConfig {
                host: server_host,
                port: server_port,
                shutdown_timeout_secs: shutdown_timeout,
            },
            kill_switch: KillSwitchConfig {
                enabled: kill_switch_enabled,
                state_key: kill_switch_state_key,
            },
            observability: ObservabilityConfig {
                log_level,
                json_logs,
            },
        })
    }

    /// A [`Display`][fmt::Display]-able view of the settings with all secret
    /// values replaced by `[REDACTED]`.
    ///
    /// Safe to emit at startup. Asserted by tests not to leak secrets.
    #[must_use]
    pub fn redacted_display(&self) -> RedactedSettings<'_> {
        RedactedSettings(self)
    }
}

/// Wrapper that formats [`Settings`] with secrets redacted.
pub struct RedactedSettings<'a>(&'a Settings);

impl fmt::Display for RedactedSettings<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0;
        write!(
            f,
            "Settings {{ environment={env}, \
             database={{ url=[REDACTED], max_connections={max_conn}, \
             connect_timeout_secs={conn_timeout} }}, \
             server={{ host={host}, port={port}, shutdown_timeout_secs={shutdown} }}, \
             kill_switch={{ enabled={ks_enabled}, state_key={ks_key} }}, \
             observability={{ log_level={log_level}, json_logs={json_logs} }} }}",
            env = s.environment,
            max_conn = s.database.max_connections,
            conn_timeout = s.database.connect_timeout_secs,
            host = s.server.host,
            port = s.server.port,
            shutdown = s.server.shutdown_timeout_secs,
            ks_enabled = s.kill_switch.enabled,
            ks_key = s.kill_switch.state_key,
            log_level = s.observability.log_level,
            json_logs = s.observability.json_logs,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// `std::env` is process-global, but `cargo test` runs the tests in this
    /// module concurrently on separate threads. Without this lock one test's
    /// `APP_ENVIRONMENT=staging` lands in another test's `Settings::load`,
    /// and the failure looks like a config bug rather than a test bug.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Take the environment lock, ignoring poisoning: a previous test failing
    /// an assertion says nothing about whether the environment is safe to
    /// rewrite, and refusing to run would cascade the failure.
    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_minimal_env() {
        std::env::set_var("APP_ENVIRONMENT", "local");
        std::env::set_var("DATABASE_URL", "postgres://test:test@localhost/test");
        // Kill switch not required in local
    }

    fn clear_env() {
        for key in &[
            "APP_ENVIRONMENT",
            "DATABASE_URL",
            "SERVER_HOST",
            "SERVER_PORT",
            "SERVER_SHUTDOWN_TIMEOUT_SECS",
            "DATABASE_MAX_CONNECTIONS",
            "KILL_SWITCH_ENABLED",
            "KILL_SWITCH_STATE_KEY",
            "LOG_LEVEL",
            "JSON_LOGS",
        ] {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn loads_successfully_with_minimal_local_env() {
        let _guard = env_guard();
        clear_env();
        set_minimal_env();
        let s = Settings::load().expect("should load");
        assert_eq!(s.environment, Environment::Local);
    }

    #[test]
    fn missing_required_keys_are_all_reported() {
        let _guard = env_guard();
        clear_env();
        // Only set environment, omit DATABASE_URL
        std::env::set_var("APP_ENVIRONMENT", "local");
        let err = Settings::load().expect_err("should fail");
        assert!(
            err.errors.iter().any(|e| e.contains("DATABASE_URL")),
            "expected DATABASE_URL in errors: {err}"
        );
    }

    #[test]
    fn staging_without_kill_switch_is_startup_error() {
        let _guard = env_guard();
        clear_env();
        std::env::set_var("APP_ENVIRONMENT", "staging");
        std::env::set_var("DATABASE_URL", "postgres://test:test@localhost/test");
        std::env::set_var("KILL_SWITCH_ENABLED", "false");
        let err = Settings::load().expect_err("should fail");
        assert!(
            err.errors.iter().any(|e| e.contains("KILL_SWITCH_ENABLED")),
            "expected kill switch error: {err}"
        );
    }

    #[test]
    fn redacted_display_does_not_contain_database_url() {
        let _guard = env_guard();
        clear_env();
        set_minimal_env();
        let s = Settings::load().expect("should load");
        let display = s.redacted_display().to_string();
        assert!(
            !display.contains("postgres://"),
            "DATABASE_URL must not appear in redacted display: {display}"
        );
        assert!(display.contains("[REDACTED]"));
    }

    #[test]
    fn invalid_environment_value_is_reported() {
        let _guard = env_guard();
        clear_env();
        std::env::set_var("APP_ENVIRONMENT", "purple");
        std::env::set_var("DATABASE_URL", "postgres://test:test@localhost/test");
        let err = Settings::load().expect_err("should fail");
        assert!(
            err.errors.iter().any(|e| e.contains("APP_ENVIRONMENT")),
            "expected APP_ENVIRONMENT in errors: {err}"
        );
    }
}
