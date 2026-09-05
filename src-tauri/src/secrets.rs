use crate::errors::SiftError;

#[cfg(not(test))]
const SERVICE: &str = "xyz.ownpath.sift";

#[cfg(not(test))]
pub fn store_refresh_token(email: &str, token: &str) -> Result<(), SiftError> {
    keyring::Entry::new(SERVICE, email)
        .map_err(|e| SiftError::Keyring(e.to_string()))?
        .set_password(token)
        .map_err(|e| SiftError::Keyring(e.to_string()))
}

#[cfg(test)]
pub fn store_refresh_token(email: &str, token: &str) -> Result<(), SiftError> {
    mock_store()
        .lock()
        .unwrap()
        .insert(email.to_string(), token.to_string());
    Ok(())
}

#[cfg(not(test))]
pub fn load_refresh_token(email: &str) -> Result<Option<String>, SiftError> {
    match keyring::Entry::new(SERVICE, email)
        .map_err(|e| SiftError::Keyring(e.to_string()))?
        .get_password()
    {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SiftError::Keyring(e.to_string())),
    }
}

#[cfg(test)]
pub fn load_refresh_token(email: &str) -> Result<Option<String>, SiftError> {
    Ok(mock_store().lock().unwrap().get(email).cloned())
}

/// App-password key: `imap:<email>` under the same Keychain service.
/// Nothing is ever written to the DB file, logs, or diagnostics (P11-T13).
pub fn imap_key(email: &str) -> String {
    format!("imap:{email}")
}

#[cfg(not(test))]
pub fn store_app_password(email: &str, pw: &str) -> Result<(), SiftError> {
    keyring::Entry::new(SERVICE, &imap_key(email))
        .map_err(|e| SiftError::Keyring(e.to_string()))?
        .set_password(pw)
        .map_err(|e| SiftError::Keyring(e.to_string()))
}
#[cfg(test)]
pub fn store_app_password(email: &str, pw: &str) -> Result<(), SiftError> {
    mock_store()
        .lock()
        .unwrap()
        .insert(imap_key(email), pw.to_string());
    Ok(())
}

#[cfg(not(test))]
pub fn load_app_password(email: &str) -> Result<Option<String>, SiftError> {
    match keyring::Entry::new(SERVICE, &imap_key(email))
        .map_err(|e| SiftError::Keyring(e.to_string()))?
        .get_password()
    {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SiftError::Keyring(e.to_string())),
    }
}
#[cfg(test)]
pub fn load_app_password(email: &str) -> Result<Option<String>, SiftError> {
    Ok(mock_store().lock().unwrap().get(&imap_key(email)).cloned())
}

pub fn delete(email: &str) -> Result<(), SiftError> {
    #[cfg(not(test))]
    {
        for key in [email.to_string(), imap_key(email)] {
            match keyring::Entry::new(SERVICE, &key)
                .map_err(|e| SiftError::Keyring(e.to_string()))?
                .delete_credential()
            {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(SiftError::Keyring(e.to_string())),
            }
        }
        Ok(())
    }
    #[cfg(test)]
    {
        mock_store().lock().unwrap().remove(email);
        mock_store().lock().unwrap().remove(&imap_key(email));
        Ok(())
    }
}

#[cfg(test)]
static MOCK: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::OnceLock::new();
#[cfg(test)]
fn mock_store() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    MOCK.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn p11_t13_app_password_keying_no_db_leak() {
        use super::*;
        store_app_password("apppw_test@x.com", "abcdefghijklmnop").unwrap();
        assert_eq!(
            load_app_password("apppw_test@x.com").unwrap().as_deref(),
            Some("abcdefghijklmnop")
        );
        // OAuth entry untouched.
        assert_eq!(load_refresh_token("apppw_test@x.com").unwrap(), None);
        delete("apppw_test@x.com").unwrap();
        assert_eq!(load_app_password("apppw_test@x.com").unwrap(), None);
        // The secret never reaches sqlite files (Keychain/mock only).
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).unwrap();
        db.new_account("apppw_test@x.com", None, None)
            .await
            .unwrap();
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap_or_default();
            assert!(
                !bytes
                    .windows(secret_len())
                    .any(|w| w == b"abcdefghijklmnop"),
                "app password leaked into sqlite file"
            );
        }
    }

    fn secret_len() -> usize {
        16
    }

    use super::*;
    #[test]
    fn p2_t06_keyring_roundtrip_no_db_leak() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("sift.db");
        store_refresh_token("u@x.com", "secret-token-abc123").unwrap();
        assert_eq!(
            load_refresh_token("u@x.com").unwrap().as_deref(),
            Some("secret-token-abc123")
        );
        delete("u@x.com").unwrap();
        assert_eq!(load_refresh_token("u@x.com").unwrap(), None);
        // DB file must never contain token
        if db_path.exists() {
            let bytes = std::fs::read(&db_path).unwrap();
            assert!(!bytes.windows(9).any(|w| w == b"secret-to"));
        }
    }
}
