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
//! samples) - events of the gate's **parent population** (not of the whole
//! file) on the gate's two parameters, in the plot's own units: all of them
//! up to [`KEPT_EVENTS`], and for a rule that reads a thin tail enough that
//! the tail itself keeps about [`TAIL_EVENTS`] - see [`kept_for`].
//!
//! The workspace keeps its last run's in one binary file,
//! `reviews/run_events.bin`, each coordinate as 16 bits across the range the
//! events span: to within 1/131,070 of that range, a thousandth of a pixel on
//! a plot. About 20 KB per 5,000 events. The file is stamped with the run it
//! belongs to, so a stale one is never read as a newer run's.
//!
//! A review library keeps each population once however many runs read it
//! ([`Pool`]): rerunning the rules leaves every population whose parent
//! gates did not move exactly as it was, and those are stored once and
//! named from each run's list ([`Manifest`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file a workspace keeps its last run's events in, in `reviews`.
pub const EVENTS_FILE: &str = "run_events.bin";
/// A reviewed run's list of populations in the library, each named by its
/// place in the library's pool.
pub const MANIFEST_FILE: &str = "run_events.json";
/// The library's pool of populations, beside the reviewed runs' folders.
pub const POOL_DIR: &str = "events";
/// Events of a population kept for any rule.
pub const KEPT_EVENTS: usize = 5_000;
/// How many events a rule's tail should keep, for a rule that reads one.
pub const TAIL_EVENTS: f64 = 100.0;
/// The most events of one population kept, however thin its tail.
pub const MOST_KEPT_EVENTS: usize = 50_000;

const MAGIC: &[u8; 4] = b"CGEV";
const POPULATION_MAGIC: &[u8; 4] = b"CGEP";
/// 2 added each file's metadata and each gate as it stood before the run -
/// what a replay starts from. A file of version 1 still reads, without them.
const VERSION: u32 = 2;

/// How many of a population's events a run keeps for `rule`.
///
/// A band rule counts a fraction of the population, and a percentile rule
/// reads an order statistic near one end of it. Either is decided by the
/// events in its tail - a band of 0.2% to 0.5% by the 0.2% at the top - and
/// an even sample of 5,000 leaves ten of them, too few for a replay to put
/// the gate where the run did. So those keep enough that the thinner side
/// of the rule holds about [`TAIL_EVENTS`], up to [`MOST_KEPT_EVENTS`]. The
/// other rules read the body of the population - a peak, a median, a valley
/// - which 5,000 events describe well.
pub fn kept_for(rule: &crate::gate_rules::rule::Rule) -> usize {
    use crate::gate_rules::rule::Rule;
    let sides: Vec<f64> = match rule {
        Rule::TailFraction(r) => vec![r.band.0, r.band.1, 1.0 - r.band.0, 1.0 - r.band.1],
        Rule::PercentileOffset(r) => {
            vec![r.percentile / 100.0, 1.0 - r.percentile / 100.0]
        }
        _ => Vec::new(),
    };
    let thinnest = sides
        .into_iter()
        .filter(|f| f.is_finite() && *f > 0.0)
        .fold(f64::INFINITY, f64::min);
    if !thinnest.is_finite() {
        return KEPT_EVENTS;
    }
    ((TAIL_EVENTS / thinnest).ceil() as usize).clamp(KEPT_EVENTS, MOST_KEPT_EVENTS)
}

/// Up to [`KEPT_EVENTS`] of `points` - see [`subsample_to`].
pub fn subsample(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    subsample_to(points, KEPT_EVENTS)
}

/// Every `n`th event, so what is kept runs evenly through the file rather
/// than stopping at its first few thousand - and always the events at the
/// population's extremes on each axis. All of them when there are no more
/// than `most`.
///
/// Even through the file is a fair sample: events are in the order they were
/// acquired, which says nothing about how bright they are.
///
/// The extremes are kept because a rule's search starts from them: a band
/// rule slides the gate between the population's lowest and highest events
/// and stops at the first position inside the band, so a sample missing
/// either end starts the search elsewhere and can stop somewhere else.
pub fn subsample_to(points: &[(f32, f32)], most: usize) -> Vec<(f32, f32)> {
    let most = most.max(4);
    if points.len() <= most {
        return points.to_vec();
    }
    // The events at each end of each axis, by position in the file.
    let extreme = |key: fn(&(f32, f32)) -> f32, highest: bool| -> Option<usize> {
        points
            .iter()
            .enumerate()
            .filter(|(_, p)| p.0.is_finite() && p.1.is_finite())
            .max_by(|a, b| {
                let o = key(a.1).total_cmp(&key(b.1));
                if highest { o } else { o.reverse() }
            })
            .map(|(i, _)| i)
    };
    let mut ends: Vec<usize> = [
        extreme(|p| p.0, true),
        extreme(|p| p.0, false),
        extreme(|p| p.1, true),
        extreme(|p| p.1, false),
    ]
    .into_iter()
    .flatten()
    .collect();
    ends.sort_unstable();
    ends.dedup();
    let even = most - ends.len();
    let step = points.len() as f64 / even as f64;
    let mut chosen: Vec<usize> = (0..even).map(|i| (i as f64 * step) as usize).collect();
    chosen.extend(ends);
    chosen.sort_unstable();
    chosen.dedup();
    chosen.into_iter().map(|i| points[i]).collect()
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
    /// The gates on its plot the run kept it clear of, by name, as outlines.
    pub beside: Vec<KeptNeighbour>,
}

/// A gate kept clear of: its name, and its outline on the plot.
pub type KeptNeighbour = (String, Vec<(f64, f64)>);

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
            beside: m
                .beside
                .iter()
                .map(|(name, outline)| (name.to_string(), outline.clone()))
                .collect(),
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
    #[serde(default)]
    beside: Vec<KeptNeighbour>,
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

/// The value a stored coordinate stands for. The ends of the range come
/// back exactly, so the range of what is read is the range written, and
/// writing what was read gives the same bytes again.
fn restore(q: u16, (lo, hi): (f32, f32)) -> f32 {
    if hi <= lo || q == 0 {
        return lo;
    }
    if q == u16::MAX {
        return hi;
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
            beside: s.beside.clone(),
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
        let mut points = Vec::with_capacity(head.kept.min(MOST_KEPT_EVENTS));
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
            beside: head.beside,
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

// ── the library's pool ────────────────────────────────────────────────────

/// One population as the pool keeps it: everything about it but which gate
/// and file it was read for, since the same population is read for several.
#[derive(Serialize, Deserialize)]
struct PopulationHead {
    x: String,
    y: String,
    events: usize,
    kept: usize,
    x_range: (f32, f32),
    y_range: (f32, f32),
}

/// A population's bytes: identical populations, identical bytes.
fn encode_population(s: &EventSample) -> Vec<u8> {
    let points: Vec<(f32, f32)> = s
        .points
        .iter()
        .copied()
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .collect();
    let head = PopulationHead {
        x: s.x.clone(),
        y: s.y.clone(),
        events: s.events,
        kept: points.len(),
        x_range: range(points.iter().map(|p| p.0)),
        y_range: range(points.iter().map(|p| p.1)),
    };
    let json = serde_json::to_vec(&head).expect("a head always serialises");
    let mut out = Vec::with_capacity(12 + json.len() + points.len() * 4);
    out.extend_from_slice(POPULATION_MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    for (x, y) in &points {
        out.extend_from_slice(&quantise(*x, head.x_range).to_le_bytes());
        out.extend_from_slice(&quantise(*y, head.y_range).to_le_bytes());
    }
    out
}

fn decode_population(bytes: &[u8]) -> anyhow::Result<Population> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(4)? != POPULATION_MAGIC {
        anyhow::bail!("not a clingate population");
    }
    let version = r.u32()?;
    if version > VERSION {
        anyhow::bail!("written by a newer clingate (events format {version})");
    }
    let len = r.u32()? as usize;
    let head: PopulationHead = serde_json::from_slice(r.take(len)?)?;
    let mut points = Vec::with_capacity(head.kept.min(MOST_KEPT_EVENTS));
    for _ in 0..head.kept {
        let x = restore(r.u16()?, head.x_range);
        let y = restore(r.u16()?, head.y_range);
        points.push((x, y));
    }
    if r.at != bytes.len() {
        anyhow::bail!("the population has more in it than it says");
    }
    Ok(Population {
        x: head.x,
        y: head.y,
        events: head.events,
        points,
    })
}

/// A population read back from the pool.
struct Population {
    x: String,
    y: String,
    events: usize,
    points: Vec<(f32, f32)>,
}

/// What names a population in the pool: the hash of its bytes, so two runs
/// that read the same events on the same axes name the same file, and no
/// two different populations can.
pub fn population_key(s: &EventSample) -> String {
    blake3::hash(&encode_population(s)).to_hex().to_string()
}

/// The populations a review library keeps, each once, in `events` beside
/// the reviewed runs' folders.
pub struct Pool {
    dir: PathBuf,
}

impl Pool {
    pub fn in_library(library: &Path) -> Self {
        Self {
            dir: library.join(POOL_DIR),
        }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.bin"))
    }

    /// Keep a population, unless the pool has it already. Returns its key.
    pub fn put(&self, s: &EventSample) -> anyhow::Result<String> {
        let bytes = encode_population(s);
        let key = blake3::hash(&bytes).to_hex().to_string();
        let path = self.path(&key);
        if !path.is_file() {
            std::fs::create_dir_all(&self.dir)?;
            // Written whole and then named, so a pool never holds half a
            // population under a name that says it is whole.
            let partial = self.dir.join(format!("{key}.partial"));
            std::fs::write(&partial, &bytes)?;
            std::fs::rename(&partial, &path)?;
        }
        Ok(key)
    }

    fn get(&self, key: &str) -> anyhow::Result<Population> {
        let path = self.path(key);
        let bytes = std::fs::read(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        if blake3::hash(&bytes).to_hex().as_str() != key {
            anyhow::bail!(
                "{}: damaged - its contents are not what it is named for",
                path.display()
            );
        }
        decode_population(&bytes)
    }

    /// How many populations the pool holds.
    pub fn len(&self) -> usize {
        std::fs::read_dir(&self.dir)
            .map(|d| {
                d.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "bin"))
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A reviewed run's populations in the library: which gate on which file
/// each was read for, and its key in the pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub run_applied_at: String,
    pub metadata: BTreeMap<String, BTreeMap<String, String>>,
    pub samples: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub gate_id: String,
    pub parent_gate: Option<String>,
    pub file: String,
    pub gate: Option<flow_gates::Gate>,
    pub population: String,
    #[serde(default)]
    pub beside: Vec<KeptNeighbour>,
}

/// Keep a run's events in a library: each population in the pool, once,
/// and the run's list of them in its own folder.
pub fn save_to_library(run_folder: &Path, pool: &Pool, kept: &KeptEvents) -> anyhow::Result<()> {
    let samples = kept
        .samples
        .iter()
        .map(|s| {
            Ok(ManifestEntry {
                gate_id: s.gate_id.clone(),
                parent_gate: s.parent_gate.clone(),
                file: s.file.clone(),
                gate: s.gate.clone(),
                population: pool.put(s)?,
                beside: s.beside.clone(),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let manifest = Manifest {
        run_applied_at: kept.run_applied_at.clone(),
        metadata: kept.metadata.clone(),
        samples,
    };
    std::fs::create_dir_all(run_folder)?;
    std::fs::write(
        run_folder.join(MANIFEST_FILE),
        serde_json::to_vec(&manifest)?,
    )?;
    Ok(())
}

/// A reviewed run's events from the library: from its list and the pool
/// beside it, or - for a run copied before the pool - its own events file.
pub fn load_from_library(run_folder: &Path) -> anyhow::Result<Option<KeptEvents>> {
    let listed = run_folder.join(MANIFEST_FILE);
    if listed.is_file() {
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(&listed)?)?;
        let library = run_folder
            .parent()
            .ok_or_else(|| anyhow::anyhow!("{} is in no library", run_folder.display()))?;
        let pool = Pool::in_library(library);
        let samples = manifest
            .samples
            .into_iter()
            .map(|e| {
                let population = pool.get(&e.population)?;
                Ok(EventSample {
                    gate_id: e.gate_id,
                    parent_gate: e.parent_gate,
                    file: e.file,
                    x: population.x,
                    y: population.y,
                    events: population.events,
                    points: population.points,
                    gate: e.gate,
                    beside: e.beside,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        return Ok(Some(KeptEvents {
            run_applied_at: manifest.run_applied_at,
            metadata: manifest.metadata,
            samples,
        }));
    }
    match std::fs::read(run_folder.join(EVENTS_FILE)) {
        Ok(bytes) => Ok(Some(decode(&bytes)?)),
        Err(_) => Ok(None),
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
            beside: Vec::new(),
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
    fn the_gates_a_population_was_kept_clear_of_come_back_as_written() {
        let mut held = sample("g1", "f1", spread(10, 0.0, 1.0));
        held.beside = vec![(
            "teff_naive".into(),
            vec![(0.5, 1.0), (2.0, 1.0), (1.25, 3.0)],
        )];
        let written = kept("2026-10-01T12:00:00Z", vec![held.clone()]);

        let back = decode(&encode(&written)).unwrap();

        assert_eq!(back.samples[0].beside, held.beside);
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
        assert!(back.samples[0].beside.is_empty());
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
    fn a_subsample_always_keeps_the_extremes_of_each_axis() {
        let mut points: Vec<(f32, f32)> = (0..20_000)
            .map(|i| ((i % 997) as f32, (i % 991) as f32))
            .collect();
        // Extremes placed where an even step would miss them.
        points[12_345] = (5_000.0, 0.5);
        points[7_777] = (-3_000.0, 0.5);
        points[1_001] = (1.0, 9_999.0);
        points[19_999] = (1.0, -9_999.0);
        let kept = subsample(&points);
        assert!(kept.len() <= KEPT_EVENTS);
        for must in [
            (5_000.0, 0.5),
            (-3_000.0, 0.5),
            (1.0, 9_999.0),
            (1.0, -9_999.0),
        ] {
            assert!(kept.contains(&must), "{must:?} was dropped");
        }
        // Still in the file's order.
        let at = |p: (f32, f32)| points.iter().position(|q| *q == p).unwrap();
        assert!(at((1.0, 9_999.0)) < at((-3_000.0, 0.5)));
        assert_eq!(subsample(&points), kept, "the same every time");
    }

    #[test]
    fn a_subsample_keeps_a_small_population_whole_and_spreads_through_a_large_one() {
        let small: Vec<(f32, f32)> = (0..4_000).map(|i| (i as f32, 0.0)).collect();
        assert_eq!(subsample(&small), small);
        let large: Vec<(f32, f32)> = (0..12_000).map(|i| (i as f32, 0.0)).collect();
        let kept = subsample(&large);
        // The extremes among them - one fewer where an extreme is also on
        // the even step.
        assert!(
            (KEPT_EVENTS - 4..=KEPT_EVENTS).contains(&kept.len()),
            "{}",
            kept.len()
        );
        assert!(kept.contains(&(11_999.0, 0.0)), "the far end");
        assert_eq!(kept[0].0, 0.0);
        assert!(kept.last().unwrap().0 > 11_990.0);
        assert!(kept.windows(2).all(|w| w[1].0 > w[0].0));
    }

    #[test]
    fn a_rule_that_reads_a_thin_tail_keeps_enough_events_for_the_tail() {
        use crate::gate_rules::rule::{
            AboveTheNegativeRule, PercentileOffsetRule, Rule, TailFractionRule, ValleyOrSmearRule,
        };
        // 0.2% to 0.5%: 100 events in the 0.2% takes 50,000.
        let band = |lo, hi| Rule::TailFraction(TailFractionRule::new((lo, hi)));
        assert_eq!(kept_for(&band(0.002, 0.005)), 50_000);
        // 1% to 2%: 10,000.
        assert_eq!(kept_for(&band(0.01, 0.02)), 10_000);
        // A negative gate holding 99% to 99.7% is decided by the 0.3% it cuts.
        assert_eq!(kept_for(&band(0.99, 0.997)), 33_334);
        // A band starting at nothing is read by its upper end.
        assert_eq!(kept_for(&band(0.0, 0.05)), KEPT_EVENTS);
        // Never fewer than for any rule, never more than the most.
        assert_eq!(kept_for(&band(0.3, 0.5)), KEPT_EVENTS);
        assert_eq!(kept_for(&band(0.0001, 0.0002)), MOST_KEPT_EVENTS);
        let percentile = |p| Rule::PercentileOffset(PercentileOffsetRule::new(p, 0.1));
        assert_eq!(kept_for(&percentile(99.0)), 10_000);
        assert_eq!(kept_for(&percentile(0.5)), 20_000);
        assert_eq!(kept_for(&percentile(50.0)), KEPT_EVENTS);
        // The body of the population, which 5,000 describe.
        assert_eq!(
            kept_for(&Rule::AboveTheNegative(AboveTheNegativeRule::default())),
            KEPT_EVENTS
        );
        assert_eq!(
            kept_for(&Rule::ValleyOrSmear(ValleyOrSmearRule::default())),
            KEPT_EVENTS
        );
    }

    #[test]
    fn a_sample_to_any_size_keeps_that_many_evenly_with_the_extremes() {
        let points = spread(100_000, -1.0, 5.0);
        for most in [5_000, 12_345, 50_000] {
            let kept = subsample_to(&points, most);
            assert!(
                (most - 4..=most).contains(&kept.len()),
                "{most}: {}",
                kept.len()
            );
            assert!(kept.contains(&points[0]) && kept.contains(&points[99_999]));
        }
        // No more than there are.
        assert_eq!(subsample_to(&points[..10], 5_000), points[..10].to_vec());
        assert!((KEPT_EVENTS - 4..=KEPT_EVENTS).contains(&subsample(&points).len()));
    }

    #[test]
    fn what_is_read_back_writes_as_the_same_bytes() {
        // Wide and narrow ranges, far and near zero.
        for points in [
            spread(5_000, -0.33, 4.2),
            spread(3_000, 0.0, 4.2e6),
            spread(2_000, 1_000.0, 1_001.0),
        ] {
            let written = kept("t", vec![sample("g", "f", points)]);
            let once = encode(&written);
            let back = decode(&once).unwrap();
            // The ends of the range exactly.
            let xs: Vec<f32> = back.samples[0].points.iter().map(|p| p.0).collect();
            let was: Vec<f32> = written.samples[0].points.iter().map(|p| p.0).collect();
            assert_eq!(
                xs.iter().copied().fold(f32::INFINITY, f32::min),
                was.iter().copied().fold(f32::INFINITY, f32::min)
            );
            assert_eq!(
                xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
                was.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            );
            assert_eq!(encode(&back), once);
            assert_eq!(
                population_key(&back.samples[0]),
                population_key(&decode(&encode(&back)).unwrap().samples[0])
            );
        }
    }

    fn library(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "clingate-events-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_library_keeps_each_population_once_however_many_runs_read_it() {
        let lib = library("pool");
        let pool = Pool::in_library(&lib);
        assert!(pool.is_empty());
        let fmx = sample("g1", "fmx", spread(4_000, -0.3, 3.0));
        let fs = sample("g1", "fs", spread(4_000, -0.2, 3.5));
        // The same population read for another gate under the same parent.
        let mut other_gate = fs.clone();
        other_gate.gate_id = "g2".into();
        other_gate.gate = Some(a_gate("g2"));
        let first = kept(
            "2026-01-01T00:00:00Z",
            vec![fmx.clone(), fs.clone(), other_gate],
        );
        save_to_library(&lib.join("run1"), &pool, &first).unwrap();
        assert_eq!(pool.len(), 2, "fs is one population, read for two gates");

        // Run again: the FMX as it was, the full stain's parent moved.
        let mut moved = fs.clone();
        moved.points[7].0 += 0.25;
        let second = kept("2026-01-02T00:00:00Z", vec![fmx.clone(), moved.clone()]);
        save_to_library(&lib.join("run2"), &pool, &second).unwrap();
        assert_eq!(pool.len(), 3, "only the population that changed is added");

        // Each run reads back as it was kept.
        for (folder, was) in [("run1", &first), ("run2", &second)] {
            let back = load_from_library(&lib.join(folder)).unwrap().unwrap();
            assert_eq!(back.run_applied_at, was.run_applied_at);
            assert_eq!(back.metadata, was.metadata);
            assert_eq!(back.samples.len(), was.samples.len());
            for (now, then) in back.samples.iter().zip(&was.samples) {
                assert_eq!(
                    (&now.gate_id, &now.file, &now.gate),
                    (&then.gate_id, &then.file, &then.gate)
                );
                assert_eq!(now.events, then.events);
                assert_eq!(population_key(now), population_key(then));
            }
        }
        // No half-written files left behind.
        assert!(
            std::fs::read_dir(lib.join(POOL_DIR))
                .unwrap()
                .flatten()
                .all(|e| e.path().extension().is_some_and(|x| x == "bin"))
        );
    }

    #[test]
    fn a_damaged_or_missing_population_is_said_and_an_old_copy_still_reads() {
        let lib = library("damaged");
        let pool = Pool::in_library(&lib);
        let run = kept("t", vec![sample("g", "f", spread(1_000, 0.0, 1.0))]);
        save_to_library(&lib.join("run"), &pool, &run).unwrap();
        let key = population_key(&run.samples[0]);
        let path = lib.join(POOL_DIR).join(format!("{key}.bin"));
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        let damaged = load_from_library(&lib.join("run")).unwrap_err().to_string();
        assert!(damaged.contains("damaged"), "{damaged}");
        std::fs::remove_file(&path).unwrap();
        assert!(load_from_library(&lib.join("run")).is_err());

        // A run copied before the pool: its own events file.
        let old = lib.join("old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(EVENTS_FILE), encode(&run)).unwrap();
        assert_eq!(
            load_from_library(&old).unwrap(),
            Some(decode(&encode(&run)).unwrap())
        );
        // And one with no events at all.
        let none = lib.join("none");
        std::fs::create_dir_all(&none).unwrap();
        assert_eq!(load_from_library(&none).unwrap(), None);
    }

    #[test]
    fn a_population_key_changes_with_any_event_or_axis_but_not_the_gate_it_was_read_for() {
        let a = sample("g1", "f", spread(500, 0.0, 2.0));
        let mut for_another_gate = a.clone();
        for_another_gate.gate_id = "g9".into();
        for_another_gate.gate = None;
        assert_eq!(population_key(&a), population_key(&for_another_gate));
        let mut one_event = a.clone();
        one_event.points[250].1 += 0.01;
        assert_ne!(population_key(&a), population_key(&one_event));
        let mut axis = a.clone();
        axis.y = "CD8".into();
        assert_ne!(population_key(&a), population_key(&axis));
        let mut count = a.clone();
        count.events += 1;
        assert_ne!(population_key(&a), population_key(&count));
    }
}
