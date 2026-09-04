//! IMAP sync state: per-folder cursors and the UID→message map.
//! UIDs are only meaningful within (account, folder, uidvalidity); the map is
//! rebuilt whenever UIDVALIDITY changes (caller returns `NeedsFull` first).
use super::Db;
use anyhow::Result;
use rusqlite::params;

#[derive(Debug, Clone, PartialEq)]
pub struct FolderCursor {
    pub role: String,
    pub name: String,
    pub uidvalidity: i64,
    pub uidnext: i64,
    pub highestmodseq: Option<i64>,
    pub exists_count: i64,
    pub last_full_scan: Option<i64>,
}

impl Db {
    pub async fn imap_set_folder(&self, account_id: &str, cur: &FolderCursor) -> Result<()> {
        let (a, c) = (account_id.to_string(), cur.clone());
        self.write(move |db| {
            db.execute(
                "INSERT INTO imap_folders (account_id,role,name,uidvalidity,uidnext,highestmodseq,exists_count,last_full_scan) VALUES (?,?,?,?,?,?,?,?) \
                 ON CONFLICT(account_id,role) DO UPDATE SET name=excluded.name, uidvalidity=excluded.uidvalidity, uidnext=excluded.uidnext, highestmodseq=excluded.highestmodseq, exists_count=excluded.exists_count, last_full_scan=excluded.last_full_scan",
                params![a, c.role, c.name, c.uidvalidity, c.uidnext, c.highestmodseq, c.exists_count, c.last_full_scan],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn imap_get_folder(
        &self,
        account_id: &str,
        role: &str,
    ) -> Result<Option<FolderCursor>> {
        let (a, r) = (account_id.to_string(), role.to_string());
        self.read(move |db| {
            let mut s = db.prepare(
                "SELECT role,name,uidvalidity,uidnext,highestmodseq,exists_count,last_full_scan FROM imap_folders WHERE account_id=? AND role=?",
            )?;
            let mut rows = s.query_map(params![a, r], |row| {
                Ok(FolderCursor {
                    role: row.get(0)?,
                    name: row.get(1)?,
                    uidvalidity: row.get(2)?,
                    uidnext: row.get(3)?,
                    highestmodseq: row.get(4)?,
                    exists_count: row.get(5)?,
                    last_full_scan: row.get(6)?,
                })
            })?;
            Ok(rows.next().transpose()?)
        })
        .await
    }

    pub async fn imap_clear_folder(&self, account_id: &str, role: &str) -> Result<()> {
        let (a, r) = (account_id.to_string(), role.to_string());
        self.write(move |db| {
            // imap_uids rows cascade via FOREIGN KEY.
            db.execute(
                "DELETE FROM imap_folders WHERE account_id=? AND role=?",
                params![a, r],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn imap_put_uids(
        &self,
        account_id: &str,
        role: &str,
        pairs: &[(i64, String)],
    ) -> Result<()> {
        let (a, r, ps) = (account_id.to_string(), role.to_string(), pairs.to_vec());
        // Serialized by the write lane; UID diffs self-heal on the next sync.
        self.write(move |db| {
            let mut s = db.prepare(
                "INSERT OR REPLACE INTO imap_uids (account_id,role,uid,message_id) VALUES (?,?,?,?)",
            )?;
            for (uid, mid) in &ps {
                s.execute(params![a, r, uid, mid])?;
            }
            Ok(())
        })
        .await
    }

    pub async fn imap_delete_uids(&self, account_id: &str, role: &str, uids: &[i64]) -> Result<()> {
        let (a, r, us) = (account_id.to_string(), role.to_string(), uids.to_vec());
        self.write(move |db| {
            let mut s =
                db.prepare("DELETE FROM imap_uids WHERE account_id=? AND role=? AND uid=?")?;
            for uid in &us {
                s.execute(params![a, r, uid])?;
            }
            Ok(())
        })
        .await
    }

    /// All (uid, message_id) pairs known for a folder, ordered by uid.
    pub async fn imap_uid_map(&self, account_id: &str, role: &str) -> Result<Vec<(i64, String)>> {
        let (a, r) = (account_id.to_string(), role.to_string());
        self.read(move |db| {
            let mut s = db.prepare(
                "SELECT uid,message_id FROM imap_uids WHERE account_id=? AND role=? ORDER BY uid",
            )?;
            let rows = s
                .query_map(params![a, r], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<(i64, String)>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Every folder/uid holding a message (for resolving op targets).
    pub async fn uids_for_message(
        &self,
        account_id: &str,
        message_id: &str,
    ) -> Result<Vec<(String, i64)>> {
        let (a, m) = (account_id.to_string(), message_id.to_string());
        self.read(move |db| {
            let mut s =
                db.prepare("SELECT role,uid FROM imap_uids WHERE account_id=? AND message_id=?")?;
            let rows = s
                .query_map(params![a, m], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<(String, i64)>, _>>()?;
            Ok(rows)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn p11_imap_folder_cursor_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("u@x.com", None, None).await.unwrap();
        let cur = FolderCursor {
            role: "all".into(),
            name: "[Gmail]/Alle Nachrichten".into(),
            uidvalidity: 987654,
            uidnext: 4523,
            highestmodseq: Some(89123),
            exists_count: 412,
            last_full_scan: Some(1700000000000),
        };
        db.imap_set_folder(&a.id, &cur).await.unwrap();
        assert_eq!(db.imap_get_folder(&a.id, "all").await.unwrap(), Some(cur));
        assert_eq!(db.imap_get_folder(&a.id, "trash").await.unwrap(), None);
    }

    #[tokio::test]
    async fn p11_imap_uid_map_crud() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("u@x.com", None, None).await.unwrap();
        db.imap_set_folder(
            &a.id,
            &FolderCursor {
                role: "all".into(),
                name: "All".into(),
                uidvalidity: 1,
                uidnext: 10,
                highestmodseq: None,
                exists_count: 2,
                last_full_scan: None,
            },
        )
        .await
        .unwrap();
        db.imap_put_uids(&a.id, "all", &[(1, "abc".into()), (2, "def".into())])
            .await
            .unwrap();
        assert_eq!(
            db.imap_uid_map(&a.id, "all").await.unwrap(),
            vec![(1, "abc".into()), (2, "def".into())]
        );
        assert_eq!(
            db.uids_for_message(&a.id, "def").await.unwrap(),
            vec![("all".into(), 2)]
        );
        db.imap_delete_uids(&a.id, "all", &[1]).await.unwrap();
        assert_eq!(
            db.imap_uid_map(&a.id, "all").await.unwrap(),
            vec![(2, "def".into())]
        );
        // cascade on folder clear
        db.imap_clear_folder(&a.id, "all").await.unwrap();
        assert!(db.imap_uid_map(&a.id, "all").await.unwrap().is_empty());
    }
}
