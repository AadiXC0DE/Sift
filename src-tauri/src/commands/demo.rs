use crate::errors::SiftError;

/// Screenshot-pipeline driver (spec 18.5). Compiled into every build, but only
/// functional with the `demo` feature (`SIFT_DEMO=1` + demo data); otherwise it
/// refuses so release binaries never expose test hooks.
#[cfg(feature = "demo")]
#[tauri::command]
pub async fn demo_goto(app: tauri::AppHandle, scene: String) -> Result<(), SiftError> {
    use tauri::Emitter;
    app.emit("demo:goto", serde_json::json!({ "scene": scene }))
        .map_err(|e| SiftError::app("demo", e.to_string(), false))
}

#[cfg(not(feature = "demo"))]
#[tauri::command]
pub async fn demo_goto(_app: tauri::AppHandle, _scene: String) -> Result<(), SiftError> {
    Err(SiftError::app("demo", "demo feature not enabled", false))
}
