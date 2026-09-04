#[derive(Debug, Clone)]
pub struct NewMail {
    pub account_id: String,
    pub thread_id: String,
    pub from: String,
    pub subject: String,
}

pub fn should_notify(setting: &str, in_inbox: bool, per_account_off: bool) -> bool {
    if per_account_off {
        return false;
    }
    match setting {
        "off" => false,
        "everything" => true,
        _ => in_inbox, // inbox only
    }
}

pub fn group_notices(items: Vec<NewMail>) -> Vec<Vec<NewMail>> {
    // Group when >3 arrive within 5s window: caller batches; here group all >3 into one
    if items.len() > 3 {
        vec![items]
    } else {
        items.into_iter().map(|i| vec![i]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn p9_t04_grouping() {
        let items: Vec<NewMail> = (0..5)
            .map(|i| NewMail {
                account_id: "a".into(),
                thread_id: format!("t{i}"),
                from: "x".into(),
                subject: "s".into(),
            })
            .collect();
        assert_eq!(group_notices(items).len(), 1);
        assert!(!should_notify("off", true, false));
        assert!(should_notify("inbox", true, false));
        assert!(!should_notify("inbox", false, false));
        assert!(!should_notify("everything", true, true));
    }
}
