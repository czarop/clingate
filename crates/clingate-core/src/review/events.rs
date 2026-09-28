//! A sample of the events behind every gate a run measured - kept with the
//! run, and with its review in the library.
//!
//! A run's record says what the rules decided and why; it does not keep the
//! data they decided on. Reports keep their own, but a report is only made
//! for a placement that went wrong. Changing a rule safely needs the others
//! too: a change that fixes the reported placements has to be checked
//! against the placements that were right, or it trades one set of mistakes
//! for another. So each run keeps, for every gate on every file it measured
//! - placed, left alone, and every file the rules read (FMOs, reference
//! samples) - up to [`KEPT_EVENTS`] events of the parent population on the
//! gate's two parameters, in the plot's own units.
//!
//! Kept compactly in one binary file, `reviews/run_events.bin`, each
//! coordinate as 16 bits across the range the events span: to within
//! 1/131,070 of that range, a thousandth of a pixel on a plot. About 20 KB a
//! gate a file. The file is stamped with the run it belongs to, so a stale
//! one is never read as a newer run's.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file a workspace keeps its last run's events in, in `reviews`.
pub const EVENTS_FILE: &str = "run_events.bin";
/// At most this many events of a population are kept.
pub const KEPT_EVENTS: usize = 5_000;

const MAGIC: &[u8; 4] = b"CGEV";
const VERSION: u32 = 1;

/// Every `n`th event, so what is kept runs evenly through the file rather
/// than stopping at its first few thousand.
pub fn subsample(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if points.len() <= KEPT_EVENTS {
        return points.to_vec();
    }
    let step = points.len() as f64 / KEPT_EVENTS as f64;
    (0..KEPT_EVENTS)
        .map(|i| points[(i as f64 * step) as usize])
        .collect()
}

/// One gate's parent population on one file, as the run read it.
#[derive(Debug, Clone, PartialEq)]
pub struct EventSample {
    pub gate_id: String,
    /// The gate above - a gate applied in two places is two populations.
    pub parent_gate: Option<String>,
    pub file: String,
    /// The gate's two parameters: the plot's x and y.
    pub x: String,
    pub y: String,
    /// How many events the population held; `points` is a sample of them.
    pub events: usize,
    pub points: Vec<(f32, f32)>,
}

impl EventSample {
    /// What a run keeps of one measurement.
    pub fn of(m: &crate::gate_rules::autogate::Measurement) -> Self {
        Self {
            gate_id: m.gate_id.to_string(),
            parent_gate: m.parent_gate.as_ref().map(|g| g.to_string()),
            file: m.file.to_string(),
            x: m.params.0.to_string(),
            y: m.params.1.to_string(),
            events: m.events,
            points: m.kept_events.to_vec(),
        }
    }
}

/// The part of a sample written as JSON ahead of its points.
#[derive(Serialize, Deserialize)]
struct Head {
    gate_id: String,
    parent_gate: Option<String>,
    file: String,
    x: String,
    y: String,
    events: usize,
    kept: usize,
    x_range: (f32, f32),
    y_range: (f32, f32),
}

/// The span of `values`; nothing for none, so an empty population writes
/// as numbers rather than infinities JSON cannot hold.
fn range(values: impl Iterator<Item = f32>) -> (f32, f32) {
    let (lo, hi) = values.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
        (lo.min(v), hi.max(v))
    });
    if lo <= hi { (lo, hi) } else { (0.0, 0.0) }
}

fn quantise(v: f32, (lo, hi): (f32, f32)) -> u16 {
    if hi <= lo {
        return 0;
    }
    (((v - lo) as f64 / (hi - lo) as f64) * u16::MAX as f64).round() as u16
}

fn restore(q: u16, (lo, hi): (f32, f32)) -> f32 {
    if hi <= lo {
        return lo;
    }
    (lo as f64 + q as f64 / u16::MAX as f64 * (hi - lo) as f64) as f32
}

/// The run's events as bytes, stamped with the run they belong to.
pub fn encode(run_applied_at: &str, samples: &[EventSample]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    let stamp = run_applied_at.as_bytes();
    out.extend_from_slice(&(stamp.len() as u32).to_le_bytes());
    out.extend_from_slice(stamp);
    out.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    for s in samples {
        // Events a file cannot place on the plot are not events of it.
        let points: Vec<(f32, f32)> = s
            .points
            .iter()
            .copied()
            .filter(|(x, y)| x.is_finite() && y.is_finite())
            .collect();
        let head = Head {
            gate_id: s.gate_id.clone(),
            parent_gate: s.parent_gate.clone(),
            file: s.file.clone(),
            x: s.x.clone(),
            y: s.y.clone(),
            events: s.events,
            kept: points.len(),
            x_range: range(points.iter().map(|p| p.0)),
            y_range: range(points.iter().map(|p| p.1)),
        };
        let json = serde_json::to_vec(&head).expect("a head always serialises");
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&json);
        for (x, y) in &points {
            out.extend_from_slice(&quantise(*x, head.x_range).to_le_bytes());
            out.extend_from_slice(&quantise(*y, head.y_range).to_le_bytes());
        }
    }
    out
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| anyhow::anyhow!("the events file ends early"))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u32(&mut self) -> anyhow::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn u16(&mut self) -> anyhow::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
}

/// The run a file of events belongs to, and its samples.
pub fn decode(bytes: &[u8]) -> anyhow::Result<(String, Vec<EventSample>)> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(4)? != MAGIC {
        anyhow::bail!("not a clingate events file");
    }
    let version = r.u32()?;
    if version > VERSION {
        anyhow::bail!(
            "written by a newer clingate (events format {version}); this one reads {VERSION}"
        );
    }
    let stamp_len = r.u32()? as usize;
    let stamp = String::from_utf8(r.take(stamp_len)?.to_vec())?;
    let count = r.u32()? as usize;
    let mut samples = Vec::with_capacity(count.min(100_000));
    for _ in 0..count {
        let head_len = r.u32()? as usize;
        let head: Head = serde_json::from_slice(r.take(head_len)?)?;
        let mut points = Vec::with_capacity(head.kept.min(KEPT_EVENTS * 4));
        for _ in 0..head.kept {
            let x = restore(r.u16()?, head.x_range);
            let y = restore(r.u16()?, head.y_range);
            points.push((x, y));
        }
        samples.push(EventSample {
            gate_id: head.gate_id,
            parent_gate: head.parent_gate,
            file: head.file,
            x: head.x,
            y: head.y,
            events: head.events,
            points,
        });
    }
    if r.at != bytes.len() {
        anyhow::bail!("the events file has more in it than it says");
    }
    Ok((stamp, samples))
}

/// Where a workspace keeps its last run's events.
pub fn file_in(folder: &Path) -> PathBuf {
    folder.join(super::REVIEWS_DIR).join(EVENTS_FILE)
}

/// Keep the events of the run applied at `run_applied_at`, replacing the
/// last run's.
pub fn save(
    folder: &Path,
    run_applied_at: &str,
    samples: &[EventSample],
) -> anyhow::Result<PathBuf> {
    let path = file_in(folder);
    crate::workspace::make_parent(&path)?;
    std::fs::write(&path, encode(run_applied_at, samples))?;
    Ok(path)
}

/// The events kept for the run applied at `run_applied_at`: `None` where
/// none were kept, or those kept belong to another run.
pub fn load(folder: &Path, run_applied_at: &str) -> anyhow::Result<Option<Vec<EventSample>>> {
    let path = file_in(folder);
    if !path.is_file() {
        return Ok(None);
    }
    let (stamp, samples) =
        decode(&std::fs::read(&path)?).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok((stamp == run_applied_at).then_some(samples))
}

/// Every gate on every file the run measured, one each.
pub fn of_run(measured: &[crate::gate_rules::autogate::Measurement]) -> Vec<EventSample> {
    let mut seen = std::collections::HashSet::new();
    measured
        .iter()
        .filter(|m| seen.insert((m.gate_id.clone(), m.parent_gate.clone(), m.file.clone())))
        .map(EventSample::of)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(gate: &str, file: &str, points: Vec<(f32, f32)>) -> EventSample {
        EventSample {
            gate_id: gate.into(),
            parent_gate: Some("CD4+".into()),
            file: file.into(),
            x: "BV605-A".into(),
            y: "BV421-A".into(),
            events: points.len() * 3,
            points,
        }
    }

    fn spread(n: usize, lo: f32, hi: f32) -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                (lo + t * (hi - lo), hi - t * (hi - lo) * 0.5)
            })
            .collect()
    }

    #[test]
    fn events_come_back_to_within_a_65536th_of_their_range() {
        let wide = sample("g1", "f1", spread(5_000, -0.33, 4.2));
        let linear = sample("g2", "f2", spread(3_000, 0.0, 4.2e6));
        let bytes = encode("2026-09-28T22:18:00Z", &[wide.clone(), linear.clone()]);
        let (stamp, back) = decode(&bytes).unwrap();
        assert_eq!(stamp, "2026-09-28T22:18:00Z");
        assert_eq!(back.len(), 2);
        for (was, now) in [(&wide, &back[0]), (&linear, &back[1])] {
            assert_eq!(
                (
                    &now.gate_id,
                    &now.parent_gate,
                    &now.file,
                    &now.x,
                    &now.y,
                    now.events
                ),
                (
                    &was.gate_id,
                    &was.parent_gate,
                    &was.file,
                    &was.x,
                    &was.y,
                    was.events
                )
            );
            assert_eq!(now.points.len(), was.points.len());
            let (xr, yr) = (
                range(was.points.iter().map(|p| p.0)),
                range(was.points.iter().map(|p| p.1)),
            );
            for (a, b) in was.points.iter().zip(&now.points) {
                assert!((a.0 - b.0).abs() <= (xr.1 - xr.0) / 65_535.0, "{a:?} {b:?}");
                assert!((a.1 - b.1).abs() <= (yr.1 - yr.0) / 65_535.0, "{a:?} {b:?}");
            }
            // The ends exactly.
            assert_eq!(now.points[0].0, was.points[0].0);
        }
        // Four bytes an event, and a little for each head.
        assert!(bytes.len() < 8_000 * 4 + 1_000, "{}", bytes.len());
    }

    #[test]
    fn events_off_the_plot_are_dropped_and_one_value_everywhere_comes_back_as_it() {
        let mut points = vec![(1.5, 2.0); 10];
        points.push((f32::NAN, 1.0));
        points.push((1.0, f32::INFINITY));
        let (_, back) = decode(&encode("t", &[sample("g", "f", points)])).unwrap();
        assert_eq!(back[0].points, vec![(1.5, 2.0); 10]);
    }

    #[test]
    fn a_run_with_nothing_measured_keeps_an_empty_file() {
        let (stamp, back) = decode(&encode("t", &[])).unwrap();
        assert_eq!((stamp.as_str(), back.len()), ("t", 0));
        let empty = sample("g", "f", Vec::new());
        let (_, back) = decode(&encode("t", &[empty.clone()])).unwrap();
        assert_eq!(back, vec![empty]);
    }

    #[test]
    fn a_damaged_or_foreign_file_is_an_error_not_a_panic() {
        let good = encode("t", &[sample("g", "f", spread(100, 0.0, 1.0))]);
        assert!(decode(b"PNG....").is_err());
        assert!(decode(&[]).is_err());
        for cut in [3, 9, 20, good.len() - 1] {
            assert!(decode(&good[..cut]).is_err(), "cut at {cut}");
        }
        let mut longer = good.clone();
        longer.push(0);
        assert!(decode(&longer).is_err());
        let mut newer = good.clone();
        newer[4..8].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(decode(&newer).unwrap_err().to_string().contains("newer"));
        // A count far past what the file holds.
        let mut lying = encode("t", &[]);
        let at = lying.len() - 4;
        lying[at..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&lying).is_err());
    }

    #[test]
    fn events_are_kept_for_their_own_run_only() {
        let folder = crate::file_load_tests::scratch("run-events");
        assert_eq!(load(&folder, "t1").unwrap(), None);
        let s = sample("g", "f", spread(50, 0.0, 1.0));
        let path = save(&folder, "t1", &[s.clone()]).unwrap();
        assert_eq!(path, folder.join("reviews").join("run_events.bin"));
        let back = load(&folder, "t1").unwrap().expect("this run's");
        assert_eq!(back[0].points.len(), 50);
        assert_eq!(load(&folder, "t2").unwrap(), None, "another run's");
        std::fs::write(&path, b"junk").unwrap();
        assert!(load(&folder, "t1").is_err());
    }

    #[test]
    fn a_subsample_keeps_a_small_population_whole_and_spreads_through_a_large_one() {
        let small: Vec<(f32, f32)> = (0..4_000).map(|i| (i as f32, 0.0)).collect();
        assert_eq!(subsample(&small), small);
        let large: Vec<(f32, f32)> = (0..12_000).map(|i| (i as f32, 0.0)).collect();
        let kept = subsample(&large);
        assert_eq!(kept.len(), KEPT_EVENTS);
        assert_eq!(kept[0].0, 0.0);
        assert!(kept[KEPT_EVENTS - 1].0 > 11_990.0);
        assert!(kept.windows(2).all(|w| w[1].0 > w[0].0));
    }
}
