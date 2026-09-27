//! Compensation: taking each fluorochrome's spillover into other detectors
//! back out of the events.
//!
//! ## The matrix
//!
//! A [`Spillover`] matrix is the standard's `$SPILLOVER`: row `i` is the
//! fluorochrome measured in channel `i`, and the value in column `j` is the
//! fraction of its signal that shows up in channel `j`. So the diagonal is 1,
//! and an event's observed values `o` are its true values `t` times the
//! matrix, `o = t · S`. Compensating is `t = o · S⁻¹`: each compensated
//! channel `k` is `Σⱼ oⱼ (S⁻¹)ⱼₖ`. That is what FlowJo and Omiq do.
//!
//! ## Where a matrix comes from
//!
//! - A file's own, from its `$SPILLOVER` (or `$SPILL`, as BD writes it):
//!   what the acquisition software computed. [`Spillover::from_keywords`].
//! - A CSV exported from Omiq: the channel names across the top row, then one
//!   row per channel in the same order, no row names, values in percent (the
//!   diagonal 100). [`Spillover::from_omiq_csv`]. Fractions (diagonal 1) are
//!   read too, and so is a CSV with its channels named down the first column
//!   as well, as long as they are in the same order as across the top.
//!
//! Which one a file is compensated with is the workspace's choice, per
//! compensation group - see [`groups`].
//!
//! ## What Omiq does
//!
//! Established on one Aurora file exported from Omiq with no compensation
//! task, with the compensation at 0, and with 10% of BUV395 put into BUV805
//! (the fixtures `omiq_export_*.fcs` and
//! `omiq_compensation_buv395_into_buv805.csv`):
//!
//! - Omiq applies its compensation to the events when it exports a file,
//!   and writes no `$SPILLOVER`. The compensated export's BUV805 is the
//!   uncompensated one's less a tenth of its BUV395, event for event; every
//!   other channel is the same. No task and 0% export identically.
//! - So an Omiq export has no matrix of its own, and needs none: it arrives
//!   compensated as it was in Omiq. Loading Omiq's matrix over it again
//!   would compensate it twice, which [`groups::Compensation::check`] says.
//! - Omiq's matrix, copied from its compensation view, has the fluorochrome
//!   that spills in the row and the detector it spills into in the column,
//!   with the same channels in the same order down and across - the
//!   standard's orientation. Compensating the uncompensated export with it
//!   here gives Omiq's compensated export.
//!
//! ## Matching a matrix to a file
//!
//! A matrix names channels as the file's `$PnN` does. Each name is looked
//! for as a `$PnN`, then as a `$PnN` ignoring case, then as a `$PnN` with
//! `-A` after it, then as a marker label (`$PnS`) - the first of those that
//! finds exactly one channel. A name that finds none, or several, is an
//! error naming it: a file is never compensated for some channels and not
//! others.

pub mod groups;
#[cfg(test)]
mod tests;

use anyhow::{Result, anyhow};
use flow_fcs::Metadata;
use flow_fcs::keyword::{Keyword, MixedKeyword, StringableKeyword};
use polars::prelude::*;
use rayon::prelude::*;
use std::sync::Arc;

/// How far a diagonal entry may be from 1 (or 100) and still be one.
const DIAGONAL_TOLERANCE: f64 = 1e-3;

/// A spillover matrix over named channels. See the module documentation for
/// which way round it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Spillover {
    channels: Vec<Arc<str>>,
    /// Row-major, `channels.len()` squared, as fractions.
    values: Vec<f64>,
}

impl Spillover {
    /// A matrix over `channels`, row-major, as fractions.
    ///
    /// Refused unless it is square over distinct, non-empty names, every
    /// value is finite, and the diagonal is 1: anything else is not a
    /// spillover matrix, and compensating with it would be quietly wrong.
    pub fn new(channels: Vec<Arc<str>>, values: Vec<f64>) -> Result<Self> {
        let n = channels.len();
        if n == 0 {
            return Err(anyhow!("the matrix names no channels"));
        }
        if values.len() != n * n {
            return Err(anyhow!(
                "{n} channels need {} values, and there are {}",
                n * n,
                values.len()
            ));
        }
        for (i, name) in channels.iter().enumerate() {
            if name.trim().is_empty() {
                return Err(anyhow!("channel {} has no name", i + 1));
            }
            if channels[..i].contains(name) {
                return Err(anyhow!("channel {name} is named twice"));
            }
        }
        if let Some(at) = values.iter().position(|v| !v.is_finite()) {
            return Err(anyhow!(
                "the value for {} into {} is not a number",
                channels[at / n],
                channels[at % n]
            ));
        }
        for (i, name) in channels.iter().enumerate() {
            let d = values[i * n + i];
            if (d - 1.0).abs() > DIAGONAL_TOLERANCE {
                return Err(anyhow!(
                    "{name}'s spillover into itself is {d}, where a spillover matrix has 1"
                ));
            }
        }
        Ok(Self { channels, values })
    }

    /// A file's own matrix, from its `$SPILLOVER` or `$SPILL` keyword.
    ///
    /// `None` if it has neither. An error if the one it has is not a
    /// spillover matrix.
    pub fn from_keywords(metadata: &Metadata) -> Result<Option<Self>> {
        let found = ["$SPILLOVER", "$SPILL"]
            .iter()
            .find_map(|k| metadata.keywords.get(*k).map(|v| (*k, v)));
        let Some((keyword, value)) = found else {
            return Ok(None);
        };
        match value {
            Keyword::Mixed(MixedKeyword::SPILLOVER {
                parameter_names,
                matrix_values,
                ..
            }) => Self::new(
                parameter_names
                    .iter()
                    .map(|n| Arc::from(n.trim()))
                    .collect(),
                matrix_values.iter().map(|&v| f64::from(v)).collect(),
            )
            .map(Some)
            .map_err(|e| anyhow!("its {keyword} is not a spillover matrix: {e}")),
            // BD writes `SPILL` without the `$`, which flow keeps as text.
            Keyword::String(text) => Self::from_keyword_text(&text.get_str())
                .map(Some)
                .map_err(|e| anyhow!("its {keyword} is not a spillover matrix: {e}")),
            _ => Err(anyhow!("its {keyword} could not be read as a matrix")),
        }
    }

    /// A matrix in the standard's keyword form: the channel count, the
    /// channel names, then the values row by row, all separated by commas.
    fn from_keyword_text(text: &str) -> Result<Self> {
        let parts: Vec<&str> = text.split(',').map(str::trim).collect();
        let n: usize = parts
            .first()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| anyhow!("it does not start with a channel count"))?;
        if parts.len() != 1 + n + n * n {
            return Err(anyhow!(
                "{n} channels need {} entries after the count, and there are {}",
                n + n * n,
                parts.len() - 1
            ));
        }
        let values = parts[1 + n..]
            .iter()
            .map(|v| {
                v.parse::<f64>()
                    .map_err(|_| anyhow!("{v:?} is not a number"))
            })
            .collect::<Result<Vec<_>>>()?;
        Self::new(parts[1..=n].iter().map(|&c| Arc::from(c)).collect(), values)
    }

    /// A matrix exported from Omiq as CSV. See the module documentation for
    /// the layout.
    pub fn from_omiq_csv(text: &str) -> Result<Self> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut rows = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .map(split_csv_line);
        let header = rows.next().ok_or_else(|| anyhow!("the file is empty"))?;
        let rows: Vec<Vec<String>> = rows.collect();

        // Channels named down the first column too: the top-left cell is
        // empty and every row starts with its channel's name.
        let row_names = header.first().is_some_and(|c| c.is_empty())
            && rows.iter().all(|r| r.len() == header.len());
        let names: Vec<&str> = header
            .iter()
            .skip(usize::from(row_names))
            .map(String::as_str)
            .collect();
        let n = names.len();
        if rows.len() != n {
            return Err(anyhow!(
                "the top row names {n} channels, and there are {} rows of values under it",
                rows.len()
            ));
        }

        let mut values = Vec::with_capacity(n * n);
        for (i, row) in rows.iter().enumerate() {
            let cells: &[String] = if row_names {
                if row[0] != names[i] {
                    return Err(anyhow!(
                        "row {} is named {}, where the top row puts {} in that place",
                        i + 1,
                        row[0],
                        names[i]
                    ));
                }
                &row[1..]
            } else {
                row
            };
            if cells.len() != n {
                return Err(anyhow!(
                    "the row for {} has {} values, not {n}",
                    names[i],
                    cells.len()
                ));
            }
            for (j, cell) in cells.iter().enumerate() {
                let value: f64 = cell.parse().map_err(|_| {
                    anyhow!(
                        "the value for {} into {}, {cell:?}, is not a number",
                        names[i],
                        names[j]
                    )
                })?;
                values.push(value);
            }
        }

        // Percent, as Omiq writes it, if the diagonal is 100; fractions if 1.
        let diagonal =
            |at: f64| (0..n).all(|i| (values[i * n + i] - at).abs() <= at * DIAGONAL_TOLERANCE);
        if diagonal(100.0) {
            values.iter_mut().for_each(|v| *v /= 100.0);
        } else if !diagonal(1.0) {
            let (i, d) = (0..n)
                .map(|i| (i, values[i * n + i]))
                .find(|&(_, d)| (d - 100.0).abs() > 100.0 * DIAGONAL_TOLERANCE)
                .expect("not all 100");
            return Err(anyhow!(
                "each channel's spillover into itself should be 100 (or 1, in fractions), \
                 and {}'s is {d}",
                names[i]
            ));
        }
        Self::new(names.into_iter().map(Arc::from).collect(), values)
    }

    /// [`Spillover::from_omiq_csv`], from a file.
    pub fn read_omiq_csv(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("could not read {}: {e}", path.display()))?;
        Self::from_omiq_csv(&text)
    }

    pub fn channels(&self) -> &[Arc<str>] {
        &self.channels
    }

    /// The fraction of `from`'s signal that shows up in `into`.
    pub fn value(&self, from: usize, into: usize) -> f64 {
        self.values[from * self.channels.len() + into]
    }

    /// Whether compensating with this changes nothing.
    pub fn is_identity(&self) -> bool {
        let n = self.channels.len();
        (0..n).all(|i| (0..n).all(|j| i == j || self.value(i, j) == 0.0))
    }

    /// The matrix over only the channels that take part: those that spill
    /// into another channel or have another spill into them. `None` if no
    /// channel does - an identity, which changes nothing.
    ///
    /// Compensating with this is exactly compensating with the whole matrix.
    /// A channel with nothing off its diagonal, in its row or its column, is
    /// a block of its own, and the inverse of a block-diagonal matrix is the
    /// inverses of its blocks: a 1, which leaves the channel as it is. So a
    /// file need not carry such a channel at all - which matters for a
    /// matrix exported from Omiq for a whole panel, most of it identity,
    /// applied to files that carry some of that panel.
    pub fn involved(&self) -> Option<Self> {
        let n = self.channels.len();
        let keep: Vec<usize> = (0..n)
            .filter(|&i| {
                (0..n).any(|j| j != i && (self.value(i, j) != 0.0 || self.value(j, i) != 0.0))
            })
            .collect();
        if keep.is_empty() {
            return None;
        }
        if keep.len() == n {
            return Some(self.clone());
        }
        let values = keep
            .iter()
            .flat_map(|&i| keep.iter().map(move |&j| (i, j)))
            .map(|(i, j)| self.value(i, j))
            .collect();
        Some(Self {
            channels: keep.iter().map(|&i| self.channels[i].clone()).collect(),
            values,
        })
    }

    /// Whether `other` is the same matrix: the same channels, each pair with
    /// the same value, in whatever order the channels are listed. How files
    /// are grouped by their own matrices.
    pub fn same_as(&self, other: &Self) -> bool {
        let n = self.channels.len();
        if other.channels.len() != n {
            return false;
        }
        let Some(order): Option<Vec<usize>> = self
            .channels
            .iter()
            .map(|c| other.channels.iter().position(|o| o == c))
            .collect()
        else {
            return false;
        };
        (0..n).all(|i| {
            (0..n).all(|j| (self.value(i, j) - other.value(order[i], order[j])).abs() <= 1e-6)
        })
    }

    /// The largest difference between this and `other`, entry by entry over
    /// the channels both name, in percentage points; and whether `other`
    /// matches this better read the other way round.
    ///
    /// For showing how far a loaded matrix is from a file's own: a large
    /// difference usually means the wrong matrix, and a transposed one means
    /// it was written with its rows and columns swapped.
    pub fn compare(&self, other: &Self) -> Option<Comparison> {
        let shared: Vec<(usize, usize)> = self
            .channels
            .iter()
            .enumerate()
            .filter_map(|(i, c)| other.channels.iter().position(|o| o == c).map(|k| (i, k)))
            .collect();
        if shared.len() < 2 {
            return None;
        }
        let (mut as_is, mut swapped) = (0.0f64, 0.0f64);
        for &(i, ki) in &shared {
            for &(j, kj) in &shared {
                as_is = as_is.max((self.value(i, j) - other.value(ki, kj)).abs());
                swapped = swapped.max((self.value(i, j) - other.value(kj, ki)).abs());
            }
        }
        Some(Comparison {
            shared: shared.len(),
            largest_difference: as_is * 100.0,
            transposed_fits_better: swapped + 1e-9 < as_is,
        })
    }

    /// Which of `channels` (a file's `$PnN`s, with its `$PnS` labels) each of
    /// this matrix's channels is. See the module documentation.
    pub fn resolve(&self, channels: &[(&str, Option<&str>)]) -> Result<Vec<usize>> {
        let mut unmatched = Vec::new();
        let mut resolved = Vec::with_capacity(self.channels.len());
        for name in &self.channels {
            match find_channel(name, channels) {
                Ok(at) => resolved.push(at),
                Err(why) => unmatched.push(why),
            }
        }
        if !unmatched.is_empty() {
            return Err(anyhow!("{}", unmatched.join("; ")));
        }
        for (a, &i) in resolved.iter().enumerate() {
            if let Some(b) = resolved[..a].iter().position(|&k| k == i) {
                return Err(anyhow!(
                    "{} and {} are both channel {}",
                    self.channels[b],
                    self.channels[a],
                    channels[i].0
                ));
            }
        }
        Ok(resolved)
    }

    /// `S⁻¹`, row-major. Gauss-Jordan with partial pivoting, in `f64`.
    fn inverse(&self) -> Result<Vec<f64>> {
        let n = self.channels.len();
        let mut a = self.values.clone();
        let mut inv: Vec<f64> = (0..n * n)
            .map(|k| if k / n == k % n { 1.0 } else { 0.0 })
            .collect();
        for col in 0..n {
            let pivot = (col..n)
                .max_by(|&x, &y| a[x * n + col].abs().total_cmp(&a[y * n + col].abs()))
                .expect("col < n");
            if a[pivot * n + col].abs() < 1e-12 {
                return Err(anyhow!(
                    "the matrix cannot be inverted: no combination of the channels separates {}",
                    self.channels[col]
                ));
            }
            if pivot != col {
                for k in 0..n {
                    a.swap(pivot * n + k, col * n + k);
                    inv.swap(pivot * n + k, col * n + k);
                }
            }
            let p = a[col * n + col];
            for k in 0..n {
                a[col * n + k] /= p;
                inv[col * n + k] /= p;
            }
            for row in (0..n).filter(|&r| r != col) {
                let f = a[row * n + col];
                if f != 0.0 {
                    for k in 0..n {
                        a[row * n + k] -= f * a[col * n + k];
                        inv[row * n + k] -= f * inv[col * n + k];
                    }
                }
            }
        }
        Ok(inv)
    }
}

/// How two matrices differ. See [`Spillover::compare`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Comparison {
    /// How many channels both matrices name.
    pub shared: usize,
    /// The largest difference between two entries, in percentage points.
    pub largest_difference: f64,
    /// Whether the other matrix, read with rows and columns swapped, is
    /// closer.
    pub transposed_fits_better: bool,
}

/// Which of `channels` is `name`, or why none is.
fn find_channel(name: &str, channels: &[(&str, Option<&str>)]) -> Result<usize, String> {
    let with_area = format!("{name}-A");
    let ways: [&dyn Fn(&(&str, Option<&str>)) -> bool; 4] = [
        &|(n, _)| *n == name,
        &|(n, _)| n.eq_ignore_ascii_case(name),
        &|(n, _)| n.eq_ignore_ascii_case(&with_area),
        &|(_, label)| label.is_some_and(|l| l.eq_ignore_ascii_case(name)),
    ];
    for matches in ways {
        let found: Vec<usize> = (0..channels.len())
            .filter(|&i| matches(&channels[i]))
            .collect();
        match found.as_slice() {
            [one] => return Ok(*one),
            [] => continue,
            several => {
                return Err(format!(
                    "{name} could be any of {}",
                    several
                        .iter()
                        .map(|&i| channels[i].0)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }
    Err(format!("the file has no channel {name}"))
}

/// One CSV line's cells, trimmed, with any quotes around a cell taken off.
fn split_csv_line(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => cells.push(std::mem::take(&mut cell).trim().to_string()),
            c => cell.push(c),
        }
    }
    cells.push(cell.trim().to_string());
    cells
}

/// `frame` with the matrix's channels compensated: every other column, and
/// the rows, as they were. `labels` gives a column's marker label, if it has
/// one, for matching a matrix that names channels by marker.
///
/// An error, and nothing compensated, if a channel is missing or the matrix
/// cannot be inverted.
pub fn compensate(
    frame: &DataFrame,
    matrix: &Spillover,
    labels: impl Fn(&str) -> Option<Arc<str>>,
) -> Result<DataFrame> {
    let names: Vec<(String, Option<Arc<str>>)> = frame
        .get_column_names()
        .iter()
        .map(|n| (n.to_string(), labels(n.as_str())))
        .collect();
    let lookup: Vec<(&str, Option<&str>)> = names
        .iter()
        .map(|(n, l)| (n.as_str(), l.as_deref()))
        .collect();
    // Only the channels that take part: see [`Spillover::involved`].
    let Some(matrix) = matrix.involved() else {
        return Ok(frame.clone());
    };
    let at = matrix.resolve(&lookup)?;
    let columns: Vec<&str> = at.iter().map(|&i| lookup[i].0).collect();
    compensate_columns(frame, &matrix, &columns)
}

/// Each file's own matrix, read from the keywords already loaded with it.
pub fn own_matrices(
    stubs: &[crate::file_load::FcsSampleStub],
) -> Vec<(
    std::path::PathBuf,
    std::result::Result<Option<Spillover>, String>,
)> {
    stubs
        .iter()
        .map(|stub| {
            (
                stub.get_filepath().to_path_buf(),
                Spillover::from_keywords(&stub.metadata).map_err(|e| e.to_string()),
            )
        })
        .collect()
}

/// A file's channels as [`Spillover::resolve`] takes them: its `$PnN`s, with
/// the `$PnS` label where it has one.
pub fn channels_of(stub: &crate::file_load::FcsSampleStub) -> Vec<(String, Option<String>)> {
    let mut parameters: Vec<&flow_fcs::Parameter> = stub.parameters.values().collect();
    parameters.sort_by_key(|p| p.parameter_number);
    parameters
        .into_iter()
        .map(|p| {
            let label = (p.label_name != p.channel_name).then(|| p.label_name.to_string());
            (p.channel_name.to_string(), label)
        })
        .collect()
}

/// Whether Omiq wrote the file: it signs its exports `WRITTEN_BY`
/// `OMIQ (www.omiq.ai)`. Omiq applies its compensation to the events when it
/// exports them and writes no `$SPILLOVER`, so an Omiq export is already
/// compensated as it was in Omiq, if it was at all.
pub fn written_by_omiq(metadata: &Metadata) -> bool {
    ["$WRITTEN_BY", "WRITTEN_BY"].iter().any(|k| {
        matches!(metadata.keywords.get(*k), Some(Keyword::String(v)) if v.get_str().to_ascii_uppercase().contains("OMIQ"))
    })
}

/// What [`groups::Compensation::check`] needs to know of a file.
pub fn facts_of(stub: &crate::file_load::FcsSampleStub) -> groups::FileFacts {
    groups::FileFacts {
        channels: channels_of(stub),
        written_by_omiq: written_by_omiq(&stub.metadata),
    }
}

/// What a file is to be compensated with: nothing, a matrix, or why it
/// cannot be - see [`groups::Compensation::matrix_for`].
pub type Choice = std::result::Result<Option<Arc<Spillover>>, String>;

/// Open a file and compensate its events as `choice` says: how every plot,
/// gallery image and rules run reads a file, so they cannot disagree.
///
/// A file that cannot be compensated as chosen is an error, not a file read
/// uncompensated: drawn or measured that way it would look like a result.
pub fn open_compensated(path: &std::path::Path, choice: &Choice) -> Result<flow_fcs::Fcs> {
    let matrix = choice
        .as_ref()
        .map_err(|why| anyhow!("it cannot be compensated: {why}"))?;
    let mut fcs = flow_fcs::Fcs::open(
        path.to_str()
            .ok_or_else(|| anyhow!("file path is not valid UTF-8"))?,
    )?;
    if let Some(matrix) = matrix {
        compensate_fcs(&mut fcs, matrix).map_err(|e| anyhow!("it cannot be compensated: {e}"))?;
    }
    Ok(fcs)
}

/// An opened file with its events compensated, in place; its keywords and
/// parameters are unchanged. Marker labels come from its `$PnS`.
pub fn compensate_fcs(fcs: &mut flow_fcs::Fcs, matrix: &Spillover) -> Result<()> {
    let labels: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = fcs
        .parameters
        .values()
        .filter(|p| p.label_name != p.channel_name)
        .map(|p| {
            (
                Arc::from(p.channel_name.as_ref()),
                Arc::from(p.label_name.as_ref()),
            )
        })
        .collect();
    let compensated = compensate(&fcs.data_frame, matrix, |c| labels.get(c).cloned())?;
    fcs.data_frame = Arc::new(compensated);
    Ok(())
}

/// [`compensate`], with the matrix's channels already found: `columns[i]` is
/// the column for the matrix's channel `i`.
pub fn compensate_columns(
    frame: &DataFrame,
    matrix: &Spillover,
    columns: &[&str],
) -> Result<DataFrame> {
    let n = matrix.channels.len();
    if columns.len() != n {
        return Err(anyhow!("{} columns for {n} channels", columns.len()));
    }
    let inverse = matrix.inverse()?;
    let observed: Vec<Vec<f32>> = columns
        .iter()
        .map(|&c| -> Result<Vec<f32>> {
            let column = frame.column(c)?.cast(&DataType::Float32)?;
            Ok(column
                .f32()?
                .into_iter()
                .map(|v| v.unwrap_or(f32::NAN))
                .collect())
        })
        .collect::<Result<_>>()?;
    let events = frame.height();

    let compensated: Vec<Vec<f32>> = (0..n)
        .into_par_iter()
        .map(|k| {
            let mut out = vec![0.0f32; events];
            for (j, column) in observed.iter().enumerate() {
                let coefficient = inverse[j * n + k] as f32;
                if coefficient == 0.0 {
                    continue;
                }
                for (o, &v) in out.iter_mut().zip(column) {
                    *o += coefficient * v;
                }
            }
            out
        })
        .collect();

    let mut frame = frame.clone();
    for (values, &name) in compensated.into_iter().zip(columns) {
        frame.replace(name, Column::new(name.into(), values))?;
    }
    Ok(frame)
}
