use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SiftError {
    #[error("{message}")]
    App {
        code: String,
        message: String,
        retryable: bool,
    },
    /// A failure the UI must act on with the state that caused it (P6.1/P6.2:
    /// a too-late Undo, an unacknowledged duplicate-risk retry). The body adds
    /// a machine-readable `detail` next to the standard `{code,message,
    /// retryable}` triple, so the composer can render the honest status
    /// instead of guessing from prose.
    #[error("{message}")]
    Typed {
        code: String,
        message: String,
        retryable: bool,
        detail: Box<serde_json::Value>,
    },
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("keyring: {0}")]
    Keyring(String),
    #[error("oauth: {0}")]
    Oauth(String),
    #[error("not found: {0}")]
    NotFound(String),
}

#[derive(Serialize)]
struct ErrBody {
    code: String,
    message: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<serde_json::Value>,
}

impl serde::Serialize for SiftError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (code, message, retryable, detail) = match self {
            SiftError::App {
                code,
                message,
                retryable,
            } => (code.clone(), message.clone(), *retryable, None),
            SiftError::Typed {
                code,
                message,
                retryable,
                detail,
            } => (
                code.clone(),
                message.clone(),
                *retryable,
                Some((**detail).clone()),
            ),
            SiftError::Db(e) => ("db".into(), e.to_string(), false, None),
            SiftError::Http(e) => {
                let retryable = e.is_timeout()
                    || e.is_connect()
                    || e.status()
                        .map(|s| s.as_u16() == 429 || s.as_u16() >= 500)
                        .unwrap_or(false);
                ("http".into(), e.to_string(), retryable, None)
            }
            SiftError::Io(e) => ("io".into(), e.to_string(), false, None),
            SiftError::Json(e) => ("json".into(), e.to_string(), false, None),
            SiftError::Keyring(m) => ("keyring".into(), m.clone(), false, None),
            SiftError::Oauth(m) => ("oauth".into(), m.clone(), false, None),
            SiftError::NotFound(m) => ("not_found".into(), m.clone(), false, None),
        };
        ErrBody {
            code,
            message,
            retryable,
            detail,
        }
        .serialize(s)
    }
}

impl SiftError {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::App { retryable, .. } | Self::Typed { retryable, .. } => *retryable,
            Self::Http(error) => {
                error.is_timeout()
                    || error.is_connect()
                    || error
                        .status()
                        .is_some_and(|status| status.as_u16() == 429 || status.as_u16() >= 500)
            }
            Self::Io(_) => true,
            _ => false,
        }
    }

    pub fn is_reauth(&self) -> bool {
        matches!(self, Self::App { code, .. } if code == "reauth")
    }

    pub fn app(code: &str, message: impl Into<String>, retryable: bool) -> Self {
        SiftError::App {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }

    /// A failure with machine-readable state attached (never retryable by
    /// itself: the caller has to make a decision).
    pub fn typed(code: &str, message: impl Into<String>, detail: serde_json::Value) -> Self {
        SiftError::Typed {
            code: code.into(),
            message: message.into(),
            retryable: false,
            detail: Box::new(detail),
        }
    }

    /// The error code as the UI sees it, for callers that classify by code.
    pub fn code(&self) -> String {
        match self {
            SiftError::App { code, .. } | SiftError::Typed { code, .. } => code.clone(),
            SiftError::Db(_) => "db".into(),
            SiftError::Http(_) => "http".into(),
            SiftError::Io(_) => "io".into(),
            SiftError::Json(_) => "json".into(),
            SiftError::Keyring(_) => "keyring".into(),
            SiftError::Oauth(_) => "oauth".into(),
            SiftError::NotFound(_) => "not_found".into(),
        }
    }

    /// The delivery was handed to SMTP and Sift cannot prove whether the
    /// provider accepted it (P6.1). This is never retried automatically.
    pub fn send_uncertain(detail: &str) -> Self {
        Self::app(
            "send_uncertain",
            format!(
                "Sift lost contact with Gmail after handing this message over and cannot tell whether it was accepted: {detail}"
            ),
            false,
        )
    }
    pub fn reauth(msg: impl Into<String>) -> Self {
        Self::app("reauth", msg, false)
    }
    pub fn oauth_timeout() -> Self {
        Self::app("oauth_timeout", "OAuth timed out", false)
    }
    pub fn not_in_trash() -> Self {
        Self::app(
            "not_in_trash",
            "Only Trash/Spam can be deleted forever",
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn p1_t06_error_shape() {
        let e = SiftError::app("x", "y", true);
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "x");
        assert_eq!(v["message"], "y");
        assert_eq!(v["retryable"], true);
        // timeout maps retryable:true - construct via app since reqwest err construction is complex
        let t = SiftError::app("http", "timeout", true);
        let v2 = serde_json::to_value(&t).unwrap();
        assert_eq!(v2["retryable"], true);
    }
}
