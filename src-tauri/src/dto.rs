use serde::{Deserialize, Serialize};

// ---- Accounts ----
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub color: String,
    pub auth_kind: String,
    pub history_id: Option<String>,
    pub sync_state: String,
    pub last_sync_at: Option<i64>,
    pub created_at: i64,
    pub sort_order: i64,
    pub signature_html: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatus {
    pub account_id: String,
    pub phase: String,
    pub done: i64,
    pub total: i64,
    /// Whether `total` is a real message count (P4.6).
    ///
    /// `false` means the mailbox is still being listed: the UI must show
    /// indeterminate progress, never "mailbox empty" — an unknown total is not
    /// a zero-message account.
    #[serde(rename = "totalKnown")]
    pub total_known: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub account_id: String,
    pub id: String,
    pub name: String,
    pub kind: String,
    pub color_bg: Option<String>,
    pub color_fg: Option<String>,
    pub visible: bool,
    pub unread_count: i64,
    pub total_count: i64,
    pub sort_order: i64,
}

// ---- Views / queries ----
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum View {
    Inbox,
    Starred,
    Snoozed,
    Sent,
    Drafts,
    Archive,
    Spam,
    Trash,
    /// Every thread with at least one message outside Trash/Junk (P3.6).
    ///
    /// This is a MAILBOX scope, not an account scope: it never maps onto the
    /// unified "all accounts" selection. Inbox and Sent qualify; Archive stays
    /// non-Inbox, non-Trash/Junk, non-draft. Membership is decided per message,
    /// so a mixed thread (one archived message plus one inbox message) appears.
    #[serde(rename = "all_mail")]
    AllMail,
    #[serde(rename = "label")]
    Label {
        #[serde(rename = "labelId")]
        label_id: String,
    },
    #[serde(rename = "search")]
    Search {
        #[serde(rename = "q")]
        q: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadsQuery {
    #[serde(rename = "accountIds")]
    pub account_ids: Vec<String>,
    pub view: View,
    pub cursor: Option<String>,
    pub limit: i64,
    #[serde(default)]
    pub unread_only: bool,
    #[serde(default)]
    pub has_attachment: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub n: Option<String>,
    pub e: String,
    pub me: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadRow {
    #[serde(rename = "accountId")]
    pub account_id: String,
    pub id: String,
    pub subject: String,
    pub snippet: String,
    pub participants: Vec<Address>,
    #[serde(rename = "lastMessageAt")]
    pub last_message_at: i64,
    #[serde(rename = "messageCount")]
    pub message_count: i64,
    #[serde(rename = "unreadCount")]
    pub unread_count: i64,
    #[serde(rename = "isStarred")]
    pub is_starred: bool,
    #[serde(rename = "hasAttachments")]
    pub has_attachments: bool,
    #[serde(rename = "labelIds")]
    pub label_ids: Vec<String>,
    #[serde(rename = "snoozedUntil", skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<i64>,
    #[serde(default, rename = "serverOnly")]
    pub server_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadsPage {
    pub rows: Vec<ThreadRow>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub total: Option<i64>,
    pub generation: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub id: String,
    pub filename: Option<String>,
    pub mime: String,
    pub size: i64,
    #[serde(rename = "isInline")]
    pub is_inline: bool,
    pub downloaded: bool,
}

/// Cache lifecycle of one attachment row (P2.2). `Unverified` is the state of
/// bytes written before `cache_version` verification existed: readable, but
/// confirmed only on first read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheState {
    Missing,
    Downloading,
    Ready,
    Corrupt,
    Unverified,
}

impl CacheState {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheState::Missing => "missing",
            CacheState::Downloading => "downloading",
            CacheState::Ready => "ready",
            CacheState::Corrupt => "corrupt",
            CacheState::Unverified => "unverified",
        }
    }
    /// Unknown text degrades to `Missing` (never a false `ready`).
    pub fn parse(s: &str) -> Self {
        match s {
            "downloading" => CacheState::Downloading,
            "ready" => CacheState::Ready,
            "corrupt" => CacheState::Corrupt,
            "unverified" => CacheState::Unverified,
            _ => CacheState::Missing,
        }
    }
    /// True when this state could satisfy a read without the network.
    pub fn is_local(self) -> bool {
        matches!(self, CacheState::Ready | CacheState::Unverified)
    }
}

/// One attachment row joined to its message for account ownership (P2.1).
///
/// `id` is the local row key; `part_id` is the IMAP section path (and only
/// that); `gmail_att_id` is the REST attachment id (and only that). A UI key
/// such as a Content-ID or a row id selects a row and is never a network
/// locator.
#[derive(Debug, Clone)]
pub struct AttachmentRecord {
    pub id: String,
    pub account_id: String,
    pub message_id: String,
    pub part_id: String,
    pub gmail_att_id: Option<String>,
    pub filename: Option<String>,
    pub mime: String,
    pub size: i64,
    pub content_id: Option<String>,
    pub is_inline: bool,
    /// Decoded bytes when they fit in the row cache; `None` when only metadata
    /// or a cache file exists.
    pub data: Option<Vec<u8>>,
    /// Set when the row has a stored payload that cannot be decoded. The
    /// payload is never reported as empty bytes; callers refetch instead.
    pub data_error: Option<String>,
    pub local_path: Option<String>,
    pub cache_state: CacheState,
    pub decoded_size: Option<i64>,
}

impl AttachmentRecord {
    /// Transport locator for the IMAP transport: the stored MIME section.
    pub fn imap_locator(&self) -> Option<&str> {
        let section = self.part_id.trim();
        (!section.is_empty()).then_some(section)
    }

    /// Transport locator for the Gmail REST transport: the API attachment id.
    pub fn rest_locator(&self) -> Option<&str> {
        let id = self.gmail_att_id.as_deref()?.trim();
        (!id.is_empty()).then_some(id)
    }

    /// The locator the given transport must be asked for, if the row has one.
    pub fn locator_for(&self, kind: crate::provider::ProviderKind) -> Option<&str> {
        match kind {
            crate::provider::ProviderKind::GmailApi => self.rest_locator(),
            crate::provider::ProviderKind::GmailImap => self.imap_locator(),
        }
    }

    /// Ordering used when listing a message's attachments for Save All: real
    /// attachments first, then inline, then by display name.
    pub fn order_key(&self) -> (bool, String) {
        (
            self.is_inline,
            self.filename.clone().unwrap_or_default().to_lowercase(),
        )
    }
}

/// Typed cache metadata returned by the attachment service. `path` is the
/// verified cache file; callers convert to bytes only when a URI scheme needs
/// them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentCacheInfo {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "attachmentId")]
    pub attachment_id: String,
    pub state: String,
    pub path: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "cacheBasename")]
    pub cache_basename: String,
    pub mime: String,
    pub size: i64,
    #[serde(rename = "decodedSize")]
    pub decoded_size: Option<i64>,
    /// Downloaded app/script/executable: a system open needs an explicit
    /// confirmation result from the caller.
    #[serde(rename = "requiresConfirmation")]
    pub requires_confirmation: bool,
}

/// Account-qualified attachment key (appendix A).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRefKey {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "attachmentId")]
    pub attachment_id: String,
}

/// One attachment state record, emitted at most 10x/second while a transfer
/// runs (P2.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentProgress {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "attachmentId")]
    pub attachment_id: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    /// queued | downloading | ready | failed | cancelled
    pub state: String,
    #[serde(rename = "transferredBytes")]
    pub transferred_bytes: u64,
    #[serde(rename = "totalBytes")]
    pub total_bytes: Option<u64>,
    #[serde(rename = "errorCode")]
    pub error_code: Option<String>,
}

/// Result of a message-level Save All (P2.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveAllResult {
    pub saved: usize,
    pub failed: Vec<AttachmentRefKey>,
}

/// Result of `attachments_save_as`. A cancelled dialog is a normal outcome,
/// not an error toast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveAsResult {
    pub path: Option<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListUnsub {
    pub url: Option<String>,
    pub mailto: Option<String>,
    #[serde(rename = "oneClick")]
    pub one_click: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMeta {
    pub id: String,
    #[serde(rename = "internalDate")]
    pub internal_date: i64,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    #[serde(rename = "replyTo", skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// This message's own Message-ID header, and the References chain it
    /// carries (P5.4). The composer needs them to build a reply draft that
    /// threads correctly, and the server re-derives the same values from the
    /// stored parent row when the draft is saved.
    #[serde(rename = "rfcMessageId", skip_serializing_if = "Option::is_none", default)]
    pub rfc_message_id: Option<String>,
    #[serde(rename = "references", default)]
    pub references_json: Vec<String>,
    pub subject: String,
    pub snippet: String,
    #[serde(rename = "isUnread")]
    pub is_unread: bool,
    #[serde(rename = "isStarred")]
    pub is_starred: bool,
    #[serde(rename = "isDraft")]
    pub is_draft: bool,
    #[serde(rename = "isSentByMe")]
    pub is_sent_by_me: bool,
    #[serde(rename = "labelIds")]
    pub label_ids: Vec<String>,
    #[serde(rename = "hasAttachments")]
    pub has_attachments: bool,
    pub attachments: Vec<AttachmentMeta>,
    #[serde(rename = "bodyState")]
    pub body_state: String,
    #[serde(rename = "listUnsubscribe", skip_serializing_if = "Option::is_none")]
    pub list_unsubscribe: Option<ListUnsub>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadDetail {
    #[serde(rename = "accountId")]
    pub account_id: String,
    pub id: String,
    pub subject: String,
    #[serde(rename = "labelIds")]
    pub label_ids: Vec<String>,
    pub messages: Vec<MessageMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageBody {
    #[serde(rename = "messageId")]
    pub message_id: String,
    pub state: String,
    pub html: Option<String>,
    pub text: Option<String>,
    #[serde(rename = "remoteImageCount")]
    pub remote_image_count: i64,
    #[serde(rename = "trackerCount")]
    pub tracker_count: i64,
    #[serde(rename = "darkSafe")]
    pub dark_safe: bool,
    #[serde(rename = "remoteImagesAllowed")]
    pub remote_images_allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ActionKind {
    Archive,
    Unarchive,
    Trash,
    Untrash,
    Spam,
    Unspam,
    DeleteForever,
    Star {
        on: bool,
    },
    Read {
        on: bool,
    },
    AddLabel {
        #[serde(rename = "labelId")]
        label_id: String,
    },
    RemoveLabel {
        #[serde(rename = "labelId")]
        label_id: String,
    },
    MoveTo {
        #[serde(rename = "labelId")]
        label_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadAction {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "threadIds")]
    pub thread_ids: Vec<String>,
    pub action: ActionKind,
}

/// State machine of a draft row (P5.1/P5.2). `editing` and `queued` are the
/// only states a user-visible draft can be edited from; `sent`/`failed` are
/// terminal and only ever set by the outbox (P6).
pub const DRAFT_STATE_EDITING: &str = "editing";
pub const DRAFT_STATE_QUEUED: &str = "queued";
pub const DRAFT_STATE_SENT: &str = "sent";
pub const DRAFT_STATE_FAILED: &str = "failed";

fn draft_state_default() -> String {
    DRAFT_STATE_EDITING.to_string()
}

fn draft_mode_default() -> String {
    "new".to_string()
}

/// One draft. `localId` is allocated by the composer when a new draft is
/// opened and never changes afterwards, so every save addresses the same row.
/// `revision` counts local content changes and never decreases; a save that
/// carries a stale `expectedRevision` is rejected instead of overwriting the
/// newer content (P5.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Draft {
    #[serde(rename = "localId", default)]
    pub local_id: String,
    #[serde(rename = "accountId", default)]
    pub account_id: String,
    /// Sending identity selected by the user. None means the account's own
    /// address.
    #[serde(rename = "fromEmail", skip_serializing_if = "Option::is_none", default)]
    pub from_email: Option<String>,
    #[serde(rename = "remoteDraftId", skip_serializing_if = "Option::is_none", default)]
    pub remote_draft_id: Option<String>,
    #[serde(rename = "remoteMessageId", skip_serializing_if = "Option::is_none", default)]
    pub remote_message_id: Option<String>,
    #[serde(rename = "threadId", skip_serializing_if = "Option::is_none", default)]
    pub thread_id: Option<String>,
    #[serde(rename = "inReplyToMessageId", skip_serializing_if = "Option::is_none", default)]
    pub in_reply_to_message_id: Option<String>,
    /// Stable Message-ID of this draft's send lineage. Set when the draft is
    /// first prepared or queued and reused by every retry of that attempt.
    #[serde(rename = "rfcMessageId", skip_serializing_if = "Option::is_none", default)]
    pub rfc_message_id: Option<String>,
    /// RFC Message-ID of the message being replied to, and the References
    /// chain up to it (P5.4). Both are carried into MIME and the REST body.
    #[serde(rename = "parentRfcMessageId", skip_serializing_if = "Option::is_none", default)]
    pub parent_rfc_message_id: Option<String>,
    #[serde(rename = "references", default)]
    pub references_json: Vec<String>,
    #[serde(default = "draft_mode_default")]
    pub mode: String,
    #[serde(rename = "toJson", default)]
    pub to_json: Vec<Address>,
    #[serde(rename = "ccJson", default)]
    pub cc_json: Vec<Address>,
    #[serde(rename = "bccJson", default)]
    pub bcc_json: Vec<Address>,
    #[serde(default)]
    pub subject: String,
    #[serde(rename = "bodyHtml", default)]
    pub body_html: String,
    #[serde(rename = "attachmentsJson", default)]
    pub attachments_json: Vec<AttachmentRef>,
    /// Monotonic local content revision.
    #[serde(default)]
    pub revision: i64,
    /// The revision the remote copy was last built from. `revision >
    /// saved_revision` is a locally-unsaved edit (P5.2).
    #[serde(rename = "savedRevision", default)]
    pub saved_revision: i64,
    /// Last remote-side revision reconciled into this row.
    #[serde(rename = "remoteRevision", default)]
    pub remote_revision: i64,
    #[serde(default = "draft_state_default")]
    pub state: String,
    /// Queued send deadline (undo send / send later).
    #[serde(rename = "notBefore", skip_serializing_if = "Option::is_none", default)]
    pub not_before: Option<i64>,
    #[serde(rename = "scheduledAt", skip_serializing_if = "Option::is_none", default)]
    pub scheduled_at: Option<i64>,
    #[serde(rename = "scheduledTimezone", skip_serializing_if = "Option::is_none", default)]
    pub scheduled_timezone: Option<String>,
    #[serde(rename = "scheduledLocalTime", skip_serializing_if = "Option::is_none", default)]
    pub scheduled_local_time: Option<String>,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none", default)]
    pub updated_at: Option<i64>,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            local_id: String::new(),
            account_id: String::new(),
            from_email: None,
            remote_draft_id: None,
            remote_message_id: None,
            thread_id: None,
            in_reply_to_message_id: None,
            rfc_message_id: None,
            parent_rfc_message_id: None,
            references_json: Vec::new(),
            mode: draft_mode_default(),
            to_json: Vec::new(),
            cc_json: Vec::new(),
            bcc_json: Vec::new(),
            subject: String::new(),
            body_html: String::new(),
            attachments_json: Vec::new(),
            revision: 0,
            saved_revision: 0,
            remote_revision: 0,
            state: draft_state_default(),
            not_before: None,
            scheduled_at: None,
            scheduled_timezone: None,
            scheduled_local_time: None,
            updated_at: None,
        }
    }
}

impl Draft {
    /// True when the local content changed since the remote copy was built.
    pub fn has_unsaved_revision(&self) -> bool {
        self.revision > self.saved_revision
    }

    /// Every field a save carries, ignoring server-owned bookkeeping
    /// (revision counters, state, ids, timestamps). Used to decide whether an
    /// autosave actually changed anything: an unchanged snapshot must not
    /// burn a revision or wake remote sync.
    pub fn content_eq(&self, other: &Draft) -> bool {
        self.account_id == other.account_id
            && self.from_email == other.from_email
            && self.thread_id == other.thread_id
            && self.in_reply_to_message_id == other.in_reply_to_message_id
            && self.parent_rfc_message_id == other.parent_rfc_message_id
            && self.references_json == other.references_json
            && self.mode == other.mode
            && self.to_json == other.to_json
            && self.cc_json == other.cc_json
            && self.bcc_json == other.bcc_json
            && self.subject == other.subject
            && self.body_html == other.body_html
            && self.attachments_json == other.attachments_json
    }
}

/// One page of drafts, keyset-paged by `(updatedAt, localId)` (P5.1/P5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftPage {
    pub drafts: Vec<Draft>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// A draft that exists on the server, as the provider reports it. The mailbox
/// copy of its content comes from the normal message sync, so this carries
/// identity only (P5.2 remote import/reconciliation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDraft {
    #[serde(rename = "remoteDraftId")]
    pub remote_draft_id: String,
    #[serde(rename = "messageId", skip_serializing_if = "Option::is_none", default)]
    pub message_id: Option<String>,
    #[serde(rename = "threadId", skip_serializing_if = "Option::is_none", default)]
    pub thread_id: Option<String>,
    /// RFC Message-ID of the remote draft when the transport knows it (IMAP
    /// locator); `None` for REST, where the mailbox copy is authoritative.
    #[serde(rename = "rfcMessageId", skip_serializing_if = "Option::is_none", default)]
    pub rfc_message_id: Option<String>,
}

/// One queued send, frozen at queue time (P6.2 reuses this shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendHandle {
    #[serde(rename = "opId")]
    pub op_id: i64,
    #[serde(rename = "notBefore")]
    pub not_before: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub email: String,
    pub name: Option<String>,
    #[serde(rename = "lastUsedAt")]
    pub last_used_at: i64,
    #[serde(rename = "useCount")]
    pub use_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub theme: String,
    pub accent: String,
    #[serde(rename = "uiFont")]
    pub ui_font: String,
    pub density: String,
    #[serde(rename = "readingPane")]
    pub reading_pane: String,
    #[serde(rename = "avatarsInList")]
    pub avatars_in_list: bool,
    #[serde(rename = "snippetLines")]
    pub snippet_lines: i64,
    #[serde(rename = "sidebarLabels")]
    pub sidebar_labels: String,
    #[serde(rename = "messageFont")]
    pub message_font: String,
    #[serde(rename = "messageFontSize")]
    pub message_font_size: i64,
    #[serde(rename = "darkModeEmails")]
    pub dark_mode_emails: String,
    #[serde(rename = "afterArchive")]
    pub after_archive: String,
    #[serde(rename = "undoSendDelay")]
    pub undo_send_delay: i64,
    #[serde(rename = "undoToastDuration")]
    pub undo_toast_duration: i64,
    #[serde(rename = "markAsRead")]
    pub mark_as_read: String,
    #[serde(rename = "replyDefault")]
    pub reply_default: String,
    #[serde(rename = "sendAndArchiveDefault")]
    pub send_and_archive_default: bool,
    #[serde(rename = "wakeSnoozedUnread")]
    pub wake_snoozed_unread: bool,
    #[serde(rename = "splitInbox")]
    pub split_inbox: bool,
    #[serde(rename = "notifications")]
    pub notifications: String,
    #[serde(rename = "sound")]
    pub sound: String,
    #[serde(rename = "dockBadge")]
    pub dock_badge: String,
    #[serde(rename = "remoteImages")]
    pub remote_images: String,
    #[serde(rename = "stripTrackers")]
    pub strip_trackers: bool,
    #[serde(rename = "offlineBodyCache")]
    pub offline_body_cache: String,
    #[serde(rename = "attachmentCacheSize")]
    pub attachment_cache_size: String,
    #[serde(rename = "pollFocused")]
    pub poll_focused: i64,
    #[serde(rename = "pollBackground")]
    pub poll_background: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            accent: "blue".into(),
            ui_font: "system".into(),
            density: "default".into(),
            reading_pane: "right".into(),
            avatars_in_list: true,
            snippet_lines: 1,
            sidebar_labels: "collapse".into(),
            message_font: "system".into(),
            message_font_size: 14,
            dark_mode_emails: "auto".into(),
            after_archive: "next".into(),
            undo_send_delay: 10,
            undo_toast_duration: 8,
            mark_as_read: "on-open".into(),
            reply_default: "reply".into(),
            send_and_archive_default: true,
            wake_snoozed_unread: true,
            split_inbox: false,
            notifications: "inbox".into(),
            sound: "subtle".into(),
            dock_badge: "unread".into(),
            remote_images: "always".into(),
            strip_trackers: true,
            offline_body_cache: "2y".into(),
            attachment_cache_size: "512MB".into(),
            poll_focused: 15,
            poll_background: 60,
        }
    }
}

/// Account-qualified message identity (appendix A). Provider message ids are
/// only unique within an account; every DB lookup and IPC boundary that names
/// a message carries both halves. Provider calls still receive
/// `message_id` — the original Gmail hex id, never a concatenated local key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageRef {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "messageId")]
    pub message_id: String,
}

impl MessageRef {
    pub fn new(account_id: impl Into<String>, message_id: impl Into<String>) -> Self {
        Self {
            account_id: account_id.into(),
            message_id: message_id.into(),
        }
    }
}

/// One class of app-owned cache; `items` counts rows or files, whichever
/// applies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageCategory {
    pub bytes: i64,
    pub items: i64,
}

/// Settings -> Storage payload (P10.4). Sizes are exact app-owned bytes, not
/// filesystem allocation, so a clear action can be verified against them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageUsage {
    pub metadata: StorageCategory,
    pub bodies: StorageCategory,
    pub attachments: StorageCategory,
    #[serde(rename = "draftCache")]
    pub draft_cache: StorageCategory,
    /// Configured cap for `attachments`, in bytes (eviction target, not usage).
    #[serde(rename = "attachmentCacheLimitBytes")]
    pub attachment_cache_limit_bytes: i64,
    /// Backend-reported sum of the four categories.
    #[serde(rename = "totalBytes")]
    pub total_bytes: i64,
    /// Unix ms when the backend measured.
    #[serde(rename = "computedAt")]
    pub computed_at: i64,
}
