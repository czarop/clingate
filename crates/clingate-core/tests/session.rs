//! A workspace opened with no window, and asked about by name.
//!
//! The folder holds what an Omiq user's would: two FCS files, a metadata
//! export, a scaling export and a gating file (the checked-in fixture, whose
//! file ids the metadata names). Everything a tool would do goes through
//! [`Session`].

mod common;

use clingate_core::session::{Refusal, Session};
use common::*;
use rand::SeedableRng;
use rand_distr::{Distribution, Normal, Uniform};

const FLUORESCENCE: [&str; 8] = [
    "BUV661-A",
    "BV785-A",
    "Alexa Fluor 700-A",
    "BUV737-A",
    "BUV805-A",
    "BUV563-A",
    "Alexa Fluor 647-A",
    "Vio Bright 423-A",
];

/// Scatter spread evenly, each fluorescence channel a negative and a
/// positive population.
fn events(seed: u64, n: usize) -> Vec<Vec<f32>> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let scatter = Uniform::new(1_000.0f32, 4_000_000.0).unwrap();
    let negative = Normal::new(0.0f32, 300.0).unwrap();
    let positive = Normal::new(40_000.0f32, 8_000.0).unwrap();
    (0..n)
        .map(|i| {
            let mut row = vec![scatter.sample(&mut rng), scatter.sample(&mut rng)];
            for c in 0..FLUORESCENCE.len() {
                row.push(if (i + c) % 3 == 0 {
                    positive.sample(&mut rng)
                } else {
                    negative.sample(&mut rng)
                });
            }
            row
        })
        .collect()
}

fn workspace(name: &str) -> std::path::PathBuf {
    let dir = scratch(name);
    let mut channels = vec!["FSC-A", "SSC-A"];
    channels.extend(FLUORESCENCE);
    write_fcs(&dir.join("sample1_FMX.fcs"), &channels, &events(1, 3_000));
    write_fcs(&dir.join("sample2_FS.fcs"), &channels, &events(2, 3_000));
    write_metadata(
        &dir.join("metadata.csv"),
        &["test", "Type", "SampleType"],
        &[
            ("sample1", "sample1_FMX.fcs", &["one", "one", "FMX"]),
            ("sample2", "sample2_FS.fcs", &["two", "two", "FS"]),
        ],
    );
    let mut scaling = vec![
        ("FSC-A", "", "None (linear)", 0, 0, 4_194_304),
        ("SSC-A", "", "None (linear)", 0, 0, 4_194_304),
    ];
    for channel in FLUORESCENCE {
        scaling.push((channel, channel, "Arcsinh", 6000, -2000, 200_000));
    }
    write_scaling(&dir.join("scaling.csv"), &scaling);
    std::fs::copy(
        fixture("quadrant_with_boolean_child.omiqgt"),
        dir.join("gating.omiqgt"),
    )
    .unwrap();
    dir
}

fn clarification(refusal: Refusal) -> clingate_core::session::lookup::Clarification {
    match refusal {
        Refusal::NeedsClarification(c) => c,
        other => panic!("expected a question, got {other:?}"),
    }
}

#[test]
fn a_folder_opens_whole_and_says_what_it_holds() {
    let session = Session::open(&workspace("session-open")).unwrap();
    let overview = session.overview();
    let json = serde_json::to_value(&overview).unwrap();
    for part in ["metadata", "scaling", "gating"] {
        assert_eq!(json["parts"][part]["state"], "loaded", "{part}: {json}");
    }
    assert_eq!(json["parts"]["rules"]["state"], "missing");
    assert_eq!(overview.samples, 2);
    assert!(overview.samples_without_metadata.is_empty());
    assert!(overview.populations > 5);
    let kinds = overview
        .metadata_columns
        .iter()
        .find(|c| c.column == "SampleType")
        .unwrap();
    assert_eq!(kinds.values, ["FMX", "FS"]);
}

#[test]
fn samples_are_found_by_name_or_metadata_and_a_near_miss_is_asked_about() {
    let session = Session::open(&workspace("session-samples")).unwrap();
    let found = session.find_samples("fmx").unwrap();
    assert_eq!(found.samples.len(), 1);
    assert_eq!(found.samples[0].name, "sample1_FMX.fcs");
    assert_eq!(
        session.find_samples("two").unwrap().samples[0].name,
        "sample2_FS.fcs"
    );

    let asked = clarification(session.find_samples("fm").unwrap_err());
    assert!(
        asked.suggestions.iter().any(|s| s.starts_with("fmx")),
        "{asked:?}"
    );
}

#[test]
fn every_population_can_be_asked_for_by_the_name_it_is_listed_under() {
    let session = Session::open(&workspace("session-names")).unwrap();
    let populations = session.populations(None).unwrap();
    assert!(!populations.is_empty());
    for p in populations.iter().filter(|p| !p.name.contains("(id:")) {
        let stats = session.population_stats(&p.name, "fmx");
        match stats {
            Ok(stats) => assert_eq!(stats.population, p.name),
            // A population with nothing above it to count against.
            Err(Refusal::Failed { .. }) => {}
            Err(other) => panic!("{} did not name itself: {other:?}", p.name),
        }
    }
}

#[test]
fn a_population_is_counted_in_every_sample_named() {
    let session = Session::open(&workspace("session-stats")).unwrap();
    let stats = session.population_stats("Tmem", "all").unwrap();
    assert_eq!(stats.rows.len(), 2);
    for row in &stats.rows {
        assert!(row.problem.is_none(), "{row:?}");
        let (total, parent, events) = (
            row.total_events.unwrap(),
            row.parent_events.unwrap(),
            row.events.unwrap(),
        );
        assert_eq!(total, 3_000);
        assert!(events <= parent && parent <= total, "{row:?}");
        let expected = 100.0 * events as f64 / parent as f64;
        assert!((row.percent_of_parent.unwrap() - expected).abs() < 1e-2);
    }
    assert_eq!(stats.summary.unwrap().samples, 2);
}

#[test]
fn a_distribution_accounts_for_every_event_and_places_the_gate() {
    let session = Session::open(&workspace("session-distribution")).unwrap();
    let tmem = session.populations(Some("Tmem")).unwrap();
    let parameter = tmem[0].parameters[0].clone();
    let d = session.distribution("Tmem", "fmx", &parameter).unwrap();
    let h = d.histogram.as_ref().unwrap();
    assert_eq!(h.counts.iter().sum::<usize>() + h.below + h.above, d.events);
    let p = d.percentiles.as_ref().unwrap();
    assert!(p.p1 <= p.p50 && p.p50 <= p.p99);
    assert!(d.gate_on_this_parameter.is_some(), "{d:?}");
}

#[test]
fn one_sample_is_needed_for_a_distribution_and_several_are_asked_about() {
    let session = Session::open(&workspace("session-one-sample")).unwrap();
    let tmem = session.populations(Some("Tmem")).unwrap();
    let parameter = tmem[0].parameters[0].clone();
    let asked = clarification(session.distribution("Tmem", "all", &parameter).unwrap_err());
    assert_eq!(asked.suggestions.len(), 2);
    let asked = clarification(session.distribution("Tmem", "fmx", "nothing").unwrap_err());
    assert_eq!(asked.about, "parameter");
}

#[test]
fn an_unknown_compensation_group_is_asked_about() {
    let mut session = Session::open(&workspace("session-comp")).unwrap();
    let asked = clarification(session.answer_omiq("no such group", None).unwrap_err());
    assert!(!asked.suggestions.is_empty());
}
