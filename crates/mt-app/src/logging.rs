//! Per-process application logging.
//!
//! The packaged nsh runner installs its own logger before the app starts, so
//! this module never assumes it can replace it. It always writes a startup
//! record to the per-process log file; when no logger is installed yet (the
//! direct `cargo run` path), that logger also targets the file.

use std::io::Write as _;
use std::sync::Once;

use tracing::subscriber::NoSubscriber;

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

/// The crates whose records follow `RUST_LOG`. A dependency's debug record
/// can quote the document it parses (html5ever logs every token), so other
/// crates log errors only, whatever `RUST_LOG` asks for.
const OWN_CRATES: [&str; 3] = ["markturbo", "mt_app", "mt_core"];

fn own_target(target: &str) -> bool {
    OWN_CRATES.iter().any(|name| {
        target
            .strip_prefix(name)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
    })
}

/// `env_logger` with dependencies capped at `Error`.
struct OwnRecords(env_logger::Logger);

impl log::Log for OwnRecords {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        (own_target(metadata.target()) || metadata.level() <= log::Level::Error)
            && self.0.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.0.log(record);
        }
    }

    fn flush(&self) {
        self.0.flush();
    }
}

/// Installs `builder`'s logger behind the dependency cap, unless a logger is
/// already installed.
fn try_install(mut builder: env_logger::Builder) {
    let logger = builder.build();
    let max = logger.filter();
    if log::set_boxed_logger(Box::new(OwnRecords(logger))).is_ok() {
        log::set_max_level(max);
    }
}

/// Claims tracing's dispatcher so dependency events can never reach the log.
///
/// `genai` traces complete provider response bodies. Where a dependency enables
/// tracing's optional `log` bridge (the UI stack has done so on Linux), an
/// unclaimed dispatcher would forward those events to `env_logger` as soon as
/// `RUST_LOG` is raised. A no-op subscriber discards them, and the claim is
/// skipped when a runner already installed its own subscriber.
fn claim_tracing_dispatcher() {
    if tracing::dispatcher::has_been_set() {
        return;
    }
    let _ = tracing::subscriber::set_global_default(NoSubscriber::new());
}

/// Installs file logging when possible and records this process's start.
pub fn init() {
    claim_tracing_dispatcher();
    INIT.call_once(|| {
        let Some(path) = mt_core::runtime_paths::log_path() else {
            try_install(env_logger::Builder::from_default_env());
            return;
        };
        if let Some(directory) = path.parent()
            && let Err(error) = std::fs::create_dir_all(directory)
        {
            eprintln!("markturbo: cannot create log directory {directory:?}: {error}");
            try_install(env_logger::Builder::from_default_env());
            return;
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        else {
            try_install(env_logger::Builder::from_default_env());
            return;
        };

        if let Ok(tee_file) = file.try_clone() {
            let mut builder = env_logger::Builder::from_default_env();
            builder.target(env_logger::Target::Pipe(Box::new(LogTee {
                file: tee_file,
            })));
            try_install(builder);
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
    fn dependency_traces_never_reach_the_application_log() {
        const CHILD_CASE: &str = "MARKTURBO_TRACING_GUARD_TEST";
        const CANARY: &str = "MARKTURBO_TRACING_CANARY";
        if std::env::var_os(CHILD_CASE).is_some() {
            super::init();
            assert!(
                tracing::dispatcher::has_been_set(),
                "the tracing privacy guard must claim the dispatcher"
            );
            // A dropped guard only leaks once a dependency enables tracing's
            // `log` bridge; the canary then names the response body in the log.
            tracing::error!(target: "genai", "{CANARY} provider response body");
            return;
        }

        let temporary = tempfile::tempdir().expect("test data directory");
        let data_dir = temporary.path().join("data");
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "logging::tests::dependency_traces_never_reach_the_application_log",
                "--nocapture",
            ])
            .env(CHILD_CASE, "1")
            .env("MARKTURBO_DATA_DIR", &data_dir)
            .output()
            .expect("run tracing guard child");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let logs = data_dir.join("logs");
        let log = std::fs::read_dir(&logs)
            .expect("the child wrote its log directory")
            .map(|entry| {
                std::fs::read_to_string(entry.expect("log entry").path()).expect("read the log")
            })
            .collect::<String>();
        assert!(
            !log.contains(CANARY),
            "dependency traces reached the application log: {log:?}"
        );
    }

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

    #[test]
    fn a_raised_rust_log_keeps_dependency_debug_records_out_of_the_log() {
        const CHILD_CASE: &str = "MARKTURBO_DEPENDENCY_LOG_TEST";
        const CANARY: &str = "MARKTURBO_DEPENDENCY_LOG_CANARY";
        const OWN: &str = "MARKTURBO_OWN_DEBUG_RECORD";
        if std::env::var_os(CHILD_CASE).is_some() {
            super::init();
            // What html5ever logs while it parses a document's comment.
            log::debug!(target: "html5ever::tree_builder", "processing {CANARY}");
            log::debug!(target: "mt_app::logging", "{OWN}");
            log::logger().flush();
            return;
        }

        let temporary = tempfile::tempdir().expect("test data directory");
        let data_dir = temporary.path().join("data");
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "logging::tests::a_raised_rust_log_keeps_dependency_debug_records_out_of_the_log",
                "--nocapture",
            ])
            .env(CHILD_CASE, "1")
            .env("RUST_LOG", "debug")
            .env("MARKTURBO_DATA_DIR", &data_dir)
            .output()
            .expect("run the logging child");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let log = std::fs::read_dir(data_dir.join("logs"))
            .expect("the child wrote its log directory")
            .map(|entry| {
                std::fs::read_to_string(entry.expect("log entry").path()).expect("read the log")
            })
            .collect::<String>();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            log.contains(OWN),
            "the app's own debug record is kept: {log:?}"
        );
        for (what, text) in [("log", log.as_str()), ("stderr", stderr.as_ref())] {
            assert!(
                !text.contains(CANARY),
                "a dependency's debug record reached the {what}"
            );
        }
    }
}
