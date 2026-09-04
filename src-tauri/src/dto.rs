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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Draft {
    #[serde(rename = "localId")]
    pub local_id: Option<String>,
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "remoteDraftId", skip_serializing_if = "Option::is_none")]
    pub remote_draft_id: Option<String>,
    #[serde(rename = "remoteMessageId", skip_serializing_if = "Option::is_none")]
    pub remote_message_id: Option<String>,
    #[serde(rename = "threadId", skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(rename = "inReplyToMessageId", skip_serializing_if = "Option::is_none")]
    pub in_reply_to_message_id: Option<String>,
    pub mode: String,
    #[serde(rename = "toJson", default)]
    pub to_json: Vec<Address>,
    #[serde(rename = "ccJson", default)]
    pub cc_json: Vec<Address>,
    #[serde(rename = "bccJson", default)]
    pub bcc_json: Vec<Address>,
    pub subject: String,
    #[serde(rename = "bodyHtml")]
    pub body_html: String,
    #[serde(rename = "attachmentsJson", default)]
    pub attachments_json: Vec<AttachmentRef>,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            remote_images: "ask".into(),
            strip_trackers: true,
            offline_body_cache: "2y".into(),
            attachment_cache_size: "2GB".into(),
            poll_focused: 15,
            poll_background: 60,
        }
    }
}
