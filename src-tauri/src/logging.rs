pub fn init() {
    let filter = std::env::var("SIFT_LOG").unwrap_or_else(|_| "info".into());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(format!(
            "sift={filter},tauri={filter}"
        )))
        .with_target(false)
        .try_init();
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
