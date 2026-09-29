//! What a gate's populations look like across a dataset, summed up so a
//! person - or Claude - can choose a rule for it.
//!
//! Choosing a rule is a question about shape: does the positive separate from
//! the negative, or smear out of it; does the negative keep its shape from
//! sample to sample, or shift and spread; is there signal on the full stain
//! above its FMX. Each sample's parent population is read on each of the
//! gate's two markers and put in a **shape class**, measured the way the rules
//! measure - the same valley finder, the same reading of the negative's two
//! sides - and the classes and measures are summed up per sample type.
//!
//! The classes are labels for the measures, decided by fixed thresholds
//! ([`classify`]), so what they mean is written down and the same every time.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::gate_rules::autogate::{Measurement, admitted_by, describe};
use crate::gate_rules::rule::{Rule, ValleyRule};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gate_rules::run::{RunInputs, measure_many};
use crate::gate_rules::trial::Spread;
use crate::gates::GateState;

/// Fewer events than this and a population is not classed.
pub const MIN_EVENTS: usize = 200;
/// A dip at least this deep, against the lower peak beside it, separates.
pub const DEEP_VALLEY: f64 = 0.5;
/// A right side at least this much wider than the left, with no dip, is a
/// smear of positives out of the negative. Read on a smoothed density, which
/// evens the two sides a little: a population twice as wide on the right as
/// the left reads about 1.6, an even one about 1.
pub const SMEAR_RATIO: f64 = 1.4;

/// What a population on one marker looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeClass {
    /// A negative and a positive with a dip between them at least
    /// [`DEEP_VALLEY`] deep.
    Separate,
    /// A dip, but a shallow one.
    Shoulder,
    /// No dip, and the right side at least [`SMEAR_RATIO`] times the left:
    /// positives smearing out of the negative.
    Smear,
    /// No dip, and the negative's right side never falls to a quarter of its
    /// peak before the data ends: merged with what is above it.
    Merged,
    /// One peak, roughly even either side: a negative alone.
    NegativeOnly,
    /// Three or more peaks.
    SeveralPeaks,
    /// Fewer than [`MIN_EVENTS`] events.
    TooFew,
}

impl ShapeClass {
    pub fn label(self) -> &'static str {
        match self {
            ShapeClass::Separate => "separate",
            ShapeClass::Shoulder => "shoulder",
            ShapeClass::Smear => "smear",
            ShapeClass::Merged => "merged",
            ShapeClass::NegativeOnly => "negative only",
            ShapeClass::SeveralPeaks => "several peaks",
            ShapeClass::TooFew => "too few events",
        }
    }
}

/// One population on one marker.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reading {
    pub class: ShapeClass,
    /// The negative's peak - the leftmost peak tall enough to count.
    pub peak: Option<f64>,
    /// Its sides, from the peak down to a quarter of its height.
    pub left: Option<f64>,
    pub right: Option<f64>,
    /// The first real dip's depth, when there is one.
    pub valley_depth: Option<f64>,
    pub peaks: usize,
}

/// Class a population, and say what the class was decided from.
pub fn classify(values: &[f64]) -> Reading {
    let finite = values.iter().filter(|v| v.is_finite()).count();
    let sides = crate::gate_rules::threshold::peak_sides(values);
    let peaks = crate::review::shape::summarise(values).map_or(0, |s| s.peaks.len());
    let valley = crate::gate_rules::threshold::first_valley(values, 1.0).ok();
    let class = if finite < MIN_EVENTS {
        ShapeClass::TooFew
    } else if peaks >= 3 {
        ShapeClass::SeveralPeaks
    } else if let Some(v) = &valley {
        if v.depth >= DEEP_VALLEY {
            ShapeClass::Separate
        } else {
            ShapeClass::Shoulder
        }
    } else {
        match sides.and_then(|s| s.right.zip(s.left)) {
            None if sides.is_some_and(|s| s.right.is_none()) => ShapeClass::Merged,
            Some((right, left)) if left > 0.0 && right / left >= SMEAR_RATIO => ShapeClass::Smear,
            _ => ShapeClass::NegativeOnly,
        }
    };
    Reading {
        class,
        peak: sides.map(|s| s.peak),
        left: sides.and_then(|s| s.left),
        right: sides.and_then(|s| s.right),
        valley_depth: valley.map(|v| v.depth),
        peaks,
    }
}

/// One gated sample's population on one marker.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SampleReading {
    pub file: String,
    pub specimen: Option<String>,
    pub sample_type: Option<String>,
    pub events: usize,
    pub reading: Reading,
    /// Where the gate's lower side sits on the marker now.
    pub line: Option<f64>,
    /// How many right-side widths above the negative's peak that is.
    pub line_in_right_widths: Option<f64>,
    /// What the gate holds now.
    pub holds: Option<f64>,
    /// The population's 99.5th percentile - the top of a control's negative.
    pub p995: Option<f64>,
    /// The events on the marker, for comparisons between a specimen's files.
    #[serde(skip)]
    values: Vec<f64>,
}

/// One kind of sample's populations on one marker.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TypeProfile {
    pub sample_type: String,
    pub samples: usize,
    pub parent_events_median: usize,
    /// How many samples are in each shape class.
    pub classes: BTreeMap<ShapeClass, usize>,
    /// Where the negative's peak sits - how much it shifts between samples.
    pub negative_peak: Option<Spread>,
    /// The negative's right side against its left - how its shape changes.
    pub right_to_left: Option<Spread>,
    pub valley_depth: Option<Spread>,
    /// Where the gate sits now, in right-side widths above the negative.
    pub gate_in_right_widths: Option<Spread>,
    /// What the gate holds now.
    pub holds: Option<Spread>,
}

/// A gate's populations on one of its markers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkerProfile {
    pub parameter: String,
    pub by_type: Vec<TypeProfile>,
    /// On each specimen's last sample type (the full stain), the fraction of
    /// events above its first type's (the control's) 99.5th percentile.
    pub signal_over_control: Option<(String, Spread)>,
    /// The file a rule for this gate is calibrated on, as it looks now.
    pub reference: Option<SampleReading>,
    /// Samples unlike the rest of their kind, in a line each.
    pub unusual: Vec<String>,
    /// Every sample read, for choosing which to look at.
    #[serde(skip)]
    pub samples: Vec<SampleReading>,
}

/// A gate, on both its markers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GateProfile {
    pub gate: String,
    pub markers: Vec<MarkerProfile>,
}

impl GateProfile {
    /// The profile in a line per marker, for an overview of many gates.
    pub fn lines(&self) -> Vec<String> {
        let pct = |v: f64| format!("{:.2}%", v * 100.0);
        self.markers
            .iter()
            .map(|m| {
                let types: Vec<String> = m
                    .by_type
                    .iter()
                    .map(|t| {
                        let classes: Vec<String> = t
                            .classes
                            .iter()
                            .map(|(c, n)| format!("{} {n}", c.label()))
                            .collect();
                        let mut said =
                            format!("{} x{} [{}]", t.sample_type, t.samples, classes.join(", "));
                        if let Some(r) = &t.right_to_left {
                            said.push_str(&format!(
                                "; negative R/L {:.2} ({:.2}-{:.2})",
                                r.median, r.p10, r.p90
                            ));
                        }
                        if let Some(k) = &t.gate_in_right_widths {
                            said.push_str(&format!(
                                "; gate {:.1} widths up ({:.1}-{:.1})",
                                k.median, k.p10, k.p90
                            ));
                        }
                        if let Some(h) = &t.holds {
                            said.push_str(&format!(
                                "; holds {} ({}-{})",
                                pct(h.median),
                                pct(h.p10),
                                pct(h.p90)
                            ));
                        }
                        said
                    })
                    .collect();
                let mut line = format!("{} | {}: {}", self.gate, m.parameter, types.join(" / "));
                if let Some((control, s)) = &m.signal_over_control {
                    line.push_str(&format!(
                        " | above the {control}'s top: {} ({}-{})",
                        pct(s.median),
                        pct(s.p10),
                        pct(s.p90)
                    ));
                }
                if !m.unusual.is_empty() {
                    line.push_str(&format!(" | {} unusual", m.unusual.len()));
                }
                line
            })
            .collect()
    }
}

/// A gate to profile: where it is in the tree, and its two markers.
pub struct ProfileTarget {
    pub target: RuleTarget,
    pub markers: (String, String),
}

fn percentile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let i = q * (sorted.len() - 1) as f64;
    let (lo, hi) = (i.floor() as usize, i.ceil() as usize);
    Some(sorted[lo] + (sorted[hi] - sorted[lo]) * (i - lo as f64))
}

/// Profile `targets` on the files in `inputs`. The gates as they stand are
/// read, nothing is moved; `inputs.rules` gives the pairing, and says which
/// file each gate's rule is calibrated on, if any.
pub fn profile(
    gates: &GateState,
    inputs: &RunInputs,
    targets: &[ProfileTarget],
    cancel: &std::sync::atomic::AtomicBool,
) -> (Vec<GateProfile>, Vec<String>) {
    // A line rule per gate per marker, so the run's own measuring reads each
    // parent population on that marker - one set of rules per axis.
    let store_on = |axis: usize| {
        let mut store = RuleStore::with_pairing(inputs.rules.pairing.clone());
        for t in targets {
            let marker = if axis == 0 {
                &t.markers.0
            } else {
                &t.markers.1
            };
            store.insert(
                t.target.clone(),
                GateRule {
                    parameter: marker.as_str().into(),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::Itself,
                    rule: Rule::InTheValley(ValleyRule::default()),
                },
            );
        }
        store
    };
    let stores = [store_on(0), store_on(1)];
    let (measured, problems) = measure_many(
        gates,
        &inputs.files,
        &inputs.compensation,
        &inputs.names,
        &inputs.cofactors,
        &inputs.metadata,
        &stores,
        cancel,
        |_, _| {},
    );
    let metadata = &inputs.metadata;
    let pairing = &inputs.rules.pairing;
    let sample_type = |file: &str| {
        metadata
            .get(file)
            .and_then(|row| pairing.sample_type_of(row))
            .map(|t| t.to_string())
    };
    let specimen = |file: &str| {
        metadata
            .get(file)
            .and_then(|row| row.get(&pairing.sample_id_column))
            .map(|s| s.to_string())
    };
    let read = |m: &Measurement| -> Option<SampleReading> {
        let line = m.line.as_ref()?;
        let reading = classify(&line.values);
        let mut sorted: Vec<f64> = line
            .values
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .collect();
        sorted.sort_by(f64::total_cmp);
        let holds = gates
            .gate_for_file(&m.gate_id, &m.file, metadata)
            .and_then(|g| admitted_by(&g, &m.index));
        let line_in_right_widths = reading
            .peak
            .zip(reading.right)
            .filter(|(_, r)| *r > 0.0)
            .map(|(p, r)| (line.current - p) / r);
        Some(SampleReading {
            file: m.file.to_string(),
            specimen: specimen(&m.file),
            sample_type: sample_type(&m.file),
            events: m.events,
            reading,
            line: Some(line.current),
            line_in_right_widths,
            holds,
            p995: percentile(&sorted, 0.995),
            values: line.values.clone(),
        })
    };

    let mut out = Vec::new();
    for t in targets {
        let name = describe(&t.target.gate, t.target.parent.as_deref());
        let mut markers = Vec::new();
        for (axis, marker) in [&t.markers.0, &t.markers.1].into_iter().enumerate() {
            let samples: Vec<SampleReading> = measured[axis]
                .0
                .iter()
                .filter(|m| {
                    *m.gate == *t.target.gate
                        && t.target
                            .parent
                            .as_deref()
                            .is_none_or(|p| m.parent_gate.as_deref() == Some(p))
                })
                .filter_map(read)
                .collect();
            if samples.is_empty() {
                continue;
            }
            markers.push(marker_profile(
                marker,
                samples,
                &pairing.display_order,
                inputs
                    .rules
                    .rule_for(&t.target.gate, t.target.parent.as_deref())
                    .and_then(|r| match &r.measured_on {
                        MeasuredOn::File(f) => Some(f.to_string()),
                        _ => None,
                    }),
            ));
        }
        out.push(GateProfile {
            gate: name,
            markers,
        });
    }
    (out, problems)
}

fn marker_profile(
    marker: &str,
    samples: Vec<SampleReading>,
    display_order: &[std::sync::Arc<str>],
    reference_file: Option<String>,
) -> MarkerProfile {
    let mut by: BTreeMap<String, Vec<&SampleReading>> = BTreeMap::new();
    for s in &samples {
        by.entry(s.sample_type.clone().unwrap_or_else(|| "unknown".into()))
            .or_default()
            .push(s);
    }
    let mut unusual = Vec::new();
    let by_type: Vec<TypeProfile> = by
        .iter()
        .map(|(kind, of)| {
            let spread = |f: &dyn Fn(&SampleReading) -> Option<f64>| {
                Spread::of(of.iter().filter_map(|s| f(s)).collect())
            };
            let mut classes: BTreeMap<ShapeClass, usize> = BTreeMap::new();
            for s in of {
                *classes.entry(s.reading.class).or_default() += 1;
            }
            let mut events: Vec<usize> = of.iter().map(|s| s.events).collect();
            events.sort_unstable();
            let right_to_left = spread(&|s| {
                s.reading
                    .right
                    .zip(s.reading.left)
                    .filter(|(_, l)| *l > 0.0)
                    .map(|(r, l)| r / l)
            });
            let gate_in = spread(&|s| s.line_in_right_widths);
            // Unlike the rest of its kind: another class than most of them,
            // or its gate well outside where the others' sit.
            let usual = classes.iter().max_by_key(|(_, n)| **n).map(|(c, _)| *c);
            for s in of {
                let mut why = Vec::new();
                if of.len() >= 3 && Some(s.reading.class) != usual {
                    why.push(format!(
                        "a {} where most are {}",
                        s.reading.class.label(),
                        usual.map_or("", |c| c.label())
                    ));
                }
                if let (Some(k), Some(g)) = (s.line_in_right_widths, &gate_in)
                    && of.len() >= 5
                    && (k < g.p10 - (g.p90 - g.p10) || k > g.p90 + (g.p90 - g.p10))
                {
                    why.push(format!(
                        "its gate {k:.1} widths up where most are {:.1}-{:.1}",
                        g.p10, g.p90
                    ));
                }
                if !why.is_empty() {
                    unusual.push(format!("{} ({kind}): {}", s.file, why.join("; ")));
                }
            }
            TypeProfile {
                sample_type: kind.clone(),
                samples: of.len(),
                parent_events_median: events[events.len() / 2],
                classes,
                negative_peak: spread(&|s| s.reading.peak),
                right_to_left,
                valley_depth: spread(&|s| s.reading.valley_depth),
                gate_in_right_widths: gate_in,
                holds: spread(&|s| s.holds),
            }
        })
        .collect();
    unusual.truncate(8);

    // Signal: on each specimen's full stain, what lies above its control's
    // negative.
    let signal_over_control = match (display_order.first(), display_order.last()) {
        (Some(control), Some(full)) if control != full => {
            let mut fractions = Vec::new();
            let mut by_specimen: BTreeMap<&str, (Option<&SampleReading>, Option<&SampleReading>)> =
                BTreeMap::new();
            for s in &samples {
                let (Some(specimen), Some(kind)) = (&s.specimen, &s.sample_type) else {
                    continue;
                };
                let slot = by_specimen.entry(specimen.as_str()).or_default();
                if **kind == **control {
                    slot.0 = Some(s);
                } else if **kind == **full {
                    slot.1 = Some(s);
                }
            }
            for (c, f) in by_specimen.into_values() {
                if let (Some(c), Some(f)) = (c, f)
                    && let Some(top) = c.p995
                    && !f.values.is_empty()
                {
                    fractions.push(
                        f.values.iter().filter(|v| **v > top).count() as f64
                            / f.values.len() as f64,
                    );
                }
            }
            Spread::of(fractions).map(|s| (control.to_string(), s))
        }
        _ => None,
    };
    MarkerProfile {
        parameter: marker.to_string(),
        by_type,
        signal_over_control,
        reference: reference_file.and_then(|f| samples.iter().find(|s| s.file == f).cloned()),
        unusual,
        samples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_distr::{Distribution, Normal, StandardNormal};

    fn normal(rng: &mut rand::rngs::StdRng, n: usize, centre: f64, sd: f64) -> Vec<f64> {
        let d = Normal::new(centre, sd).unwrap();
        (0..n).map(|_| d.sample(rng)).collect()
    }

    fn rng() -> rand::rngs::StdRng {
        rand::rngs::StdRng::seed_from_u64(3)
    }

    #[test]
    fn a_negative_and_a_positive_with_a_deep_dip_are_separate() {
        let mut r = rng();
        let mut v = normal(&mut r, 8_000, 0.5, 0.2);
        v.extend(normal(&mut r, 2_000, 3.0, 0.3));
        let read = classify(&v);
        assert_eq!(read.class, ShapeClass::Separate, "{read:?}");
        assert!(read.valley_depth.unwrap() >= DEEP_VALLEY);
        assert!((read.peak.unwrap() - 0.5).abs() < 0.1);
    }

    #[test]
    fn a_shallow_dip_is_a_shoulder() {
        let mut r = rng();
        let mut v = normal(&mut r, 8_000, 0.5, 0.3);
        v.extend(normal(&mut r, 3_000, 1.6, 0.35));
        let read = classify(&v);
        assert_eq!(read.class, ShapeClass::Shoulder, "{read:?}");
        assert!(read.valley_depth.unwrap() < DEEP_VALLEY);
    }

    #[test]
    fn positives_trailing_out_of_the_negative_are_a_smear() {
        let mut r = rng();
        // One population, twice as wide on the right as the left.
        let v: Vec<f64> = (0..10_000)
            .map(|i| {
                let z: f64 = StandardNormal.sample(&mut r);
                if (i as f64 + 0.5) / 10_000.0 <= 0.2 / 0.6 {
                    1.0 - z.abs() * 0.2
                } else {
                    1.0 + z.abs() * 0.4
                }
            })
            .collect();
        let read = classify(&v);
        assert_eq!(read.class, ShapeClass::Smear, "{read:?}");
        assert!(read.right.unwrap() / read.left.unwrap() >= SMEAR_RATIO);
    }

    #[test]
    fn a_negative_running_into_a_dense_smear_is_merged() {
        let mut r = rng();
        let mut v = normal(&mut r, 10_000, 100.0, 10.0);
        v.extend((0..40_000).map(|i| 100.0 + 200.0 * i as f64 / 40_000.0));
        assert_eq!(classify(&v).class, ShapeClass::Merged);
    }

    #[test]
    fn a_negative_alone_three_peaks_and_too_few_events() {
        let mut r = rng();
        let alone = normal(&mut r, 10_000, 0.5, 0.2);
        let read = classify(&alone);
        assert_eq!(read.class, ShapeClass::NegativeOnly, "{read:?}");
        assert_eq!(read.valley_depth, None);

        let mut three = normal(&mut r, 5_000, 0.0, 0.2);
        three.extend(normal(&mut r, 5_000, 2.0, 0.2));
        three.extend(normal(&mut r, 5_000, 4.0, 0.2));
        let read = classify(&three);
        assert_eq!(read.class, ShapeClass::SeveralPeaks, "{read:?}");
        assert!(read.peaks >= 3);

        assert_eq!(classify(&alone[..MIN_EVENTS - 1]).class, ShapeClass::TooFew);
        assert_eq!(classify(&[]).class, ShapeClass::TooFew);
    }

    #[test]
    fn every_class_has_its_own_label_and_name() {
        let all = [
            ShapeClass::Separate,
            ShapeClass::Shoulder,
            ShapeClass::Smear,
            ShapeClass::Merged,
            ShapeClass::NegativeOnly,
            ShapeClass::SeveralPeaks,
            ShapeClass::TooFew,
        ];
        let labels: std::collections::HashSet<_> = all.iter().map(|c| c.label()).collect();
        assert_eq!(labels.len(), all.len());
        assert_eq!(
            serde_json::to_value(ShapeClass::NegativeOnly).unwrap(),
            "negative_only"
        );
    }
}
