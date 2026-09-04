//! P10-T21: quick-mode guide (permissions, pause on input, skip rows).

pub fn quick(apps: Option<String>, runs: String) -> anyhow::Result<()> {
  println!("Sift benchmark quick mode (apps={apps:?}, runs={runs})");
  println!("1. Grant Screen Recording + Input Monitoring when prompted.");
  println!("2. Open each app, sign in, show inbox, press Enter.");
  println!("3. Do not touch keyboard/mouse while driving (pauses on input).");
  Ok(())
}

pub fn should_pause_on_input(user_typing: bool) -> bool {
  user_typing
}

pub fn skip_row(app: &str, signed_in: bool) -> Option<String> {
  if !signed_in {
    Some(format!("{app}: skipped: not signed in"))
  } else {
    None
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn p10_t21_pause_and_skip() {
    assert!(should_pause_on_input(true));
    assert!(!should_pause_on_input(false));
    assert_eq!(skip_row("Mail", false), Some("Mail: skipped: not signed in".into()));
    assert_eq!(skip_row("Mail", true), None);
  }
}
