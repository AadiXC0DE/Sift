//! The account-aware label service (P8.5).
//!
//! One service answers every label question: the sidebar, the label picker and
//! the rule editor all read the same rows through the same hierarchy rules, so
//! two surfaces cannot disagree about a label's name, its nested depth or its
//! colour. Provider ids are an implementation detail — they are the key, never
//! the display.
//!
//! Rename and delete are local-first and durable. The local change and the
//! queued provider operation commit in one transaction, so an offline rename
//! is not a lie: the label really is renamed in Sift, and the server catches up
//! when it can. For a transport whose label id is derived from the name (IMAP
//! folders) the id moves with the name, and everything that referred to the old
//! id — message membership, the denormalised label lists and **pending
//! operations** — moves in the same transaction. A queued change can therefore
//! never address the label's old identity after the rename.
//!
//! Deleting a label removes organisation only. Messages keep their place, their
//! flags and every other label; if the provider call fails, the local
//! organisation is already gone and the operation reports the failure rather
//! than pretending otherwise.

use crate::db::Db;
use crate::dto::{label_hierarchy, Label};
use crate::errors::SiftError;

/// The labels a provider defines itself. They cannot be renamed or deleted:
/// Sift maps them onto folders and flags.
pub const SYSTEM_LABELS: [&str; 8] = [
    "INBOX",
    "SENT",
    "DRAFT",
    "STARRED",
    "IMPORTANT",
    "TRASH",
    "SPAM",
    "UNREAD",
];

/// The longest label name Gmail accepts.
pub const MAX_LABEL_NAME_LEN: usize = 225;

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

pub fn is_system_label(id: &str) -> bool {
    SYSTEM_LABELS.contains(&id)
}

/// A name Sift is willing to store and send.
///
/// Slashes are meaningful (they nest a label), so leading, trailing and doubled
/// separators are refused rather than silently normalised: a rename that
/// produced a different hierarchy than the user typed would be a quiet lie.
pub fn valid_label_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_LABEL_NAME_LEN {
        return false;
    }
    if trimmed != name {
        return false;
    }
    if trimmed.starts_with('/') || trimmed.ends_with('/') || trimmed.contains("//") {
        return false;
    }
    !trimmed
        .chars()
        .any(|c| c.is_control() || c == '\n' || c == '\r')
}

/// Where a label's id goes when its name changes.
///
/// An opaque provider id (Gmail's `Label_...`) stays as it is. A name-derived
/// id — an IMAP folder (`imap:`) or a not-yet-created placeholder
/// (`sift-local:`) — follows the name.
pub fn renamed_id(account_id: &str, old_id: &str, new_name: &str) -> String {
    if old_id.starts_with("imap:") {
        format!("imap:{new_name}")
    } else if old_id.starts_with("sift-local:") {
        format!("sift-local:{account_id}:{new_name}")
    } else {
        old_id.to_string()
    }
}

/// Whether this label exists only in Sift (a placeholder for a label that has
/// not been created on the server yet).
pub fn is_local_placeholder(id: &str) -> bool {
    id.starts_with("sift-local:")
}

impl Db {
    pub async fn label_get(
        &self,
        account_id: &str,
        label_id: &str,
    ) -> Result<Option<Label>, SiftError> {
        let (a, id) = (account_id.to_string(), label_id.to_string());
        self.read(move |c| {
            let row = c
                .query_row(
                    "SELECT account_id,id,name,kind,color_bg,color_fg,visible,unread_count,total_count,sort_order \
                     FROM labels WHERE account_id=? AND id=?",
                    rusqlite::params![a, id],
                    |r| {
                        Ok(Label {
                            account_id: r.get(0)?,
                            id: r.get(1)?,
                            name: r.get(2)?,
                            kind: r.get(3)?,
                            color_bg: r.get(4)?,
                            color_fg: r.get(5)?,
                            visible: r.get::<_, i64>(6)? != 0,
                            unread_count: r.get(7)?,
                            total_count: r.get(8)?,
                            sort_order: r.get(9)?,
                            ..Default::default()
                        })
                    },
                )
                .ok();
            Ok(row)
        })
        .await
        .map_err(db_error)
    }
}

/// Every label of one account, in the order the sidebar shows them, with the
/// hierarchy filled in: the display leaf, the nesting depth and the parent's id
/// resolved from the account's own label set.
pub async fn list(db: &Db, account_id: &str) -> Result<Vec<Label>, SiftError> {
    let mut labels = db.labels_list(account_id).await.map_err(db_error)?;
    let by_name: Vec<(String, String)> = labels
        .iter()
        .map(|l| (l.name.clone(), l.id.clone()))
        .collect();
    for label in &mut labels {
        let (leaf, depth, parent_name) = label_hierarchy(&label.name);
        label.display_name = leaf;
        label.depth = depth;
        label.parent_id = parent_name.and_then(|name| {
            by_name
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, id)| id.clone())
        });
    }
    Ok(labels)
}

/// The colour a rename must not lose.
#[derive(Debug, Clone, Default)]
pub struct LabelChange {
    pub op_id: Option<i64>,
    pub previous_name: String,
}

/// Rename a label, locally and durably.
pub async fn rename(
    db: &Db,
    account_id: &str,
    label_id: &str,
    new_name: &str,
    gesture_id: &str,
) -> Result<Label, SiftError> {
    if !valid_label_name(new_name) {
        return Err(SiftError::app(
            "bad_label_name",
            "A label name cannot be empty or start or end with a slash, and cannot be longer than 225 characters.",
            false,
        ));
    }
    let existing = db
        .label_get(account_id, label_id)
        .await?
        .ok_or_else(|| SiftError::NotFound("label".into()))?;
    if is_system_label(&existing.id) {
        return Err(SiftError::app(
            "unsupported_operation",
            "Sift cannot rename a system label.",
            false,
        ));
    }
    if existing.name == new_name {
        return Ok(existing);
    }
    let new_id = renamed_id(account_id, &existing.id, new_name);
    let lookup_id = new_id.clone();
    let (account, old_id, old_name) = (
        account_id.to_string(),
        existing.id.clone(),
        existing.name.clone(),
    );
    let new_name_owned = new_name.to_string();
    let gesture = gesture_id.to_string();
    db.write_tx(move |tx| {
        /// One label row moving to a new id and/or name.
        struct Move {
            old_id: String,
            old_name: String,
            new_id: String,
            new_name: String,
        }

        let mut moves: Vec<Move> = vec![Move {
            old_id: old_id.clone(),
            old_name: old_name.clone(),
            new_id: new_id.clone(),
            new_name: new_name_owned.clone(),
        }];
        // A nested label's name *is* its path, so the subtree moves with the
        // parent. Leaving the children behind would leave them named by a path
        // that no longer exists — a depth-1 label with no parent in the tree.
        {
            let mut s = tx.prepare(
                "SELECT id, name FROM labels \
                 WHERE account_id=?1 AND substr(name, 1, length(?2) + 1) = ?2 || '/' \
                 ORDER BY name",
            )?;
            let rows = s
                .query_map(rusqlite::params![account, old_name], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<(String, String)>, _>>()?;
            for (child_id, child_name) in rows {
                let moved_name = format!("{new_name_owned}{}", &child_name[old_name.len()..]);
                let moved_id = renamed_id(&account, &child_id, &moved_name);
                moves.push(Move {
                    old_id: child_id,
                    old_name: child_name,
                    new_id: moved_id,
                    new_name: moved_name,
                });
            }
        }

        // Account-local duplicate names are a collision, not a merge: the
        // provider would refuse it (Gmail) or silently create two folders
        // (IMAP), so it is refused here with a message the UI can show.
        for m in &moves {
            let clash: Option<String> = tx
                .query_row(
                    "SELECT id FROM labels WHERE account_id=? AND name=? AND id<>?",
                    rusqlite::params![account, m.new_name, m.old_id],
                    |r| r.get(0),
                )
                .ok();
            if let Some(other) = clash {
                return Err(anyhow::anyhow!(SiftError::app(
                    "label_exists",
                    format!(
                        "This account already has a label called \"{}\" (id {other}).",
                        m.new_name
                    ),
                    false,
                )));
            }
        }

        let mut touched: Vec<(String, String)> = vec![];
        for m in &moves {
            if m.new_id == m.old_id {
                tx.execute(
                    "UPDATE labels SET name=? WHERE account_id=? AND id=?",
                    rusqlite::params![m.new_name, account, m.old_id],
                )?;
            } else {
                tx.execute(
                    "UPDATE labels SET id=?, name=? WHERE account_id=? AND id=?",
                    rusqlite::params![m.new_id, m.new_name, account, m.old_id],
                )?;
                // The label id is the membership key: every reference moves
                // with it.
                tx.execute(
                    "UPDATE message_labels SET label_id=? WHERE account_id=? AND label_id=?",
                    rusqlite::params![m.new_id, account, m.old_id],
                )?;
            }
            // Denormalised copies are recomputed from the membership rows, so
            // the list, a label view and the picker all see one state.
            let rows = {
                let mut s = tx.prepare(
                    "SELECT m.id, m.thread_id FROM messages m \
                     WHERE m.account_id=? AND m.thread_id IN ( \
                       SELECT thread_id FROM messages WHERE account_id=?) \
                       AND EXISTS (SELECT 1 FROM message_labels ml \
                                   WHERE ml.account_id=m.account_id AND ml.message_id=m.id AND ml.label_id=?)",
                )?;
                let rows = s
                    .query_map(rusqlite::params![account, account, m.new_id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<(String, String)>, _>>()?;
                rows
            };
            touched.extend(rows);
        }
        touched.sort();
        touched.dedup();
        for (message_id, _) in &touched {
            crate::actions::refresh_message_flags(tx, &account, message_id)?;
        }
        let mut threads: Vec<String> = touched.iter().map(|(_, t)| t.clone()).collect();
        threads.sort();
        threads.dedup();
        for thread_id in &threads {
            crate::db::threads::recompute_thread_conn(tx, &account, thread_id)?;
        }

        // Pending work must address the new identity, in this transaction.
        for m in &moves {
            rewrite_pending_references(tx, &account, &m.old_id, &m.new_id, &m.old_name, &m.new_name)?;
        }

        // The transport is told about the label the user renamed, and about a
        // descendant only when its id is derived from its name (IMAP). An
        // opaque-id transport (Gmail) renames the whole subtree itself, and a
        // redundant folder rename there would name no folder.
        let mut first_op: Option<i64> = None;
        for (index, m) in moves.iter().enumerate() {
            let needs_remote = index == 0 || m.new_id != m.old_id;
            if !needs_remote || is_local_placeholder(&m.old_id) {
                continue;
            }
            let payload = serde_json::json!({
                "id": m.old_id,
                "name": m.new_name,
                "previousName": m.old_name,
            })
            .to_string();
            let op = crate::db::outbox::NewOp {
                account_id: account.clone(),
                kind: "label_rename".into(),
                payload,
                undo_group: Some(gesture.clone()),
                not_before: 0,
                summary_action: Some(format!("Renaming label to {}", m.new_name)),
                ..Default::default()
            };
            let inserted = crate::db::outbox::insert_op(tx, &op)?.id;
            if first_op.is_none() {
                first_op = Some(inserted);
            }
        }
        Ok(first_op)
    })
    .await
    .map_err(|e| match e.downcast::<SiftError>() {
        Ok(sift) => sift,
        Err(other) => db_error(other),
    })?;

    let mut updated = db
        .label_get(account_id, &lookup_id)
        .await?
        .ok_or_else(|| SiftError::NotFound("label".into()))?;
    let (leaf, depth, _) = label_hierarchy(&updated.name);
    updated.display_name = leaf;
    updated.depth = depth;
    Ok(updated)
}

/// Point every pending operation at a label's new id and name.
///
/// Two things can reference a label in the queue: the label id inside a
/// `modify_labels`/`rule_apply` diff (add/remove) and the symbolic
/// `name:<name>` form the snooze path uses. Both are rewritten here, and a
/// `create_label` queued for the old name is renamed so a later create does not
/// resurrect it under the name the user just changed.
fn rewrite_pending_references(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    old_id: &str,
    new_id: &str,
    old_name: &str,
    new_name: &str,
) -> anyhow::Result<()> {
    let rows: Vec<(i64, String, String)> = {
        let mut s = tx.prepare(
            "SELECT id, kind, payload FROM outbox_ops \
             WHERE account_id=? AND state IN ('pending','inflight','uncertain')",
        )?;
        let rows = s
            .query_map(rusqlite::params![account_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<(i64, String, String)>, _>>()?;
        rows
    };
    let old_ref = format!("name:{old_name}");
    let new_ref = format!("name:{new_name}");
    let mut update = tx.prepare("UPDATE outbox_ops SET payload=?1 WHERE id=?2")?;
    for (id, kind, payload) in rows {
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        let mut changed = false;
        if kind == "create_label" && value.get("name").and_then(|n| n.as_str()) == Some(old_name) {
            value["name"] = serde_json::Value::String(new_name.to_string());
            changed = true;
        }
        for key in ["add", "remove"] {
            if let Some(list) = value.get_mut(key).and_then(|l| l.as_array_mut()) {
                for entry in list.iter_mut() {
                    let Some(text) = entry.as_str() else { continue };
                    if text == old_id {
                        *entry = serde_json::Value::String(new_id.to_string());
                        changed = true;
                    } else if text == old_ref {
                        *entry = serde_json::Value::String(new_ref.clone());
                        changed = true;
                    }
                }
            }
        }
        if changed {
            update.execute(rusqlite::params![value.to_string(), id])?;
        }
    }
    Ok(())
}

/// Delete a label's organisation, never its mail.
pub async fn delete(db: &Db, account_id: &str, label_id: &str) -> Result<Option<i64>, SiftError> {
    let existing = db
        .label_get(account_id, label_id)
        .await?
        .ok_or_else(|| SiftError::NotFound("label".into()))?;
    if is_system_label(&existing.id) {
        return Err(SiftError::app(
            "unsupported_operation",
            "Sift cannot delete a system label.",
            false,
        ));
    }
    let (account, id, name) = (
        account_id.to_string(),
        existing.id.clone(),
        existing.name.clone(),
    );
    db.write_tx(move |tx| {
        // Which threads are affected, before the membership rows go.
        let threads: Vec<String> = {
            let mut s = tx.prepare(
                "SELECT DISTINCT m.thread_id FROM messages m \
                 JOIN message_labels ml ON ml.account_id=m.account_id AND ml.message_id=m.id \
                 WHERE ml.label_id=? AND m.account_id=?",
            )?;
            let rows = s
                .query_map(rusqlite::params![id, account], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            rows
        };
        let messages: Vec<String> = {
            let mut s = tx.prepare(
                "SELECT message_id FROM message_labels WHERE account_id=? AND label_id=?",
            )?;
            let rows = s
                .query_map(rusqlite::params![account, id], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            rows
        };
        // Organisation only: the label row and its membership rows go, and the
        // messages stay exactly where they are.
        tx.execute(
            "DELETE FROM message_labels WHERE account_id=? AND label_id=?",
            rusqlite::params![account, id],
        )?;
        tx.execute(
            "DELETE FROM labels WHERE account_id=? AND id=?",
            rusqlite::params![account, id],
        )?;
        for message_id in &messages {
            crate::actions::refresh_message_flags(tx, &account, message_id)?;
        }
        for thread_id in &threads {
            crate::db::threads::recompute_thread_conn(tx, &account, thread_id)?;
        }

        // Pending work that only mentioned this label is dropped rather than
        // sent: applying it would re-create the organisation the user removed.
        let rows: Vec<(i64, String, String)> = {
            let mut s = tx.prepare(
                "SELECT id, kind, payload FROM outbox_ops \
                 WHERE account_id=? AND state='pending' AND kind IN ('modify_labels','rule_apply','create_label')",
            )?;
            let rows = s
                .query_map(rusqlite::params![account], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<Result<Vec<(i64, String, String)>, _>>()?;
            rows
        };
        let now = crate::db::now_ms();
        for (op_id, kind, payload) in rows {
            let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&payload) else {
                continue;
            };
            if kind == "create_label" {
                if value.get("name").and_then(|n| n.as_str()) == Some(name.as_str()) {
                    tx.execute(
                        "UPDATE outbox_ops SET state='cancelled', completed_at=?1, \
                           failure_code='label_deleted', last_error='the label was deleted' WHERE id=?2",
                        rusqlite::params![now, op_id],
                    )?;
                }
                continue;
            }
            let mut emptied = true;
            let mut changed = false;
            for key in ["add", "remove"] {
                if let Some(list) = value.get_mut(key).and_then(|l| l.as_array_mut()) {
                    let before = list.len();
                    list.retain(|entry| entry.as_str() != Some(id.as_str()));
                    if list.len() != before {
                        changed = true;
                    }
                    if !list.is_empty() {
                        emptied = false;
                    }
                }
            }
            let has_ids = value
                .get("ids")
                .and_then(|i| i.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            if changed && (!emptied || !has_ids) {
                tx.execute(
                    "UPDATE outbox_ops SET payload=?1 WHERE id=?2",
                    rusqlite::params![value.to_string(), op_id],
                )?;
            } else if changed {
                tx.execute(
                    "UPDATE outbox_ops SET state='cancelled', completed_at=?1, \
                       failure_code='label_deleted', last_error='the only label this change touched was deleted' \
                     WHERE id=?2 AND state='pending'",
                    rusqlite::params![now, op_id],
                )?;
            }
        }

        let op_id = if is_local_placeholder(&id) {
            None
        } else {
            let payload = serde_json::json!({ "id": id, "name": name }).to_string();
            let op = crate::db::outbox::NewOp {
                account_id: account.clone(),
                kind: "label_delete".into(),
                payload,
                not_before: 0,
                summary_action: Some(format!("Deleting label {name}")),
                ..Default::default()
            };
            Some(crate::db::outbox::insert_op(tx, &op)?.id)
        };
        Ok(op_id)
    })
    .await
    .map_err(|e| match e.downcast::<SiftError>() {
        Ok(sift) => sift,
        Err(other) => db_error(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn seeded() -> Db {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        std::mem::forget(dir);
        db.write(|c| {
            c.execute("INSERT INTO accounts (id,email,created_at) VALUES ('a','a@x',0)", [])?;
            c.execute(
                "INSERT INTO labels (account_id,id,name,kind) VALUES ('a','Label_1','Client Work','user')",
                [],
            )?;
            c.execute(
                "INSERT INTO labels (account_id,id,name,kind) VALUES ('a','Label_2','Client Work/Acme','user')",
                [],
            )?;
            c.execute(
                "INSERT INTO threads (account_id,id,last_message_at,first_message_at,in_inbox,label_ids) \
                 VALUES ('a','t1',100,100,0,'[\"Label_1\"]')",
                [],
            )?;
            c.execute(
                "INSERT INTO messages (id,account_id,thread_id,internal_date,label_ids) \
                 VALUES ('m1','a','t1',100,'[\"Label_1\"]')",
                [],
            )?;
            c.execute(
                "INSERT INTO message_labels (account_id,message_id,label_id) VALUES ('a','m1','Label_1')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn p8_5_rename_moves_ids_membership_and_pending_work() {
        let db = seeded().await;
        // A queued change that mentions the old id, and one symbolic reference.
        db.write(|c| {
            c.execute(
                "INSERT INTO outbox_ops (id,account_id,kind,payload,state,attempts,not_before,created_at) \
                 VALUES (9,'a','modify_labels','{\"ids\":[\"m1\"],\"add\":[\"Label_1\"],\"remove\":[]}','pending',0,0,0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let updated = rename(&db, "a", "Label_1", "Clients", "g1").await.unwrap();
        assert_eq!(updated.name, "Clients");
        // Gmail-style opaque ids stay put.
        assert_eq!(updated.id, "Label_1");
        let payload: String = db
            .read(|c| {
                Ok(
                    c.query_row("SELECT payload FROM outbox_ops WHERE id=9", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["add"][0], "Label_1");
        // The label row, membership and the denormalised thread list all agree.
        let rows = list(&db, "a").await.unwrap();
        let clients = rows.iter().find(|l| l.id == "Label_1").unwrap();
        assert_eq!(clients.name, "Clients");
        assert_eq!(clients.display_name, "Clients");
        assert_eq!(clients.depth, 0);
        let nested = rows.iter().find(|l| l.id == "Label_2").unwrap();
        assert_eq!(nested.depth, 1);
        assert_eq!(nested.display_name, "Acme");
        assert_eq!(nested.parent_id.as_deref(), Some("Label_1"));
        // A rename op was queued for the provider.
        let kinds: Vec<String> = db
            .read(|c| {
                Ok(
                    c.prepare("SELECT kind FROM outbox_ops WHERE kind='label_rename'")?
                        .query_map([], |r| r.get(0))?
                        .collect::<Result<Vec<String>, _>>()?,
                )
            })
            .await
            .unwrap();
        assert_eq!(kinds, vec!["label_rename".to_string()]);
    }

    #[tokio::test]
    async fn p8_5_imap_rename_moves_the_name_derived_id_everywhere() {
        let db = seeded().await;
        db.write(|c| {
            c.execute(
                "INSERT INTO labels (account_id,id,name,kind) VALUES ('a','imap:Büro','Büro','user')",
                [],
            )?;
            c.execute(
                "INSERT INTO message_labels (account_id,message_id,label_id) VALUES ('a','m1','imap:Büro')",
                [],
            )?;
            c.execute(
                "INSERT INTO outbox_ops (id,account_id,kind,payload,state,attempts,not_before,created_at) \
                 VALUES (11,'a','modify_labels','{\"ids\":[\"m1\"],\"add\":[],\"remove\":[\"imap:Büro\"]}','pending',0,0,0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        // The new name is the folder name; the id is derived from it for this
        // transport, so it changes with the name.
        let updated = rename(&db, "a", "imap:Büro", "Büros", "g2").await.unwrap();
        assert_eq!(updated.id, "imap:Büros");
        assert_eq!(updated.name, "Büros");
        // Membership followed the id.
        let members: Vec<String> = db
            .read(|c| {
                Ok(
                    c.prepare("SELECT label_id FROM message_labels WHERE message_id='m1'")?
                        .query_map([], |r| r.get(0))?
                        .collect::<Result<Vec<String>, _>>()?,
                )
            })
            .await
            .unwrap();
        assert!(members.contains(&"imap:Büros".to_string()));
        assert!(!members.contains(&"imap:Büro".to_string()));
        // A pending operation now addresses the new id, in one transaction.
        let payload: String = db
            .read(|c| {
                Ok(
                    c.query_row("SELECT payload FROM outbox_ops WHERE id=11", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["remove"][0], "imap:Büros");
        // The rename op still names the old id: that is the identity the server
        // knows until it runs.
        let rename_payload: String = db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT payload FROM outbox_ops WHERE kind='label_rename'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        let rename_value: serde_json::Value = serde_json::from_str(&rename_payload).unwrap();
        assert_eq!(rename_value["id"], "imap:Büro");
        assert_eq!(rename_value["name"], "Büros");
    }

    #[tokio::test]
    async fn p8_5_collisions_and_duplicate_names_are_refused_per_account() {
        let db = seeded().await;
        let err = rename(&db, "a", "Label_2", "Client Work", "g")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "label_exists");
        // The other account can still use the same name.
        db.write(|c| {
            c.execute("INSERT INTO accounts (id,email,created_at) VALUES ('b','b@x',0)", [])?;
            c.execute(
                "INSERT INTO labels (account_id,id,name,kind) VALUES ('b','Label_1','Client Work','user')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let renamed = rename(&db, "b", "Label_1", "Clients", "g").await.unwrap();
        assert_eq!(renamed.name, "Clients");
        // Invalid names never reach the row.
        for bad in ["", " leading", "trailing ", "a//b", "/a", "a/"] {
            assert_eq!(
                rename(&db, "a", "Label_1", bad, "g")
                    .await
                    .unwrap_err()
                    .code(),
                "bad_label_name",
                "{bad:?}"
            );
        }
    }

    #[tokio::test]
    async fn p8_5_delete_removes_organisation_and_never_mail() {
        let db = seeded().await;
        // A queued change that mentions the deleted label *and* another one:
        // the operation survives, minus the label the user removed.
        db.write(|c| {
            c.execute(
                "INSERT INTO outbox_ops (id,account_id,kind,payload,state,attempts,not_before,created_at) \
                 VALUES (21,'a','modify_labels','{\"ids\":[\"m1\"],\"add\":[\"Label_1\",\"Label_2\"],\"remove\":[]}','pending',0,0,0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let op = delete(&db, "a", "Label_1").await.unwrap();
        assert!(op.is_some(), "a provider delete is queued");
        // The messages are untouched.
        let messages: i64 = db
            .read(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert_eq!(messages, 1);
        // The organisation is gone, and the pending op no longer mentions the
        // deleted label while keeping the one that remains.
        assert!(db.label_get("a", "Label_1").await.unwrap().is_none());
        let payload: String = db
            .read(|c| {
                Ok(
                    c.query_row("SELECT payload FROM outbox_ops WHERE id=21", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["add"], serde_json::json!(["Label_2"]));
    }

    #[tokio::test]
    async fn p8_5_delete_cancels_an_operation_that_only_touched_it() {
        let db = seeded().await;
        db.write(|c| {
            c.execute(
                "INSERT INTO outbox_ops (id,account_id,kind,payload,state,attempts,not_before,created_at) \
                 VALUES (22,'a','modify_labels','{\"ids\":[\"m1\"],\"add\":[\"Label_1\"],\"remove\":[]}','pending',0,0,0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        delete(&db, "a", "Label_1").await.unwrap();
        let (state, code): (String, Option<String>) = db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT state, failure_code FROM outbox_ops WHERE id=22",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(state, "cancelled");
        assert_eq!(code.as_deref(), Some("label_deleted"));
    }
}
