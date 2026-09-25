//! A damaged gating file must be refused, not crash or hang the app.
//!
//! Gating files come from Omiq, from older versions of this program, and
//! from people editing them by hand. The import (`omiq::deserialise` and
//! `GateState::upload_gates_from_file`) is here fed the checked-in fixture
//! with one random damage at a time - a key removed, a value nulled, a
//! value turned into a string, a number made enormous - and must either load
//! it or return an error. A panic or an import that never finishes fails.
//!
//! A `parentId` pointing into a loop is left out: it hangs the import today
//! (B-OMIQ-2), pinned in `document_round_trip`.

mod common;
use clingate::gate_editor::gates::GateState;
use common::*;
use rand::prelude::*;

fn paths(v: &serde_json::Value, at: Vec<String>, out: &mut Vec<Vec<String>>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, c) in m {
                let mut p = at.clone();
                p.push(k.clone());
                out.push(p.clone());
                paths(c, p, out);
            }
        }
        serde_json::Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                let mut p = at.clone();
                p.push(format!("#{i}"));
                out.push(p.clone());
                paths(c, p, out);
            }
        }
        _ => {}
    }
}

fn get_mut<'a>(v: &'a mut serde_json::Value, path: &[String]) -> Option<&'a mut serde_json::Value> {
    let mut cur = v;
    for p in path {
        cur = if let Some(i) = p.strip_prefix('#') {
            cur.get_mut(i.parse::<usize>().ok()?)?
        } else {
            cur.get_mut(p.as_str())?
        };
    }
    Some(cur)
}

#[test]
fn one_damaged_field_is_refused_or_loaded_never_a_crash() {
    let text = std::fs::read_to_string(fixture("quadrant_with_boolean_child.omiqgt")).unwrap();
    let original: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut all = Vec::new();
    paths(&original, Vec::new(), &mut all);
    let mut rng = StdRng::seed_from_u64(1);
    let mut problems = std::collections::BTreeMap::<String, String>::new();
    for trial in 0..600 {
        let mut doc = original.clone();
        let path = all[rng.random_range(0..all.len())].clone();
        let kind = rng.random_range(0..4);
        let (parent, last) = path.split_at(path.len() - 1);
        let desc;
        match kind {
            0 => {
                if let Some(serde_json::Value::Object(m)) = get_mut(&mut doc, parent) {
                    m.remove(&last[0]);
                }
                desc = format!("remove {}", path.join("."));
            }
            1 => {
                if let Some(v) = get_mut(&mut doc, &path) {
                    *v = serde_json::Value::Null;
                }
                desc = format!("null {}", path.join("."));
            }
            2 => {
                if let Some(v) = get_mut(&mut doc, &path) {
                    *v = serde_json::json!("x");
                }
                desc = format!("string {}", path.join("."));
            }
            _ => {
                if let Some(v) = get_mut(&mut doc, &path) {
                    *v = serde_json::json!(-1e30);
                }
                desc = format!("huge {}", path.join("."));
            }
        }
        if path.last().is_some_and(|l| l == "parentId") {
            continue;
        }
        let file = scratch(&format!("mut-{trial}")).join("m.omiqgt");
        std::fs::write(&file, serde_json::to_string(&doc).unwrap()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| {
                let mut s = GateState::default();
                s.upload_gates_from_file(file, &Default::default(), fixture_axes())
                    .is_ok()
            });
            let _ = tx.send(r.map_err(|e| {
                e.downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default()
            }));
        });
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(_)) => {}
            Ok(Err(msg)) => {
                problems
                    .entry(format!("PANIC {}", msg.lines().next().unwrap_or("")))
                    .or_insert(desc);
            }
            Err(_) => {
                problems.entry("HANG".into()).or_insert(desc);
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{}",
        problems
            .iter()
            .map(|(what, first)| format!("{what}, first from: {first}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
