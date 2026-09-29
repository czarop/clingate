//! A population's distribution on one parameter, kept small enough to store
//! for every placement of every run: its percentiles and its peaks.
//!
//! A run keeps one of these for each gate it places or leaves alone, taken
//! from the parent population the rule read. They are what a run is
//! assessed on afterwards - where a gate sits against its own sample's peaks
//! and spread, and how that sample's distribution compares with the others
//! of its kind - without reading a file again.

use serde::{Deserialize, Serialize};

/// Bins the peaks are looked for in, across the middle 99% of the events.
const PEAK_BINS: usize = 128;
/// The smoothing, in bins, before peaks are looked for: enough to stop noise
/// in a sparse tail reading as structure.
const SMOOTHING_BINS: f64 = 2.5;
/// A peak lower than this fraction of the highest is not counted.
const PEAK_FLOOR: f64 = 0.05;
/// A dip must fall this far below the lower of the two peaks beside it for
/// them to count as two peaks rather than one.
const DIP_DEPTH: f64 = 0.15;
/// At most this many peaks are kept, highest first.
const MAX_PEAKS: usize = 4;

/// One peak of a distribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Peak {
    /// Where it is, in the parameter's units.
    pub at: f64,
    /// Its height, as a fraction of the highest peak's.
    pub height: f64,
    /// The fraction of the events under it - between the dips either side.
    pub share: f64,
}

/// A population on one parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub events: usize,
    /// The 0th to 100th percentiles, 101 of them.
    pub percentiles: Vec<f64>,
    /// Its peaks, highest first.
    pub peaks: Vec<Peak>,
}

impl Shape {
    /// The percentile `p`, 0 to 100, read between the stored ones.
    pub fn percentile(&self, p: f64) -> f64 {
        let p = p.clamp(0.0, 100.0);
        let lo = p.floor() as usize;
        let hi = (lo + 1).min(100);
        let t = p - lo as f64;
        self.percentiles[lo] * (1.0 - t) + self.percentiles[hi] * t
    }

    pub fn median(&self) -> f64 {
        self.percentiles[50]
    }

    pub fn iqr(&self) -> f64 {
        self.percentiles[75] - self.percentiles[25]
    }

    /// The fraction of the events at or below `x`, read off the percentiles.
    pub fn fraction_below(&self, x: f64) -> f64 {
        let p = &self.percentiles;
        if x <= p[0] {
            return 0.0;
        }
        if x >= p[100] {
            return 1.0;
        }
        let i = p.partition_point(|v| *v <= x).clamp(1, 100);
        let (a, b) = (p[i - 1], p[i]);
        let t = if b > a { (x - a) / (b - a) } else { 0.0 };
        ((i - 1) as f64 + t) / 100.0
    }

    /// The two highest peaks, lowest on the axis first - the negative and
    /// the positive, for a marker that separates.
    pub fn two_peaks(&self) -> Option<(f64, f64)> {
        let [a, b, ..] = self.peaks.as_slice() else {
            return None;
        };
        Some(if a.at <= b.at {
            (a.at, b.at)
        } else {
            (b.at, a.at)
        })
    }
}

/// Summarise `values`. `None` with too few finite values to say anything.
pub fn summarise(values: &[f64]) -> Option<Shape> {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.len() < 5 {
        return None;
    }
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let at = |q: f64| -> f64 {
        let pos = q * (n - 1) as f64;
        let lo = pos.floor() as usize;
        let hi = (lo + 1).min(n - 1);
        let t = pos - lo as f64;
        sorted[lo] * (1.0 - t) + sorted[hi] * t
    };
    let percentiles: Vec<f64> = (0..=100).map(|p| at(p as f64 / 100.0)).collect();
    Some(Shape {
        events: n,
        peaks: peaks(&sorted, at(0.005), at(0.995)),
        percentiles,
    })
}

/// The peaks of sorted `values` between `lo` and `hi`.
fn peaks(sorted: &[f64], lo: f64, hi: f64) -> Vec<Peak> {
    if hi <= lo {
        // All the events in one place: one peak.
        return vec![Peak {
            at: lo,
            height: 1.0,
            share: 1.0,
        }];
    }
    let width = (hi - lo) / PEAK_BINS as f64;
    let mut counts = vec![0.0f64; PEAK_BINS];
    for v in sorted {
        if *v < lo || *v > hi {
            continue;
        }
        let i = (((v - lo) / width) as usize).min(PEAK_BINS - 1);
        counts[i] += 1.0;
    }
    // A Gaussian kernel, cut at three widths.
    let reach = (3.0 * SMOOTHING_BINS).ceil() as isize;
    let kernel: Vec<f64> = (-reach..=reach)
        .map(|k| (-0.5 * (k as f64 / SMOOTHING_BINS).powi(2)).exp())
        .collect();
    let smooth: Vec<f64> = (0..PEAK_BINS as isize)
        .map(|i| {
            let mut sum = 0.0;
            let mut weight = 0.0;
            for (j, w) in kernel.iter().enumerate() {
                let k = i + j as isize - reach;
                if (0..PEAK_BINS as isize).contains(&k) {
                    sum += counts[k as usize] * w;
                    weight += w;
                }
            }
            sum / weight
        })
        .collect();
    let top = smooth.iter().copied().fold(0.0, f64::max);
    if top <= 0.0 {
        return Vec::new();
    }

    // Local maxima above the floor.
    let mut maxima: Vec<usize> = (0..PEAK_BINS)
        .filter(|&i| {
            let left = if i == 0 { f64::MIN } else { smooth[i - 1] };
            let right = if i + 1 == PEAK_BINS {
                f64::MIN
            } else {
                smooth[i + 1]
            };
            smooth[i] >= left && smooth[i] > right && smooth[i] >= PEAK_FLOOR * top
        })
        .collect();
    // Merge neighbours without a real dip between them, keeping the higher.
    let mut merged = true;
    while merged && maxima.len() > 1 {
        merged = false;
        for w in 0..maxima.len() - 1 {
            let (a, b) = (maxima[w], maxima[w + 1]);
            let dip = smooth[a..=b].iter().copied().fold(f64::MAX, f64::min);
            let lower = smooth[a].min(smooth[b]);
            if dip > lower * (1.0 - DIP_DEPTH) {
                let lose = if smooth[a] >= smooth[b] { w + 1 } else { w };
                maxima.remove(lose);
                merged = true;
                break;
            }
        }
    }

    // Each peak's share: the events between the dips either side of it.
    let total: f64 = counts.iter().sum();
    let mut bounds = vec![0usize];
    for w in 0..maxima.len().saturating_sub(1) {
        let (a, b) = (maxima[w], maxima[w + 1]);
        let dip = (a..=b)
            .min_by(|x, y| smooth[*x].total_cmp(&smooth[*y]))
            .unwrap_or(a);
        bounds.push(dip);
    }
    bounds.push(PEAK_BINS);
    let mut found: Vec<Peak> = maxima
        .iter()
        .enumerate()
        .map(|(k, &i)| Peak {
            at: lo + (i as f64 + 0.5) * width,
            height: smooth[i] / top,
            share: if total > 0.0 {
                counts[bounds[k]..bounds[k + 1]].iter().sum::<f64>() / total
            } else {
                0.0
            },
        })
        .collect();
    found.sort_by(|a, b| b.height.total_cmp(&a.height));
    found.truncate(MAX_PEAKS);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_distr::{Distribution, Normal};

    fn draw(parts: &[(f64, f64, usize)], seed: u64) -> Vec<f64> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        parts
            .iter()
            .flat_map(|&(mean, sd, n)| {
                let d = Normal::new(mean, sd).unwrap();
                (0..n).map(|_| d.sample(&mut rng)).collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn a_negative_and_a_positive_are_two_peaks_in_their_places() {
        let shape = summarise(&draw(&[(0.0, 0.3, 8_000), (3.0, 0.4, 2_000)], 1)).unwrap();
        assert_eq!(shape.events, 10_000);
        let (neg, pos) = shape.two_peaks().expect("two peaks");
        assert!((neg - 0.0).abs() < 0.2, "{shape:?}");
        assert!((pos - 3.0).abs() < 0.3, "{shape:?}");
        // The negative is the higher, and holds most of the events.
        assert!(shape.peaks[0].at < 1.0 && shape.peaks[0].share > 0.7);
        assert!((shape.peaks[1].share - 0.2).abs() < 0.05, "{shape:?}");
    }

    #[test]
    fn one_population_is_one_peak_however_noisy() {
        let shape = summarise(&draw(&[(1.0, 0.5, 3_000)], 2)).unwrap();
        assert_eq!(shape.peaks.len(), 1, "{shape:?}");
        assert!((shape.median() - 1.0).abs() < 0.05);
    }

    #[test]
    fn percentiles_and_fractions_read_both_ways() {
        let values: Vec<f64> = (0..=1000).map(|i| i as f64 / 10.0).collect();
        let shape = summarise(&values).unwrap();
        assert!((shape.percentile(25.0) - 25.0).abs() < 1e-9);
        assert!((shape.iqr() - 50.0).abs() < 1e-9);
        assert!((shape.fraction_below(90.0) - 0.9).abs() < 1e-6);
        assert_eq!(shape.fraction_below(-1.0), 0.0);
        assert_eq!(shape.fraction_below(101.0), 1.0);
    }

    #[test]
    fn every_event_in_one_place_is_one_peak_holding_them_all() {
        let shape = summarise(&[2.5; 40]).unwrap();
        assert_eq!(shape.events, 40);
        assert_eq!(shape.peaks.len(), 1);
        assert_eq!(shape.peaks[0].at, 2.5);
        assert_eq!(shape.peaks[0].share, 1.0);
        assert!(shape.percentiles.iter().all(|p| *p == 2.5));
        assert_eq!(shape.iqr(), 0.0);
        assert!(shape.two_peaks().is_none());
        assert_eq!(shape.fraction_below(2.0), 0.0);
        assert_eq!(shape.fraction_below(3.0), 1.0);
    }

    #[test]
    fn values_that_are_not_numbers_are_left_out_and_not_counted() {
        let mut values: Vec<f64> = (0..100).map(|i| i as f64).collect();
        values.extend([f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::NAN]);
        let shape = summarise(&values).unwrap();
        assert_eq!(shape.events, 100);
        assert_eq!(shape.percentiles[0], 0.0);
        assert_eq!(shape.percentiles[100], 99.0);
        assert!(shape.percentiles.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn exactly_five_events_are_enough_and_four_are_not() {
        assert!(summarise(&[1.0, 2.0, 3.0, 4.0]).is_none());
        let shape = summarise(&[1.0, 2.0, 3.0, 4.0, 5.0]).unwrap();
        assert_eq!(shape.median(), 3.0);
        assert_eq!(shape.percentiles.len(), 101);
    }

    #[test]
    fn a_population_below_the_floor_is_not_a_peak_and_one_above_it_is() {
        // 2% at 6 is under the 5%-of-the-highest floor once smoothed...
        let faint = summarise(&draw(&[(0.0, 0.3, 9_800), (6.0, 0.3, 200)], 3)).unwrap();
        assert_eq!(faint.peaks.len(), 1, "{faint:?}");
        // ...15% is not.
        let clear = summarise(&draw(&[(0.0, 0.3, 8_500), (6.0, 0.3, 1_500)], 3)).unwrap();
        assert_eq!(clear.peaks.len(), 2, "{clear:?}");
    }

    #[test]
    fn two_populations_without_a_dip_between_them_are_one_peak() {
        // Means one standard deviation apart: a shoulder, not two peaks.
        let shoulder = summarise(&draw(&[(0.0, 0.5, 5_000), (0.5, 0.5, 5_000)], 4)).unwrap();
        assert_eq!(shoulder.peaks.len(), 1, "{shoulder:?}");
        // Six apart: two.
        let apart = summarise(&draw(&[(0.0, 0.5, 5_000), (3.0, 0.5, 5_000)], 4)).unwrap();
        assert_eq!(apart.peaks.len(), 2, "{apart:?}");
    }

    #[test]
    fn at_most_four_peaks_are_kept_the_highest_first() {
        let parts: Vec<(f64, f64, usize)> = (0..6)
            .map(|k| (k as f64 * 5.0, 0.3, 1_000 + k * 400))
            .collect();
        let shape = summarise(&draw(&parts, 5)).unwrap();
        assert_eq!(shape.peaks.len(), 4, "{shape:?}");
        for w in shape.peaks.windows(2) {
            assert!(w[0].height >= w[1].height);
        }
        assert_eq!(shape.peaks[0].height, 1.0);
        // The two smallest populations, at 0 and 5, are the ones left out.
        assert!(shape.peaks.iter().all(|p| p.at > 7.0), "{shape:?}");
    }

    #[test]
    fn the_shares_of_the_peaks_add_up_to_the_middle_of_the_population() {
        let shape = summarise(&draw(&[(0.0, 0.3, 6_000), (3.0, 0.3, 4_000)], 6)).unwrap();
        let total: f64 = shape.peaks.iter().map(|p| p.share).sum();
        assert!((total - 1.0).abs() < 1e-9, "{total}");
        let low = shape.peaks.iter().find(|p| p.at < 1.5).unwrap();
        assert!((low.share - 0.6).abs() < 0.03, "{shape:?}");
    }

    #[test]
    fn a_positive_taller_than_the_negative_still_comes_second() {
        let shape = summarise(&draw(&[(0.0, 0.3, 2_000), (3.0, 0.3, 8_000)], 7)).unwrap();
        assert!(shape.peaks[0].at > 2.0, "the positive is the highest peak");
        let (neg, pos) = shape.two_peaks().unwrap();
        assert!(neg < 1.0 && pos > 2.0);
    }

    #[test]
    fn a_percentile_outside_0_to_100_is_the_nearest_end_and_between_is_read_across() {
        let values: Vec<f64> = (0..=100).map(|i| i as f64).collect();
        let shape = summarise(&values).unwrap();
        assert_eq!(shape.percentile(-5.0), 0.0);
        assert_eq!(shape.percentile(150.0), 100.0);
        assert!((shape.percentile(12.5) - 12.5).abs() < 1e-9);
        assert!((shape.fraction_below(12.5) - 0.125).abs() < 1e-9);
    }

    #[test]
    fn the_same_events_in_any_order_give_the_same_summary() {
        let values = draw(&[(0.0, 0.3, 3_000), (3.0, 0.4, 1_000)], 8);
        let mut reversed = values.clone();
        reversed.reverse();
        assert_eq!(summarise(&values), summarise(&reversed));
    }

    #[test]
    fn a_summary_is_written_and_read_back_unchanged() {
        let shape = summarise(&draw(&[(0.0, 0.3, 3_000), (3.0, 0.4, 1_000)], 9)).unwrap();
        let text = serde_json::to_string(&shape).unwrap();
        let back: Shape = serde_json::from_str(&text).unwrap();
        assert_eq!(back, shape);
    }

    #[test]
    fn too_few_events_say_nothing() {
        assert!(summarise(&[1.0, 2.0]).is_none());
        assert!(summarise(&[f64::NAN; 20]).is_none());
    }
}
