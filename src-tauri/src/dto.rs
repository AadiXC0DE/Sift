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
    /// The scope the provider actually granted, when it reports one (P6.4).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub granted_scope: Option<String>,
}

impl Account {
    /// May Sift permanently delete mail from this account?
    ///
    /// An app-password account has full IMAP access by construction. An OAuth
    /// account is gated on the granted scope: a consent screen that returned
    /// only read access cannot authorize a permanent deletion, and a scope we
    /// have never seen is treated as "try it and report the provider's own
    /// refusal" rather than as a silent grant.
    pub fn allows_permanent_delete(&self) -> bool {
        if self.auth_kind != "oauth" {
            return true;
        }
        match self.granted_scope.as_deref() {
            Some(scope) => scope
                .split_whitespace()
                .any(|s| s.contains("mail.google.com") || s.ends_with("/gmail.modify")),
            None => true,
        }
    }
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
    /// P8.5: the parent label's id for a nested label (`Client Work/2026`), so
    /// the sidebar and the picker can indent from data instead of re-parsing
    /// the display name.
    #[serde(rename = "parentId", skip_serializing_if = "Option::is_none", default)]
    pub parent_id: Option<String>,
    /// The leaf of a hierarchical name (`2026` for `Client Work/2026`), which
    /// is what an indented row shows. Empty for a flat label, where `name` is
    /// already the whole thing.
    #[serde(rename = "displayName", default)]
    pub display_name: String,
    /// Nesting depth of the display name: 0 for a top-level label.
    #[serde(default)]
    pub depth: i64,
}

/// The display leaf and nesting depth of a hierarchical label name (`a/b` is
/// one level below `a`). Provider ids are never parsed for structure, and the
/// full name stays intact for queries.
pub fn label_hierarchy(name: &str) -> (String, i64, Option<String>) {
    let parts: Vec<&str> = name.split('/').collect();
    let depth = parts.len().saturating_sub(1) as i64;
    let leaf = parts.last().copied().unwrap_or(name).to_string();
    let parent = if parts.len() > 1 {
        Some(parts[..parts.len() - 1].join("/"))
    } else {
        None
    };
    (leaf, depth, parent)
}

impl Default for Label {
    fn default() -> Self {
        Self {
            account_id: String::new(),
            id: String::new(),
            name: String::new(),
            kind: "user".into(),
            color_bg: None,
            color_fg: None,
            visible: true,
            unread_count: 0,
            total_count: 0,
            sort_order: 0,
            parent_id: None,
            display_name: String::new(),
            depth: 0,
        }
    }
}

/// Fill the derived presentation fields from the display name. Called on the
/// read path, so a label stored by any transport shows the same hierarchy.
pub fn label_presentation(label: &mut Label, parent_id: Option<String>) {
    let (leaf, depth, _parent_name) = label_hierarchy(&label.name);
    label.display_name = leaf;
    label.depth = depth;
    label.parent_id = parent_id;
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
    /// P8.2: an open reminder on this thread, if any. The list row uses it for
    /// a subtle indicator; a reminder never moves the thread, so this is the
    /// only place the row shows it.
    #[serde(
        rename = "reminderAt",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub reminder_at: Option<i64>,
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

/// One page of search results (P7.1/P7.3). A superset of `ThreadsPage` so a
/// caller that only reads rows keeps working.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPage {
    pub rows: Vec<ThreadRow>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub total: Option<i64>,
    pub generation: i64,
    /// Recoverable problems with the query itself: an invalid date, an
    /// unclosed quote, an operator Sift keeps as literal text.
    pub hints: Vec<crate::search::query::QueryHint>,
    /// The query text as the backend interpreted it.
    #[serde(rename = "query")]
    pub query: String,
    /// Accounts whose *server* search failed while others succeeded, so the UI
    /// can say the result is partial instead of pretending it is complete.
    #[serde(rename = "failedAccounts", default)]
    pub failed_accounts: Vec<String>,
    /// `true` when the local result is limited to downloaded mail.
    #[serde(rename = "localOnly", default)]
    pub local_only: bool,
}

/// A saved search (P7.4). It stores a validated query and an account scope —
/// never a copy of any message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSearch {
    pub id: String,
    pub name: String,
    pub query: String,
    /// The accounts this search was saved against. An empty list matches
    /// nothing: a scope is never silently widened to every account.
    #[serde(rename = "accountScope")]
    pub account_scope: Vec<String>,
    /// Scope entries whose account no longer exists. The query is kept and the
    /// UI offers the scope for editing.
    #[serde(rename = "missingAccounts", default)]
    pub missing_accounts: Vec<String>,
    #[serde(rename = "sortOrder")]
    pub sort_order: i64,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    /// The AST version the stored query was validated against.
    #[serde(rename = "astVersion")]
    pub ast_version: i64,
    /// Lazy count: `None` until the UI asks for a visible mailbox.
    #[serde(rename = "matchCount", skip_serializing_if = "Option::is_none")]
    pub match_count: Option<i64>,
    #[serde(rename = "countedAt", skip_serializing_if = "Option::is_none")]
    pub counted_at: Option<i64>,
}

/// Create/update payload for a saved search. `id` is absent for a new one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSearchInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub query: String,
    #[serde(rename = "accountScope")]
    pub account_scope: Vec<String>,
    #[serde(rename = "sortOrder", default)]
    pub sort_order: Option<i64>,
}

/// A lazily computed saved-search count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSearchCount {
    pub id: String,
    pub count: i64,
    #[serde(rename = "computedAt")]
    pub computed_at: i64,
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
    /// Every structured entry of the header, in header order (P9.4).
    #[serde(default)]
    pub targets: Vec<UnsubscribeTarget>,
}

/// One parsed `List-Unsubscribe` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsubscribeTarget {
    /// `https` | `http` | `mailto` | `other`.
    pub scheme: String,
    pub url: String,
}

/// Outcome of one user-initiated unsubscribe (P9.4). Every path is explicit:
/// when Sift will not POST on the user's behalf it says why and hands back the
/// URL or `mailto:` for the user to complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsubscribeResult {
    /// `one_click` | `open_link` | `mailto` | `none`.
    pub method: String,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mailto: Option<String>,
    /// Why a one-click POST was not performed or did not succeed:
    /// `insecure_transport`, `missing_post_value`, `untrusted_authentication`,
    /// `blocked_target`, `redirected`, `post_failed`, `no_targets`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Human-readable explanation shown next to the button.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub targets: Vec<UnsubscribeTarget>,
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
    #[serde(
        rename = "rfcMessageId",
        skip_serializing_if = "Option::is_none",
        default
    )]
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
    /// Whether this read was rendered with external images permitted. The UI
    /// must never treat this as a request to change policy: it reports what
    /// the backend actually did.
    #[serde(rename = "remoteImagesAllowed")]
    pub remote_images_allowed: bool,
    /// P9.1: the effective policy used for this read (`block`|`ask`|`allow`).
    #[serde(rename = "remoteContentMode")]
    pub remote_content_mode: String,
    /// P9.1: monotonic per-account permission generation. It changes whenever
    /// this account's effective remote-content permission changes, so a body
    /// cache must key on it.
    #[serde(rename = "privacyGeneration")]
    pub privacy_generation: i64,
    /// P9.2/P9.1: monotonic rendering version; a cache key must include it so
    /// a sanitizer change invalidates stored renderings.
    #[serde(rename = "renderVersion")]
    pub render_version: i64,
    /// P9.1: inline `cid:` references that could not be resolved from the
    /// cached parts. A broken CID is reported, never worked around by turning
    /// remote content on.
    #[serde(rename = "unresolvedInlineCount", default)]
    pub unresolved_inline_count: i64,
}

/// Bumped whenever the sanitizer's output for the same input changes.
pub const RENDER_VERSION: i64 = 2;

/// The remote-content policy (P9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteContentMode {
    /// Never load anything external.
    Block,
    /// Ask per message; a sender allow-list entry or a session "load once"
    /// grant permits the load.
    Ask,
    /// Load the HTTPS images a message needs without asking.
    Allow,
}

impl RemoteContentMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "block" => Some(Self::Block),
            "ask" => Some(Self::Ask),
            "allow" => Some(Self::Allow),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Ask => "ask",
            Self::Allow => "allow",
        }
    }

    /// Whether external resources load without a per-message decision.
    pub fn loads_without_asking(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// The visible policy plus the state the privacy panel needs (P9.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteContentPolicy {
    #[serde(rename = "mode")]
    pub mode: String,
    /// True while the one-time compact privacy choice is still unanswered for
    /// an upgraded database. Effective behaviour stays `allow` until then.
    #[serde(rename = "choicePending")]
    pub choice_pending: bool,
    /// Senders this account has permanently allowed.
    #[serde(rename = "allowedSenders")]
    pub allowed_senders: Vec<String>,
    #[serde(rename = "generation")]
    pub generation: i64,
    /// The copy the UI must show; kept next to the values it describes so the
    /// two cannot drift apart.
    #[serde(rename = "privacyNotice")]
    pub privacy_notice: String,
}

/// P9.1 visible privacy copy. Sift talks to the image host directly.
pub const PRIVACY_NOTICE: &str = "External images are loaded straight from their own servers, which can see your IP address and when you opened the message. Sift does not hide your IP address through a relay.";

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
    #[serde(
        rename = "remoteDraftId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub remote_draft_id: Option<String>,
    #[serde(
        rename = "remoteMessageId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub remote_message_id: Option<String>,
    #[serde(rename = "threadId", skip_serializing_if = "Option::is_none", default)]
    pub thread_id: Option<String>,
    #[serde(
        rename = "inReplyToMessageId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub in_reply_to_message_id: Option<String>,
    /// Stable Message-ID of this draft's send lineage. Set when the draft is
    /// first prepared or queued and reused by every retry of that attempt.
    #[serde(
        rename = "rfcMessageId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub rfc_message_id: Option<String>,
    /// RFC Message-ID of the message being replied to, and the References
    /// chain up to it (P5.4). Both are carried into MIME and the REST body.
    #[serde(
        rename = "parentRfcMessageId",
        skip_serializing_if = "Option::is_none",
        default
    )]
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
    #[serde(
        rename = "scheduledAt",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub scheduled_at: Option<i64>,
    #[serde(
        rename = "scheduledTimezone",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub scheduled_timezone: Option<String>,
    #[serde(
        rename = "scheduledLocalTime",
        skip_serializing_if = "Option::is_none",
        default
    )]
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
    #[serde(
        rename = "rfcMessageId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub rfc_message_id: Option<String>,
}

/// One queued send, frozen at queue time (P6.2 reuses this shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendHandle {
    #[serde(rename = "opId")]
    pub op_id: i64,
    #[serde(rename = "notBefore")]
    pub not_before: i64,
    /// P8.1: the UTC deadline the user chose, which is `not_before` for a send
    /// later and `None` for an immediate send (undo window only).
    #[serde(
        rename = "scheduledAt",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub scheduled_at: Option<i64>,
    /// The exact local wall time the user picked (`YYYY-MM-DDTHH:MM`), kept
    /// verbatim so the UI shows what was intended rather than a re-derived
    /// value that a DST change could shift.
    #[serde(
        rename = "scheduledLocalTime",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub scheduled_local_time: Option<String>,
    /// The IANA zone that local time belongs to. Shown before queueing, and
    /// shown again on the scheduled item, because the send itself is UTC.
    #[serde(
        rename = "scheduledTimezone",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub scheduled_timezone: Option<String>,
}

/// One value of the Send Later menu (P8.1). The composer renders these; the
/// backend decides the actual instant so "tomorrow 08:00" cannot mean two
/// different things in two places.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendLaterOption {
    pub id: String,
    pub label: String,
    /// Present for the fixed choices; the "choose" option has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<i64>,
}

/// One account-qualified target of a UI gesture (P6.3).
///
/// The account travels with every target: a mixed selection is one gesture
/// that happens to span accounts, not two unrelated requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GestureTarget {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
}

/// The compact Outbox row (P6.6).
///
/// Deliberately payload-free: the panel shows a recipient summary, the local
/// subject, the schedule/retry time and the error, and never deserializes a
/// queued MIME body to do it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxOpSummary {
    pub op_id: i64,
    pub account_id: String,
    pub kind: String,
    /// `pending | inflight | uncertain | done | failed | cancelled`.
    pub state: String,
    pub action: String,
    pub recipient_summary: Option<String>,
    pub subject: Option<String>,
    /// When a queued send is due (send later / undo window).
    pub scheduled_at: Option<i64>,
    /// When the next attempt or reconciliation happens.
    pub retry_at: Option<i64>,
    pub created_at: i64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub draft_id: Option<String>,
    pub revision: Option<i64>,
    /// A send whose acceptance is unknown and whose bounded reconciliation is
    /// exhausted: only an acknowledged retry may move it.
    pub requires_duplicate_ack: bool,
}

impl From<&crate::db::outbox::Op> for OutboxOpSummary {
    fn from(op: &crate::db::outbox::Op) -> Self {
        use crate::db::outbox::{STATE_PENDING, STATE_UNCERTAIN};
        let is_send = op.kind == "send";
        Self {
            op_id: op.id,
            account_id: op.account_id.clone(),
            kind: op.kind.clone(),
            state: op.state.clone(),
            action: op
                .summary_action
                .clone()
                .unwrap_or_else(|| crate::db::outbox::op_label(&op.kind, &op.payload).to_string()),
            recipient_summary: op.summary_recipient.clone(),
            subject: op.summary_subject.clone(),
            scheduled_at: (is_send && op.state == STATE_PENDING && op.not_before > 0)
                .then_some(op.not_before),
            retry_at: if op.state == STATE_UNCERTAIN {
                op.reconcile_at
            } else if op.state == STATE_PENDING && op.attempts > 0 {
                Some(op.not_before)
            } else {
                None
            },
            created_at: op.created_at,
            error_code: op.failure_code.clone(),
            error_message: op.last_error.clone(),
            draft_id: op.draft_id.clone(),
            revision: op.draft_revision,
            requires_duplicate_ack: op.requires_duplicate_ack(),
        }
    }
}

/// One page of the Outbox panel, newest first.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxPage {
    pub operations: Vec<OutboxOpSummary>,
    pub next_cursor: Option<String>,
    pub total: i64,
    pub counts: crate::db::outbox::OutboxCounts,
}

/// The result of one gesture: the operation ids it created, per account.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GestureResponse {
    pub gesture_id: String,
    pub operations: Vec<OutboxOpSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<crate::actions::GestureFailure>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsActionRequest {
    pub gesture_id: Option<String>,
    pub targets: Vec<GestureTarget>,
    pub action: ActionKind,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozeSetRequest {
    pub gesture_id: Option<String>,
    pub targets: Vec<GestureTarget>,
    pub wake_at: i64,
    #[serde(default)]
    pub wake_unread: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozeClearRequest {
    pub gesture_id: Option<String>,
    pub targets: Vec<GestureTarget>,
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
    /// P8.4: hide the subject line in the notification body. The notification
    /// still says who wrote; it just does not put mailbox content on a lock
    /// screen.
    #[serde(rename = "notificationsHideSubject", default)]
    pub notifications_hide_subject: bool,
    /// P8.4: accounts whose notifications are turned off. Empty means every
    /// account notifies; a disabled account is quiet no matter what arrives.
    #[serde(rename = "notificationsMutedAccounts", default)]
    pub notifications_muted_accounts: Vec<String>,
    #[serde(rename = "sound")]
    pub sound: String,
    #[serde(rename = "dockBadge")]
    pub dock_badge: String,
    #[serde(rename = "remoteImages")]
    pub remote_images: String,
    /// P9.1: the explicit remote-content policy. `block` never loads anything
    /// external, `ask` waits for a per-message or per-sender decision, and
    /// `allow` loads the HTTPS images a message needs without asking. New
    /// installations default to `ask`.
    #[serde(rename = "remoteContentMode")]
    pub remote_content_mode: String,
    /// P9.1: true when this database was upgraded from a build that treated a
    /// legacy ambiguous `ask` as `allow`. The effective policy stays `allow`
    /// until the one-time compact privacy choice is answered, so nothing
    /// changes silently on upgrade.
    #[serde(rename = "remoteContentChoicePending", default)]
    pub remote_content_choice_pending: bool,
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
    /// P11.1: check for a newer release once per launch. The check is network
    /// only — it never downloads or installs anything — so the switch governs
    /// a read, not an install.
    #[serde(rename = "updatesAutoCheck", default = "settings_true")]
    pub updates_auto_check: bool,
    /// P11.1: unix ms of the last completed check, 0 when there has never been
    /// one. Persisted so "last checked" survives a restart instead of resetting.
    #[serde(rename = "updatesLastCheckAt", default)]
    pub updates_last_check_at: i64,
    /// P11.1: `''` (never checked) | `up-to-date` | `available` | `failed`.
    #[serde(rename = "updatesLastCheckState", default)]
    pub updates_last_check_state: String,
    /// P11.1: the version the last check found, empty when it found none.
    #[serde(rename = "updatesLastCheckVersion", default)]
    pub updates_last_check_version: String,
    /// P11.1: the exact reason the last check failed, empty when it did not.
    #[serde(rename = "updatesLastCheckError", default)]
    pub updates_last_check_error: String,
}

/// A `bool` setting that defaults to on. `#[serde(default)]` alone would make
/// it off for every database written before the field existed.
fn settings_true() -> bool {
    true
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
            notifications_hide_subject: false,
            notifications_muted_accounts: Vec::new(),
            sound: "subtle".into(),
            dock_badge: "unread".into(),
            remote_images: "always".into(),
            remote_content_mode: "ask".into(),
            remote_content_choice_pending: false,
            strip_trackers: true,
            offline_body_cache: "2y".into(),
            attachment_cache_size: "512MB".into(),
            poll_focused: 15,
            poll_background: 60,
            updates_auto_check: true,
            updates_last_check_at: 0,
            updates_last_check_state: String::new(),
            updates_last_check_version: String::new(),
            updates_last_check_error: String::new(),
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
    /// P10.4: bytes held by pinned (offline) attachments, which eviction never
    /// touches. Reported separately so the panel can explain why usage can
    /// exceed the cap without anything being wrong.
    #[serde(rename = "pinnedBytes", default)]
    pub pinned_bytes: i64,
    /// Backend-reported sum of the four categories.
    #[serde(rename = "totalBytes")]
    pub total_bytes: i64,
    /// Unix ms when the backend measured.
    #[serde(rename = "computedAt")]
    pub computed_at: i64,
}

// ---------------------------------------------------------------------------
// P8.3 Rules
// ---------------------------------------------------------------------------

/// One local rule (P8.3).
///
/// The vocabulary is closed: conditions are sender/recipient/subject/has
/// attachment, actions are add label, archive, mark read, star and move to
/// Junk. There is deliberately no script, regex, auto-reply, forward, permanent
/// delete or URL action, and no field here could express one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailRule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub account_id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    /// `all` or `any`.
    #[serde(rename = "match", default = "rule_match_default")]
    pub match_mode: String,
    #[serde(default)]
    pub conditions: Vec<RuleCondition>,
    #[serde(default)]
    pub actions: Vec<RuleAction>,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default)]
    pub revision: i64,
    /// Set when the rule disabled itself because an action failed; the UI shows
    /// it so a silent rule is never a mystery.
    #[serde(rename = "lastError", skip_serializing_if = "Option::is_none", default)]
    pub last_error: Option<String>,
}

fn rule_match_default() -> String {
    "all".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleCondition {
    /// `sender` | `recipient` | `subject` | `hasAttachment`.
    pub field: String,
    /// `contains` | `is` | `domain` | `isTrue`.
    pub op: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleAction {
    /// `addLabel` | `archive` | `markRead` | `star` | `junk`.
    pub kind: String,
    #[serde(rename = "labelId", skip_serializing_if = "Option::is_none", default)]
    pub label_id: Option<String>,
}

/// One row of a rule preview. The user sees what would happen before anything
/// does.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePreviewRow {
    #[serde(rename = "messageId")]
    pub message_id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    pub subject: String,
    #[serde(rename = "fromName", skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
    #[serde(rename = "fromEmail")]
    pub from_email: String,
    #[serde(rename = "wouldJunk")]
    pub would_junk: bool,
}

/// The count and sample an "Apply to existing mail" run starts from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePreview {
    pub count: i64,
    pub sample: Vec<RulePreviewRow>,
}

// ---------------------------------------------------------------------------
// P8.4 Notification state
// ---------------------------------------------------------------------------

/// What the settings surface needs to describe notification behaviour
/// truthfully, including whether the OS actually granted permission.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationState {
    pub enabled: bool,
    /// `granted` | `denied` | `prompt` | `unsupported`.
    pub permission: String,
    /// `off` | `inbox` | `vip`.
    pub filter: String,
    #[serde(rename = "hideSubject")]
    pub hide_subject: bool,
    /// `none` | `native`. There is no in-app sound path any more.
    pub sound: String,
    /// Accounts the user turned notifications off for.
    #[serde(rename = "mutedAccounts")]
    pub muted_accounts: Vec<String>,
}

// ---------------------------------------------------------------------------
// P9.3 mailto and raw export
// ---------------------------------------------------------------------------

/// A parsed `mailto:` request. Only these fields are ever carried, so no other
/// header can be injected into the composed draft.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingMailto {
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
}

/// The result of `message_raw_export`. `path` is `null` when the user
/// cancelled the save panel; it is never a path Sift wrote on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawExport {
    pub path: Option<String>,
}
