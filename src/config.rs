use std::fmt;
use std::time::Duration;

const DEFAULT_LISTEN_ADDR: &str = ":8080";
const DEFAULT_DB_PATH: &str = "/data/planista.db";
const DEFAULT_MAX_PLAN_BYTES: i64 = 10 << 20;
const DEFAULT_MAX_PLANS: usize = 1000;
const DEFAULT_WIPE_INTERVAL: Duration = Duration::from_secs(120);

#[derive(Clone, Debug)]
pub struct Config {
    pub base_url: String,
    pub listen_addr: String,
    pub db_path: String,
    pub max_plan_bytes: i64,
    pub max_plans: usize,
    pub wipe_interval: Duration,
}

#[derive(Debug)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

pub fn load_config() -> Result<Config, ConfigError> {
    load_config_from(|key| std::env::var(key).ok())
}

pub fn load_config_from(getenv: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
    let base_url = normalize_base_url(getenv("PLANISTA_BASE_URL").unwrap_or_default())?;

    let mut cfg = Config {
        base_url,
        listen_addr: value_or_default(getenv("PLANISTA_LISTEN_ADDR"), DEFAULT_LISTEN_ADDR),
        db_path: value_or_default(getenv("PLANISTA_DB_PATH"), DEFAULT_DB_PATH),
        max_plan_bytes: DEFAULT_MAX_PLAN_BYTES,
        max_plans: DEFAULT_MAX_PLANS,
        wipe_interval: DEFAULT_WIPE_INTERVAL,
    };

    if let Some(value) = getenv("PLANISTA_MAX_PLAN_BYTES").filter(|v| !v.is_empty()) {
        cfg.max_plan_bytes = value
            .parse::<i64>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| {
                ConfigError("PLANISTA_MAX_PLAN_BYTES must be a positive integer".into())
            })?;
    }
    if let Some(value) = getenv("PLANISTA_MAX_PLANS").filter(|v| !v.is_empty()) {
        cfg.max_plans = value
            .parse::<usize>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| ConfigError("PLANISTA_MAX_PLANS must be a positive integer".into()))?;
    }
    if cfg.listen_addr.trim().is_empty() {
        return Err(ConfigError("PLANISTA_LISTEN_ADDR must not be empty".into()));
    }
    if cfg.db_path.trim().is_empty() {
        return Err(ConfigError("PLANISTA_DB_PATH must not be empty".into()));
    }

    Ok(cfg)
}

fn normalize_base_url(raw: String) -> Result<String, ConfigError> {
    if raw.is_empty() {
        return Err(ConfigError("PLANISTA_BASE_URL is required".into()));
    }
    if raw.contains('#') {
        return Err(ConfigError(
            "PLANISTA_BASE_URL must not contain credentials, a query, or a fragment".into(),
        ));
    }

    let uri: http::Uri = raw
        .parse()
        .map_err(|_| ConfigError("parse PLANISTA_BASE_URL: invalid URL".into()))?;

    let scheme = uri.scheme_str().unwrap_or("");
    if (scheme != "http" && scheme != "https") || uri.authority().is_none() {
        return Err(ConfigError(
            "PLANISTA_BASE_URL must be an absolute http or https URL".into(),
        ));
    }
    let authority = uri.authority().unwrap().as_str();
    if authority.contains('@') || uri.query().is_some() {
        return Err(ConfigError(
            "PLANISTA_BASE_URL must not contain credentials, a query, or a fragment".into(),
        ));
    }
    let path = uri.path();
    if !path.is_empty() && path != "/" {
        return Err(ConfigError(
            "PLANISTA_BASE_URL must not contain a path".into(),
        ));
    }

    Ok(format!("{scheme}://{authority}")
        .trim_end_matches('/')
        .to_string())
}

fn value_or_default(value: Option<String>, fallback: &str) -> String {
    match value {
        Some(v) if !v.is_empty() => v,
        _ => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn map_env(values: HashMap<String, String>) -> impl Fn(&str) -> Option<String> {
        move |key| values.get(key).cloned()
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn load_config_defaults() {
        let cfg = load_config_from(map_env(env(&[(
            "PLANISTA_BASE_URL",
            "https://plans.example.com/",
        )])))
        .unwrap();
        assert_eq!(cfg.base_url, "https://plans.example.com");
        assert_eq!(cfg.listen_addr, DEFAULT_LISTEN_ADDR);
        assert_eq!(cfg.db_path, DEFAULT_DB_PATH);
        assert_eq!(cfg.max_plan_bytes, DEFAULT_MAX_PLAN_BYTES);
        assert_eq!(cfg.max_plans, DEFAULT_MAX_PLANS);
        assert_eq!(cfg.wipe_interval, DEFAULT_WIPE_INTERVAL);
    }

    #[test]
    fn load_config_overrides() {
        let cfg = load_config_from(map_env(env(&[
            ("PLANISTA_BASE_URL", "http://127.0.0.1:9090"),
            ("PLANISTA_LISTEN_ADDR", "127.0.0.1:9090"),
            ("PLANISTA_DB_PATH", "/tmp/custom.db"),
            ("PLANISTA_MAX_PLAN_BYTES", "2048"),
            ("PLANISTA_MAX_PLANS", "25"),
        ])))
        .unwrap();
        assert_eq!(cfg.listen_addr, "127.0.0.1:9090");
        assert_eq!(cfg.db_path, "/tmp/custom.db");
        assert_eq!(cfg.max_plan_bytes, 2048);
        assert_eq!(cfg.max_plans, 25);
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn load_config_rejects_invalid_values() {
        let cases: &[(&str, &[(&str, &str)], &str)] = &[
            ("missing base URL", &[], "required"),
            (
                "relative base URL",
                &[("PLANISTA_BASE_URL", "/plans")],
                "absolute",
            ),
            (
                "wrong scheme",
                &[("PLANISTA_BASE_URL", "ftp://example.com")],
                "absolute",
            ),
            (
                "credentials",
                &[("PLANISTA_BASE_URL", "https://user@example.com")],
                "credentials",
            ),
            (
                "query",
                &[("PLANISTA_BASE_URL", "https://example.com?x=1")],
                "query",
            ),
            (
                "path",
                &[("PLANISTA_BASE_URL", "https://example.com/plans")],
                "path",
            ),
            (
                "empty listen address",
                &[
                    ("PLANISTA_BASE_URL", "https://example.com"),
                    ("PLANISTA_LISTEN_ADDR", " "),
                ],
                "LISTEN_ADDR",
            ),
            (
                "empty database path",
                &[
                    ("PLANISTA_BASE_URL", "https://example.com"),
                    ("PLANISTA_DB_PATH", " "),
                ],
                "DB_PATH",
            ),
            (
                "zero bytes",
                &[
                    ("PLANISTA_BASE_URL", "https://example.com"),
                    ("PLANISTA_MAX_PLAN_BYTES", "0"),
                ],
                "MAX_PLAN_BYTES",
            ),
            (
                "bad bytes",
                &[
                    ("PLANISTA_BASE_URL", "https://example.com"),
                    ("PLANISTA_MAX_PLAN_BYTES", "many"),
                ],
                "MAX_PLAN_BYTES",
            ),
            (
                "zero plans",
                &[
                    ("PLANISTA_BASE_URL", "https://example.com"),
                    ("PLANISTA_MAX_PLANS", "0"),
                ],
                "MAX_PLANS",
            ),
        ];

        for (name, pairs, want) in cases {
            let err = load_config_from(map_env(env(pairs))).unwrap_err();
            assert!(
                err.to_string().contains(want),
                "{name}: error = {err}, want substring {want:?}"
            );
        }
    }
}
