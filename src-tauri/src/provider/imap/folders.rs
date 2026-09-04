//! Folder discovery (Phase 11 task 5): SPECIAL-USE attributes → roles.
//!
//! Roles come ONLY from attributes, never from names (names are localized).
//! A Gmail-side rename surfaces as delete + create; messages keep their
//! labels because `X-GM-LABELS` carries the new name (spec limitation note).
use super::conn::Conn;
use super::proto::decode_utf7_mailbox;
use crate::errors::SiftError;

/// Server folder names by role, plus user-label folders.
#[derive(Debug, Clone, Default)]
pub struct FolderMap {
    pub all: String,
    pub trash: String,
    pub junk: String,
    pub drafts: String,
    pub sent: String,
    /// Selectable non-system folders: (decoded display name, label id).
    /// Label id is `imap:<decoded name>` with `/` nesting preserved.
    pub user: Vec<(String, String)>,
}

impl FolderMap {
    pub fn name_for_role(&self, role: &str) -> Option<&str> {
        match role {
            "all" => Some(&self.all),
            "trash" => Some(&self.trash),
            "junk" => Some(&self.junk),
            "drafts" => Some(&self.drafts),
            "sent" => Some(&self.sent),
            "inbox" => Some("INBOX"),
            _ => None,
        }
    }
}

/// Pure mapping over parsed LIST entries: (attributes, delimiter, raw name).
/// Separated for unit testing without a connection.
pub fn map_folders(
    entries: Vec<(Vec<String>, Option<String>, String)>,
) -> Result<FolderMap, SiftError> {
    let mut map = FolderMap::default();
    for (attrs, _delim, raw) in entries {
        let name = decode_utf7_mailbox(&raw);
        let is_attr = |a: &str| attrs.iter().any(|x| x.eq_ignore_ascii_case(a));
        if is_attr("\\All") {
            if map.all.is_empty() {
                map.all = name;
            }
            continue;
        }
        if is_attr("\\Trash") {
            if map.trash.is_empty() {
                map.trash = name;
            }
            continue;
        }
        if is_attr("\\Junk") {
            if map.junk.is_empty() {
                map.junk = name;
            }
            continue;
        }
        if is_attr("\\Drafts") {
            if map.drafts.is_empty() {
                map.drafts = name;
            }
            continue;
        }
        if is_attr("\\Sent") {
            if map.sent.is_empty() {
                map.sent = name;
            }
            continue;
        }
        if raw.eq_ignore_ascii_case("INBOX") {
            continue;
        }
        if is_attr("\\Noselect") {
            continue;
        }
        if name.is_empty() {
            continue;
        }
        let id = format!("imap:{name}");
        if !map.user.iter().any(|(_, i)| *i == id) {
            map.user.push((name, id));
        }
    }
    if map.all.is_empty() {
        return Err(SiftError::app(
            "imap_all_mail_hidden",
            "All Mail is hidden from IMAP. In Gmail → Settings → Labels, turn on 'Show in IMAP' for All Mail.",
            false,
        ));
    }
    for (role, field) in [
        ("trash", &map.trash),
        ("junk", &map.junk),
        ("drafts", &map.drafts),
        ("sent", &map.sent),
    ] {
        if field.is_empty() {
            return Err(SiftError::app(
                "imap_protocol",
                format!("Gmail did not advertise a {role} folder"),
                true,
            ));
        }
    }
    Ok(map)
}

/// LIST with SPECIAL-USE return options, mapped to roles.
pub async fn discover(conn: &mut Conn) -> Result<FolderMap, SiftError> {
    let entries = conn.list_special().await?;
    map_folders(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::imap::proto::{parse_transcript, Response, Untagged};

    fn fixture_entries() -> Vec<(Vec<String>, Option<String>, String)> {
        let bytes = std::fs::read(format!(
            "{}/../fixtures/imap/list-special.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        parse_transcript(&bytes)
            .unwrap()
            .into_iter()
            .filter_map(|r| match r {
                Response::Untagged(Untagged::List { attrs, delim, name }) => {
                    Some((attrs, delim, name))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn p11_t04_localized_special_use_roles() {
        let map = map_folders(fixture_entries()).unwrap();
        assert_eq!(map.all, "[Gmail]/Alle Nachrichten");
        assert_eq!(map.trash, "[Gmail]/Papierkorb");
        assert_eq!(map.junk, "[Gmail]/Spam");
        assert_eq!(map.drafts, "[Gmail]/Entwürfe");
        assert_eq!(map.sent, "[Gmail]/Gesendet");
        // user folders decoded, ids namespaced
        assert!(map
            .user
            .contains(&("Büro".to_string(), "imap:Büro".to_string())));
        assert!(map
            .user
            .contains(&("Receipts".to_string(), "imap:Receipts".to_string())));
        // INBOX and the [Gmail] container are not user labels
        assert!(!map.user.iter().any(|(n, _)| n == "INBOX" || n == "[Gmail]"));
    }

    #[test]
    fn p11_t04_missing_all_mail_errors() {
        let mut entries = fixture_entries();
        entries.retain(|(_, _, n)| n != "[Gmail]/Alle Nachrichten");
        let e = map_folders(entries).unwrap_err();
        assert_eq!(
            serde_json::to_value(&e).unwrap()["code"],
            "imap_all_mail_hidden"
        );
    }
}
