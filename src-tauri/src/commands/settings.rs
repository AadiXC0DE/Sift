use crate::app_state::AppState;
use crate::dto::Settings;
use crate::errors::SiftError;
use tauri::State;

#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> Result<Settings, SiftError> {
    state
        .db
        .settings_get()
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

#[tauri::command]
pub async fn settings_set(
    state: State<'_, AppState>,
    patch: serde_json::Value,
) -> Result<Settings, SiftError> {
    state
        .db
        .settings_set(patch)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

#[tauri::command]
pub async fn shortcuts_get(state: State<'_, AppState>) -> Result<serde_json::Value, SiftError> {
    let s: Option<String> = state
        .db
        .read(|c| {
            let mut v: Option<String> = None;
            if let Ok(mut st) = c.prepare("SELECT value FROM settings WHERE key='shortcuts'") {
                if let Ok(mut rows) = st.query([]) {
                    if let Ok(Some(r)) = rows.next() {
                        v = r.get(0).ok();
                    }
                }
            }
            Ok(v)
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    Ok(s.and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_else(crate::keymap_default))
}

#[tauri::command]
pub async fn shortcuts_set(
    state: State<'_, AppState>,
    keymap: serde_json::Value,
) -> Result<serde_json::Value, SiftError> {
    let json = serde_json::to_string(&keymap).unwrap_or_default();
    state
        .db
        .write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO settings (key,value) VALUES ('shortcuts',?)",
                rusqlite::params![json],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    Ok(keymap)
}
