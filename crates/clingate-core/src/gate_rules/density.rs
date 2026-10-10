//! The smoothed density of a population's values, worked out once for each
//! set of values and smoothing: a search reads the same population at the
//! same smoothing for every candidate it tries.

use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError};

use rustc_hash::FxHashMap;

/// Points on the grid a density is worked out at.
const GRID: usize = 512;
/// The most densities kept at once; the least recently used half go when
/// more are made.
pub(crate) const MOST_KEPT: usize = 4096;

/// A smoothed density of some values, on a grid from the least to the
/// greatest.
#[derive(Debug, PartialEq)]
pub struct Density {
    pub xs: Vec<f64>,
    pub density: Vec<f64>,
    /// How many finite values made it.
    pub events: usize,
    pub lo: f64,
    pub hi: f64,
    pub bandwidth: f64,
}

/// The density of `values` with Silverman's bandwidth times `smoothing`;
/// none for fewer than two values, values all in one place, or a smoothing
/// that is not a positive number.
pub fn smoothed(values: &[f64], smoothing: f64) -> Option<Arc<Density>> {
    if values.len() < 2 || !smoothing.is_finite() || smoothing <= 0.0 {
        return None;
    }
    // Hashed and worked out outside the lock: one search asks for the same
    // density from many threads at once, and all but the first wait for it
    // rather than each working it out.
    let key = key_of(values, smoothing);
    let cell = kept().cell(key);
    cell.get_or_init(|| worked_out(values, smoothing).map(Arc::new))
        .clone()
}

fn worked_out(values: &[f64], smoothing: f64) -> Option<Density> {
    let bandwidth = crate::gate_move::kde::silverman_bandwidth(values) * smoothing;
    if !bandwidth.is_finite() || bandwidth <= 0.0 {
        return None;
    }
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() || !hi.is_finite() || hi <= lo {
        return None;
    }
    let (xs, density) = crate::gate_move::kde::kde_1d(values, (lo, hi), GRID, bandwidth);
    Some(Density {
        xs,
        density,
        events: values.iter().filter(|v| v.is_finite()).count(),
        lo,
        hi,
        bandwidth,
    })
}

/// What `values` and `smoothing` are kept under: their content, hashed.
fn key_of(values: &[f64], smoothing: f64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    let mut bytes = Vec::with_capacity(8 * 1024);
    for chunk in values.chunks(1024) {
        bytes.clear();
        bytes.extend(chunk.iter().flat_map(|v| v.to_le_bytes()));
        hasher.update(&bytes);
    }
    hasher.update(&smoothing.to_le_bytes());
    *hasher.finalize().as_bytes()
}

type Cell = Arc<OnceLock<Option<Arc<Density>>>>;

#[derive(Default)]
struct Kept {
    cells: FxHashMap<[u8; 32], (Cell, u64)>,
    uses: u64,
}

static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(Mutex::default);

fn kept() -> std::sync::MutexGuard<'static, Kept> {
    KEPT.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Kept {
    /// The cell `key`'s density is kept in, empty until it is worked out.
    fn cell(&mut self, key: [u8; 32]) -> Cell {
        self.uses += 1;
        if let Some((cell, used)) = self.cells.get_mut(&key) {
            *used = self.uses;
            return cell.clone();
        }
        if self.cells.len() >= MOST_KEPT {
            self.forget_least_used();
        }
        let cell = Cell::default();
        self.cells.insert(key, (cell.clone(), self.uses));
        cell
    }

    fn forget_least_used(&mut self) {
        let mut used: Vec<u64> = self.cells.values().map(|(_, used)| *used).collect();
        let middle = used.len() / 2;
        let (_, &mut median, _) = used.select_nth_unstable(middle);
        self.cells.retain(|_, (_, used)| *used > median);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spread(n: usize, from: f64) -> Vec<f64> {
        (0..n)
            .map(|i| from + (i as f64 * 0.37).sin() * 10.0)
            .collect()
    }

    #[test]
    fn the_same_values_at_the_same_smoothing_share_one_density() {
        let values = spread(200, 0.0);
        let first = smoothed(&values, 0.75).unwrap();
        let again = smoothed(&values.clone(), 0.75).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
    }

    #[test]
    fn a_density_is_worked_out_as_a_kernel_density_of_the_values() {
        let values = spread(300, 5.0);
        let bandwidth = crate::gate_move::kde::silverman_bandwidth(&values) * 1.5;
        let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let (xs, density) = crate::gate_move::kde::kde_1d(&values, (lo, hi), 512, bandwidth);
        let found = smoothed(&values, 1.5).unwrap();
        assert_eq!(found.xs, xs);
        assert_eq!(found.density, density);
        assert_eq!(found.events, 300);
    }

    #[test]
    fn other_values_or_another_smoothing_get_their_own_density() {
        let values = spread(200, 100.0);
        let mut moved = values.clone();
        moved[150] += 1.0;
        let first = smoothed(&values, 1.0).unwrap();
        assert_ne!(*smoothed(&moved, 1.0).unwrap(), *first);
        assert_ne!(*smoothed(&values, 2.0).unwrap(), *first);
    }

    #[test]
    fn nothing_to_smooth_gives_no_density() {
        assert!(smoothed(&[1.0], 1.0).is_none());
        assert!(smoothed(&[2.0, 2.0, 2.0], 1.0).is_none());
        assert!(smoothed(&spread(50, 0.0), 0.0).is_none());
        assert!(smoothed(&spread(50, 0.0), f64::NAN).is_none());
    }

    /// A key no other test's values hash to.
    fn key(at: usize) -> [u8; 32] {
        let mut key = [7u8; 32];
        key[..8].copy_from_slice(&at.to_le_bytes());
        key
    }

    #[test]
    fn the_least_recently_used_are_forgotten_beyond_the_most_kept() {
        // A shelf of its own, so no other test's densities are forgotten.
        let mut kept = Kept::default();
        let (used_key, unused_key) = (key(usize::MAX), key(usize::MAX - 1));
        let used = kept.cell(used_key);
        let unused = kept.cell(unused_key);
        for at in 0..MOST_KEPT {
            if at % 64 == 0 {
                assert!(Arc::ptr_eq(&kept.cell(used_key), &used));
            }
            kept.cell(key(at));
        }
        assert!(Arc::ptr_eq(&kept.cell(used_key), &used));
        assert!(!Arc::ptr_eq(&kept.cell(unused_key), &unused));
        assert!(kept.cells.len() <= MOST_KEPT);
    }
}
