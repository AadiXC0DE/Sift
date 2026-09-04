//! P10-T20: detect installed apps from /Applications tree.

use std::path::Path;

pub fn detect(apps_dir: &Path) -> Vec<(String, String)> {
  let known = [
    ("Sift", "Sift.app"),
    ("Mail", "Mail.app"),
    ("Spark", "Spark.app"),
    ("Thunderbird", "Thunderbird.app"),
  ];
  let mut out = vec![];
  for (name, bundle) in known {
    let p = apps_dir.join(bundle);
    if p.exists() {
      let ver = version_of(&p).unwrap_or_else(|| "unknown".into());
      out.push((name.into(), ver));
    }
  }
  out
}

fn version_of(bundle: &Path) -> Option<String> {
  let plist = bundle.join("Contents/Info.plist");
  let text = std::fs::read_to_string(plist).ok()?;
  // crude parse CFBundleShortVersionString
  text.split("CFBundleShortVersionString").nth(1)?
    .split("<string>").nth(1)?
    .split("</string>").next()
    .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn p10_t20_detects_present_absent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Mail.app")).unwrap();
    std::fs::create_dir_all(dir.path().join("Sift.app/Contents")).unwrap();
    std::fs::write(dir.path().join("Sift.app/Contents/Info.plist"), "<plist><key>CFBundleShortVersionString</key><string>1.0.0</string></plist>").unwrap();
    let found = detect(dir.path());
    assert!(found.iter().any(|(n, _)| n == "Mail"));
    assert!(found.iter().any(|(n, v)| n == "Sift" && v == "1.0.0"));
    assert!(!found.iter().any(|(n, _)| n == "Spark"));
  }
}
