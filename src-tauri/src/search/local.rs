//! Local search: compile Query -> FTS + SQL, group by thread.
use super::query::Query;
use crate::db::Db;
use anyhow::Result;

pub async fn search(
    db: &Db,
    account_ids: &[String],
    q: &Query,
    limit: i64,
) -> Result<Vec<(String, String)>> {
    // returns (account_id, thread_id) ordered newest first
    let hits = db
        .fts_search_local(
            account_ids,
            &q.terms,
            q.from.as_deref(),
            q.to.as_deref(),
            q.subject.as_deref(),
            q.after,
            q.before,
            q.is_unread,
            q.is_starred,
            if q.has_attachment { Some(true) } else { None },
            limit * 3,
        )
        .await?;
    // group by thread: need thread per message (query messages table)
    let mut out: Vec<(String, String)> = vec![];
    let mut seen = std::collections::HashSet::new();
    for (mid, aid, _date) in hits {
        if let Some((a, t)) = db.message_thread(&mid).await? {
            let _ = aid;
            if seen.insert((a.clone(), t.clone())) {
                // label filter
                if !q.labels.is_empty() {
                    // check thread has label
                    let has: bool = db
                        .read({
                            let (a2, t2) = (a.clone(), t.clone());
                            let labels = q.labels.clone();
                            move |c| {
                                let lj: String = c
                                    .query_row(
                                        "SELECT label_ids FROM threads WHERE account_id=? AND id=?",
                                        rusqlite::params![a2, t2],
                                        |r| r.get(0),
                                    )
                                    .unwrap_or("[]".into());
                                let arr: Vec<String> =
                                    serde_json::from_str(&lj).unwrap_or_default();
                                Ok(labels.iter().all(|l| arr.contains(l)))
                            }
                        })
                        .await?;
                    if !has {
                        continue;
                    }
                }
                out.push((a, t));
                if out.len() as i64 >= limit {
                    break;
                }
            }
        }
        if out.len() as i64 >= limit {
            break;
        }
    }
    Ok(out)
}
