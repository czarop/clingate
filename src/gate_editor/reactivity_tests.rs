//! Tests for the reactive wiring itself.
//!
//! Four bugs this session had the same shape: a dependency wider or narrower
//! than the thing that actually mattered, invisible to a suite that runs the
//! stores without a Dioxus runtime. `dioxus-signals` tests its own reactivity
//! with a headless `VirtualDom` and a run counter, and so can we - these run
//! under `cargo test --lib --no-default-features` like everything else, with no
//! renderer and no GTK.
//!
//! Two mechanics matter. A memo is lazy, so its closure only re-runs when the
//! value is *read* after being invalidated - the consumer has to be read for
//! the counter to move. And a chain of memos takes one extra render pass to
//! propagate, which is what `advance` below is for.

#![cfg(test)]

use dioxus::prelude::*;
use dioxus_core::{NoOpMutations, generation};
use std::cell::RefCell;
use std::rc::Rc;

/// Re-render the app and let a chain of memos propagate.
fn advance(dom: &mut VirtualDom) {
    dom.mark_dirty(ScopeId::APP);
    dom.render_immediate(&mut NoOpMutations);
    // A memo invalidated by this pass is only re-read on the next one.
    dom.render_immediate(&mut NoOpMutations);
}

/// The fix, as a test: a narrow memo shields expensive downstream work from
/// changes it does not care about, and still lets through the ones it does.
///
/// This is `chain_gates` in `plot_window`, in miniature - the vector stands in
/// for the resolver, element 0 for this plot's gating chain, and the counted
/// memo for re-filtering the dataframe and rebuilding the event index.
#[test]
fn a_narrow_memo_shields_downstream_work_from_unrelated_writes() {
    let runs = Rc::new(RefCell::new(0usize));

    let mut dom = VirtualDom::new_with_props(
        |runs: Rc<RefCell<usize>>| {
            let mut source = use_signal(|| vec![1usize, 2, 3]);

            // Generation 1 writes what the narrow memo ignores, generation 2
            // what it watches. Before the reads, so each lands in this pass.
            if generation() == 1 {
                source.write()[2] = 99;
            }
            if generation() == 2 {
                source.write()[0] = 42;
            }

            let narrow = use_memo(move || source.read()[0]);

            let expensive = use_memo({
                let runs = runs.clone();
                move || {
                    *runs.borrow_mut() += 1;
                    narrow()
                }
            });
            let _ = expensive();

            rsx! { div {} }
        },
        runs.clone(),
    );

    dom.rebuild_in_place();
    assert_eq!(*runs.borrow(), 1, "the expensive memo runs once to start");

    advance(&mut dom);
    assert_eq!(
        *runs.borrow(),
        1,
        "a write the narrow memo ignores must not reach the expensive consumer"
    );

    advance(&mut dom);
    assert_eq!(
        *runs.borrow(),
        2,
        "a write it does watch must still reach it"
    );
}

/// The bug, as a test: read the whole thing and every write lands on the
/// expensive path. This is what `plot_window` did between 2a92561 and 213f167 -
/// every gate edit anywhere re-filtered every plot.
#[test]
fn a_wide_dependency_puts_every_write_on_the_expensive_path() {
    let runs = Rc::new(RefCell::new(0usize));

    let mut dom = VirtualDom::new_with_props(
        |runs: Rc<RefCell<usize>>| {
            let mut source = use_signal(|| vec![1usize, 2, 3]);

            if generation() == 1 {
                source.write()[2] = 99;
            }

            let expensive = use_memo({
                let runs = runs.clone();
                move || {
                    *runs.borrow_mut() += 1;
                    source.read().clone()
                }
            });
            let _ = expensive();

            rsx! { div {} }
        },
        runs.clone(),
    );

    dom.rebuild_in_place();
    advance(&mut dom);

    // Contrast with the narrow memo above, which stays at 1 for the same write.
    assert!(
        *runs.borrow() > 1,
        "the same unrelated write re-ran the expensive memo - the bug, \
         reproduced; it ran {} times",
        runs.borrow()
    );
}

/// And the other shape: `peek` subscribes to nothing, so the consumer never
/// re-runs at all. This is the axis selector, the gate resolver and the
/// filtered frame - three bugs, one mistake.
#[test]
fn a_peeked_dependency_never_reaches_the_consumer() {
    let runs = Rc::new(RefCell::new(0usize));

    let mut dom = VirtualDom::new_with_props(
        |runs: Rc<RefCell<usize>>| {
            let mut source = use_signal(|| 1usize);

            if generation() == 1 {
                *source.write() = 99;
            }

            let expensive = use_memo({
                let runs = runs.clone();
                move || {
                    *runs.borrow_mut() += 1;
                    *source.peek()
                }
            });
            let _ = expensive();

            rsx! { div {} }
        },
        runs.clone(),
    );

    dom.rebuild_in_place();
    advance(&mut dom);

    assert_eq!(
        *runs.borrow(),
        1,
        "a peeked signal registers no dependency, so the change never arrives"
    );
}

/// `plot_window` scales its file from `cofactors_of` the axis settings, through
/// a memo: moving an axis's range must not scale the file again, and changing
/// a cofactor must. The memo chain here mirrors `plot_window`'s, which needs a
/// file loaded to run at all.
#[test]
fn only_a_changed_cofactor_scales_the_plotted_file_again() {
    use clingate_core::AxisInfo;
    use clingate_core::axis_store::Param;
    use clingate_core::gate_rules::run::RunInputs;
    use clingate_core::omiq::serialise::AxisSettings;
    use flow_fcs::TransformType;
    use std::sync::Arc;

    fn arcsinh(cofactor: f32) -> AxisInfo {
        AxisInfo {
            param: Param {
                marker: Arc::from("CD3"),
                fluoro: Arc::from("CD3"),
            },
            axis_lower: 0.0,
            axis_upper: 10.0,
            transform: TransformType::Arcsinh { cofactor },
        }
    }

    let scalings = Rc::new(RefCell::new(0usize));
    let mut dom = VirtualDom::new_with_props(
        |scalings: Rc<RefCell<usize>>| {
            let mut settings = use_signal(|| {
                let mut settings = AxisSettings::default();
                settings.insert(Arc::from("CD3"), arcsinh(150.0));
                settings
            });
            if generation() == 1 {
                settings.write().get_mut("CD3").unwrap().axis_lower = -1.0;
            }
            if generation() == 2 {
                settings.write().insert(Arc::from("CD3"), arcsinh(300.0));
            }

            let cofactors = use_memo(move || RunInputs::cofactors_of(&settings.read()));
            let scaled = use_memo({
                let scalings = scalings.clone();
                move || {
                    *scalings.borrow_mut() += 1;
                    cofactors()
                }
            });
            let _ = scaled();

            rsx! { div {} }
        },
        scalings.clone(),
    );

    dom.rebuild_in_place();
    assert_eq!(*scalings.borrow(), 1);

    advance(&mut dom);
    assert_eq!(
        *scalings.borrow(),
        1,
        "a moved axis range scaled the file again"
    );

    advance(&mut dom);
    assert_eq!(*scalings.borrow(), 2, "a changed cofactor did not");
}
