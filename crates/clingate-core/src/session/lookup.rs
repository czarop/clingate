//! Finding what a question names: samples by their file name or any metadata
//! value, populations by the words of their gate path, parameters by marker or
//! channel.
//!
//! Matching is strict. A word has to be one of the words of a name or value -
//! `fmx` finds `..._FMX_Plate_10.fcs` and a `SampleType` of `FMX`, but `qc` does
//! not find `QC4`, and `cd4` does not find `CD4+`. Anything short of that is a
//! [`Clarification`]: what was asked, why it did not match, and what might have
//! been meant. The suggestions are for whoever asked to choose from; nothing
//! here ever acts on one.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// What to ask, when a name matched nothing - or more than one thing where
/// one was needed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Clarification {
    /// What was being looked for: "samples", "population", "parameter" or
    /// "compensation group".
    pub about: &'static str,
    /// The words as they were given.
    pub asked: String,
    pub problem: String,
    /// What might have been meant. Offered, never chosen.
    pub suggestions: Vec<String>,
}

impl Clarification {
    fn new(about: &'static str, asked: &str, problem: String, suggestions: Vec<String>) -> Self {
        Self {
            about,
            asked: asked.to_string(),
            problem,
            suggestions,
        }
    }
}

/// The words of a file name or metadata value, lowercased: runs of letters
/// and digits, split on everything else. `[Plate_10] B6 002911_WK1_FMX.fcs` is
/// `plate`, `10`, `b6`, `002911`, `wk1`, `fmx`, `fcs`.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The markers of a gate name, lowercased, each with its sign: `CD3+CD56-` is
/// `cd3+` and `cd56-`, `CD279+ AND Ki67+` is `cd279+`, `and`, `ki67+`, and
/// `Va7.2+` keeps its point. The sign is part of the word - `CD4+` and `CD4-`
/// are different populations. A hyphen before a digit is part of the name
/// (`IL-22+` is one word); before a letter it is a sign (`CD197-CD45RA+`).
pub fn marker_words(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let point_in_number = c == '.'
            && i > 0
            && chars[i - 1].is_ascii_digit()
            && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        let hyphen_in_name =
            c == '-' && !current.is_empty() && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        if c.is_alphanumeric() || point_in_number || hyphen_in_name {
            current.extend(c.to_lowercase());
        } else if (c == '+' || c == '-') && !current.is_empty() {
            // The sign, and any more signs straight after it (`CD4++`), end
            // the word.
            current.push(c);
            while let Some(&next) = chars.get(i + 1) {
                if next == '+' || next == '-' {
                    current.push(next);
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(std::mem::take(&mut current));
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        i += 1;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Whether `known` is near enough to `wanted` to be worth suggesting: one
/// contains the other, or they are one edit apart.
fn near(wanted: &str, known: &str) -> bool {
    if wanted == known {
        return false;
    }
    (known.contains(wanted) && !wanted.is_empty())
        || (wanted.contains(known) && known.len() > 1)
        || edit_distance(wanted, known) <= 1
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut row = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != cb);
            row.push(substitute.min(previous[j + 1] + 1).min(row[j] + 1));
        }
        previous = row;
    }
    previous[b.len()]
}

// ─── Samples ──────────────────────────────────────────────────────────────────

/// What a sample can be found by: its name in the program and its metadata.
#[derive(Debug, Clone)]
pub struct SampleFacts {
    pub name: String,
    /// Column and value, for each metadata column the sample has.
    pub metadata: Vec<(String, String)>,
}

/// The samples a question named, and what each word of it matched.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SampleMatch {
    /// Indices into the samples searched, in their order.
    #[serde(skip)]
    pub indices: Vec<usize>,
    /// For each word or `column=value` asked: where it was found, and in how
    /// many samples.
    pub matched_by: Vec<MatchedWord>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MatchedWord {
    pub word: String,
    /// "file name", or the metadata column it was found in.
    pub found_in: Vec<String>,
    pub samples: usize,
}

/// The samples `query` names: every sample in which each of its words is a
/// word of the file name or of a metadata value. A `column=value` item asks
/// for that column alone. An empty query, `all` or `*` is every sample.
pub fn match_samples(query: &str, samples: &[SampleFacts]) -> Result<SampleMatch, Clarification> {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed == "*" || trimmed.eq_ignore_ascii_case("all") {
        return Ok(SampleMatch {
            indices: (0..samples.len()).collect(),
            matched_by: Vec::new(),
        });
    }

    // Items separated by spaces or commas; `column=value` kept whole.
    let mut asked: Vec<(Option<String>, String)> = Vec::new();
    for item in trimmed.split(|c: char| c.is_whitespace() || c == ',') {
        if item.is_empty() {
            continue;
        }
        match item.split_once('=') {
            Some((column, value)) => {
                for word in words(value) {
                    asked.push((Some(column.trim().to_lowercase()), word));
                }
            }
            None => asked.extend(words(item).into_iter().map(|w| (None, w))),
        }
    }
    if asked.is_empty() {
        return Err(Clarification::new(
            "samples",
            query,
            "there are no words in this to look for".to_string(),
            Vec::new(),
        ));
    }

    let mut chosen: Option<BTreeSet<usize>> = None;
    let mut matched_by = Vec::new();
    let mut unmatched = Vec::new();
    for (column, word) in &asked {
        let mut hits = BTreeSet::new();
        let mut found_in = BTreeSet::new();
        for (i, sample) in samples.iter().enumerate() {
            if column.is_none() && words(&sample.name).contains(word) {
                hits.insert(i);
                found_in.insert("file name".to_string());
            }
            for (name, value) in &sample.metadata {
                if column.as_ref().is_none_or(|c| *c == name.to_lowercase())
                    && words(value).contains(word)
                {
                    hits.insert(i);
                    found_in.insert(name.clone());
                }
            }
        }
        let shown = match column {
            Some(column) => format!("{column}={word}"),
            None => word.clone(),
        };
        if hits.is_empty() {
            unmatched.push((column.clone(), word.clone(), shown));
            continue;
        }
        matched_by.push(MatchedWord {
            word: shown,
            found_in: found_in.into_iter().collect(),
            samples: hits.len(),
        });
        chosen = Some(match chosen {
            None => hits,
            Some(so_far) => so_far.intersection(&hits).copied().collect(),
        });
    }

    if !unmatched.is_empty() {
        let mut suggestions = Vec::new();
        for (column, word, _) in &unmatched {
            suggestions.extend(sample_suggestions(column.as_deref(), word, samples));
        }
        let shown: Vec<&str> = unmatched.iter().map(|(_, _, s)| s.as_str()).collect();
        return Err(Clarification::new(
            "samples",
            query,
            format!(
                "no sample's file name or metadata has the word {}",
                shown.join(", ")
            ),
            suggestions,
        ));
    }
    let indices: Vec<usize> = chosen.unwrap_or_default().into_iter().collect();
    if indices.is_empty() {
        return Err(Clarification::new(
            "samples",
            query,
            "each word matches some samples, but no sample has all of them".to_string(),
            matched_by
                .iter()
                .map(|m| {
                    format!(
                        "{} alone: {} sample(s), in {}",
                        m.word,
                        m.samples,
                        m.found_in.join(", ")
                    )
                })
                .collect(),
        ));
    }
    Ok(SampleMatch {
        indices,
        matched_by,
    })
}

/// Words near `word` in the samples' names and metadata, each with where it
/// is and how many samples have it.
fn sample_suggestions(column: Option<&str>, word: &str, samples: &[SampleFacts]) -> Vec<String> {
    // word -> (where, how many samples)
    let mut seen: BTreeMap<String, (BTreeSet<String>, BTreeSet<usize>)> = BTreeMap::new();
    for (i, sample) in samples.iter().enumerate() {
        if column.is_none() {
            for w in words(&sample.name) {
                if near(word, &w) {
                    let entry = seen.entry(w).or_default();
                    entry.0.insert("file name".to_string());
                    entry.1.insert(i);
                }
            }
        }
        for (name, value) in &sample.metadata {
            if column.is_some_and(|c| c != name.to_lowercase()) {
                continue;
            }
            for w in words(value) {
                if near(word, &w) {
                    let entry = seen.entry(w).or_default();
                    entry.0.insert(name.clone());
                    entry.1.insert(i);
                }
            }
        }
    }
    let mut out: Vec<String> = seen
        .into_iter()
        .map(|(w, (places, hits))| {
            format!(
                "{w} ({} sample(s), in {})",
                hits.len(),
                places.into_iter().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    if let Some(column) = column
        && !samples
            .iter()
            .any(|s| s.metadata.iter().any(|(n, _)| n.to_lowercase() == column))
    {
        let mut columns: BTreeSet<&str> = BTreeSet::new();
        for s in samples {
            columns.extend(s.metadata.iter().map(|(n, _)| n.as_str()));
        }
        out.push(format!(
            "there is no metadata column {column}; the columns are {}",
            columns.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    out.truncate(12);
    out
}

// ─── Populations ──────────────────────────────────────────────────────────────

/// How a path is written: `CD4+ > CD279+`. Not `/`, which Omiq puts inside
/// quadrant names (`Q1 IL-22- / IL-17A+`).
pub const PATH_SEPARATOR: &str = " > ";

/// A population as it can be found: the names of the gates from the root down
/// to it, its own last, and what to call it when offering it as a choice.
#[derive(Debug, Clone)]
pub struct PopulationFacts {
    pub path: Vec<String>,
    /// As short as it can be while naming only this population. Set by
    /// [`label_populations`]; the full path until then.
    pub label: String,
}

impl PopulationFacts {
    pub fn new(path: Vec<String>) -> Self {
        let label = path.join(PATH_SEPARATOR);
        Self { path, label }
    }

    pub fn full_path(&self) -> String {
        self.path.join(PATH_SEPARATOR)
    }
}

/// Give each population the shortest end of its path that no other
/// population's path also ends with - `CD4+`, or `CD197-CD45RA- > CD279+`
/// where `CD279+` alone is several. Two populations with the very same path
/// (the corners of a quadrant drawn twice) cannot be told apart by name, so
/// each is given `extra(i)` as well.
pub fn label_populations(populations: &mut [PopulationFacts], extra: impl Fn(usize) -> String) {
    let paths: Vec<Vec<String>> = populations.iter().map(|p| p.path.clone()).collect();
    for (i, p) in populations.iter_mut().enumerate() {
        let path = &paths[i];
        let mut depth = 1;
        let ends_like = |depth: usize, other: &Vec<String>| {
            other.len() >= depth && other[other.len() - depth..] == path[path.len() - depth..]
        };
        while depth < path.len()
            && paths
                .iter()
                .enumerate()
                // A twin ends like it at every depth; it is told apart below.
                .any(|(j, other)| j != i && *other != *path && ends_like(depth, other))
        {
            depth += 1;
        }
        let mut label = path[path.len() - depth..].join(PATH_SEPARATOR);
        if paths
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && *other == *path)
        {
            label = format!("{label} ({})", extra(i));
        }
        p.label = label;
    }
}

/// The populations `query` names.
///
/// Without a `/`, every marker of the query has to be in the population's path
/// and at least one in its own name, so `cd4+` finds the gate named `CD4+`
/// rather than everything under it; and `cd4+ cd279+` finds `CD279+` under
/// `CD4+`. Where some of those found are named by the query and nothing more
/// (`CD4+` for `cd4+`) and others carry more (`CD4+CD8-`), only the first are
/// kept. With `>`, each part names a gate on the way down, in order, the last
/// the population itself: `CD4+ > CD279+`.
///
/// A name given out by [`label_populations`], or a population's full path,
/// always finds that one population, first: the shortest unique name says
/// `CD4+ > CD279+` for the `CD279+` directly under `CD4+`, where the looser
/// reading below would also find a `CD279+` further down, and a twin's name
/// carries an id no gate name has.
///
/// Several populations can answer; it is for the caller to say whether that is
/// a list or a question.
pub fn match_populations(
    query: &str,
    populations: &[PopulationFacts],
) -> Result<Vec<usize>, Clarification> {
    let given = query.trim();
    let named: Vec<usize> = (0..populations.len())
        .filter(|&i| {
            populations[i].label.eq_ignore_ascii_case(given)
                || populations[i].full_path().eq_ignore_ascii_case(given)
        })
        .collect();
    if !named.is_empty() {
        return Ok(named);
    }
    let segments: Vec<Vec<String>> = query
        .split('>')
        .map(marker_words)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Err(Clarification::new(
            "population",
            query,
            "there are no gate names in this to look for".to_string(),
            Vec::new(),
        ));
    }

    let tokens: Vec<Vec<Vec<String>>> = populations
        .iter()
        .map(|p| p.path.iter().map(|n| marker_words(n)).collect())
        .collect();
    let every_word: BTreeSet<&str> = tokens
        .iter()
        .flatten()
        .flatten()
        .map(String::as_str)
        .collect();

    // A word no gate has anywhere: nothing to match, only to suggest.
    let unknown: Vec<&String> = segments
        .iter()
        .flatten()
        .filter(|w| !every_word.contains(w.as_str()))
        .collect();
    if !unknown.is_empty() {
        let mut suggestions: BTreeSet<String> = BTreeSet::new();
        for word in &unknown {
            for (p, path_tokens) in populations.iter().zip(&tokens) {
                let own = path_tokens.last().expect("a population has a name");
                if own.iter().any(|w| near(word, w)) {
                    suggestions.insert(p.label.clone());
                }
            }
        }
        return Err(Clarification::new(
            "population",
            query,
            format!(
                "no gate is named with {}",
                unknown
                    .iter()
                    .map(|w| w.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            suggestions.into_iter().take(12).collect(),
        ));
    }

    let found: Vec<usize> = (0..populations.len())
        .filter(|&i| {
            let path = &tokens[i];
            let own = path.last().expect("a population has a name");
            if segments.len() == 1 {
                let wanted = &segments[0];
                let in_path = |w: &String| path.iter().any(|names| names.contains(w));
                wanted.iter().all(in_path) && wanted.iter().any(|w| own.contains(w))
            } else {
                // The last part names the population; the others, in order,
                // gates above it.
                let (last, above) = segments.split_last().expect("not empty");
                if !last.iter().all(|w| own.contains(w)) {
                    return false;
                }
                let ancestors = &path[..path.len() - 1];
                let mut at = 0;
                for part in above {
                    match ancestors[at..]
                        .iter()
                        .position(|names| part.iter().all(|w| names.contains(w)))
                    {
                        Some(offset) => at += offset + 1,
                        None => return false,
                    }
                }
                true
            }
        })
        .collect();

    if found.is_empty() {
        let last = segments.last().expect("not empty");
        let suggestions: BTreeSet<String> = populations
            .iter()
            .zip(&tokens)
            .filter(|(_, path)| {
                let own = path.last().expect("a population has a name");
                last.iter().any(|w| own.contains(w))
            })
            .map(|(p, _)| p.label.clone())
            .take(12)
            .collect();
        return Err(Clarification::new(
            "population",
            query,
            "no population is named like this in that place in the tree".to_string(),
            suggestions.into_iter().collect(),
        ));
    }

    // Prefer the populations the query names exactly over those whose names
    // say more.
    let asked: BTreeSet<&String> = segments.iter().flatten().collect();
    let exact: Vec<usize> = found
        .iter()
        .copied()
        .filter(|&i| {
            tokens[i]
                .last()
                .expect("a population has a name")
                .iter()
                .all(|w| asked.contains(w))
        })
        .collect();
    Ok(if exact.is_empty() { found } else { exact })
}

/// The one population `query` names, or a question listing the candidates.
pub fn one_population(
    query: &str,
    populations: &[PopulationFacts],
) -> Result<usize, Clarification> {
    let found = match_populations(query, populations)?;
    match found.as_slice() {
        [one] => Ok(*one),
        several => Err(Clarification::new(
            "population",
            query,
            format!(
                "{} populations match; say which, by more of its path",
                several.len()
            ),
            several
                .iter()
                .map(|&i| populations[i].label.clone())
                .take(20)
                .collect(),
        )),
    }
}

// ─── Parameters ───────────────────────────────────────────────────────────────

/// A parameter as it can be named: its marker and its channel.
#[derive(Debug, Clone)]
pub struct ParameterFacts {
    pub marker: String,
    pub channel: String,
}

/// The one parameter `query` names: its marker (`CD4`), its channel
/// (`BUV395-A`), the channel without its `-A`, or both as the app shows them
/// (`CD4-BUV395`) - ignoring case, and nothing looser.
pub fn one_parameter(query: &str, parameters: &[ParameterFacts]) -> Result<usize, Clarification> {
    let wanted = query.trim().to_lowercase();
    let names = |p: &ParameterFacts| -> Vec<String> {
        let channel = p.channel.to_lowercase();
        let bare = channel
            .strip_suffix("-a")
            .map(str::to_string)
            .unwrap_or_else(|| channel.clone());
        vec![
            p.marker.to_lowercase(),
            channel,
            bare.clone(),
            format!("{}-{bare}", p.marker.to_lowercase()),
        ]
    };
    let found: Vec<usize> = (0..parameters.len())
        .filter(|&i| names(&parameters[i]).contains(&wanted))
        .collect();
    let describe = |p: &ParameterFacts| {
        if p.marker == p.channel {
            p.channel.clone()
        } else {
            format!("{} ({})", p.marker, p.channel)
        }
    };
    match found.as_slice() {
        [one] => Ok(*one),
        [] => Err(Clarification::new(
            "parameter",
            query,
            "no marker or channel is called this".to_string(),
            parameters
                .iter()
                .filter(|p| names(p).iter().any(|n| near(&wanted, n)))
                .map(describe)
                .take(12)
                .collect(),
        )),
        several => Err(Clarification::new(
            "parameter",
            query,
            format!("{} parameters are called this; say which", several.len()),
            several.iter().map(|&i| describe(&parameters[i])).collect(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_is_split_into_words() {
        assert_eq!(
            words("[Plate_10] [002911_WK1] B6 002911_WK1_FMX_Plate_10.fcs"),
            [
                "plate", "10", "002911", "wk1", "b6", "002911", "wk1", "fmx", "plate", "10", "fcs"
            ]
        );
    }

    #[test]
    fn a_gate_name_is_split_into_markers_with_their_signs() {
        assert_eq!(marker_words("CD3+CD56-"), ["cd3+", "cd56-"]);
        assert_eq!(marker_words("CD279+ AND Ki67+"), ["cd279+", "and", "ki67+"]);
        assert_eq!(marker_words("NotCD161+Va7.2+"), ["notcd161+", "va7.2+"]);
        assert_eq!(marker_words("CD197+CD45RA-"), ["cd197+", "cd45ra-"]);
        assert_eq!(marker_words("Live"), ["live"]);
    }

    fn plate() -> Vec<SampleFacts> {
        let sample = |name: &str, kind: &str, id: &str| SampleFacts {
            name: name.to_string(),
            metadata: vec![
                ("SampleType".to_string(), kind.to_string()),
                ("Sample ID".to_string(), id.to_string()),
            ],
        };
        vec![
            sample("B6 002911_WK1_FMX_Plate_10.fcs", "FMX", "002911_WK1"),
            sample("C6 002911_WK1_FS_Plate_10.fcs", "FS", "002911_WK1"),
            sample("B4 002911_WK48_FMX_Plate_10.fcs", "FMX", "002911_WK48"),
            sample("C12 QC4_unstim_FS_Plate_10.fcs", "FS", "QC4"),
        ]
    }

    #[test]
    fn a_word_finds_samples_by_name_or_metadata() {
        let found = match_samples("fmx", &plate()).unwrap();
        assert_eq!(found.indices, [0, 2]);
        assert_eq!(found.matched_by[0].found_in, ["SampleType", "file name"]);
        assert_eq!(match_samples("fmx wk1", &plate()).unwrap().indices, [0]);
        assert_eq!(
            match_samples("SampleType=FS", &plate()).unwrap().indices,
            [1, 3]
        );
        assert_eq!(
            match_samples("all", &plate()).unwrap().indices,
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn a_near_miss_is_a_question_with_suggestions_not_a_match() {
        let asked = match_samples("qc", &plate()).unwrap_err();
        assert_eq!(asked.about, "samples");
        assert!(
            asked.suggestions.iter().any(|s| s.starts_with("qc4")),
            "{asked:?}"
        );
        let asked = match_samples("wk4", &plate()).unwrap_err();
        assert!(
            asked.suggestions.iter().any(|s| s.starts_with("wk48")),
            "{asked:?}"
        );
    }

    #[test]
    fn words_that_each_match_but_never_together_are_a_question() {
        let asked = match_samples("qc4 fmx", &plate()).unwrap_err();
        assert!(asked.problem.contains("no sample has all"), "{asked:?}");
    }

    #[test]
    fn an_unknown_column_is_named_as_such() {
        let asked = match_samples("Tube=FMX", &plate()).unwrap_err();
        assert!(
            asked
                .suggestions
                .iter()
                .any(|s| s.contains("no metadata column tube")),
            "{asked:?}"
        );
    }

    fn tree() -> Vec<PopulationFacts> {
        let at = |path: &[&str]| PopulationFacts::new(path.iter().map(|s| s.to_string()).collect());
        let mut tree = vec![
            at(&["Live"]),
            at(&["Live", "CD3+CD56-"]),
            at(&["Live", "CD3+CD56-", "CD4+"]),
            at(&["Live", "CD3+CD56-", "CD4+", "CD197+CD45RA+"]),
            at(&["Live", "CD3+CD56-", "CD4+", "CD197+CD45RA+", "CD279+"]),
            at(&["Live", "CD3+CD56-", "CD4+", "CD197-CD45RA-"]),
            at(&["Live", "CD3+CD56-", "CD4+", "CD197-CD45RA-", "CD279+"]),
            at(&["Live", "CD3+CD56-", "CD4+CD8-"]),
            at(&["Live", "Q1 IL-22- / IL-17A+"]),
            at(&["Live", "Q1 IL-22- / IL-17A+"]),
        ];
        label_populations(&mut tree, |i| format!("id {i}"));
        tree
    }

    #[test]
    fn a_population_is_found_by_its_own_name_not_its_descendants() {
        assert_eq!(match_populations("cd4+", &tree()).unwrap(), [2]);
        assert_eq!(one_population("CD4+", &tree()).unwrap(), 2);
    }

    #[test]
    fn a_name_used_in_two_places_is_a_question_listing_both() {
        let asked = one_population("cd279+", &tree()).unwrap_err();
        assert_eq!(
            asked.suggestions,
            ["CD197+CD45RA+ > CD279+", "CD197-CD45RA- > CD279+"],
        );
    }

    #[test]
    fn more_of_the_path_settles_it() {
        assert_eq!(
            one_population("CD197-CD45RA- > CD279+", &tree()).unwrap(),
            6
        );
        assert_eq!(one_population("cd45ra- cd279+", &tree()).unwrap(), 6);
    }

    #[test]
    fn a_quadrant_corner_is_found_by_its_markers_and_twins_are_told_apart() {
        assert_eq!(
            marker_words("Q1 IL-22- / IL-17A+"),
            ["q1", "il-22-", "il-17a+"]
        );
        let asked = one_population("q1 il-22- il-17a+", &tree()).unwrap_err();
        assert_eq!(
            asked.suggestions,
            ["Q1 IL-22- / IL-17A+ (id 8)", "Q1 IL-22- / IL-17A+ (id 9)"]
        );
    }

    #[test]
    fn every_name_given_out_finds_its_population_and_only_it() {
        // `CD279+` directly under `CD4+`, and more `CD279+`s further down
        // under `CD4+` - so `CD4+ > CD279+` is unique as a name's end but not
        // as a path read loosely - and twins, told apart by id.
        let at = |path: &[&str]| PopulationFacts::new(path.iter().map(|s| s.to_string()).collect());
        let mut tree = vec![
            at(&["Live", "CD4+"]),
            at(&["Live", "CD4+", "CD279+"]),
            at(&["Live", "CD4+", "NeTy"]),
            at(&["Live", "CD4+", "NeTy", "CD279+"]),
            at(&["Live", "CD4+", "NeTy"]),
            at(&["Live", "CD4+", "NeTy", "CD279+"]),
        ];
        label_populations(&mut tree, |i| format!("id:{i}"));
        assert_eq!(tree[1].label, "CD4+ > CD279+");
        for (i, p) in tree.iter().enumerate() {
            assert_eq!(one_population(&p.label, &tree).unwrap(), i, "{}", p.label);
            // A twin's full path is its twin's too: only the id tells them
            // apart, so the path alone is a question.
            let twinned = tree.iter().filter(|o| o.path == p.path).count() > 1;
            match one_population(&p.full_path(), &tree) {
                Ok(found) => assert!(!twinned && found == i, "{}", p.full_path()),
                Err(asked) => assert!(twinned, "{asked:?}"),
            }
            let padded = format!("  {}\t", p.label);
            assert_eq!(one_population(&padded, &tree).unwrap(), i, "{padded:?}");
            let shouted = p.label.to_uppercase();
            assert_eq!(one_population(&shouted, &tree).unwrap(), i, "{shouted}");
        }
    }

    #[test]
    fn a_marker_without_its_sign_is_a_question() {
        let asked = one_population("cd4", &tree()).unwrap_err();
        assert!(asked.problem.contains("cd4"), "{asked:?}");
        assert!(
            asked.suggestions.iter().any(|s| s.ends_with("CD4+")),
            "{asked:?}"
        );
    }

    #[test]
    fn a_parameter_is_found_by_marker_or_channel_and_nothing_looser() {
        let panel = vec![
            ParameterFacts {
                marker: "CD4".into(),
                channel: "BUV395-A".into(),
            },
            ParameterFacts {
                marker: "CD45".into(),
                channel: "Alexa Fluor 532-A".into(),
            },
        ];
        assert_eq!(one_parameter("cd4", &panel).unwrap(), 0);
        assert_eq!(one_parameter("BUV395-A", &panel).unwrap(), 0);
        assert_eq!(one_parameter("buv395", &panel).unwrap(), 0);
        assert_eq!(one_parameter("CD4-BUV395", &panel).unwrap(), 0);
        let asked = one_parameter("cd45ra", &panel).unwrap_err();
        assert!(
            asked.suggestions.iter().any(|s| s.starts_with("CD45")),
            "{asked:?}"
        );
    }
}
