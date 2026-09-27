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
            return Err(anyhow!("No channels"));
        }
        if values.len() != n * n {
            return Err(anyhow!(
                "{n} channels need {} values, found {}",
                n * n,
                values.len()
            ));
        }
        for (i, name) in channels.iter().enumerate() {
            if name.trim().is_empty() {
                return Err(anyhow!("Column {} has no channel name", i + 1));
            }
            if channels[..i].contains(name) {
                return Err(anyhow!("{name} is listed twice"));
            }
        }
        if let Some(at) = values.iter().position(|v| !v.is_finite()) {
            return Err(anyhow!(
                "{} → {} isn't a number",
                channels[at / n],
                channels[at % n]
            ));
        }
        for (i, name) in channels.iter().enumerate() {
            let d = values[i * n + i];
            if (d - 1.0).abs() > DIAGONAL_TOLERANCE {
                return Err(anyhow!(
                    "{name} → {name} should be 100%, found {}%",
                    d * 100.0
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
            .map_err(|e| anyhow!("its {keyword} matrix is invalid: {e}")),
            // BD writes `SPILL` without the `$`, which flow keeps as text.
            Keyword::String(text) => Self::from_keyword_text(&text.get_str())
                .map(Some)
                .map_err(|e| anyhow!("its {keyword} matrix is invalid: {e}")),
            _ => Err(anyhow!("its {keyword} matrix can't be read")),
        }
    }

    /// A matrix in the standard's keyword form: the channel count, the
    /// channel names, then the values row by row, all separated by commas.
    fn from_keyword_text(text: &str) -> Result<Self> {
        let parts: Vec<&str> = text.split(',').map(str::trim).collect();
        let n: usize = parts
            .first()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| anyhow!("it doesn't start with a channel count"))?;
        if parts.len() != 1 + n + n * n {
            return Err(anyhow!(
                "{n} channels need {} entries after the count, found {}",
                n + n * n,
                parts.len() - 1
            ));
        }
        let values = parts[1 + n..]
            .iter()
            .map(|v| {
                v.parse::<f64>()
                    .map_err(|_| anyhow!("'{v}' isn't a number"))
            })
            .collect::<Result<Vec<_>>>()?;
        Self::new(parts[1..=n].iter().map(|&c| Arc::from(c)).collect(), values)
    }

    /// A matrix exported from Omiq as CSV. See [`Spillover::from_omiq_text`].
    pub fn from_omiq_csv(text: &str) -> Result<Self> {
        Self::from_omiq_text(text)
    }

    /// A matrix as copied out of Omiq and pasted, or saved as CSV: the
    /// channel names across the top, then one row per channel in the same
    /// order, in percent (or as fractions, with 1 on the diagonal).
    ///
    /// Tab-separated, as a paste from Omiq or a spreadsheet is, or comma-
    /// separated. Rows may start with their channel's name - with the corner
    /// cell empty or labelled, as Omiq's view has "Features" there - as long
    /// as they are in the order the top row gives.
    pub fn from_omiq_text(text: &str) -> Result<Self> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let lines: Vec<&str> = text
            .lines()
            .map(|l| l.trim_end_matches(['\r', '\n']))
            .filter(|line| !line.trim().is_empty())
            .collect();
        let first = *lines.first().ok_or_else(|| anyhow!("Nothing to read"))?;
        let separator = if first.contains('\t') { '\t' } else { ',' };
        let mut rows = lines.iter().map(|l| split_line(l, separator));
        let mut header = rows.next().expect("not empty");
        // A paste can carry a trailing separator on every line.
        let trailing = header.len() > 1 && header.last().is_some_and(|c| c.is_empty());
        if trailing {
            header.pop();
        }
        let rows: Vec<Vec<String>> = rows
            .map(|mut r| {
                if trailing && r.last().is_some_and(|c| c.is_empty()) {
                    r.pop();
                }
                r
            })
            .collect();

        // Rows labelled with their channel: every row starts with a name,
        // not a number.
        let is_number = |cell: &str| cell.trim_end_matches('%').trim().parse::<f64>().is_ok();
        let row_names = !rows.is_empty()
            && rows.iter().all(|r| {
                r.first()
                    .is_some_and(|c| !c.trim().is_empty() && !is_number(c))
            });
        let names: Vec<&str> = header
            .iter()
            // The corner cell, when the header has one above the names.
            .skip(usize::from(row_names && rows[0].len() == header.len()))
            .map(String::as_str)
            .collect();
        let n = names.len();
        if rows.is_empty() {
            return Err(anyhow!(
                "Only the channel names were found - paste the whole matrix, names and values"
            ));
        }
        if rows.len() != n {
            return Err(anyhow!(
                "The top row names {n} channels but there are {} rows of values - paste the whole matrix",
                rows.len()
            ));
        }

        let mut values = Vec::with_capacity(n * n);
        for (i, row) in rows.iter().enumerate() {
            let cells: &[String] = if row_names {
                if row[0] != names[i] {
                    return Err(anyhow!(
                        "Row {} is labelled {}, but the header has {} in that position",
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
                    "Row {} ({}) has {} values, expected {n}",
                    i + 1,
                    names[i],
                    cells.len()
                ));
            }
            for (j, cell) in cells.iter().enumerate() {
                let value: f64 = cell.trim_end_matches('%').trim().parse().map_err(|_| {
                    anyhow!("'{cell}' in {} → {} isn't a number", names[i], names[j])
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
                "The diagonal should be 100 throughout, but {} → {} is {d}",
                names[i],
                names[i]
            ));
        }
        Self::new(names.into_iter().map(Arc::from).collect(), values)
    }

    /// No compensation over `channels`: 1 on the diagonal, 0 elsewhere.
    pub fn identity(channels: &[Arc<str>]) -> Result<Self> {
        let n = channels.len();
        Self::new(
            channels.to_vec(),
            (0..n * n)
                .map(|k| if k / n == k % n { 1.0 } else { 0.0 })
                .collect(),
        )
    }

    /// This matrix with one entry changed: `fraction` of `from`'s signal
    /// showing up in `into`.
    pub fn with_value(&self, from: &str, into: &str, fraction: f64) -> Result<Self> {
        let at = |name: &str| {
            self.channels
                .iter()
                .position(|c| c.as_ref() == name)
                .ok_or_else(|| anyhow!("the matrix has no channel {name}"))
        };
        let (i, j) = (at(from)?, at(into)?);
        if i == j {
            return Err(anyhow!("a channel's spillover into itself is 1"));
        }
        let mut values = self.values.clone();
        values[i * self.channels.len() + j] = fraction;
        Self::new(self.channels.clone(), values)
    }

    /// The matrix as Omiq shows it, as CSV: the channel names across the top
    /// row, then one row per channel in the same order with no names, in
    /// percent. What [`Spillover::from_omiq_csv`] reads, so it round-trips.
    pub fn to_omiq_csv(&self) -> String {
        write_grid(&self.channels, &self.values, ',')
    }

    /// The same as tab-separated rows, as a spreadsheet or Omiq's
    /// compensation view takes a paste.
    pub fn to_omiq_paste(&self) -> String {
        write_grid(&self.channels, &self.values, '\t')
    }

    /// [`Spillover::from_omiq_csv`], from a file.
    pub fn read_omiq_csv(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("Can't read {}: {e}", path.display()))?;
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
                    "The matrix can't be applied: {} can't be separated from the other channels",
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

/// What to apply to events already compensated with `applied` to end with
/// them compensated with `wanted`, in the same layout as a matrix: the
/// channels of both, and `wanted · applied⁻¹` over them, row-major, as
/// fractions. For software that will compensate files Omiq has already
/// compensated - applying this to them is applying `wanted` to the events as
/// recorded.
///
/// Not always a spillover matrix: where the two differ in both directions
/// between a pair of channels, its diagonal is not exactly 1.
pub fn residual(wanted: &Spillover, applied: &Spillover) -> Result<(Vec<Arc<str>>, Vec<f64>)> {
    let mut channels: Vec<Arc<str>> = wanted.channels.clone();
    for c in &applied.channels {
        if !channels.contains(c) {
            channels.push(c.clone());
        }
    }
    let n = channels.len();
    let over = |m: &Spillover| -> Spillover {
        let index: Vec<Option<usize>> = channels
            .iter()
            .map(|c| m.channels.iter().position(|x| x == c))
            .collect();
        let values = (0..n * n)
            .map(|k| match (index[k / n], index[k % n]) {
                (Some(i), Some(j)) => m.value(i, j),
                _ if k / n == k % n => 1.0,
                _ => 0.0,
            })
            .collect();
        Spillover {
            channels: channels.clone(),
            values,
        }
    };
    let (t, a) = (over(wanted), over(applied));
    let a_inverse = a.inverse()?;
    let values = (0..n * n)
        .map(|k| {
            let (i, j) = (k / n, k % n);
            (0..n)
                .map(|m| t.values[i * n + m] * a_inverse[m * n + j])
                .sum()
        })
        .collect();
    Ok((channels, values))
}

/// A matrix in Omiq's layout, separated by `separator`: names across the
/// top, then the rows in percent.
pub fn write_grid(channels: &[Arc<str>], values: &[f64], separator: char) -> String {
    let n = channels.len();
    let cell = |name: &str| {
        if name.contains(separator) || name.contains('"') {
            format!("\"{}\"", name.replace('"', "\"\""))
        } else {
            name.to_string()
        }
    };
    let mut out = channels
        .iter()
        .map(|c| cell(c))
        .collect::<Vec<_>>()
        .join(&separator.to_string());
    out.push('\n');
    for i in 0..n {
        let row: Vec<String> = (0..n).map(|j| percent(values[i * n + j])).collect();
        out.push_str(&row.join(&separator.to_string()));
        out.push('\n');
    }
    out
}

/// A fraction as a percentage, as short as it can be without losing what a
/// person could have typed: 0.1 is `10`, 0.0253 is `2.53`.
fn percent(fraction: f64) -> String {
    let p = fraction * 100.0;
    let rounded = (p * 1e6).round() / 1e6;
    let text = format!("{rounded:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_string()
    } else {
        text.to_string()
    }
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
                    "{name} matches more than one channel: {}",
                    several
                        .iter()
                        .map(|&i| channels[i].0)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }
    Err(format!("no channel {name}"))
}

/// One line's cells, split at `separator`, trimmed, with any quotes around
/// a cell taken off.
fn split_line(line: &str, separator: char) -> Vec<String> {
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
            c if c == separator && !quoted => {
                cells.push(std::mem::take(&mut cell).trim().to_string())
            }
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
pub fn own_matrices(stubs: &[crate::file_load::FcsSampleStub]) -> Vec<groups::FileMatrix> {
    stubs
        .iter()
        .map(|stub| groups::FileMatrix {
            path: stub.get_filepath().to_path_buf(),
            own: Spillover::from_keywords(&stub.metadata).map_err(|e| e.to_string()),
            written_by_omiq: written_by_omiq(&stub.metadata),
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

/// A file's fluorescence channels, in order: every channel but scatter and
/// time - the channels Omiq's compensation matrix lists. What a matrix made
/// here from nothing is over.
pub fn fluorescence_channels(stub: &crate::file_load::FcsSampleStub) -> Vec<Arc<str>> {
    channels_of(stub)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| {
            let upper = name.to_ascii_uppercase();
            !(upper.starts_with("FSC") || upper.starts_with("SSC") || upper.starts_with("TIME"))
        })
        .map(|name| Arc::from(name.as_str()))
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
    }
}

/// What reading a file does to its events - see [`groups::Correction`] - or
/// why it cannot be compensated: [`groups::Compensation::matrix_for`].
pub type Choice = std::result::Result<groups::Correction, String>;

/// Open a file and compensate its events as `choice` says: how every plot,
/// gallery image and rules run reads a file, so they cannot disagree.
///
/// A file that cannot be compensated as chosen is an error, not a file read
/// uncompensated: drawn or measured that way it would look like a result.
pub fn open_compensated(path: &std::path::Path, choice: &Choice) -> Result<flow_fcs::Fcs> {
    let correction = choice
        .as_ref()
        .map_err(|why| anyhow!("it can't be compensated: {why}"))?;
    let mut fcs = flow_fcs::Fcs::open(
        path.to_str()
            .ok_or_else(|| anyhow!("file path is not valid UTF-8"))?,
    )?;
    correct_fcs(&mut fcs, correction).map_err(|e| anyhow!("it can't be compensated: {e}"))?;
    Ok(fcs)
}

/// Take `correction.undo` back out of an opened file's events, then apply
/// `correction.apply` - see [`groups::Correction`].
pub fn correct_fcs(fcs: &mut flow_fcs::Fcs, correction: &groups::Correction) -> Result<()> {
    if let Some(applied) = &correction.undo {
        spill_fcs(fcs, applied)?;
    }
    if let Some(wanted) = &correction.apply {
        compensate_fcs(fcs, wanted)?;
    }
    Ok(())
}

/// Put the spillover back into events compensated with `matrix`: each
/// channel `k` becomes `Σⱼ eⱼ Sⱼₖ` - the events as the cytometer recorded
/// them, from events compensated with `matrix`. The inverse of
/// [`compensate_fcs`].
pub fn spill_fcs(fcs: &mut flow_fcs::Fcs, matrix: &Spillover) -> Result<()> {
    let Some(matrix) = matrix.involved() else {
        return Ok(());
    };
    let (frame, columns) = locate(fcs, &matrix)?;
    let spilt = mix_columns(&frame, &matrix.values, &columns)?;
    fcs.data_frame = Arc::new(spilt);
    Ok(())
}

/// The file's frame, and the column for each of the matrix's channels.
fn locate(fcs: &flow_fcs::Fcs, matrix: &Spillover) -> Result<(DataFrame, Vec<String>)> {
    let labels: rustc_hash::FxHashMap<&str, &str> = fcs
        .parameters
        .values()
        .filter(|p| p.label_name != p.channel_name)
        .map(|p| (p.channel_name.as_ref(), p.label_name.as_ref()))
        .collect();
    let names: Vec<String> = fcs
        .data_frame
        .get_column_names()
        .iter()
        .map(|n| n.to_string())
        .collect();
    let lookup: Vec<(&str, Option<&str>)> = names
        .iter()
        .map(|n| (n.as_str(), labels.get(n.as_str()).copied()))
        .collect();
    let at = matrix.resolve(&lookup)?;
    Ok((
        (*fcs.data_frame).clone(),
        at.iter().map(|&i| names[i].clone()).collect(),
    ))
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
    let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
    mix_columns(frame, &inverse, &columns)
}

/// `frame` with `columns` replaced by their mix under `coefficients`
/// (row-major, one row and one column per entry of `columns`): each new
/// column `k` is `Σⱼ columnⱼ · coefficients[j][k]`. Every other column, and
/// the rows, as they were.
fn mix_columns(frame: &DataFrame, coefficients: &[f64], columns: &[String]) -> Result<DataFrame> {
    let n = columns.len();
    if coefficients.len() != n * n {
        return Err(anyhow!(
            "{} coefficients for {n} columns",
            coefficients.len()
        ));
    }
    let observed: Vec<Vec<f32>> = columns
        .iter()
        .map(|c| -> Result<Vec<f32>> {
            let column = frame.column(c)?.cast(&DataType::Float32)?;
            Ok(column
                .f32()?
                .into_iter()
                .map(|v| v.unwrap_or(f32::NAN))
                .collect())
        })
        .collect::<Result<_>>()?;
    let events = frame.height();

    let mixed: Vec<Vec<f32>> = (0..n)
        .into_par_iter()
        .map(|k| {
            let mut out = vec![0.0f32; events];
            for (j, column) in observed.iter().enumerate() {
                let coefficient = coefficients[j * n + k] as f32;
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
    for (values, name) in mixed.into_iter().zip(columns) {
        frame.replace(name, Column::new(name.as_str().into(), values))?;
    }
    Ok(frame)
}
