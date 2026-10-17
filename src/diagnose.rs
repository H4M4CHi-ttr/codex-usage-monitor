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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_context_and_redacts_panic_payload() {
        const CHILD_ENV: &str = "CODEX_USAGE_DIAGNOSE_TEST_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            init().unwrap();
            log("diagnostic-test-message");
            let previous_hook = std::panic::take_hook();
            install_panic_hook();
            let panic_result = std::panic::catch_unwind(|| panic!("private panic payload"));
            std::panic::set_hook(previous_hook);
            assert!(panic_result.is_err());

            let contents =
                std::fs::read_to_string(std::env::temp_dir().join("codex-usage.log")).unwrap();
            assert!(contents.starts_with("existing diagnostic session\n"));
            assert!(contents.contains("panic thread="));
            assert!(contents.contains("location=diagnose.rs:"));
            assert!(!contents.contains("private panic payload"));
            let line = contents
                .lines()
                .find(|line| line.ends_with("diagnostic-test-message"))
                .unwrap();
            let timestamp = line.split(']').next().unwrap();
            let (seconds, millis) = timestamp.split_once('.').unwrap();
            assert!(seconds.starts_with('['));
            assert!(seconds[1..].bytes().all(|byte| byte.is_ascii_digit()));
            assert_eq!(millis.len(), 3);
            assert!(millis.bytes().all(|byte| byte.is_ascii_digit()));
            assert!(line.contains("[pid="));
            assert!(line.contains("[thread=ThreadId("));
            return;
        }

        // Isolate the global logger and panic hook from parallel tests and real logs.
        let directory = std::env::temp_dir().join(format!(
            "codex-usage-diagnose-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("codex-usage.log");
        std::fs::write(&path, "existing diagnostic session\n").unwrap();
        for _ in 0..2 {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "diagnose::tests::appends_context_and_redacts_panic_payload",
                ])
                .env(CHILD_ENV, "1")
                .env("TEMP", &directory)
                .env("TMP", &directory)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "diagnostic child failed: {output:?}"
            );
        }
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.matches("diagnostic logging enabled").count(), 2);
        assert_eq!(contents.matches("diagnostic-test-message").count(), 2);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
