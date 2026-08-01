use std::io::{Cursor, Read};
use std::sync::Mutex;

#[derive(Debug)]
pub struct RandomError;

impl std::fmt::Display for RandomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("could not read random bytes")
    }
}

impl std::error::Error for RandomError {}

pub trait RandomSource: Send + Sync {
    fn fill(&self, buf: &mut [u8]) -> Result<(), RandomError>;
}

pub struct OsRandom;

impl RandomSource for OsRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), RandomError> {
        getrandom::fill(buf).map_err(|_| RandomError)
    }
}

pub struct SeqRandom {
    inner: Mutex<Cursor<Vec<u8>>>,
}

impl SeqRandom {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Mutex::new(Cursor::new(bytes)),
        }
    }
}

impl RandomSource for SeqRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), RandomError> {
        self.inner
            .lock()
            .unwrap()
            .read_exact(buf)
            .map_err(|_| RandomError)
    }
}
