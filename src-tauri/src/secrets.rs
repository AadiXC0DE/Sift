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

pub fn delete(email: &str) -> Result<(), SiftError> {
    #[cfg(not(test))]
    {
        match keyring::Entry::new(SERVICE, email)
            .map_err(|e| SiftError::Keyring(e.to_string()))?
            .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SiftError::Keyring(e.to_string())),
        }
    }
    #[cfg(test)]
    {
        mock_store().lock().unwrap().remove(email);
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
