//! Test support: shared fakes. Each integration test does
//! `#[path = "support/mod.rs"] mod support;` (compiled per test binary).
#![allow(dead_code)] // compiled per test binary; items used across different suites.
pub mod fake_imap;
pub mod fake_smtp;
