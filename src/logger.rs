use std::fmt;
use std::io::{self, Write};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Logger {
    out: Mutex<Box<dyn Write + Send>>,
}

impl Logger {
    pub fn stdout() -> Self {
        Self {
            out: Mutex::new(Box::new(io::stdout())),
        }
    }

    pub fn print(&self, message: &str) {
        self.write_line(message);
    }

    pub fn printf(&self, args: fmt::Arguments<'_>) {
        self.write_line(&format!("{args}"));
    }

    pub fn fatalf(&self, args: fmt::Arguments<'_>) -> ! {
        self.write_line(&format!("{args}"));
        std::process::exit(1);
    }

    fn write_line(&self, message: &str) {
        let stamp = go_log_stamp(SystemTime::now());
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(out, "{stamp} {message}");
        let _ = out.flush();
    }
}

fn go_log_stamp(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = secs / 86400;
    let tod = secs % 86400;
    let (y, m, d) = civil_from_days(days as i64);
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    format!("{y:04}/{m:02}/{d:02} {hh:02}:{mm:02}:{ss:02}")
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

pub struct CaptureLogger {
    buf: Mutex<Vec<u8>>,
}

impl CaptureLogger {
    pub fn new() -> Self {
        Self {
            buf: Mutex::new(Vec::new()),
        }
    }

    pub fn print(&self, message: &str) {
        let mut buf = self.buf.lock().unwrap();
        let _ = writeln!(buf, "{message}");
    }

    pub fn printf(&self, args: fmt::Arguments<'_>) {
        self.print(&format!("{args}"));
    }

    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.buf.lock().unwrap()).into_owned()
    }
}

impl Default for CaptureLogger {
    fn default() -> Self {
        Self::new()
    }
}

pub trait Log: Send + Sync {
    fn print(&self, message: &str);
    fn printf(&self, args: fmt::Arguments<'_>);
}

impl Log for Logger {
    fn print(&self, message: &str) {
        Logger::print(self, message);
    }
    fn printf(&self, args: fmt::Arguments<'_>) {
        Logger::printf(self, args);
    }
}

impl Log for CaptureLogger {
    fn print(&self, message: &str) {
        CaptureLogger::print(self, message);
    }
    fn printf(&self, args: fmt::Arguments<'_>) {
        CaptureLogger::printf(self, args);
    }
}
