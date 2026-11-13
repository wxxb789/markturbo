//! Per-process application logging.
//!
//! The packaged nsh runner installs its own logger before the app starts, so
//! this module never assumes it can replace it. It always writes a startup
//! record to the per-process log file; when no logger is installed yet (the
//! direct `cargo run` path), that logger also targets the file.

use std::io::Write as _;
use std::sync::Once;

/// Writes logger output to both stderr and the per-process application log.
struct LogTee {
    file: std::fs::File,
}

impl std::io::Write for LogTee {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(bytes);
        self.file.write_all(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        self.file.flush()
    }
}

static INIT: Once = Once::new();

/// Installs file logging when possible and records this process's start.
pub fn init() {
    INIT.call_once(|| {
        let Some(path) = mt_core::runtime_paths::log_path() else {
            let _ = env_logger::try_init();
            return;
        };
        if let Some(directory) = path.parent()
            && let Err(error) = std::fs::create_dir_all(directory)
        {
            eprintln!("markturbo: cannot create log directory {directory:?}: {error}");
            let _ = env_logger::try_init();
            return;
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        else {
            let _ = env_logger::try_init();
            return;
        };

        if let Ok(tee_file) = file.try_clone() {
            let _ = env_logger::Builder::from_default_env()
                .target(env_logger::Target::Pipe(Box::new(LogTee {
                    file: tee_file,
                })))
                .try_init();
        }
        let _ = writeln!(
            file,
            "markturbo {} started; pid={}; log={}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            path.display()
        );
        let _ = file.flush();
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn logging_fallbacks_preserve_an_existing_logger() {
        const CHILD_CASE: &str = "MARKTURBO_LOGGING_TEST_CASE";
        if let Some(case) = std::env::var_os(CHILD_CASE) {
            env_logger::try_init().expect("install the runner's logger");
            if case == "file-open" {
                let path = mt_core::runtime_paths::log_path().expect("absolute test root");
                std::fs::create_dir_all(path).expect("block the log file with a directory");
            }
            super::init();
            super::init();
            return;
        }

        let temporary = tempfile::tempdir().expect("test data directory");
        let blocked = temporary.path().join("blocked");
        std::fs::write(&blocked, b"not a directory").expect("block directory creation");
        for (case, data_dir) in [
            (
                "missing-path",
                std::path::PathBuf::from("relative-test-root"),
            ),
            ("directory-create", blocked),
            ("file-open", temporary.path().join("data")),
        ] {
            // Each process gets its own logger and Once without mutating test env.
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "logging::tests::logging_fallbacks_preserve_an_existing_logger",
                    "--nocapture",
                ])
                .env(CHILD_CASE, case)
                .env("MARKTURBO_DATA_DIR", data_dir)
                .output()
                .expect("run logging fallback child");
            assert!(
                output.status.success(),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
