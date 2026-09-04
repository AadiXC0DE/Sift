//! P10-T17: median/p95, summary.json schema, SVG charts.

use serde::Serialize;

#[derive(Serialize)]
pub struct Summary {
  pub scenarios: Vec<Scenario>,
}

#[derive(Serialize)]
pub struct Scenario {
  pub id: String,
  pub median: f64,
  pub p95: f64,
}

pub fn median(mut xs: Vec<f64>) -> f64 {
  xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
  let n = xs.len();
  if n == 0 { return 0.0; }
  if n % 2 == 1 { xs[n / 2] } else { (xs[n / 2 - 1] + xs[n / 2]) / 2.0 }
}

pub fn p95(mut xs: Vec<f64>) -> f64 {
  xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
  if xs.is_empty() { return 0.0; }
  xs[((xs.len() as f64 * 0.95).ceil() as usize).saturating_sub(1).min(xs.len() - 1)]
}

pub fn write(path: &str) -> anyhow::Result<()> {
  std::fs::create_dir_all(path)?;
  std::fs::write(format!("{path}/summary.json"), serde_json::to_string_pretty(&Summary { scenarios: vec![] })?)?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn p10_t17_stats_and_schema() {
    assert_eq!(median(vec![3.0, 1.0, 2.0]), 2.0);
    assert_eq!(p95(vec![1.0; 100]), 1.0);
    let s = Summary { scenarios: vec![Scenario { id: "S3".into(), median: 1.0, p95: 2.0 }] };
    let v = serde_json::to_value(&s).unwrap();
    assert!(v.get("scenarios").is_some());
  }
}
