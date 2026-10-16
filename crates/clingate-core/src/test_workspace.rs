//! A workspace folder as an Omiq user's would be, for tests here and in the
//! app: two FCS files, a metadata export, a scaling export, the checked-in
//! gating fixture, and - when asked - one rule in the rules folder.
//!
//! The events are seeded, so two folders made by these functions are the
//! same byte for byte. The parity tests rely on that: the tools for Claude
//! and the app each work in their own copy, and their results are compared.

use std::path::{Path, PathBuf};

use rand::SeedableRng;
use rand_distr::{Distribution, Normal, Uniform};

use crate::file_load_tests::{scratch, write_fcs_rows};

pub const FLUORESCENCE: [&str; 8] = [
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
pub fn events(seed: u64, n: usize) -> Vec<Vec<f32>> {
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

/// Two samples of one specimen each - `sample1_FMX.fcs` (specimen "one", an
/// FMX) and `sample2_FS.fcs` (specimen "two", a full stain) - with metadata,
/// scaling and gating, in a fresh scratch folder.
pub fn two_samples(name: &str) -> PathBuf {
    workspace_of(
        name,
        &[
            ("sample1", "one", "FMX", events(1, 3_000)),
            ("sample2", "two", "FS", events(2, 3_000)),
        ],
    )
}

/// A workspace folder holding `samples` - each its id, specimen, sample type
/// and events, written as `{id}_{type}.fcs` - with metadata, scaling and
/// gating.
fn workspace_of(name: &str, samples: &[(&str, &str, &str, Vec<Vec<f32>>)]) -> PathBuf {
    let dir = scratch(name);
    let mut channels: Vec<(&str, Option<&str>)> = vec![("FSC-A", None), ("SSC-A", None)];
    channels.extend(FLUORESCENCE.iter().map(|c| (*c, None)));
    let mut metadata = String::from("OmiqID,Filename,test,Type,SampleType\n");
    for (id, specimen, kind, rows) in samples {
        let file = format!("{id}_{kind}.fcs");
        write_fcs_rows(&dir.join(&file), &channels, rows, &[]);
        metadata.push_str(&format!("{id},{file},{specimen},{specimen},{kind}\n"));
    }
    std::fs::write(dir.join("metadata.csv"), metadata).unwrap();
    let mut scaling = String::from(
        "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\n\
         FSC-A,,None (linear),0,0,4194304,0,0\n\
         SSC-A,,None (linear),0,0,4194304,0,0\n",
    );
    for c in FLUORESCENCE {
        scaling.push_str(&format!("{c},{c},Arcsinh,6000,-2000,200000,0,0\n"));
    }
    std::fs::write(dir.join("scaling.csv"), scaling).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/quadrant_with_boolean_child.omiqgt"),
        dir.join("gating.omiqgt"),
    )
    .unwrap();
    dir
}

/// Where [`spilling_samples`] spills from and into, by how much, and where
/// its autofluorescence shows.
pub const SPILLS_FROM: &str = "BUV805-A";
pub const SPILLS_INTO: &str = "BUV737-A";
pub const SPILL: f32 = 0.1;
pub const GLOWS_IN: &str = "BV785-A";

/// Scatter inside population "1"; every other cell negative everywhere, the
/// rest positive in one channel each. The top fifth by side scatter glow by
/// 1000 - three negative widths - in [`GLOWS_IN`]; where `spill`, a tenth of
/// [`SPILLS_FROM`] is left in [`SPILLS_INTO`].
pub fn spilling_events(seed: u64, n: usize, spill: bool) -> Vec<Vec<f32>> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let forward = Uniform::new(1_200_000.0f32, 2_100_000.0).unwrap();
    let side = Uniform::new(150_000.0f32, 900_000.0).unwrap();
    let negative = Normal::new(0.0f32, 300.0).unwrap();
    let positive = Normal::new(40_000.0f32, 8_000.0).unwrap();
    let column = |channel: &str| 2 + FLUORESCENCE.iter().position(|c| *c == channel).unwrap();
    (0..n)
        .map(|i| {
            let mut row = vec![forward.sample(&mut rng), side.sample(&mut rng)];
            for c in 0..FLUORESCENCE.len() {
                row.push(if i % 16 == c {
                    positive.sample(&mut rng)
                } else {
                    negative.sample(&mut rng)
                });
            }
            if row[1] > 750_000.0 {
                row[column(GLOWS_IN)] += 1_000.0;
            }
            if spill {
                row[column(SPILLS_INTO)] += SPILL * row[column(SPILLS_FROM)];
            }
            row
        })
        .collect()
}

/// Four full stains with [`spilling_events`]' spill and autofluorescence,
/// specimens "a" to "d", and unstained files for "a" and "b" with the
/// autofluorescence alone; a rules file with no rules, telling specimens apart
/// by the `test` column.
pub fn spilling_samples(name: &str) -> PathBuf {
    use crate::gate_rules::rule_store::{RuleStore, SamplePairing};
    let mut samples: Vec<(&str, &str, &str, Vec<Vec<f32>>)> = ["a", "b", "c", "d"]
        .iter()
        .zip(1..)
        .map(|(specimen, seed)| {
            (
                *specimen,
                *specimen,
                "FS",
                spilling_events(seed, 8_000, true),
            )
        })
        .collect();
    samples.push(("ua", "a", "U", spilling_events(11, 8_000, false)));
    samples.push(("ub", "b", "U", spilling_events(12, 8_000, false)));
    let dir = workspace_of(name, &samples);
    RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    })
    .save(&crate::workspace::rules_file(&dir))
    .unwrap();
    dir
}

/// [`two_samples`], with one rule saved where the rules tab saves them: Tmem
/// keeps the top 1-2% of its parent on its first parameter, measured on each
/// sample itself, specimens told apart by the `test` column.
pub fn two_samples_with_a_rule(name: &str) -> PathBuf {
    use crate::gate_rules::rule::{Rule, TailFractionRule};
    use crate::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, SamplePairing,
    };
    let dir = two_samples(name);
    let mut store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    });
    store.insert(
        RuleTarget::named("Tmem"),
        GateRule {
            // Tmem's first parameter in the gating fixture.
            parameter: "BUV805-A".into(),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: Rule::TailFraction(TailFractionRule::new((0.01, 0.02))),
        },
    );
    store.save(&crate::workspace::rules_file(&dir)).unwrap();
    dir
}

/// [`two_samples`] with a second gate at the top, "2", drawn as "1" is and
/// holding a Tmem drawn as 1's is and a teff_naive drawn on BV785-A rather than
/// BUV805-A - two sibling populations with the same gates under them. Rules
/// for 1's Tmem and teff_naive on BUV805-A, measured on each sample itself.
pub fn two_samples_with_siblings(name: &str) -> PathBuf {
    use crate::gate_rules::rule::{Rule, TailFractionRule};
    use crate::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, SamplePairing,
    };
    let dir = two_samples(name);
    let gating = dir.join("gating.omiqgt");
    let mut document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&gating).unwrap()).unwrap();
    let tree = &mut document["tree"];
    for (from, to, name) in [
        ("uN8Y", "uN82", "2"),
        ("4ECA", "4EC2", "Tmem"),
        ("hu4H", "hu42", "teff_naive"),
    ] {
        let mut container = tree["filterContainers"][from].clone();
        container["id"] = to.into();
        container["name"] = name.into();
        tree["filterContainers"][to] = container;
    }
    tree["filterContainers"]["hu42"]["defaultFilter"]["f1"] = "BV785-A".into();
    if let Some(files) = tree["filterContainers"]["hu42"]
        .get_mut("perFileFilters")
        .and_then(|files| files.as_object_mut())
    {
        for filter in files.values_mut() {
            filter["f1"] = "BV785-A".into();
        }
    }
    for (id, parent, container) in [
        ("fn12", "", "uN82"),
        ("URs2", "fn12", "4EC2"),
        ("7yV2", "fn12", "hu42"),
    ] {
        tree["nodes"][id] = serde_json::json!({
            "id": id, "parentId": parent, "filterContainerId": container,
            "ord": 1, "collapsed": false
        });
    }
    std::fs::write(&gating, document.to_string()).unwrap();

    let mut store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    });
    for gate in ["Tmem", "teff_naive"] {
        store.insert(
            RuleTarget::under(gate, "1"),
            GateRule {
                parameter: "BUV805-A".into(),
                bound: Bound::Above,
                measured_on: MeasuredOn::Itself,
                rule: Rule::TailFraction(TailFractionRule::new((0.01, 0.02))),
            },
        );
    }
    store.save(&crate::workspace::rules_file(&dir)).unwrap();
    dir
}
