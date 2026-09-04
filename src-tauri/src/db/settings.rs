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
        Ok(merged)
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
}
