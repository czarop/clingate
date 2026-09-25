//! Damaged metadata and scaling exports must be refused or read, never crash.
//!
//! 500 trials of one damage each - a cell blanked, a cell holding a comma, a
//! row cut short, an enormous number, a whole column dropped - to each file.

mod common;
use clingate::gate_editor::plots::axis_store::{ScalingInfoSource, read_axis_configs};
use clingate::omiq::metadata::{MetaDataOrigin, parse_metadata_csv};
use common::*;
use rand::prelude::*;

fn damage(text: &str, rng: &mut StdRng) -> String {
    let mut lines: Vec<Vec<String>> = text
        .lines()
        .map(|l| l.split(',').map(String::from).collect())
        .collect();
    let r = rng.random_range(0..lines.len());
    match rng.random_range(0..5) {
        0 => {
            let c = rng.random_range(0..lines[r].len());
            lines[r][c] = String::new();
        }
        1 => {
            let c = rng.random_range(0..lines[r].len());
            lines[r][c] = "x,y".into();
        }
        2 => {
            lines[r].pop();
        }
        3 => {
            let c = rng.random_range(0..lines[r].len());
            lines[r][c] = "-99999999999999999999".into();
        }
        _ => {
            let c = rng.random_range(0..lines[r].len());
            for l in lines.iter_mut() {
                if c < l.len() {
                    l.remove(c);
                }
            }
        }
    }
    lines
        .iter()
        .map(|l| l.join(","))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn a_damaged_export_is_refused_or_read_never_a_crash() {
    let dir = scratch("csvprobe");
    let meta = "OmiqID,Filename,Plate,SampleID\nF1,a.fcs,P1,D1\nF2,b.fcs,P2,D2\nF3,c.fcs,P1,D3\n";
    let scale = "Feature Name (Primary),Feature Name (Secondary),Scaling Type,Cofactor,Min,Max,Min Z,Max Z\nFSC-A,,None (linear),1,0,262144,0,0\nBV421-A,CD3,Arcsinh,150,-500,200000,0,0\n";
    let mut rng = StdRng::seed_from_u64(3);
    let mut problems = std::collections::BTreeSet::new();
    for t in 0..500 {
        let (m, s) = (damage(meta, &mut rng), damage(scale, &mut rng));
        let (mp, sp) = (dir.join(format!("m{t}.csv")), dir.join(format!("s{t}.csv")));
        std::fs::write(&mp, &m).unwrap();
        std::fs::write(&sp, &s).unwrap();
        if let Err(e) = std::panic::catch_unwind(|| {
            parse_metadata_csv(mp.clone(), "OmiqID", "Filename", MetaDataOrigin::Omiq).is_ok()
        }) {
            problems.insert(format!(
                "metadata panic {:?} on {m:?}",
                e.downcast_ref::<String>()
            ));
        }
        match std::panic::catch_unwind(|| read_axis_configs(sp.clone(), ScalingInfoSource::Omiq)) {
            Err(e) => {
                problems.insert(format!(
                    "scaling panic {:?} on {s:?}",
                    e.downcast_ref::<String>()
                ));
            }
            // A dropped column is read shifted rather than refused - that is
            // B-SCALE-1, pinned in the metadata tests. Only a panic fails here.
            Ok(_) => {}
        }
    }
    assert!(
        problems.is_empty(),
        "{}",
        problems.iter().cloned().collect::<Vec<_>>().join("\n")
    );
}
