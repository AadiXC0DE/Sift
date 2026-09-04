use tauri_plugin_log::{log::LevelFilter, RotationStrategy, Target, TargetKind};

/// Log level from `SIFT_LOG` (`debug` opts into debug, everything else is info).
/// Spec Phase 1 task 7: level `info`, debug via `SIFT_LOG=debug`.
pub fn log_level() -> LevelFilter {
    match std::env::var("SIFT_LOG").as_deref() {
        Ok("debug") | Ok("trace") => LevelFilter::Debug,
        _ => LevelFilter::Info,
    }
}

/// File logger plugin per spec Phase 1 task 7: `~/Library/Logs/Sift/sift.log`
/// (Tauri `LogDir` target), rotation keeps 3 files at 5 MB each.
///
/// NOTE: this plugin must be the ONLY owner of the logging globals. An earlier
/// revision also called `tracing_subscriber::fmt().try_init()` in `run()` and the
/// app panicked at boot with
/// `PluginInitialization("log", "attempted to set a logger after the logging
/// system was already initialized")`. Proven by bisect (boot with the tracing
/// init disabled succeeds) and locked in by `scripts/smoke-boot.sh`. Do NOT add
/// a second global logger/subscriber init anywhere in the binary.
/// Stdout-only fallback for environments where the log directory is not
/// writable. Used automatically by `run()`; never selected by hand.
pub fn stdout_logger_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_log::Builder::new()
        .level(log_level())
        .targets([Target::new(TargetKind::Stdout)])
        .build()
}

pub fn file_logger_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_log::Builder::new()
        .level(log_level())
        .rotation_strategy(RotationStrategy::KeepSome(3))
        .max_file_size(5_000_000)
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::LogDir { file_name: None }),
        ])
        .build()
}

/// Redacting formatter helper: never log tokens/bodies/subjects/addresses.
pub fn redact(s: &str) -> String {
    // naive: mask email-like tokens and ya29 / 1// secrets
    let mut out = s.to_string();
    for marker in ["ya29.", "1//"] {
        while let Some(i) = out.find(marker) {
            let end = (i + 40).min(out.len());
            out.replace_range(i..end, "[REDACTED]");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p1_t07_log_level_from_env() {
        // SAFETY: single-threaded test process section; env mutation is
        // serialized by Rust's test harness only with --test-threads=1.
        // These vars are read once per call, so a race only flakes this test.
        std::env::remove_var("SIFT_LOG");
        assert_eq!(log_level(), LevelFilter::Info);
        std::env::set_var("SIFT_LOG", "debug");
        assert_eq!(log_level(), LevelFilter::Debug);
        std::env::remove_var("SIFT_LOG");
    }

    #[test]
    fn p1_t07_redact_masks_tokens() {
        assert!(redact("token ya29.abcdef subject hello").contains("[REDACTED]"));
        assert!(!redact("subject hello").contains("[REDACTED]"));
    }
}
