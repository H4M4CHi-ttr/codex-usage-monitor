use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

struct DiagnoseState {
    file: Mutex<File>,
}

static DIAGNOSE_STATE: OnceLock<DiagnoseState> = OnceLock::new();

pub fn init() -> Result<(), String> {
    let path = std::env::temp_dir().join("codex-usage.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("Unable to open diagnostic log file: {e}"))?;

    let _ = DIAGNOSE_STATE.set(DiagnoseState {
        file: Mutex::new(file),
    });

    log("diagnostic logging enabled");
    Ok(())
}

pub fn is_enabled() -> bool {
    DIAGNOSE_STATE.get().is_some()
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|location| {
                let file = std::path::Path::new(location.file())
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("unknown");
                format!("{file}:{}:{}", location.line(), location.column())
            })
            .unwrap_or_else(|| "unknown".to_string());
        log(format!(
            "panic thread={:?} location={location}",
            std::thread::current().id()
        ));
    }));
}

pub fn log(message: impl AsRef<str>) {
    let Some(state) = DIAGNOSE_STATE.get() else {
        return;
    };

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    if let Ok(mut file) = state.file.lock() {
        let line = format!(
            "[{}.{:03}] [pid={}] [thread={:?}] {}\n",
            timestamp.as_secs(),
            timestamp.subsec_millis(),
            std::process::id(),
            std::thread::current().id(),
            message.as_ref()
        );
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

pub fn log_error(context: &str, error: impl std::fmt::Display) {
    log(format!("{context}: {error}"));
}
