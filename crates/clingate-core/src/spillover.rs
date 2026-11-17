//! Telling a spillover error from unextracted autofluorescence, in the data.
//!
//! Both put events where they do not belong, and both are fixed elsewhere:
//! a spillover error in the compensation matrix, autofluorescence (AF) left
//! in by the unmixing in the unmixing, by extracting it. They leave different
//! marks - see `docs/plans/spillover-and-unmixing-check.md`:
//!
//! - spillover from A into B: among cells positive for A and not for B, B
//!   rises in step with A, by the same amount on every sample compensated
//!   alike;
//! - the same lean on some samples and not others: those samples' dye (a
//!   tandem breaking down), not the matrix;
//! - AF: cells negative for everything sit higher in some channels when they
//!   are large and granular (high side scatter), and so do unstained cells -
//!   where there is nothing to spill.
//!
//! Everything here is arithmetic over events already read, in the units the
//! file holds (compensated, not scaled), so it is tested on events built by
//! hand with a known spill and a known AF.

use std::sync::Arc;

use serde::Serialize;

/// One sample's events in the population looked in.
#[derive(Debug, Clone)]
pub struct SampleEvents {
    /// What a person calls the sample.
    pub sample: String,
    /// Each fluorescence channel's values, in the order of the check's
    /// channels.
    pub values: Vec<Vec<f32>>,
    /// Side scatter, for telling large granular cells from small ones.
    pub side_scatter: Vec<f32>,
}

/// A lean must hold over at least this many cells positive for A.
pub const LEAST_POSITIVE: usize = 400;
/// Positive is this many of the negative's widths above its centre.
pub const POSITIVE_WIDTHS: f64 = 4.0;
/// A lean that moves B by this many of B's negative widths across A's
/// positives is worth saying.
pub const LEAN_WIDTHS: f64 = 1.0;
/// An AF offset of this many widths, in high against low side scatter cells,
/// is worth saying.
pub const AF_WIDTHS: f64 = 1.0;
/// Slopes across samples varying by more than this share of their median
/// are the samples', not the matrix's.
pub const MOST_VARIATION: f64 = 0.5;

/// What the check found.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finding {
    /// The same lean on every sample: the matrix takes out too little (a
    /// positive coefficient) or too much of `from` from `into`.
    Spillover {
        from: Arc<str>,
        into: Arc<str>,
        /// How much of `from` is left in `into`, as a share: what to add to
        /// the spillover matrix's `from` → `into` value.
        coefficient: f64,
        /// How far it moves `into` across `from`'s positives, in widths of
        /// `into`'s negative, typically.
        widths: f64,
        /// Samples it was measured on.
        samples: usize,
    },
    /// A lean on some samples only.
    SampleSpecific {
        from: Arc<str>,
        into: Arc<str>,
        /// The samples leaning, with how far, in widths.
        leaning: Vec<(String, f64)>,
        samples: usize,
    },
    /// Cells negative for everything sitting higher when large and granular,
    /// in these channels, by these many widths typically - AF left in.
    Autofluorescence {
        /// Worst first.
        channels: Vec<(Arc<str>, f64)>,
        /// Whether unstained cells show it too; `None` where no unstained
        /// sample was read.
        in_unstained: Option<bool>,
        /// Where to look in the unstained for AF: the channel where its large
        /// granular cells sit furthest above its small ones, in widths, or
        /// the stained samples' worst where no unstained was read.
        look_in: Arc<str>,
    },
}

impl Finding {
    /// In a sentence, with what to do.
    pub fn says(&self) -> String {
        match self {
            Finding::Spillover {
                from,
                into,
                coefficient,
                widths,
                samples,
            } => {
                let share = coefficient.abs() * 100.0;
                let (moves, matrix, change) = if *coefficient >= 0.0 {
                    (
                        "rises with",
                        format!("leaves {share:.2}% of {from} in {into}"),
                        "Raise",
                    )
                } else {
                    (
                        "falls as",
                        format!("takes {share:.2}% too much of {from} out of {into}"),
                        "Lower",
                    )
                };
                format!(
                    "{into} {moves} {from} alike on the {samples} samples read - by {:.1} of \
                     its negative's widths across {from}'s positives: the matrix {matrix}. \
                     {change} the {from} → {into} spillover value by {share:.2}%.",
                    widths.abs()
                )
            }
            Finding::SampleSpecific {
                from,
                into,
                leaning,
                samples,
            } => format!(
                "{into} rises with {from} on {} of {samples} samples only ({}): not the matrix, \
                 which is the same for all - most likely the {from} dye in those tubes (a tandem \
                 breaking down). Check those samples before trusting their gates on {into}.",
                leaning.len(),
                leaning
                    .iter()
                    .map(|(sample, widths)| format!("{sample} {widths:.1}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Finding::Autofluorescence {
                channels,
                in_unstained,
                look_in,
            } => format!(
                "Large, granular cells negative for everything sit higher in {}{}: \
                 autofluorescence left in by the unmixing, not a spillover error - editing the \
                 matrix only moves it. Extract AF: in the unstained, gate the high side scatter \
                 cells, and look in {look_in}, where they stand out most.",
                channels
                    .iter()
                    .map(|(channel, widths)| format!("{channel} ({widths:.1} widths)"))
                    .collect::<Vec<_>>()
                    .join(", "),
                match in_unstained {
                    Some(true) => ", and unstained cells do too",
                    Some(false) => " - though the unstained cells read do not",
                    None => " (no unstained sample was read to confirm it)",
                },
            ),
        }
    }
}

/// A channel's negative: centred where the values are densest - most cells
/// are negative for most markers - and as wide as its peak is at half its
/// height, on the side it falls off sooner. Positives above it, and cells
/// carried below it by too much compensation, sit well clear of the peak and
/// move neither.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Negative {
    pub centre: f64,
    pub width: f64,
}

/// A normal's half width at half its height, in its widths.
const HALF_HEIGHT_WIDTHS: f64 = 1.1774;

impl Negative {
    pub fn of(values: &[f32]) -> Option<Self> {
        let mut sorted: Vec<f64> = values
            .iter()
            .map(|v| f64::from(*v))
            .filter(|v| v.is_finite())
            .collect();
        if sorted.len() < 100 {
            return None;
        }
        sorted.sort_by(f64::total_cmp);
        let (mut centre, reach) = densest(&sorted);
        if reach <= 0.0 {
            return None;
        }
        let mut half = half_height(&sorted, centre, reach)?;
        // The densest run's middle wanders by a fraction of the width, and the
        // side that falls sooner would take the wander as narrowness: centre
        // on the middle of the cells within the half height first.
        for _ in 0..3 {
            let near = &sorted[sorted.partition_point(|v| *v < centre - half)
                ..sorted.partition_point(|v| *v <= centre + half)];
            centre = quantile(near, 0.5);
            half = half_height(&sorted, centre, reach)?;
        }
        Some(Self {
            centre,
            width: half / HALF_HEIGHT_WIDTHS,
        })
    }

    pub fn positive_above(&self) -> f64 {
        self.centre + POSITIVE_WIDTHS * self.width
    }
}

/// The middle of the narrowest run of a tenth of the sorted values - where
/// the negative peaks - and half that run's span.
fn densest(sorted: &[f64]) -> (f64, f64) {
    let run = (sorted.len() / 10).max(2);
    let start = (0..=sorted.len() - run)
        .min_by(|&x, &y| {
            (sorted[x + run - 1] - sorted[x]).total_cmp(&(sorted[y + run - 1] - sorted[y]))
        })
        .unwrap_or(0);
    (
        (sorted[start] + sorted[start + run - 1]) / 2.0,
        (sorted[start + run - 1] - sorted[start]) / 2.0,
    )
}

/// How far from `centre` the sorted values thin to half as many within
/// `reach` as at it, on the side they thin sooner.
fn half_height(sorted: &[f64], centre: f64, reach: f64) -> Option<f64> {
    let height = |at: f64| count_within(sorted, at, reach);
    let peak = height(centre);
    let span = sorted[sorted.len() - 1] - sorted[0];
    let fall = |direction: f64| {
        (1..)
            .map(|n| n as f64 * reach / 4.0)
            .take_while(|distance| *distance <= span)
            .find(|distance| height(centre + direction * distance) * 2 < peak)
    };
    [fall(-1.0), fall(1.0)]
        .into_iter()
        .flatten()
        .reduce(f64::min)
}

/// How many of the sorted values lie within `reach` of `at`.
fn count_within(sorted: &[f64], at: f64, reach: f64) -> usize {
    sorted.partition_point(|v| *v <= at + reach) - sorted.partition_point(|v| *v < at - reach)
}

/// The `q` quantile of sorted values, by linear interpolation.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let at = q * (sorted.len() - 1) as f64;
    let (low, high) = (at.floor() as usize, at.ceil() as usize);
    sorted[low] + (sorted[high] - sorted[low]) * (at - low as f64)
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(quantile(values, 0.5))
}

/// How B leans on A in one sample: the slope of B on A among cells positive
/// for A, and how far that moves B across A's positives, in B's negative's
/// widths. Cells far above the trend are positive for B and left out; cells
/// carried up by the lean itself are not. `None` with too few cells.
pub fn lean(
    a: &[f32],
    b: &[f32],
    a_negative: Negative,
    b_negative: Negative,
) -> Option<(f64, f64)> {
    let mut cells: Vec<(f64, f64)> = a
        .iter()
        .zip(b)
        .map(|(a, b)| (f64::from(*a), f64::from(*b)))
        .filter(|(a, b)| a.is_finite() && b.is_finite())
        .filter(|(a, _)| *a > a_negative.positive_above())
        .collect();
    if cells.len() < LEAST_POSITIVE {
        return None;
    }
    cells.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (slope, intercept) = trend(&cells, median)?;
    let cut = POSITIVE_WIDTHS * b_negative.width;
    let negative_for_b: Vec<(f64, f64)> = cells
        .into_iter()
        .filter(|(a, b)| *b < intercept + slope * a + cut)
        .collect();
    if negative_for_b.len() < LEAST_POSITIVE {
        return None;
    }
    // Bright B cells are out: means now, which a skewed bin does not bias.
    let (slope, _) = trend(&negative_for_b, |values| mean(values))?;
    let a_values: Vec<f64> = negative_for_b.iter().map(|c| c.0).collect();
    let span = quantile(&a_values, 0.9) - quantile(&a_values, 0.1);
    Some((slope, slope * span / b_negative.width))
}

/// The line through the `centre`s of A and of B in eight bins of A, sorted
/// by A - medians where bright B cells may still be among them, which cannot
/// tilt it. Its slope and intercept.
fn trend(cells: &[(f64, f64)], centre: fn(&mut [f64]) -> Option<f64>) -> Option<(f64, f64)> {
    const BINS: usize = 8;
    let per_bin = cells.len() / BINS;
    if per_bin == 0 {
        return None;
    }
    let points: Vec<(f64, f64)> = cells
        .chunks(per_bin)
        .filter(|bin| bin.len() == per_bin)
        .filter_map(|bin| {
            let mut a: Vec<f64> = bin.iter().map(|c| c.0).collect();
            let mut b: Vec<f64> = bin.iter().map(|c| c.1).collect();
            Some((centre(&mut a)?, centre(&mut b)?))
        })
        .collect();
    let n = points.len() as f64;
    let (mean_a, mean_b) = (
        points.iter().map(|p| p.0).sum::<f64>() / n,
        points.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let covariance: f64 = points.iter().map(|p| (p.0 - mean_a) * (p.1 - mean_b)).sum();
    let variance: f64 = points.iter().map(|p| (p.0 - mean_a).powi(2)).sum();
    if variance <= 0.0 {
        return None;
    }
    let slope = covariance / variance;
    Some((slope, mean_b - slope * mean_a))
}

/// How much higher each channel's negatives sit in large granular cells than
/// in small ones, in its widths: among cells negative for every channel, the
/// top fifth by side scatter against the bottom half. A channel with no
/// negative is 0; `None` with too few cells negative everywhere.
pub fn granular_offsets(sample: &SampleEvents, negatives: &[Option<Negative>]) -> Option<Vec<f64>> {
    let negative_everywhere: Vec<usize> = (0..sample.side_scatter.len())
        .filter(|&row| {
            sample
                .values
                .iter()
                .zip(negatives)
                .all(|(channel, negative)| {
                    negative.is_none_or(|n| f64::from(channel[row]) < n.positive_above())
                })
        })
        .collect();
    let mut by_scatter = negative_everywhere.clone();
    by_scatter.sort_by(|x, y| sample.side_scatter[*x].total_cmp(&sample.side_scatter[*y]));
    let n = by_scatter.len();
    if n < 100 {
        return None;
    }
    let (low, high) = (&by_scatter[..n / 2], &by_scatter[n - n / 5..]);
    Some(
        sample
            .values
            .iter()
            .zip(negatives)
            .map(|(channel, negative)| {
                let Some(negative) = negative else {
                    return 0.0;
                };
                let at = |rows: &[usize]| {
                    let mut values: Vec<f64> =
                        rows.iter().map(|&r| f64::from(channel[r])).collect();
                    median(&mut values).unwrap_or(0.0)
                };
                (at(high) - at(low)) / negative.width
            })
            .collect(),
    )
}

/// One sample reduced to what the check reads of it, so a plate of files
/// need not be held at once.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub sample: String,
    /// The lean of each channel on each other, `leans[from][into]`, as
    /// [`lean`] gives it; `None` where it could not be measured.
    pub leans: Vec<Vec<Option<(f64, f64)>>>,
    /// Each channel's [`granular_offsets`]; `None` where too few cells are
    /// negative everywhere.
    pub offsets: Option<Vec<f64>>,
    /// The channels with no negative to read - too few values, or all alike -
    /// left out of every lean and offset.
    pub no_negative: Vec<usize>,
}

/// What the check reads of `sample`.
pub fn measure(sample: &SampleEvents) -> Measured {
    let negatives: Vec<Option<Negative>> = sample.values.iter().map(|v| Negative::of(v)).collect();
    let channels = sample.values.len();
    let leans = (0..channels)
        .map(|from| {
            (0..channels)
                .map(|into| {
                    (from != into).then_some(())?;
                    lean(
                        &sample.values[from],
                        &sample.values[into],
                        negatives[from]?,
                        negatives[into]?,
                    )
                })
                .collect()
        })
        .collect();
    Measured {
        sample: sample.sample.clone(),
        leans,
        offsets: granular_offsets(sample, &negatives),
        no_negative: (0..channels).filter(|c| negatives[*c].is_none()).collect(),
    }
}

/// The check, on `stained` samples and any `unstained` ones, over `channels`
/// in the order their values were held.
pub fn check(channels: &[Arc<str>], stained: &[Measured], unstained: &[Measured]) -> Vec<Finding> {
    let mut findings = leans(channels, stained);
    if let Some(af) = autofluorescence(channels, stained, unstained) {
        findings.push(af);
    }
    findings
}

fn leans(channels: &[Arc<str>], stained: &[Measured]) -> Vec<Finding> {
    let mut found: Vec<(usize, usize, f64, Finding)> = Vec::new();
    for from in 0..channels.len() {
        for into in 0..channels.len() {
            if from == into {
                continue;
            }
            let measured: Vec<(&str, f64, f64)> = stained
                .iter()
                .filter_map(|sample| {
                    let (slope, widths) = sample.leans.get(from)?.get(into).copied().flatten()?;
                    Some((sample.sample.as_str(), slope, widths))
                })
                .collect();
            if let Some((slope, finding)) =
                lean_finding(&channels[from], &channels[into], &measured)
            {
                found.push((from, into, slope, finding));
            }
        }
    }
    // Spill from A into B carries A's cells over B's line, so B's "positives"
    // are A-bright and A seems to rise with B too, by about the inverse. The
    // smaller of the two is the spill.
    found
        .iter()
        .filter(|(from, into, slope, _)| {
            !found
                .iter()
                .any(|(other_from, other_into, other_slope, _)| {
                    other_from == into && other_into == from && other_slope.abs() < slope.abs()
                })
        })
        .map(|(_, _, _, finding)| finding.clone())
        .collect()
}

/// What the leans of `into` on `from`, sample by sample, say - a spill, or a
/// lean of some samples only - with their typical slope, to tell a spill
/// from its reverse.
fn lean_finding(
    from: &Arc<str>,
    into: &Arc<str>,
    measured: &[(&str, f64, f64)],
) -> Option<(f64, Finding)> {
    let leaning: Vec<(String, f64)> = measured
        .iter()
        .filter(|(_, _, widths)| widths.abs() >= LEAN_WIDTHS)
        .map(|(sample, _, widths)| (sample.to_string(), *widths))
        .collect();
    if leaning.is_empty() {
        return None;
    }
    let mut slopes: Vec<f64> = measured.iter().map(|m| m.1).collect();
    let mut widths: Vec<f64> = measured.iter().map(|m| m.2).collect();
    let typical_slope = median(&mut slopes)?;
    slopes.sort_by(f64::total_cmp);
    let spread = quantile(&slopes, 0.75) - quantile(&slopes, 0.25);
    let consistent =
        leaning.len() * 2 > measured.len() && spread <= MOST_VARIATION * typical_slope.abs();
    Some((
        typical_slope,
        if consistent {
            Finding::Spillover {
                from: from.clone(),
                into: into.clone(),
                coefficient: typical_slope,
                widths: median(&mut widths)?,
                samples: measured.len(),
            }
        } else {
            Finding::SampleSpecific {
                from: from.clone(),
                into: into.clone(),
                leaning,
                samples: measured.len(),
            }
        },
    ))
}

fn autofluorescence(
    channels: &[Arc<str>],
    stained: &[Measured],
    unstained: &[Measured],
) -> Option<Finding> {
    let typical = |samples: &[Measured]| -> Option<Vec<f64>> {
        let offsets: Vec<&Vec<f64>> = samples.iter().filter_map(|s| s.offsets.as_ref()).collect();
        if offsets.is_empty() {
            return None;
        }
        Some(
            (0..channels.len())
                .map(|c| {
                    let mut each: Vec<f64> = offsets.iter().map(|o| o[c]).collect();
                    median(&mut each).unwrap_or(0.0)
                })
                .collect(),
        )
    };
    let in_stained = typical(stained)?;
    let in_unstained = typical(unstained);
    let mut found: Vec<(Arc<str>, f64)> = channels
        .iter()
        .zip(&in_stained)
        .filter(|(_, offset)| **offset >= AF_WIDTHS)
        .map(|(channel, offset)| (channel.clone(), *offset))
        .collect();
    if found.is_empty() {
        return None;
    }
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    let at = |channel: &Arc<str>| channels.iter().position(|c| c == channel);
    let confirmed = in_unstained.as_ref().map(|unstained| {
        found
            .iter()
            .any(|(channel, _)| at(channel).is_some_and(|c| unstained[c] >= AF_WIDTHS))
    });
    let look_in = match &in_unstained {
        Some(unstained) => channels
            .iter()
            .zip(unstained)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(channel, _)| channel.clone())?,
        None => found[0].0.clone(),
    };
    Some(Finding::Autofluorescence {
        channels: found,
        in_unstained: confirmed,
        look_in,
    })
}

/// A file to read for the check.
#[derive(Clone)]
pub struct FileToRead {
    pub sample: String,
    pub path: std::path::PathBuf,
    pub compensation: crate::compensation::Choice,
    pub resolver: crate::gates::gate_store::GateOverrideResolver,
}

/// Everything the check reads: the population, the channels, and the files.
#[derive(Clone)]
pub struct SpilloverJob {
    /// The fluorescence channels.
    pub channels: Vec<Arc<str>>,
    pub side_scatter: Arc<str>,
    pub cofactors: Vec<(Arc<str>, f32)>,
    /// The gates down to the population looked in, outermost first.
    pub chain: Vec<crate::gates::gate_store::GateId>,
    pub stained: Vec<FileToRead>,
    pub unstained: Vec<FileToRead>,
}

/// What the check found, on how many samples, and what it could not read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpilloverCheck {
    pub findings: Vec<Finding>,
    pub stained: usize,
    pub unstained: usize,
    pub problems: Vec<String>,
}

/// One file's events in the population under `chain`, on `channels` and
/// side scatter, compensated but not scaled: spill is linear in the units
/// the file holds.
pub fn read_sample(
    file: &FileToRead,
    cofactors: &[(Arc<str>, f32)],
    chain: &[crate::gates::gate_store::GateId],
    channels: &[Arc<str>],
    side_scatter: &Arc<str>,
) -> anyhow::Result<SampleEvents> {
    use polars::prelude::*;
    let frame = crate::events::read_unscaled(&file.path, &file.compensation)?;
    let wanted: Vec<PlSmallStr> = channels
        .iter()
        .chain(std::iter::once(side_scatter))
        .map(|c| PlSmallStr::from(c.as_ref()))
        .collect();
    let linear = frame.select(wanted)?;
    let mut populations =
        crate::populations::Populations::new(frame, cofactors, file.resolver.clone());
    let inside = linear.filter(&populations.mask(chain)?)?;
    let column = |name: &str| -> anyhow::Result<Vec<f32>> {
        Ok(inside
            .column(name)?
            .cast(&DataType::Float32)?
            .f32()?
            .into_iter()
            .map(|v| v.unwrap_or(f32::NAN))
            .collect())
    };
    Ok(SampleEvents {
        sample: file.sample.clone(),
        values: channels
            .iter()
            .map(|c| column(c))
            .collect::<anyhow::Result<_>>()?,
        side_scatter: column(side_scatter)?,
    })
}

/// Read and measure each file, a few at a time - only its measures are kept -
/// and check them. `progress` hears how many are read of how many.
pub fn run(
    job: &SpilloverJob,
    cancel: &std::sync::atomic::AtomicBool,
    progress: impl Fn(usize, usize) + Sync,
) -> Result<SpilloverCheck, String> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let total = job.stained.len() + job.unstained.len();
    let done = AtomicUsize::new(0);
    let measure_all = |files: &[FileToRead]| -> (Vec<Measured>, Vec<String>) {
        let read: Vec<Result<Measured, String>> = files
            .par_iter()
            .map(|file| {
                if cancel.load(Ordering::Relaxed) {
                    return Err("stopped".to_string());
                }
                let measured = read_sample(
                    file,
                    &job.cofactors,
                    &job.chain,
                    &job.channels,
                    &job.side_scatter,
                )
                .map(|events| measure(&events))
                .map_err(|e| format!("{} could not be read: {e}", file.sample));
                progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                measured
            })
            .collect();
        let mut measured = Vec::new();
        let mut problems = Vec::new();
        for one in read {
            match one {
                Ok(m) => {
                    problems.extend(m.no_negative.iter().map(|c| {
                        format!(
                            "{}: {} has no negative to read, so it is left out",
                            m.sample, job.channels[*c]
                        )
                    }));
                    measured.push(m);
                }
                Err(why) => problems.push(why),
            }
        }
        (measured, problems)
    };
    progress(0, total);
    let (stained, mut problems) = measure_all(&job.stained);
    let (unstained, unread) = measure_all(&job.unstained);
    if cancel.load(Ordering::Relaxed) {
        return Err("stopped".into());
    }
    problems.extend(unread);
    if stained.is_empty() {
        return Err(format!(
            "no stained sample could be read - {}",
            problems
                .first()
                .map(String::as_str)
                .unwrap_or("there were none")
        ));
    }
    Ok(SpilloverCheck {
        findings: check(&job.channels, &stained, &unstained),
        stained: stained.len(),
        unstained: unstained.len(),
        problems,
    })
}

/// Whether a sample type names an unstained sample: "U" or "Unstained",
/// whatever its case.
pub fn is_unstained_type(sample_type: &str) -> bool {
    let kind = sample_type.trim().to_lowercase();
    kind == "u" || kind.starts_with("unstained")
}

/// The check on the workspace: the population `node`, every arcsinh channel
/// but scatter, each file's events as the gating places them; files whose
/// sample type [`is_unstained_type`] read as unstained, the rest as stained.
pub fn job_for(
    gates: &crate::gates::GateState,
    inputs: &crate::gate_rules::run::RunInputs,
    node: &crate::gates::gate_store::NodeId,
) -> Result<SpilloverJob, String> {
    let channels: Vec<Arc<str>> = inputs
        .cofactors
        .iter()
        .map(|(channel, _)| channel.clone())
        .filter(|channel| !channel.starts_with("FSC") && !channel.starts_with("SSC"))
        .collect();
    if channels.len() < 2 {
        return Err("fewer than two fluorescence channels are scaled as arcsinh".into());
    }
    let mut job = SpilloverJob {
        channels,
        side_scatter: Arc::from("SSC-A"),
        cofactors: inputs.cofactors.clone(),
        chain: gates.gate_chain_for_node(node),
        stained: Vec::new(),
        unstained: Vec::new(),
    };
    for (name, path) in &inputs.files {
        let Some(id) = inputs.names.get(name) else {
            continue;
        };
        let groups = inputs.metadata.get(id).cloned().unwrap_or_default();
        let file = FileToRead {
            sample: name.to_string(),
            path: path.clone(),
            compensation: inputs.compensation.matrix_for(path),
            resolver: gates.get_current_sample(id.clone(), &groups),
        };
        let unstained = inputs
            .metadata
            .get(id)
            .and_then(|row| inputs.rules.pairing.sample_type_of(row))
            .is_some_and(|kind| is_unstained_type(&kind));
        if unstained {
            job.unstained.push(file);
        } else {
            job.stained.push(file);
        }
    }
    if job.stained.is_empty() {
        return Err("no stained sample to look at".into());
    }
    Ok(job)
}
