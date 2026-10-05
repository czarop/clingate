//! How the rules position gates, in words and in code, for a reader - Claude
//! above all - working out with a person why a gate landed where it did and
//! how a rule could do better.
//!
//! Both are compiled in, so what is read is what the program runs: the
//! description in `docs/how-rules-position-gates.md`, and the source of the
//! modules that position gates, judge them and replay them. Nothing else of
//! the program is readable this way.

use serde::Serialize;

/// The description of how gates are positioned, reviewed and replayed.
pub const HOW_RULES_POSITION_GATES: &str =
    include_str!("../../../../docs/how-rules-position-gates.md");

/// One readable source file: its path under `crates/clingate-core/src`, what
/// it holds, and its text.
pub struct Source {
    pub path: &'static str,
    pub holds: &'static str,
    pub text: &'static str,
}

/// The files that decide where a gate goes, and how a placement is judged.
pub const SOURCES: &[Source] = &[
    Source {
        path: "gate_rules/rule.rs",
        holds: "the rule kinds and their parameters; above-the-negative calibrate and place; \
                the valley rule",
        text: include_str!("../gate_rules/rule.rs"),
    },
    Source {
        path: "gate_rules/rule_store.rs",
        holds: "rule targets, measured_on, the sample pairing, which rule applies to a gate, \
                which file a rule reads",
        text: include_str!("../gate_rules/rule_store.rs"),
    },
    Source {
        path: "gate_rules/autogate.rs",
        holds: "measuring a population, one file per specimen, position_one (every rule's \
                placement), slide_to_capture, confidence of a placement, phenotype placement",
        text: include_str!("../gate_rules/autogate.rs"),
    },
    Source {
        path: "gate_rules/threshold.rs",
        holds: "percentiles, the negative's centre and width, refining from the gate, the \
                valley finder",
        text: include_str!("../gate_rules/threshold.rs"),
    },
    Source {
        path: "gate_rules/confidence.rs",
        holds: "the confidence components and their limits",
        text: include_str!("../gate_rules/confidence.rs"),
    },
    Source {
        path: "gate_rules/phenotype.rs",
        holds: "describing a population by its markers and finding it in another sample",
        text: include_str!("../gate_rules/phenotype.rs"),
    },
    Source {
        path: "review/assess.rs",
        holds: "flagging placements unlike their peers, the typical peer",
        text: include_str!("assess.rs"),
    },
    Source {
        path: "review/shape.rs",
        holds: "a distribution's summary: percentiles, peaks, valley",
        text: include_str!("shape.rs"),
    },
    Source {
        path: "review/events.rs",
        holds: "the events a run keeps for replays",
        text: include_str!("events.rs"),
    },
    Source {
        path: "review/replay.rs",
        holds: "replaying reviewed runs with changed rules, and the verdicts",
        text: include_str!("replay.rs"),
    },
];

/// The most lines one read returns.
pub const MAX_LINES: usize = 400;

/// Part of one source file, numbered.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceText {
    pub path: &'static str,
    pub total_lines: usize,
    pub from: usize,
    pub to: usize,
    /// Each line prefixed with its number, as `{n}: {line}`.
    pub lines: Vec<String>,
    /// Where to carry on reading, when there is more.
    pub more_from: Option<usize>,
}

/// A line that matched a search.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Found {
    pub path: &'static str,
    pub line: usize,
    pub text: String,
}

/// What can be read, for a reader choosing.
pub fn contents() -> Vec<(&'static str, &'static str, usize)> {
    SOURCES
        .iter()
        .map(|s| (s.path, s.holds, s.text.lines().count()))
        .collect()
}

/// Lines `from` to `to` (1-based, inclusive) of one file - at most
/// [`MAX_LINES`] of them.
pub fn read(path: &str, from: Option<usize>, to: Option<usize>) -> Result<SourceText, String> {
    let wanted = path.trim().trim_start_matches("crates/clingate-core/src/");
    let source = SOURCES.iter().find(|s| s.path == wanted).ok_or_else(|| {
        format!(
            "{path} is not one of the files that can be read: {}",
            SOURCES
                .iter()
                .map(|s| s.path)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let all: Vec<&str> = source.text.lines().collect();
    let total = all.len();
    let from = from.unwrap_or(1).max(1);
    if from > total {
        return Err(format!("{} has {total} lines", source.path));
    }
    let asked_to = to.unwrap_or(total).min(total).max(from);
    let to = asked_to.min(from + MAX_LINES - 1);
    Ok(SourceText {
        path: source.path,
        total_lines: total,
        from,
        to,
        lines: (from..=to)
            .map(|n| format!("{n}: {}", all[n - 1]))
            .collect(),
        more_from: (to < asked_to).then_some(to + 1),
    })
}

/// Every line in the readable files containing `text`, ignoring case - at
/// most `limit` of them.
pub fn search(text: &str, limit: usize) -> Vec<Found> {
    let needle = text.to_lowercase();
    if needle.trim().is_empty() {
        return Vec::new();
    }
    SOURCES
        .iter()
        .flat_map(|s| {
            s.text
                .lines()
                .enumerate()
                .filter(|(_, l)| l.to_lowercase().contains(&needle))
                .map(|(i, l)| Found {
                    path: s.path,
                    line: i + 1,
                    text: l.trim().to_string(),
                })
                .collect::<Vec<_>>()
        })
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_is_the_document_in_the_repository() {
        assert!(HOW_RULES_POSITION_GATES.starts_with("# How gate rules position gates"));
        // It names the functions it describes, and they exist in the source
        // it can be checked against.
        for name in [
            "fn slide_to_capture",
            "fn bracket_for",
            "fn position_one",
            "fn gated_rank",
            "fn reference_file",
            "fn first_valley",
            "fn negative_peak",
            "fn refine_from",
            "fn typical_of",
            "fn replay_run",
        ] {
            assert!(
                !search(name, 1).is_empty(),
                "{name} is not in the readable source"
            );
            let bare = name.trim_start_matches("fn ");
            assert!(
                HOW_RULES_POSITION_GATES.contains(bare),
                "the description does not mention {bare}"
            );
        }
    }

    #[test]
    fn the_description_s_numbers_are_the_code_s() {
        use crate::gate_rules::confidence::ConfidenceLimits;
        let limits = ConfidenceLimits::default();
        let doc = HOW_RULES_POSITION_GATES;
        assert!(doc.contains(&format!("`events_full = {}`", limits.events_full)));
        assert!(doc.contains(&format!("`events_floor = {}`", limits.events_floor)));
        assert!(doc.contains(&format!("`swing_half = {}`", limits.swing_half)));
        assert!(doc.contains(&format!(
            "within {:.0}% of the smaller of the right\nfraction and what it leaves out, or {} of the events kept",
            super::super::replay::TOLERANCE_RELATIVE * 100.0,
            super::super::replay::TOLERANCE_EVENTS
        )));
        use super::super::events::{KEPT_EVENTS, MOST_KEPT_EVENTS, TAIL_EVENTS};
        assert!(doc.contains(&format!(
            "thinner side of the rule holds about {TAIL_EVENTS}"
        )));
        assert_eq!((KEPT_EVENTS, MOST_KEPT_EVENTS), (5_000, 50_000));
        assert!(doc.contains("All of them up to 5,000"));
        assert!(doc.contains("up to 50,000 (`kept_for`)"));
        assert!(doc.contains(&format!(
            "confident (>= {})",
            super::super::assess::CONFIDENT
        )));
        assert!(doc.contains(&format!(
            "below {:.2} is flagged",
            super::super::assess::REVIEW_FLOOR
        )));
        assert!(doc.contains(&format!(
            "control (the specimen's FMX, or the run's) of more than {}\nevents",
            crate::gate_rules::autogate::TRUSTED_CONTROL_EVENTS
        )));
    }

    #[test]
    fn the_rules_file_example_in_the_description_loads() {
        let doc = HOW_RULES_POSITION_GATES;
        let start = doc.find("```json\n").expect("an example") + "```json\n".len();
        let end = start + doc[start..].find("```").expect("the example ends");
        let entry: crate::gate_rules::rule_store::RuleEntry =
            serde_json::from_str(&doc[start..end]).expect("the example is a rule entry");
        assert_eq!(&*entry.target.gate, "CD69+");
        assert_eq!(entry.target.parent.as_deref(), Some("CD4+"));
        assert!(matches!(
            entry.rule.rule,
            crate::gate_rules::rule::Rule::TailFraction(ref r) if r.band == (0.002, 0.005)
        ));
    }

    #[test]
    fn every_readable_file_is_the_file_on_disk() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for source in SOURCES {
            let on_disk = std::fs::read_to_string(root.join(source.path)).unwrap();
            assert_eq!(source.text, on_disk, "{}", source.path);
        }
    }

    #[test]
    fn a_read_is_numbered_and_bounded() {
        let part = read("gate_rules/rule.rs", Some(2), Some(4)).unwrap();
        let lines: Vec<&str> = SOURCES[0].text.lines().collect();
        assert_eq!(part.from, 2);
        assert_eq!(part.to, 4);
        assert_eq!(
            part.lines,
            vec![
                format!("2: {}", lines[1]),
                format!("3: {}", lines[2]),
                format!("4: {}", lines[3]),
            ]
        );
        assert_eq!(part.more_from, None);
        assert_eq!(part.total_lines, lines.len());

        // A long file comes in pieces, each saying where the next starts.
        let first = read(
            "crates/clingate-core/src/gate_rules/autogate.rs",
            None,
            None,
        )
        .unwrap();
        assert_eq!((first.from, first.to), (1, MAX_LINES));
        assert_eq!(first.lines.len(), MAX_LINES);
        assert_eq!(first.more_from, Some(MAX_LINES + 1));
        let last = read("gate_rules/autogate.rs", Some(first.total_lines - 1), None).unwrap();
        assert_eq!(last.lines.len(), 2);
        assert_eq!(last.more_from, None);

        // A `to` before `from` reads the one line.
        assert_eq!(
            read("gate_rules/rule.rs", Some(5), Some(1))
                .unwrap()
                .lines
                .len(),
            1
        );
    }

    #[test]
    fn only_the_listed_files_can_be_read() {
        let refused = read("session/mod.rs", None, None).unwrap_err();
        assert!(refused.contains("gate_rules/autogate.rs"), "{refused}");
        assert!(read("../Cargo.toml", None, None).is_err());
        let past = read("gate_rules/rule.rs", Some(100_000), None).unwrap_err();
        assert!(past.contains("lines"), "{past}");
    }

    #[test]
    fn a_search_finds_lines_by_any_case_and_stops_at_the_limit() {
        let found = search("FN SLIDE_TO_CAPTURE(", 10);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "gate_rules/autogate.rs");
        let line = read(found[0].path, Some(found[0].line), Some(found[0].line)).unwrap();
        assert!(line.lines[0].contains("fn slide_to_capture("));
        assert_eq!(search("fn ", 3).len(), 3);
        assert!(search("  ", 10).is_empty());
        assert!(search("no such text anywhere at all 7f3a", 10).is_empty());
    }

    #[test]
    fn the_contents_name_every_file_with_its_length() {
        let listed = contents();
        assert_eq!(listed.len(), SOURCES.len());
        for (path, holds, lines) in listed {
            assert!(!holds.is_empty());
            assert!(lines > 50, "{path} has {lines} lines");
        }
    }
}
