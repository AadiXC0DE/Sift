//! Pixel-diff detector: first frame exceeding threshold wins; ignores sub-threshold noise.
//! Timestamps use capture presentation time, not receipt time.

pub struct Detector {
  pub threshold: f64,
  pub min_fraction: f64,
}

impl Detector {
  pub fn first_change(&self, frames: &[(u64, Vec<u8>)], reference: &[u8]) -> Option<u64> {
    for (ts, px) in frames {
      if mean_abs_diff(px, reference) > self.threshold
        && changed_fraction(px, reference) >= self.min_fraction
      {
        return Some(*ts);
      }
    }
    None
  }
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
  let n = a.len().min(b.len()).max(1) as f64;
  a.iter().zip(b.iter()).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / (n * 255.0)
}

fn changed_fraction(a: &[u8], b: &[u8]) -> f64 {
  let n = a.len().min(b.len()).max(1) as f64;
  a.iter().zip(b.iter()).filter(|(x, y)| ((**x as i16) - (**y as i16)).abs() > 2).count() as f64 / n
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn p10_t16_detector_fires_exactly_once() {
    let det = Detector { threshold: 2.0 / 255.0, min_fraction: 0.005 };
    let base = vec![10u8; 1000];
    let noise: Vec<u8> = base.iter().map(|x| x + 1).collect(); // sub-threshold
    let mut changed = base.clone();
    for b in changed.iter_mut().take(20) { *b = 200; }
    let frames = vec![(100, noise), (200, changed.clone()), (300, changed)];
    assert_eq!(det.first_change(&frames, &base), Some(200));
  }
}
