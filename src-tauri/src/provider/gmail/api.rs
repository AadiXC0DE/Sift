//! Gmail REST transport behind [`Provider`](crate::provider::Provider).
//!
//! Thin delegation over the long-tested REST code in this directory.
//! Behavior for OAuth accounts is unchanged; the free functions in
//! `sync_full`/`sync_partial`/`sync_reconcile` do the work against a
//! [`SyncSink`](crate::provider::SyncSink) so both transports share one
//! local-store contract.
use super::{client::GmailClient, mime::ParsedMessage, sync_full, sync_partial, sync_reconcile};
use crate::db::messages::MsgUpsert;
use crate::dto::Label;
use crate::errors::SiftError;
use crate::provider::{
    ApplyOutcome, BoxStream, Cursor, OutboxOp, PartialOutcome, ProfileInfo, Provider, ProviderKind,
    SendAs, SentInfo, SyncSink, ThreadRef, WatchEvent,
};
use anyhow::Result;
use async_trait::async_trait;
use base64::Engine;

pub struct GmailApiProvider {
    account_id: String,
    client: GmailClient,
}

impl GmailApiProvider {
    pub fn new(account_id: String, client: GmailClient) -> Self {
        Self { account_id, client }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Full reconcile after a lost history cursor (existing 7.5 behavior).
    /// Returns the fresh history id the account now continues from.
    pub async fn reconcile_full(&self, sink: &dyn SyncSink) -> Result<String> {
        sync_reconcile::run_reconcile(sink, &self.account_id, &self.client).await
    }
}

fn sys_order(name: &str, i: usize) -> i64 {
    match name {
        "INBOX" => 0,
        "STARRED" => 1,
        "SENT" => 2,
        "DRAFT" => 3,
        "SPAM" => 4,
        "TRASH" => 5,
        _ => 100 + i as i64,
    }
}

fn map_label(account_id: &str, l: &super::types::Label, i: usize) -> Label {
    Label {
        account_id: account_id.into(),
        id: l.id.clone(),
        name: l.name.clone(),
        kind: if l.kind == "system" {
            "system".into()
        } else {
            "user".into()
        },
        color_bg: l.color.as_ref().and_then(|c| c.bg.clone()),
        color_fg: l.color.as_ref().and_then(|c| c.fg.clone()),
        visible: l.list_visibility.as_deref() != Some("labelHide"),
        unread_count: 0,
        total_count: 0,
        sort_order: sys_order(&l.id, i),
    }
}

fn gmail_err(e: impl ToString) -> SiftError {
    SiftError::app("gmail", e.to_string(), true)
}

/// One Gmail draft resource as the local reconciliation sees it.
fn draft_resource(d: super::types::DraftResource) -> crate::dto::RemoteDraft {
    crate::dto::RemoteDraft {
        remote_draft_id: d.id,
        message_id: d.message.as_ref().map(|m| m.id.clone()),
        thread_id: d.message.as_ref().and_then(|m| m.thread_id.clone()),
        rfc_message_id: None,
    }
}

#[async_trait]
impl Provider for GmailApiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GmailApi
    }

    async fn verify(&self) -> Result<ProfileInfo, SiftError> {
        // get_profile doubles as a token validity check.
        let _profile = self.client.get_profile().await?;
        let info = self.client.userinfo().await?;
        Ok(ProfileInfo {
            email: info.email.clone(),
            display_name: info.name,
            avatar_url: info.picture,
        })
    }

    async fn list_labels(&self) -> Result<Vec<Label>, SiftError> {
        Ok(self
            .client
            .list_labels()
            .await?
            .iter()
            .enumerate()
            .map(|(i, l)| map_label(&self.account_id, l, i))
            .collect())
    }

    async fn create_label(&self, name: &str) -> Result<Label, SiftError> {
        let remote = self.client.create_label(name).await?;
        Ok(Label {
            account_id: self.account_id.clone(),
            id: remote.id,
            name: remote.name,
            kind: "user".into(),
            color_bg: None,
            color_fg: None,
            visible: true,
            unread_count: 0,
            total_count: 0,
            sort_order: 200,
        })
    }

    async fn full_sync(
        &self,
        sink: &dyn SyncSink,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Cursor, SiftError> {
        sync_full::run_full_sync(sink, &self.account_id, &self.client, cancel)
            .await
            .map_err(gmail_err)
    }

    async fn partial_sync(
        &self,
        cursor: &Cursor,
        sink: &dyn SyncSink,
    ) -> Result<PartialOutcome, SiftError> {
        // Empty or foreign cursor: reconcile first (existing behavior for
        // fresh accounts and lost history), then continue from the fresh id.
        let start = match cursor {
            Cursor::Gmail { history_id } if !history_id.is_empty() => history_id.clone(),
            _ => self.reconcile_full(sink).await.map_err(gmail_err)?,
        };
        match sync_partial::run_partial_sync(sink, &self.account_id, &self.client, &start)
            .await
            .map_err(gmail_err)?
        {
            PartialOutcome::NeedsFull => {
                // historyId went stale mid-tick: reconcile once and continue.
                let fresh = self.reconcile_full(sink).await.map_err(gmail_err)?;
                sync_partial::run_partial_sync(sink, &self.account_id, &self.client, &fresh)
                    .await
                    .map_err(gmail_err)
            }
            ok => Ok(ok),
        }
    }

    async fn fetch_body(&self, message_id: &str) -> Result<ParsedMessage, SiftError> {
        let m = self.client.get_message_full(message_id).await?;
        Ok(super::mime::parse_full(&m))
    }

    async fn fetch_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, SiftError> {
        let att = self
            .client
            .get_attachment(message_id, attachment_id)
            .await?;
        match att.data {
            Some(d) => Ok(
                base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE, d)
                    .unwrap_or_default(),
            ),
            None => Ok(vec![]),
        }
    }

    async fn fetch_raw(&self, message_id: &str) -> Result<String, SiftError> {
        Ok(self
            .client
            .get_message_raw(message_id)
            .await?
            .raw
            .unwrap_or_default())
    }

    async fn apply(&self, op: &OutboxOp) -> Result<ApplyOutcome, SiftError> {
        match op.kind.as_str() {
            "modify_labels" => {
                let ids: Vec<String> =
                    serde_json::from_value(op.payload["ids"].clone()).unwrap_or_default();
                let add: Vec<String> =
                    serde_json::from_value(op.payload["add"].clone()).unwrap_or_default();
                let remove: Vec<String> =
                    serde_json::from_value(op.payload["remove"].clone()).unwrap_or_default();
                self.client.batch_modify(ids, add, remove).await?;
            }
            "trash" => {
                let threads: Vec<String> =
                    serde_json::from_value(op.payload["threads"].clone()).unwrap_or_default();
                for t in threads {
                    self.client.trash_thread(&t).await?;
                }
            }
            "untrash" => {
                let threads: Vec<String> =
                    serde_json::from_value(op.payload["threads"].clone()).unwrap_or_default();
                for t in threads {
                    self.client.untrash_thread(&t).await?;
                }
            }
            "delete" => {
                // P6.4: delete the exact messages the user confirmed, by
                // identity. A payload from before that change only has thread
                // ids, which is the closest identity it carries.
                let ids: Vec<String> = op.payload["messages"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if ids.is_empty() {
                    let threads: Vec<String> =
                        serde_json::from_value(op.payload["threads"].clone()).unwrap_or_default();
                    for t in threads {
                        self.client.delete_thread(&t).await?;
                    }
                } else {
                    for id in ids {
                        match self.client.delete_message(&id).await {
                            Ok(()) => {}
                            // Gone already (another client, or a retried op):
                            // the intent is satisfied.
                            Err(SiftError::NotFound(_)) => {}
                            Err(e) => return Err(e),
                        }
                    }
                }
            }
            "send" => {
                // Executed by the outbox drain, which owns the prepared
                // payload and the retry policy.
                return Err(SiftError::app(
                    "op",
                    "send ops are applied by the outbox drain",
                    false,
                ));
            }
            "draft_upsert" | "draft_delete" => {
                // Superseded by `draft_sync`, which carries the revision and
                // the merge rules. An op left in the queue by an older build
                // is completed as a no-op rather than replayed blindly.
            }
            _ => {
                return Err(SiftError::app(
                    "op",
                    format!("unknown op {}", op.kind),
                    false,
                ))
            }
        }
        Ok(ApplyOutcome::Done)
    }

    async fn send(&self, req: &crate::provider::SendRequest) -> Result<SentInfo, SiftError> {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&req.raw);
        // Gmail delivers from the message headers (Bcc included) and files the
        // message into `threadId`; the API has no separate envelope.
        let m = self
            .client
            .send_raw(&encoded, req.thread_id.as_deref())
            .await?;
        Ok(SentInfo {
            id: m.id,
            thread_id: m.thread_id,
        })
    }

    /// Ask Gmail's own index whether a message with this Message-ID exists
    /// (P6.1). This is the receipt for an `uncertain` send.
    async fn sent_by_rfc_message_id(
        &self,
        rfc_message_id: &str,
    ) -> Result<Option<SentInfo>, SiftError> {
        Ok(self
            .client
            .find_message_by_rfc_message_id(rfc_message_id)
            .await?
            .map(|m| SentInfo {
                id: m.id,
                thread_id: m.thread_id,
            }))
    }

    /// Real Gmail draft operations (P5.2): create when there is no remote copy,
    /// update in place when there is one (so the user keeps one current draft),
    /// and let the caller delete drafts that no longer exist.
    async fn draft_upsert(
        &self,
        remote_id: Option<&str>,
        raw: &[u8],
        _rfc_message_id: &str,
    ) -> Result<crate::dto::RemoteDraft, SiftError> {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        if let Some(id) = remote_id.filter(|id| !id.trim().is_empty()) {
            match self.client.update_draft(id, &encoded, None).await {
                Ok(d) => return Ok(draft_resource(d)),
                // The draft was deleted elsewhere: fall through and create it
                // again instead of failing the sync forever.
                Err(SiftError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(draft_resource(self.client.create_draft(&encoded, None).await?))
    }

    async fn draft_delete(&self, remote_id: &str) -> Result<(), SiftError> {
        if remote_id.trim().is_empty() {
            return Ok(());
        }
        self.client.delete_draft(remote_id).await
    }

    async fn draft_list(&self) -> Result<Vec<crate::dto::RemoteDraft>, SiftError> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..10 {
            let resp = self.client.list_drafts(200, page.as_deref()).await?;
            for d in resp.drafts.unwrap_or_default() {
                out.push(draft_resource(d));
            }
            match resp.next_page_token {
                Some(t) if !t.is_empty() => page = Some(t),
                _ => break,
            }
        }
        Ok(out)
    }

    async fn server_search(
        &self,
        q: &str,
        limit: u32,
        sink: &dyn SyncSink,
    ) -> Result<Vec<ThreadRef>, SiftError> {
        let resp = self.client.list_messages(None, Some(q), false).await?;
        let ids: Vec<(String, String)> = resp
            .messages
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.id, m.thread_id))
            .collect();
        for (id, tid) in &ids {
            let r = crate::dto::MessageRef::new(self.account_id.clone(), id);
            if sink.message_exists(&r).await.map_err(gmail_err)? {
                continue;
            }
            // Hydrate unknown hits with metadata so the second run is local.
            if let Ok(m) = self.client.get_message_meta(id).await {
                let labels = m.label_ids.clone().unwrap_or_default();
                let up = MsgUpsert {
                    id: id.clone(),
                    account_id: self.account_id.clone(),
                    thread_id: tid.clone(),
                    history_id: m.history_id.clone(),
                    internal_date: m
                        .internal_date
                        .as_deref()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                    subject: String::new(),
                    snippet: m.snippet.clone().unwrap_or_default(),
                    is_unread: labels.contains(&"UNREAD".into()),
                    is_starred: labels.contains(&"STARRED".into()),
                    is_draft: labels.contains(&"DRAFT".into()),
                    label_ids: labels,
                    ..Default::default()
                };
                let _ = sink.upsert_message(up).await;
            }
        }
        Ok(ids
            .into_iter()
            .take(limit as usize)
            .map(|(message_id, thread_id)| ThreadRef {
                thread_id,
                message_id,
            })
            .collect())
    }

    async fn send_as_list(&self) -> Result<Vec<SendAs>, SiftError> {
        let v = self.client.send_as_list().await?;
        let arr = v
            .get("sendAs")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(arr
            .iter()
            .filter_map(|e| {
                Some(SendAs {
                    email: e.get("sendAsEmail")?.as_str()?.to_string(),
                    display_name: e
                        .get("displayName")
                        .and_then(|d| d.as_str())
                        .map(|d| d.to_string()),
                    is_primary: e
                        .get("isPrimary")
                        .and_then(|p| p.as_bool())
                        .unwrap_or(false),
                })
            })
            .collect())
    }

    fn watch(&self) -> Option<BoxStream<'static, WatchEvent>> {
        None
    }
}
