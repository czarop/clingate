//! A damaged FCS file must be refused, not crash the app.
//!
//! Two readers open FCS files: `file_load::FcsSampleStub::open`, which reads
//! the header and keywords when a workspace is opened, and flow_fcs's
//! `Fcs::open`, which reads the events for every plot, the gallery and a
//! rules run. A file damaged in copying or cut short is not unusual on a
//! shared drive. Here a valid file has random bytes changed, one damage at a
//! time, and each reader must answer with a file or an error.

mod common;

use clingate::file_load::FcsSampleStub;
use common::*;
use rand::prelude::*;

fn panic_message(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq)]
enum Reader {
    /// What the workspace opens each file with.
    Workspace,
    /// flow_fcs's full read, for plots, the gallery and rules runs.
    Events,
}

/// Damage a copy of a valid file and report any panic from `reader`.
fn damaged_reads(
    reader: Reader,
    trials: usize,
    seed: u64,
    damage: impl Fn(&mut Vec<u8>, &mut StdRng),
) -> Vec<String> {
    let dir = scratch(&format!("fcs-damage-{seed}"));
    let good = dir.join("good.fcs");
    let events: Vec<Vec<f32>> = (0..50).map(|i| vec![i as f32, (i * 2) as f32]).collect();
    write_fcs(&good, &["FSC-A", "SSC-A"], &events);
    let bytes = std::fs::read(&good).unwrap();

    let mut rng = StdRng::seed_from_u64(seed);
    let mut problems = Vec::new();
    for trial in 0..trials {
        let mut copy = bytes.clone();
        damage(&mut copy, &mut rng);
        let path = dir.join(format!("damaged-{trial}.fcs"));
        std::fs::write(&path, &copy).unwrap();
        let name = path.to_string_lossy().to_string();

        let outcome = match reader {
            Reader::Workspace => std::panic::catch_unwind(|| FcsSampleStub::open(&name).is_ok()),
            Reader::Events => std::panic::catch_unwind(|| flow_fcs::Fcs::open(&name).is_ok()),
        };
        if let Err(e) = outcome {
            problems.push(panic_message(e));
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

fn header_damage(bytes: &mut Vec<u8>, rng: &mut StdRng) {
    // The header and text segment are the first few hundred bytes.
    let limit = bytes.len().min(600);
    let at = rng.random_range(0..limit);
    bytes[at] = rng.random_range(0..=255u8);
}

fn cut_short(bytes: &mut Vec<u8>, rng: &mut StdRng) {
    let keep = rng.random_range(0..bytes.len());
    bytes.truncate(keep);
}

#[test]
fn the_workspace_refuses_a_file_damaged_in_its_header_or_keywords() {
    let problems = damaged_reads(Reader::Workspace, 400, 1, header_damage);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_workspace_refuses_a_file_cut_short_anywhere() {
    let problems = damaged_reads(Reader::Workspace, 200, 2, cut_short);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// BUG (docs/test-audit.md, B-FCS-3), upstream: flow_fcs's `Fcs::open`
/// slices the file by the offsets its header claims without checking them,
/// so a file cut short - or with a digit of an offset changed - panics
/// ("range end index 312 out of range for slice of length 110"). Every
/// caller here runs it on a worker thread, so the app survives, but see
/// B-FCS-2 for what the panic still costs.
#[test]
#[ignore = "known bug B-FCS-3 (flow_fcs): a damaged file panics Fcs::open"]
fn reading_the_events_of_a_damaged_file_is_an_error_not_a_panic() {
    let mut problems = damaged_reads(Reader::Events, 400, 1, header_damage);
    problems.extend(damaged_reads(Reader::Events, 200, 2, cut_short));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
