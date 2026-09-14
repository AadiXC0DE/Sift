use super::Db;
use crate::dto::Settings;
use anyhow::Result;
use rusqlite::params;

const KEY: &str = "settings";
impl Db {
    pub async fn settings_get(&self) -> Result<Settings> {
        let s: Option<String> = self
            .read(|c| {
                let mut v: Option<String> = None;
                let mut st = c.prepare("SELECT value FROM settings WHERE key='settings'")?;
                let mut rows = st.query([])?;
                if let Some(r) = rows.next()? {
                    v = Some(r.get(0)?);
                }
                Ok(v)
            })
            .await?;
        if let Some(json) = s {
            Ok(serde_json::from_str(&json).unwrap_or_default())
        } else {
            Ok(Settings::default())
        }
    }
    pub async fn settings_set(&self, patch: serde_json::Value) -> Result<Settings> {
        let mut cur = serde_json::to_value(self.settings_get().await?)?;
        if let (Some(map), Some(p)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                map.insert(k.clone(), v.clone());
            }
        }
        // Choosing a remote-content policy answers the one-time upgrade prompt
        // in the same write, so the stored document can never say "pending"
        // about a choice the user just made.
        let chose_policy = patch
            .as_object()
            .is_some_and(|p| p.contains_key(super::privacy::MODE_KEY));
        if chose_policy {
            if let Some(map) = cur.as_object_mut() {
                map.insert(super::privacy::PENDING_KEY.into(), serde_json::json!(false));
            }
        }
        let merged: Settings = serde_json::from_value(cur.clone())?;
        let json = serde_json::to_string(&merged)?;
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO settings (key,value) VALUES (?,?)",
                params![KEY, json],
            )?;
            Ok(())
        })
        .await?;
        if chose_policy {
            // A cached rendering was produced under the previous permission:
            // move the generation so no cache can serve it as current.
            self.privacy_bump_generations(None).await?;
        }
        Ok(merged)
    }
    /// Raw key/value cell in the settings table (feature counters, etc.).
    /// Separate keys from the typed `settings` document above.
    pub async fn setting_get_raw(&self, key: &str) -> Result<Option<String>> {
        let k = key.to_string();
        self.read(move |c| {
            let mut v: Option<String> = None;
            if let Ok(mut st) = c.prepare("SELECT value FROM settings WHERE key=?") {
                if let Ok(mut rows) = st.query(rusqlite::params![k]) {
                    if let Ok(Some(r)) = rows.next() {
                        v = r.get(0).ok();
                    }
                }
            }
            Ok(v)
        })
        .await
    }
    pub async fn setting_set_raw(&self, key: &str, value: &str) -> Result<()> {
        let (k, v) = (key.to_string(), value.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO settings (key,value) VALUES (?,?)",
                rusqlite::params![k, v],
            )?;
            Ok(())
        })
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p1_t05_settings_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let s = db.settings_get().await.unwrap();
        assert_eq!(s.theme, "system");
        let s2 = db
            .settings_set(serde_json::json!({"theme":"dark"}))
            .await
            .unwrap();
        assert_eq!(s2.theme, "dark");
        drop(db);
        let db2 = Db::open(dir.path()).unwrap();
        assert_eq!(db2.settings_get().await.unwrap().theme, "dark");
    }

    /// A database written before the updater settings existed must load as
    /// stored. Without a serde default per new field the whole document fails
    /// to deserialize, `settings_get` falls back to `Settings::default()`, and
    /// the user silently loses every preference they had set (P11.1).
    #[tokio::test]
    async fn p11_t01_legacy_settings_document_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        // A document as an older build wrote it: every field it knew about,
        // and no `updates*` key at all.
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy["theme"] = serde_json::json!("dark");
        legacy["accent"] = serde_json::json!("green");
        let map = legacy.as_object_mut().unwrap();
        for key in [
            "updatesAutoCheck",
            "updatesLastCheckAt",
            "updatesLastCheckState",
            "updatesLastCheckVersion",
            "updatesLastCheckError",
        ] {
            map.remove(key);
        }
        db.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO settings (key,value) VALUES ('settings',?)",
                rusqlite::params![legacy.to_string()],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let s = db.settings_get().await.unwrap();
        assert_eq!(s.theme, "dark");
        assert_eq!(s.accent, "green");
        // Automatic checks are on for a fresh install and for an upgrade alike,
        // and nothing has been checked yet.
        assert!(s.updates_auto_check);
        assert_eq!(s.updates_last_check_at, 0);
        assert_eq!(s.updates_last_check_state, "");
        assert_eq!(s.updates_last_check_version, "");
        assert_eq!(s.updates_last_check_error, "");
    }

    /// Recording a check result is a patch. It must not become a second way to
    /// set the user's preference, and it must not reset anything else.
    #[tokio::test]
    async fn p11_t02_recording_a_check_leaves_the_rest_alone() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.settings_set(serde_json::json!({ "theme": "dark", "updatesAutoCheck": false }))
            .await
            .unwrap();

        let s = db
            .settings_set(serde_json::json!({
                "updatesLastCheckAt": 1_700_000_000_000i64,
                "updatesLastCheckState": "available",
                "updatesLastCheckVersion": "1.4.0",
            }))
            .await
            .unwrap();

        assert_eq!(s.theme, "dark");
        assert!(!s.updates_auto_check, "a check result is not a preference");
        assert_eq!(s.updates_last_check_at, 1_700_000_000_000);
        assert_eq!(s.updates_last_check_state, "available");
        assert_eq!(s.updates_last_check_version, "1.4.0");
    }
}
