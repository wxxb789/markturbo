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
            env_logger::init();
            return;
        };
        if let Some(directory) = path.parent()
            && let Err(error) = std::fs::create_dir_all(directory)
        {
            eprintln!("markturbo: cannot create log directory {directory:?}: {error}");
            env_logger::init();
            return;
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        else {
            env_logger::init();
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
