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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file a workspace keeps its last run's events in, in `reviews`.
pub const EVENTS_FILE: &str = "run_events.bin";
/// At most this many events of a population are kept.
pub const KEPT_EVENTS: usize = 5_000;

const MAGIC: &[u8; 4] = b"CGEV";
/// 2 added each file's metadata and each gate as it stood before the run -
/// what a replay starts from. A file of version 1 still reads, without them.
const VERSION: u32 = 2;

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
    /// The gate as it stood on this file when the run measured it - before
    /// the run moved anything. What a replay starts the rule from.
    pub gate: Option<flow_gates::Gate>,
}

/// What a run keeps of what it read: every population it measured, and the
/// metadata of every file it measured them on, so a replay can tell
/// specimens and sample types apart exactly as the run did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeptEvents {
    /// The run they belong to; stamped when the run is applied.
    pub run_applied_at: String,
    pub metadata: BTreeMap<String, BTreeMap<String, String>>,
    pub samples: Vec<EventSample>,
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
            gate: Some(m.drawn.clone()),
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
    #[serde(default)]
    gate: Option<flow_gates::Gate>,
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
pub fn encode(kept: &KeptEvents) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    let stamp = kept.run_applied_at.as_bytes();
    out.extend_from_slice(&(stamp.len() as u32).to_le_bytes());
    out.extend_from_slice(stamp);
    let metadata = serde_json::to_vec(&kept.metadata).expect("metadata always serialises");
    out.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    out.extend_from_slice(&metadata);
    out.extend_from_slice(&(kept.samples.len() as u32).to_le_bytes());
    for s in &kept.samples {
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
            gate: s.gate.clone(),
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

/// A file of events, read back.
pub fn decode(bytes: &[u8]) -> anyhow::Result<KeptEvents> {
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
    let metadata = if version >= 2 {
        let len = r.u32()? as usize;
        serde_json::from_slice(r.take(len)?)?
    } else {
        BTreeMap::new()
    };
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
            gate: head.gate,
        });
    }
    if r.at != bytes.len() {
        anyhow::bail!("the events file has more in it than it says");
    }
    Ok(KeptEvents {
        run_applied_at: stamp,
        metadata,
        samples,
    })
}

/// Where a workspace keeps its last run's events.
pub fn file_in(folder: &Path) -> PathBuf {
    folder.join(super::REVIEWS_DIR).join(EVENTS_FILE)
}

/// Keep a run's events, replacing the last run's.
pub fn save(folder: &Path, kept: &KeptEvents) -> anyhow::Result<PathBuf> {
    let path = file_in(folder);
    crate::workspace::make_parent(&path)?;
    std::fs::write(&path, encode(kept))?;
    Ok(path)
}

/// The events kept in `folder` for the run applied at `run_applied_at`:
/// `None` where none were kept, or those kept belong to another run.
pub fn load(folder: &Path, run_applied_at: &str) -> anyhow::Result<Option<KeptEvents>> {
    let path = file_in(folder);
    if !path.is_file() {
        return Ok(None);
    }
    let kept =
        decode(&std::fs::read(&path)?).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok((kept.run_applied_at == run_applied_at).then_some(kept))
}

/// Every gate on every file the run measured, one each, with the metadata of
/// each file measured.
pub fn of_run(
    measured: &[crate::gate_rules::autogate::Measurement],
    metadata: &crate::omiq::metadata::MetaDataFileMap,
) -> KeptEvents {
    let mut seen = std::collections::HashSet::new();
    let samples: Vec<EventSample> = measured
        .iter()
        .filter(|m| seen.insert((m.gate_id.clone(), m.parent_gate.clone(), m.file.clone())))
        .map(EventSample::of)
        .collect();
    let metadata = samples
        .iter()
        .filter_map(|s| {
            let row = metadata.get(s.file.as_str())?;
            Some((
                s.file.clone(),
                row.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ))
        })
        .collect();
    KeptEvents {
        run_applied_at: String::new(),
        metadata,
        samples,
    }
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
            gate: Some(a_gate(gate)),
        }
    }

    fn a_gate(id: &str) -> flow_gates::Gate {
        flow_gates::Gate {
            id: std::sync::Arc::from(id),
            name: "CD69+".into(),
            geometry: flow_gates::create_rectangle_geometry(
                vec![(0.5, -1e16), (1e16, -1e16), (1e16, 1e16), (0.5, 1e16)],
                "BV605-A",
                "BV421-A",
            )
            .unwrap(),
            mode: flow_gates::GateMode::Global,
            parameters: (
                std::sync::Arc::from("BV605-A"),
                std::sync::Arc::from("BV421-A"),
            ),
            label_position: None,
        }
    }

    fn kept(stamp: &str, samples: Vec<EventSample>) -> KeptEvents {
        let mut metadata = BTreeMap::new();
        for s in &samples {
            metadata.insert(
                s.file.clone(),
                BTreeMap::from([("SampleID".to_string(), format!("donor of {}", s.file))]),
            );
        }
        KeptEvents {
            run_applied_at: stamp.into(),
            metadata,
            samples,
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
    fn events_come_back_to_within_a_65536th_of_their_range_with_their_gates_and_metadata() {
        let wide = sample("g1", "f1", spread(5_000, -0.33, 4.2));
        let linear = sample("g2", "f2", spread(3_000, 0.0, 4.2e6));
        let written = kept("2026-09-28T22:18:00Z", vec![wide.clone(), linear.clone()]);
        let bytes = encode(&written);
        let back = decode(&bytes).unwrap();
        assert_eq!(back.run_applied_at, "2026-09-28T22:18:00Z");
        assert_eq!(back.metadata, written.metadata);
        assert_eq!(back.samples.len(), 2);
        for (was, now) in [(&wide, &back.samples[0]), (&linear, &back.samples[1])] {
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
            // The gate exactly.
            assert_eq!(now.gate, was.gate);
            assert_eq!(now.points.len(), was.points.len());
            let (xr, yr) = (
                range(was.points.iter().map(|p| p.0)),
                range(was.points.iter().map(|p| p.1)),
            );
            for (a, b) in was.points.iter().zip(&now.points) {
                assert!((a.0 - b.0).abs() <= (xr.1 - xr.0) / 65_535.0, "{a:?} {b:?}");
                assert!((a.1 - b.1).abs() <= (yr.1 - yr.0) / 65_535.0, "{a:?} {b:?}");
            }
            assert_eq!(now.points[0].0, was.points[0].0);
        }
        // Four bytes an event, and a little for each head.
        assert!(bytes.len() < 8_000 * 4 + 3_000, "{}", bytes.len());
    }

    #[test]
    fn a_version_1_file_still_reads_without_metadata_or_gates() {
        // As the first version wrote it: stamp, count, heads without gates.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(b"t");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        let head = r#"{"gate_id":"g","parent_gate":null,"file":"f","x":"a","y":"b","events":1,"kept":1,"x_range":[0.0,1.0],"y_range":[0.0,1.0]}"#;
        bytes.extend_from_slice(&(head.len() as u32).to_le_bytes());
        bytes.extend_from_slice(head.as_bytes());
        bytes.extend_from_slice(&u16::MAX.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        let back = decode(&bytes).unwrap();
        assert_eq!(back.run_applied_at, "t");
        assert!(back.metadata.is_empty());
        assert_eq!(back.samples[0].gate, None);
        assert_eq!(back.samples[0].points, vec![(1.0, 0.0)]);
    }

    #[test]
    fn events_off_the_plot_are_dropped_and_one_value_everywhere_comes_back_as_it() {
        let mut points = vec![(1.5, 2.0); 10];
        points.push((f32::NAN, 1.0));
        points.push((1.0, f32::INFINITY));
        let back = decode(&encode(&kept("t", vec![sample("g", "f", points)]))).unwrap();
        assert_eq!(back.samples[0].points, vec![(1.5, 2.0); 10]);
    }

    #[test]
    fn a_run_with_nothing_measured_keeps_an_empty_file() {
        let back = decode(&encode(&kept("t", Vec::new()))).unwrap();
        assert_eq!((back.run_applied_at.as_str(), back.samples.len()), ("t", 0));
        let empty = sample("g", "f", Vec::new());
        let back = decode(&encode(&kept("t", vec![empty.clone()]))).unwrap();
        assert_eq!(back.samples, vec![empty]);
    }

    #[test]
    fn a_damaged_or_foreign_file_is_an_error_not_a_panic() {
        let good = encode(&kept("t", vec![sample("g", "f", spread(100, 0.0, 1.0))]));
        assert!(decode(b"PNG....").is_err());
        assert!(decode(&[]).is_err());
        for cut in [3, 9, 20, 40, good.len() - 1] {
            assert!(decode(&good[..cut]).is_err(), "cut at {cut}");
        }
        let mut longer = good.clone();
        longer.push(0);
        assert!(decode(&longer).is_err());
        let mut newer = good.clone();
        newer[4..8].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(decode(&newer).unwrap_err().to_string().contains("newer"));
        // A count far past what the file holds.
        let mut lying = encode(&kept("t", Vec::new()));
        let at = lying.len() - 4;
        lying[at..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&lying).is_err());
    }

    #[test]
    fn events_are_kept_for_their_own_run_only() {
        let folder = crate::file_load_tests::scratch("run-events");
        assert_eq!(load(&folder, "t1").unwrap(), None);
        let path = save(
            &folder,
            &kept("t1", vec![sample("g", "f", spread(50, 0.0, 1.0))]),
        )
        .unwrap();
        assert_eq!(path, folder.join("reviews").join("run_events.bin"));
        let back = load(&folder, "t1").unwrap().expect("this run's");
        assert_eq!(back.samples[0].points.len(), 50);
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
