//! Gmail IMAP/SMTP transport (Phase 11).
//!
//! The session is hand-rolled over tokio-rustls: real Gmail mailboxes
//! carry raw UTF-8 inside quoted `X-GM-LABELS`, which strict RFC parsers
//! reject, killing whole FETCH streams. `proto` parses exactly the
//! Appendix H subset, tolerantly.
pub mod conn;
pub mod convert;
pub mod errors;
pub mod folders;
pub mod full;
pub mod idle;
pub mod message;
pub mod ops;
pub mod partial;
pub mod proto;
pub mod provider;
pub mod smtp;
