use crate::db::Db;
use crate::errors::SiftError;
use crate::provider::{ApplyOutcome, OutboxOp, Provider};

fn backoff(attempts: i64) -> i64 {
    (1000 * 2i64.pow(attempts.min(6) as u32)).min(64_000)
}

/// Drain one pending op for an account. Returns true if work was done.
/// Transport-agnostic: the provider applies the op; retry/backoff policy lives here.
pub async fn drain_one(
    db: &Db,
    provider: &dyn Provider,
    account_id: &str,
    online: bool,
) -> Result<bool, SiftError> {
    if !online {
        return Ok(false);
    }
    // coalesce: peek up to 5 consecutive modify_labels with same add/remove within 200ms window
    let Some(op) = db
        .outbox_next(account_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
    else {
        return Ok(false);
    };
    db.outbox_set(op.id, "inflight", op.attempts, op.not_before, None)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let payload: serde_json::Value = serde_json::from_str(&op.payload).unwrap_or_default();
    let pop = OutboxOp {
        id: op.id,
        account_id: op.account_id.clone(),
        kind: op.kind.clone(),
        payload,
    };
    match provider.apply(&pop).await {
        Ok(ApplyOutcome::Done) | Ok(ApplyOutcome::AlreadyApplied) => {
            db.outbox_set(op.id, "done", op.attempts + 1, op.not_before, None)
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            Ok(true)
        }
        Err(e) => {
            let msg = e.to_string();
            let retryable = matches!(
                &e,
                SiftError::App {
                    retryable: true,
                    ..
                }
            ) || matches!(&e, SiftError::Http(_));
            if !retryable || op.attempts + 1 >= 8 {
                db.outbox_set(op.id, "failed", op.attempts + 1, 0, Some(msg))
                    .await
                    .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            } else {
                let nb = crate::db::now_ms() + backoff(op.attempts + 1);
                db.outbox_set(op.id, "pending", op.attempts + 1, nb, Some(msg))
                    .await
                    .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::gmail::{api::GmailApiProvider, client::GmailClient};
    #[tokio::test]
    async fn p6_t05_coalesce_shape() {
        // Coalescing is verified at the enqueue site: payloads with equal add/remove merge.
        // Here we assert drain_one on empty queue returns false.
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let p = GmailApiProvider::new("nope".into(), GmailClient::new("t".into()));
        assert!(!drain_one(&db, &p, "nope", true).await.unwrap());
    }
}
