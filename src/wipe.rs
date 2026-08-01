use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use subtle::ConstantTimeEq;

use crate::logger::Log;
use crate::random::{OsRandom, RandomSource};

pub const WIPE_TOKEN_BYTES: usize = 24;
pub const WIPE_TOKEN_LENGTH: usize = 32;

pub struct Wiper {
    token: RwLock<String>,
    base_url: String,
    interval: Duration,
    random: Arc<dyn RandomSource>,
    logger: Arc<dyn Log>,
    now: fn() -> SystemTime,
}

impl Wiper {
    pub fn new(base_url: String, interval: Duration, logger: Arc<dyn Log>) -> Result<Self, String> {
        let wiper = Self {
            token: RwLock::new(String::new()),
            base_url,
            interval,
            random: Arc::new(OsRandom),
            logger,
            now: SystemTime::now,
        };
        wiper.rotate()?;
        Ok(wiper)
    }

    pub async fn run(self: Arc<Self>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(err) = self.rotate() {
                        self.logger.printf(format_args!("could not rotate wipe URL: {err}"));
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }

    pub fn rotate(&self) -> Result<(), String> {
        let mut raw = vec![0u8; WIPE_TOKEN_BYTES];
        self.random
            .fill(&mut raw)
            .map_err(|e| format!("generate wipe token: {e}"))?;
        let token = URL_SAFE_NO_PAD.encode(&raw);
        let expires = (self.now)() + self.interval;

        *self.token.write().unwrap() = token.clone();

        let expires_rfc3339 = humantime::format_rfc3339_seconds(
            UNIX_EPOCH
                + Duration::from_secs(
                    expires
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                ),
        )
        .to_string();
        self.logger.printf(format_args!(
            "PLANISTA WIPE (valid until {expires_rfc3339}): curl -fsS -X POST '{}/{token}'",
            self.base_url
        ));
        Ok(())
    }

    pub fn matches(&self, candidate: &str) -> bool {
        let token = self.token.read().unwrap().clone();
        bool::from(candidate.as_bytes().ct_eq(token.as_bytes()))
    }

    pub fn token_for_test(&self) -> String {
        self.token.read().unwrap().clone()
    }

    pub fn set_random_for_test(&mut self, random: Arc<dyn RandomSource>) {
        self.random = random;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::CaptureLogger;
    use crate::random::SeqRandom;

    #[test]
    fn wiper_rotates_and_logs_command() {
        let logs = Arc::new(CaptureLogger::new());
        let wiper = Wiper {
            token: RwLock::new(String::new()),
            base_url: "https://plans.example.com".into(),
            interval: Duration::from_secs(120),
            random: Arc::new(SeqRandom::new(vec![7u8; WIPE_TOKEN_BYTES])),
            logger: logs.clone(),
            now: || UNIX_EPOCH,
        };
        wiper.rotate().unwrap();
        let token = wiper.token_for_test();
        assert_eq!(token.len(), WIPE_TOKEN_LENGTH);
        assert!(wiper.matches(&token));
        let contents = logs.contents();
        assert!(contents.contains("curl -fsS -X POST 'https://plans.example.com/"));
        assert!(contents.contains("1970-01-01T00:02:00Z"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wiper_run_replaces_old_token() {
        let mut bytes = vec![1u8; WIPE_TOKEN_BYTES];
        bytes.extend(vec![2u8; WIPE_TOKEN_BYTES]);
        let wiper = Arc::new(Wiper {
            token: RwLock::new(String::new()),
            base_url: "https://plans.example.com".into(),
            interval: Duration::from_millis(5),
            random: Arc::new(SeqRandom::new(bytes)),
            logger: Arc::new(CaptureLogger::new()),
            now: SystemTime::now,
        });
        wiper.rotate().unwrap();
        let old = wiper.token_for_test();

        let (tx, rx) = tokio::sync::watch::channel(false);
        let runner = Arc::clone(&wiper);
        let handle = tokio::spawn(async move { runner.run(rx).await });

        let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        while wiper.matches(&old) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(
            !wiper.matches(&old),
            "old token remained active after rotation"
        );
        let _ = tx.send(true);
        let _ = handle.await;
    }
}
