//! Native notification policy and delivery (P8.4).
//!
//! **Rust is the single owner of OS notifications.** The frontend receives
//! state and navigation events and must never raise a notification of its own;
//! a second sender is how one message produced two banners, and it is why the
//! old `Notifier.tsx` oscillator (an in-process beep with no relation to the
//! system's Do Not Disturb state) is gone. Sound is native, delivered by the
//! same notification, so the OS decides whether it is audible.
//!
//! Everything here is a pure decision plus one delivery call, so the policy can
//! be tested without a window: [`Policy::allows`] answers "should this message
//! notify at all", [`group`] answers "one banner or one summary", and
//! [`Notice::title_body`] answers "what does the banner say", including the
//! hidden-subject mode that keeps mailbox content off a lock screen.

use crate::db::Db;
use crate::dto::Settings;

pub const FILTER_OFF: &str = "off";
pub const FILTER_INBOX: &str = "inbox";
pub const FILTER_VIP: &str = "vip";

/// A burst larger than this becomes one summary instead of N banners.
pub const GROUP_THRESHOLD: usize = 3;

/// One message the transport says is new.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub account_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub from: String,
    pub subject: String,
    /// The message landed in the Inbox (not Archive/Junk/Trash).
    pub in_inbox: bool,
    /// The sender is on this account's VIP list.
    pub is_vip: bool,
}

/// The user's notification choices, resolved once per delivery.
#[derive(Debug, Clone)]
pub struct Policy {
    pub filter: String,
    pub muted_accounts: Vec<String>,
    pub hide_subject: bool,
    pub sound: bool,
}

impl Policy {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            filter: match s.notifications.as_str() {
                FILTER_OFF => FILTER_OFF,
                FILTER_VIP => FILTER_VIP,
                // An unknown value behaves like the documented default rather
                // than silently notifying about everything.
                _ => FILTER_INBOX,
            }
            .to_string(),
            muted_accounts: s.notifications_muted_accounts.clone(),
            hide_subject: s.notifications_hide_subject,
            sound: s.sound != "off",
        }
    }

    /// Does this message notify?
    ///
    /// A muted account is quiet whatever arrives; an account that is not muted
    /// still has to pass the filter, and VIP means VIP *in the Inbox* — a VIP's
    /// archived mail is not an interruption.
    pub fn allows(&self, notice: &Notice) -> bool {
        if self.muted_accounts.iter().any(|a| a == &notice.account_id) {
            return false;
        }
        match self.filter.as_str() {
            FILTER_OFF => false,
            FILTER_VIP => notice.in_inbox && notice.is_vip,
            _ => notice.in_inbox,
        }
    }

    /// A muted account is *never* notified, so the caller can skip the vip
    /// lookup for it entirely.
    pub fn account_is_muted(&self, account_id: &str) -> bool {
        self.muted_accounts.iter().any(|a| a == account_id)
    }
}

impl Notice {
    /// Title and body of the banner. Hidden-subject mode drops the subject and
    /// the sender's display name keeps the notification useful without putting
    /// mailbox content on a lock screen.
    pub fn title_body(&self, policy: &Policy) -> (String, String) {
        let title = if self.from.trim().is_empty() {
            "New message".to_string()
        } else {
            self.from.clone()
        };
        let body = if policy.hide_subject {
            "New message".to_string()
        } else {
            self.subject.clone()
        };
        (title, body)
    }
}

/// Group a batch: a burst becomes one summary, a single message stays one
/// banner. The summary is built from the same `Notice` values, so a grouped
/// delivery cannot say something the individual ones would not.
pub fn group(items: Vec<Notice>) -> Vec<Vec<Notice>> {
    if items.len() > GROUP_THRESHOLD {
        vec![items]
    } else {
        items.into_iter().map(|i| vec![i]).collect()
    }
}

/// The title/body of a grouped summary, and which ref a click opens: the first
/// (oldest) message of the burst, so the user reads the conversation in order.
pub fn summary(items: &[Notice], policy: &Policy) -> (String, String, String, String) {
    let first = &items[0];
    let title = "Sift".to_string();
    let body = if policy.hide_subject {
        format!("{} new messages", items.len())
    } else {
        format!("{} new messages, newest from {}", items.len(), first.from)
    };
    (title, body, first.account_id.clone(), first.thread_id.clone())
}

/// One notification to raise. The payload carries the account and thread refs
/// so the click can route back to the exact conversation, not just to the app.
#[derive(Debug, Clone, PartialEq)]
pub struct NotificationRequest {
    pub title: String,
    pub body: String,
    pub account_id: String,
    pub thread_id: String,
    pub sound: bool,
}

/// What happened to one delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The OS accepted it.
    Shown,
    /// Notification permission is denied: the caller must keep its own state
    /// visible (a due reminder stays due) instead of pretending it delivered.
    Denied,
    /// The policy said no: nothing to show and nothing to remember.
    Suppressed,
    /// A record already covered this message; asking again would be a
    /// duplicate, which is exactly the bug this table prevents.
    Duplicate,
}

/// Decide, remember and deliver new-mail notices.
///
/// The `notify_log` row is written **before** the banner is raised: a repeat
/// sync, a restart or a second scheduler tick finds the row and stays quiet, so
/// "one message, one notification" survives a crash between the two.
pub async fn deliver_new_mail(
    db: &Db,
    host: &crate::runtime::RuntimeHost,
    notices: &[Notice],
) -> Vec<Delivery> {
    if notices.is_empty() {
        return vec![];
    }
    let policy = Policy::from_settings(&db.settings_get().await.unwrap_or_default());
    let allowed: Vec<Notice> = notices
        .iter()
        .filter(|n| policy.allows(n))
        .cloned()
        .collect();
    if allowed.is_empty() {
        return notices.iter().map(|_| Delivery::Suppressed).collect();
    }
    // Fresh only: a message already reported (this run or a previous one) is
    // never reported twice.
    let mut fresh: Vec<Notice> = Vec::with_capacity(allowed.len());
    for n in allowed {
        match db.notify_claim(&n.account_id, &n.message_id, &n.thread_id).await {
            Ok(true) => fresh.push(n),
            Ok(false) => {}
            Err(_) => {}
        }
    }
    if fresh.is_empty() {
        return notices.iter().map(|_| Delivery::Duplicate).collect();
    }
    let mut out = Vec::with_capacity(notices.len());
    for batch in group(fresh) {
        if batch.len() == 1 {
            let n = &batch[0];
            let (title, body) = n.title_body(&policy);
            let req = NotificationRequest {
                title,
                body,
                account_id: n.account_id.clone(),
                thread_id: n.thread_id.clone(),
                sound: policy.sound,
            };
            let shown = (host.notify_os)(req);
            out.push(if shown {
                Delivery::Shown
            } else {
                Delivery::Denied
            });
        } else {
            let (title, body, account_id, thread_id) = summary(&batch, &policy);
            let req = NotificationRequest {
                title,
                body,
                account_id,
                thread_id,
                sound: policy.sound,
            };
            let shown = (host.notify_os)(req);
            out.push(if shown {
                Delivery::Shown
            } else {
                Delivery::Denied
            });
        }
    }
    out
}

/// Deliver one due reminder.
///
/// Order matters and is the point: permission is checked first, the durable
/// delivery mark is written second, and the banner is raised last. A denied
/// permission therefore leaves the reminder `due` (visible in the list) rather
/// than marking it done, and a crash between the mark and the banner cannot
/// raise the same reminder twice.
pub async fn deliver_reminder(
    db: &Db,
    host: &crate::runtime::RuntimeHost,
    account_id: &str,
    thread_id: &str,
    title: &str,
    subject: &str,
) -> Delivery {
    if !(host.notifications_available)() {
        return Delivery::Denied;
    }
    let policy = Policy::from_settings(&db.settings_get().await.unwrap_or_default());
    let subject_text = if policy.hide_subject {
        "Reminder".to_string()
    } else {
        subject.to_string()
    };
    match db.reminder_mark_delivered(account_id, thread_id).await {
        Ok(true) => {}
        Ok(false) => return Delivery::Duplicate,
        Err(_) => return Delivery::Denied,
    }
    let req = NotificationRequest {
        title: format!("Reminder: {title}"),
        body: subject_text,
        account_id: account_id.to_string(),
        thread_id: thread_id.to_string(),
        sound: policy.sound,
    };
    if (host.notify_os)(req) {
        Delivery::Shown
    } else {
        // The banner was refused after the mark was written. Hand the reminder
        // back so it stays visible — and retryable — instead of disappearing as
        // "delivered" when nothing was ever shown.
        let _ = db.reminder_reopen(account_id, thread_id).await;
        Delivery::Denied
    }
}

/// The permission state as the settings UI shows it.
pub fn permission_state(app: &tauri::AppHandle) -> &'static str {
    #[cfg(not(test))]
    {
        use tauri_plugin_notification::NotificationExt;
        match app.notification().permission_state() {
            Ok(tauri_plugin_notification::PermissionState::Granted) => "granted",
            Ok(tauri_plugin_notification::PermissionState::Denied) => "denied",
            Ok(_) => "prompt",
            Err(_) => "unsupported",
        }
    }
    #[cfg(test)]
    {
        let _ = app;
        "unsupported"
    }
}

/// Ask the OS for permission. Called when the user *enables* notifications —
/// never on an incoming message, which is how a denied user avoids the prompt
/// loop.
pub fn request_permission(app: &tauri::AppHandle) -> &'static str {
    #[cfg(not(test))]
    {
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().request_permission();
        permission_state(app)
    }
    #[cfg(test)]
    {
        let _ = app;
        "unsupported"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(account: &str, from: &str, subject: &str, inbox: bool, vip: bool) -> Notice {
        Notice {
            account_id: account.into(),
            thread_id: format!("t-{from}"),
            message_id: format!("m-{from}"),
            from: from.into(),
            subject: subject.into(),
            in_inbox: inbox,
            is_vip: vip,
        }
    }

    fn policy(filter: &str) -> Policy {
        Policy {
            filter: filter.into(),
            muted_accounts: vec![],
            hide_subject: false,
            sound: true,
        }
    }

    #[test]
    fn p9_t04_grouping() {
        let items: Vec<Notice> = (0..5)
            .map(|i| notice("a", &format!("n{i}"), "s", true, false))
            .collect();
        // A burst is one summary; a few messages stay individual banners.
        assert_eq!(group(items).len(), 1);
        let small: Vec<Notice> = (0..3)
            .map(|i| notice("a", &format!("n{i}"), "s", true, false))
            .collect();
        assert_eq!(group(small).len(), 3);
        assert!(!policy("off").allows(&notice("a", "x", "s", true, false)));
        assert!(policy("inbox").allows(&notice("a", "x", "s", true, false)));
        assert!(!policy("inbox").allows(&notice("a", "x", "s", false, false)));
    }

    #[test]
    fn p8_4_filters_vip_mute_and_hidden_subject() {
        // VIP only notifies a VIP *in the Inbox*.
        let vip = policy("vip");
        assert!(vip.allows(&notice("a", "Ada", "Hi", true, true)));
        assert!(!vip.allows(&notice("a", "Ada", "Hi", true, false)));
        assert!(!vip.allows(&notice("a", "Ada", "Hi", false, true)));
        // A disabled account is quiet even for a VIP in the Inbox.
        let muted = Policy {
            filter: FILTER_VIP.into(),
            muted_accounts: vec!["b".into()],
            hide_subject: false,
            sound: true,
        };
        assert!(!muted.allows(&notice("b", "Ada", "Hi", true, true)));
        assert!(muted.allows(&notice("a", "Ada", "Hi", true, true)));
        // Hidden subject never leaks the subject.
        let hidden = Policy {
            filter: FILTER_INBOX.into(),
            muted_accounts: vec![],
            hide_subject: true,
            sound: true,
        };
        let (title, body) = notice("a", "Ada", "Q3 payroll figures", true, false).title_body(&hidden);
        assert_eq!(title, "Ada");
        assert_eq!(body, "New message");
        assert!(!body.contains("payroll"));
        // ...and the sender is still named, so the banner is useful.
        let (_, shown) = notice("a", "Ada", "Q3 payroll", true, false).title_body(&policy("inbox"));
        assert_eq!(shown, "Q3 payroll");
    }

    #[test]
    fn p8_4_summary_navigates_to_the_oldest_message() {
        let items = vec![
            notice("a", "Ada", "first", true, false),
            notice("a", "Bob", "second", true, false),
            notice("a", "Cid", "third", true, false),
            notice("a", "Dee", "fourth", true, false),
        ];
        let (title, body, account, thread) = summary(&items, &policy("inbox"));
        assert_eq!(title, "Sift");
        assert!(body.contains("4 new messages"));
        assert!(body.contains("Ada"));
        assert_eq!(account, "a");
        assert_eq!(thread, "t-Ada");
    }
}
