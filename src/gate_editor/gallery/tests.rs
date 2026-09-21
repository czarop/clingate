//! What can be tested without a renderer: the cache's staleness rule, the
//! flattening, and the PDF's structure.

#![cfg(test)]

use std::sync::Arc;

use super::cache::{Fingerprint, PlotCache};
use super::overlay::{Flat, flatten};
use super::pdf::{Drawn, Sheet, write_pdf};
use super::render::PlotImage;
use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::{GateStateImplExt, ROOTGATE};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::gates::gate_types::PrimaryGateType;
use crate::gate_editor::gates::gate_types::{DEFAULT_LINE, GateRenderShape, ShapeType};
use crate::gate_editor::plots::axis_store::{Param, PlotMapper};
use flow_fcs::TransformType;

fn mapper() -> PlotMapper {
    PlotMapper::new(
        600.0,
        600.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    )
}

fn axis() -> AxisInfo {
    AxisInfo {
        param: Param {
            marker: Arc::from("FSC-A"),
            fluoro: Arc::from("FSC-A"),
        },
        axis_lower: 0.0,
        axis_upper: 1000.0,
        transform: TransformType::Linear,
    }
}

/// A registered gate, taken through the store so it is the very `Arc` the rest
/// of the program would hand round. Each call makes a new one, which is what
/// the staleness tests need: two gates of the same shape at different
/// addresses is exactly what moving a gate produces.
/// A linear scatter axis over the range a cytometer actually reports.
fn scatter(name: &str) -> AxisInfo {
    AxisInfo {
        param: Param {
            marker: Arc::from(name),
            fluoro: Arc::from(name),
        },
        axis_lower: 0.0,
        axis_upper: 4_194_304.0,
        transform: TransformType::Linear,
    }
}

fn gate(name: &str) -> Arc<dyn DrawableGate> {
    let mut state = GateState::default();
    state
        .add_gate(
            &mapper(),
            300.0,
            300.0,
            Arc::from("FSC-A"),
            Arc::from("SSC-A"),
            None,
            Some(ROOTGATE.clone()),
            PrimaryGateType::Rectangle,
            Some(name.to_string()),
        )
        .expect("a gate can be added");
    let id = state
        .placements()
        .map(|(_, p)| p.gate_id.clone())
        .next()
        .expect("the new placement");
    state.registered_gate(&id).expect("registered")
}

fn print(file: &str, gates: Vec<Arc<dyn DrawableGate>>) -> Fingerprint {
    Fingerprint {
        file: Arc::from(file),
        x: Arc::from("FSC-A"),
        y: Arc::from("SSC-A"),
        x_axis: axis(),
        y_axis: axis(),
        size: 260,
        scaling: 0,
        gates,
    }
}

fn picture() -> Arc<PlotImage> {
    Arc::new(PlotImage {
        src: String::new(),
        jpeg: Arc::new(Vec::new()),
        mapper: Arc::new(mapper()),
        parent_events: 0,
        stats: Default::default(),
    })
}

// ── the staleness rule ───────────────────────────────────────────────────

#[test]
fn the_same_gates_hit_the_same_picture() {
    let g = gate("a");
    let mut cache = PlotCache::default();
    cache.insert(print("f1", vec![g.clone()]), picture());
    assert!(cache.get(&print("f1", vec![g])).is_some());
}

#[test]
fn a_moved_gate_misses() {
    // This is the whole contract. Moving a gate replaces the `Arc` rather than
    // mutating it, so a plot drawn against the old one must not answer for the
    // new one - that would be a picture of the wrong position.
    let before = gate("a");
    let after = gate("a");
    let mut cache = PlotCache::default();
    cache.insert(print("f1", vec![before]), picture());
    assert!(cache.get(&print("f1", vec![after])).is_none());
}

#[test]
fn another_file_misses() {
    let g = gate("a");
    let mut cache = PlotCache::default();
    cache.insert(print("f1", vec![g.clone()]), picture());
    assert!(cache.get(&print("f2", vec![g])).is_none());
}

#[test]
fn another_size_misses() {
    let g = gate("a");
    let mut cache = PlotCache::default();
    cache.insert(print("f1", vec![g.clone()]), picture());
    let mut bigger = print("f1", vec![g]);
    bigger.size = 340;
    assert!(cache.get(&bigger).is_none());
}

#[test]
fn a_rescaled_axis_misses() {
    // The axis range decides where the events land, so a plot drawn against the
    // old limits is the wrong picture even with every gate unmoved.
    let g = gate("a");
    let mut cache = PlotCache::default();
    cache.insert(print("f1", vec![g.clone()]), picture());
    let mut rescaled = print("f1", vec![g]);
    rescaled.x_axis.axis_upper = 5000.0;
    assert!(cache.get(&rescaled).is_none());
}

#[test]
fn a_gate_added_to_the_plot_misses() {
    let a = gate("a");
    let mut cache = PlotCache::default();
    cache.insert(print("f1", vec![a.clone()]), picture());
    assert!(cache.get(&print("f1", vec![a, gate("b")])).is_none());
}

#[test]
fn the_cache_gives_up_its_oldest_entries() {
    let mut cache = PlotCache::with_limit(2);
    let (a, b, c) = (gate("a"), gate("b"), gate("c"));
    cache.insert(print("f1", vec![a.clone()]), picture());
    cache.insert(print("f2", vec![b.clone()]), picture());
    cache.insert(print("f3", vec![c.clone()]), picture());
    assert_eq!(cache.len(), 2);
    assert!(cache.get(&print("f1", vec![a])).is_none());
    assert!(cache.get(&print("f3", vec![c])).is_some());
}

#[test]
fn a_held_fingerprint_keeps_its_gates_alive() {
    // Pointer identity is only sound while the pointer cannot be reused, which
    // is why the fingerprint holds the `Arc`s it hashed. Dropping every other
    // reference must leave the cached entry answerable.
    let g = gate("a");
    let key = print("f1", vec![g.clone()]);
    let mut cache = PlotCache::default();
    cache.insert(key.clone(), picture());
    drop(g);
    assert!(cache.get(&key).is_some());
    assert_eq!(Arc::strong_count(&key.gates[0]), 2);
}

// ── flattening ───────────────────────────────────────────────────────────

#[test]
fn a_rectangle_flattens_to_four_closed_points() {
    let shapes = vec![GateRenderShape::Rectangle {
        x: 10.0,
        y: 10.0,
        width: 100.0,
        height: 100.0,
        style: &DEFAULT_LINE,
        shape_type: ShapeType::Gate(Arc::from("g")),
    }];
    let flat = flatten(shapes, &mapper());
    match &flat[..] {
        [Flat::Path { points, closed, .. }] => {
            assert_eq!(points.len(), 4);
            assert!(closed);
        }
        other => panic!("expected one closed path, got {other:?}"),
    }
}

#[test]
fn handles_and_vertex_dots_are_not_drawn() {
    // Both exist to be grabbed. Drawing them where nothing can be grabbed would
    // promise an interaction the gallery does not have.
    let shapes = vec![
        GateRenderShape::Handle {
            center: (1.0, 1.0),
            size: 4.0,
            shape_center: (0.0, 0.0),
            shape_type: ShapeType::Gate(Arc::from("g")),
        },
        GateRenderShape::Circle {
            center: (1.0, 1.0),
            radius: 3.0,
            fill: "red",
            shape_type: ShapeType::Point(0),
        },
    ];
    assert!(flatten(shapes, &mapper()).is_empty());
}

#[test]
fn a_line_is_never_filled() {
    // An SVG `line` ignores fill; a PDF path does not, and would shade the
    // triangle between a line's ends.
    let shapes = vec![GateRenderShape::Line {
        x1: 0.0,
        y1: 0.0,
        x2: 100.0,
        y2: 100.0,
        style: &DEFAULT_LINE,
        shape_type: ShapeType::Gate(Arc::from("g")),
    }];
    match &flatten(shapes, &mapper())[..] {
        [Flat::Path { fill, closed, .. }] => {
            assert_eq!(*fill, "none");
            assert!(!closed);
        }
        other => panic!("expected an open unfilled path, got {other:?}"),
    }
}

// ── the PDF ──────────────────────────────────────────────────────────────

/// The smallest thing a decoder will accept as a JPEG frame header: the two
/// start markers, then a baseline frame declaring its size and components.
fn tiny_jpeg() -> Arc<Vec<u8>> {
    let mut bytes = vec![0xFF, 0xD8];
    bytes.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
    bytes.extend_from_slice(&[0x01, 0x40]); // height 320
    bytes.extend_from_slice(&[0x01, 0x40]); // width 320
    bytes.push(0x03); // three components
    bytes.extend_from_slice(&[0u8; 9]);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    Arc::new(bytes)
}

fn sheet(title: &str) -> Sheet {
    Sheet {
        title: title.to_string(),
        slots: vec![
            Some(Drawn {
                name: format!("{title} FMX"),
                jpeg: tiny_jpeg(),
                shapes: flatten(
                    vec![GateRenderShape::Rectangle {
                        x: 10.0,
                        y: 10.0,
                        width: 100.0,
                        height: 100.0,
                        style: &DEFAULT_LINE,
                        shape_type: ShapeType::Gate(Arc::from("g")),
                    }],
                    &mapper(),
                ),
                rendered_at: 600.0,
            }),
            None,
        ],
    }
}

#[test]
fn a_pdf_has_a_header_a_trailer_and_one_page_per_six_specimens() {
    let sheets: Vec<Sheet> = (0..7).map(|i| sheet(&format!("D{i}"))).collect();
    let bytes = write_pdf("Ki67+ — Ki-67 / CD3", &sheets).expect("wrote");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("%PDF-1.4"));
    assert!(text.trim_end().ends_with("%%EOF"));
    // Seven specimens at six to a page is two pages.
    assert_eq!(
        text.matches("/Type /Page\n").count() + text.matches("/Type /Page ").count(),
        2
    );
    assert!(text.contains("/Count 2"));
}

#[test]
fn every_plot_is_embedded_once_as_a_jpeg() {
    let bytes = write_pdf("x", &[sheet("D1"), sheet("D2")]).expect("wrote");
    let text = String::from_utf8_lossy(&bytes);
    // One image per filled slot; the empty ones contribute nothing.
    assert_eq!(text.matches("/Filter /DCTDecode").count(), 2);
    // The frame header's size, not the size anything asked for.
    assert!(text.contains("/Width 320 /Height 320"));
}

#[test]
fn the_specimen_names_reach_the_page() {
    let bytes = write_pdf("heading", &[sheet("D85")]).expect("wrote");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("(D85) Tj"));
    assert!(text.contains("(heading) Tj"));
}

#[test]
fn a_bracket_in_a_name_does_not_end_the_string() {
    // A PDF string ends at an unbalanced parenthesis, and marker names have
    // brackets in them often enough to matter.
    let mut only = sheet("CD4+ (memory)");
    only.title = "CD4+ (memory)".to_string();
    let bytes = write_pdf("x", &[only]).expect("wrote");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(r"(CD4+ \(memory\)) Tj"));
}

#[test]
fn a_run_with_no_plots_still_writes_a_page() {
    let empty = Sheet {
        title: "D1".to_string(),
        slots: vec![None, None],
    };
    let bytes = write_pdf("x", &[empty]).expect("wrote");
    assert!(String::from_utf8_lossy(&bytes).contains("/Count 1"));
}

#[test]
fn a_gates_outline_is_stroked_not_filled() {
    // On paper a translucent wash hides the events the gate is drawn around,
    // and the events are the evidence.
    let bytes = write_pdf("x", &[sheet("D1")]).expect("wrote");
    let text = String::from_utf8_lossy(&bytes);
    // `s` closes and strokes; `f`/`B` would fill.
    assert!(text.contains("s Q") || text.contains("S Q"));
    assert!(!text.contains("f Q"));
    assert!(!text.contains("B Q"));
}

// ── end to end, against a real FCS file ──────────────────────────────────
//
// The synthetic tests above check the format and the staleness rule. They
// cannot check the thing most likely to be subtly wrong: that a real rendered
// JPEG's frame header parses, that its dimensions reach the page, and that the
// pipeline from an FCS on disk to bytes in a file runs at all. Gated on an env
// var and silently skipped without it, like the other real-file tests.
//
//     OMIQ_FCS_DIR=/path/to/fcs cargo test --lib --no-default-features a_real_gallery
//
// Set GALLERY_PDF_OUT to keep the file and look at it.

#[test]
fn a_real_gallery_page_renders_and_writes() {
    let Ok(dir) = std::env::var("OMIQ_FCS_DIR") else {
        eprintln!("skipped: set OMIQ_FCS_DIR");
        return;
    };
    let Some(file) = std::fs::read_dir(&dir)
        .expect("the FCS directory can be read")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "fcs"))
        .min()
    else {
        eprintln!("skipped: no FCS files in {dir}");
        return;
    };

    use super::render::{PlotJob, render_plot};
    use crate::gate_editor::gates::gate_store::GateOverrideResolver;

    // No chain and no gates: this is about the pipeline and the picture, not
    // about gating, and a scatter plot needs no gating file to exist.
    let job = PlotJob {
        path: file.clone(),
        cofactors: Vec::new(),
        chain: Vec::new(),
        resolver: GateOverrideResolver {
            active_gates: im::HashMap::with_hasher(rustc_hash::FxBuildHasher),
            gate_origins: im::HashMap::with_hasher(rustc_hash::FxBuildHasher),
        },
        x: Arc::from("FSC-A"),
        y: Arc::from("SSC-A"),
        // The scatter channels' own range, not the 0-1000 the synthetic mapper
        // above uses: on a real file that would put every event off the top of
        // the plot and draw an empty square, which would pass every assertion
        // here while proving nothing.
        x_axis: scatter("FSC-A"),
        y_axis: scatter("SSC-A"),
        gates: Vec::new(),
        size: 320,
    };

    let image = render_plot(&job).expect("a real file renders");
    assert!(image.parent_events > 0, "the file had no events");
    assert!(image.src.starts_with("data:image/jpeg;base64,"));
    assert_eq!(&image.jpeg[..2], &[0xFF, 0xD8], "not a JPEG");

    let sheets = vec![Sheet {
        title: "real sample".to_string(),
        slots: vec![
            Some(Drawn {
                name: "FMX".to_string(),
                jpeg: image.jpeg.clone(),
                shapes: Vec::new(),
                rendered_at: 320.0,
            }),
            None,
        ],
    }];
    let pdf = write_pdf("FSC-A / SSC-A", &sheets).expect("the PDF is written");
    // The renderer asked for 320; the page must state what the bytes say.
    assert!(String::from_utf8_lossy(&pdf).contains("/Width 320 /Height 320"));

    if let Ok(out) = std::env::var("GALLERY_PDF_OUT") {
        std::fs::write(&out, &pdf).expect("the PDF can be saved");
        eprintln!("wrote {out}");
    }
}

// ── where the two coordinate systems meet ────────────────────────────────

/// Pixel y runs down from the top of a plot; PDF y runs up from the bottom.
/// Getting that reflection wrong would draw every gate mirrored about the
/// middle of its plot - close enough to plausible on a symmetric cloud to go
/// unnoticed, and wrong on every sample.
#[test]
fn a_pixel_y_is_reflected_into_page_space() {
    let shapes = vec![Flat::Path {
        points: vec![(0.0, 0.0), (100.0, 0.0)],
        closed: false,
        stroke: "cyan",
        fill: "none",
        width: 2.0,
        dashed: false,
    }];
    let mut out = String::new();
    // A plot 100 points square whose bottom-left corner is the page origin, at
    // one point per pixel: the arithmetic is then readable by eye.
    super::pdf::draw_shapes(&mut out, &shapes, 0.0, 0.0, 100.0, 1.0);
    // The top edge in pixels (y = 0) is the top edge on the page (y = 100).
    assert!(out.contains("0.00 100.00 m"), "got {out}");
    assert!(out.contains("100.00 100.00 l"), "got {out}");
}

#[test]
fn a_plot_offset_on_the_page_carries_its_shapes_with_it() {
    let shapes = vec![Flat::Path {
        points: vec![(0.0, 100.0)],
        closed: false,
        stroke: "cyan",
        fill: "none",
        width: 2.0,
        dashed: false,
    }];
    let mut out = String::new();
    super::pdf::draw_shapes(&mut out, &shapes, 30.0, 40.0, 100.0, 1.0);
    // Bottom-left of the plot: x offset by the plot's own left edge, y at the
    // plot's own bottom.
    assert!(out.contains("30.00 40.00 m"), "got {out}");
}

#[test]
fn shapes_scale_with_the_plot() {
    // The gallery flattens against a 600-pixel render and the page gives the
    // plot 150 points, so every coordinate is a quarter of what it was.
    let shapes = vec![Flat::Path {
        points: vec![(300.0, 300.0)],
        closed: false,
        stroke: "cyan",
        fill: "none",
        width: 2.0,
        dashed: false,
    }];
    let mut out = String::new();
    super::pdf::draw_shapes(&mut out, &shapes, 0.0, 0.0, 150.0, 0.25);
    assert!(out.contains("75.00 75.00 m"), "got {out}");
}

#[test]
fn a_shape_with_no_stroke_is_not_drawn() {
    let shapes = vec![Flat::Path {
        points: vec![(0.0, 0.0), (10.0, 10.0)],
        closed: false,
        stroke: "none",
        fill: "none",
        width: 2.0,
        dashed: false,
    }];
    let mut out = String::new();
    super::pdf::draw_shapes(&mut out, &shapes, 0.0, 0.0, 100.0, 1.0);
    assert!(out.is_empty(), "got {out}");
}

#[test]
fn a_changed_cofactor_misses() {
    // A chain gate on some other channel keeps its `Arc` when that channel is
    // rescaled by a cofactor it is not drawn on, but the events it admits move.
    use super::cache::scaling_digest;
    let g = gate("a");
    let mut cache = PlotCache::default();
    let mut before = print("f1", vec![g.clone()]);
    before.scaling = scaling_digest(&[(Arc::from("CD3"), 150.0)]);
    cache.insert(before.clone(), picture());

    let mut after = print("f1", vec![g]);
    after.scaling = scaling_digest(&[(Arc::from("CD3"), 6000.0)]);
    assert!(cache.get(&before).is_some());
    assert!(cache.get(&after).is_none());
}

#[test]
fn the_scaling_digest_does_not_depend_on_order() {
    // The axis settings are a hash map; the same settings must digest the same
    // way whichever order they iterate in.
    use super::cache::scaling_digest;
    let a: Vec<(Arc<str>, f32)> = vec![
        (Arc::from("CD3"), 150.0),
        (Arc::from("CD4"), 300.0),
        (Arc::from("CD8"), 900.0),
    ];
    let b: Vec<(Arc<str>, f32)> = vec![
        (Arc::from("CD8"), 900.0),
        (Arc::from("CD3"), 150.0),
        (Arc::from("CD4"), 300.0),
    ];
    assert_eq!(scaling_digest(&a), scaling_digest(&b));
}

/// A gate that selects nothing is not an error - it is the most important
/// picture in a QC sheet, because it is the one that says a rule has failed on
/// this sample. An empty frame must draw an empty plot, not an error message
/// where a plot should be.
#[test]
fn a_real_gate_that_admits_nothing_still_draws() {
    let Ok(dir) = std::env::var("OMIQ_FCS_DIR") else {
        eprintln!("skipped: set OMIQ_FCS_DIR");
        return;
    };
    let Some(file) = std::fs::read_dir(&dir)
        .expect("the FCS directory can be read")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "fcs"))
        .min()
    else {
        eprintln!("skipped: no FCS files in {dir}");
        return;
    };

    use super::render::{PlotJob, render_plot};

    // A rectangle far outside anything a cytometer reports, so the chain lets
    // nothing through.
    let mut state = GateState::default();
    state
        .add_gate(
            &mapper(),
            300.0,
            300.0,
            Arc::from("FSC-A"),
            Arc::from("SSC-A"),
            None,
            Some(ROOTGATE.clone()),
            PrimaryGateType::Rectangle,
            Some("nothing".to_string()),
        )
        .expect("a gate can be added");
    let (node, gate_id) = state
        .placements()
        .map(|(n, p)| (n.as_arc().clone(), p.gate_id.clone()))
        .next()
        .expect("the new placement");
    let resolver = state.get_current_sample(Arc::from("f1"), &Default::default());
    let chain = super::select::chain_of(&state, &node);
    assert_eq!(chain, vec![gate_id], "the chain is the gate itself");

    let job = PlotJob {
        path: file,
        cofactors: Vec::new(),
        chain,
        resolver,
        x: Arc::from("FSC-A"),
        y: Arc::from("SSC-A"),
        x_axis: scatter("FSC-A"),
        y_axis: scatter("SSC-A"),
        gates: Vec::new(),
        size: 320,
    };
    let image = render_plot(&job).expect("an empty population still renders");
    assert_eq!(
        image.parent_events, 0,
        "the gate was meant to admit nothing"
    );
    assert_eq!(&image.jpeg[..2], &[0xFF, 0xD8], "not a JPEG");
}

// ── which gates land on a plot ───────────────────────────────────────────

/// Build a state with one gate under a parent, and return (parent node, the
/// resolver, the gates drawn on the parent's plot).
fn drawn_under(
    gate_x: &str,
    gate_y: &str,
) -> (
    Vec<Arc<dyn DrawableGate>>,
    crate::gate_editor::gates::gate_store::GateOverrideResolver,
) {
    let mut state = GateState::default();
    state
        .add_gate(
            &mapper(),
            300.0,
            300.0,
            Arc::from(gate_x),
            Arc::from(gate_y),
            None,
            Some(ROOTGATE.clone()),
            PrimaryGateType::Rectangle,
            Some("child".to_string()),
        )
        .expect("a gate can be added");
    let resolver = state.get_current_sample(Arc::from("f1"), &Default::default());
    let drawn = super::select::drawn_on(&state, &ROOTGATE, &resolver);
    (drawn, resolver)
}

#[test]
fn a_gate_already_on_the_plots_axes_is_drawn() {
    // The case that matters: `match_to_plot_axis` answers `Ok(None)` for a gate
    // that needs no rewriting, and reading that as "not on this plot" left the
    // gallery with no outlines at all.
    let (drawn, _) = drawn_under("FSC-A", "SSC-A");
    assert_eq!(drawn.len(), 1, "the child gate is under the root");
    let matched = super::select::matched_to_axes(&drawn, "FSC-A", "SSC-A");
    assert_eq!(matched.len(), 1, "a gate on these very axes must be drawn");
    assert!(
        Arc::ptr_eq(&matched[0], &drawn[0]),
        "nothing to rewrite, so it should be the gate itself"
    );
}

#[test]
fn a_gate_with_its_axes_the_other_way_round_is_transposed() {
    let (drawn, _) = drawn_under("FSC-A", "SSC-A");
    let matched = super::select::matched_to_axes(&drawn, "SSC-A", "FSC-A");
    assert_eq!(matched.len(), 1, "the same gate, read the other way round");
    assert_eq!(
        matched[0].get_params(),
        (Arc::from("SSC-A"), Arc::from("FSC-A"))
    );
}

#[test]
fn a_gate_on_another_pair_is_not_on_this_plot() {
    // Not a failure - a population usually carries gates on several pairs.
    let (drawn, _) = drawn_under("FSC-A", "SSC-A");
    assert!(super::select::matched_to_axes(&drawn, "CD3", "CD4").is_empty());
}

/// The axis settings describe the whole panel, but a given file need not carry
/// every channel in it. One absent name used to lose the entire plot, because
/// `apply_arcsinh_transforms` errors on the first parameter it cannot find.
#[test]
fn a_real_file_renders_with_cofactors_it_does_not_have() {
    let Ok(dir) = std::env::var("OMIQ_FCS_DIR") else {
        eprintln!("skipped: set OMIQ_FCS_DIR");
        return;
    };
    let Some(file) = std::fs::read_dir(&dir)
        .expect("the FCS directory can be read")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "fcs"))
        .min()
    else {
        eprintln!("skipped: no FCS files in {dir}");
        return;
    };

    use super::render::{PlotJob, render_plot};
    use crate::gate_editor::gates::gate_store::GateOverrideResolver;

    let job = PlotJob {
        path: file,
        cofactors: vec![
            // Not in any panel this program will meet.
            (Arc::from("No Such Channel-A"), 150.0),
            (Arc::from("Also Absent-A"), 6000.0),
        ],
        chain: Vec::new(),
        resolver: GateOverrideResolver {
            active_gates: im::HashMap::with_hasher(rustc_hash::FxBuildHasher),
            gate_origins: im::HashMap::with_hasher(rustc_hash::FxBuildHasher),
        },
        x: Arc::from("FSC-A"),
        y: Arc::from("SSC-A"),
        x_axis: scatter("FSC-A"),
        y_axis: scatter("SSC-A"),
        gates: Vec::new(),
        size: 320,
    };
    let image = render_plot(&job).expect("an absent channel must not lose the plot");
    assert!(image.parent_events > 0);
}

/// The shared cofactor filter, against a real panel.
///
/// Both the editor's plot and the gallery's go through this; before it existed
/// a channel in the scaling file but not in the FCS lost the whole plot.
#[test]
fn a_real_panel_keeps_its_own_channels_and_drops_the_rest() {
    let Ok(dir) = std::env::var("OMIQ_FCS_DIR") else {
        eprintln!("skipped: set OMIQ_FCS_DIR");
        return;
    };
    let Some(file) = std::fs::read_dir(&dir)
        .expect("the FCS directory can be read")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "fcs"))
        .min()
    else {
        eprintln!("skipped: no FCS files in {dir}");
        return;
    };
    let fcs = flow_fcs::Fcs::open(file.to_str().expect("utf-8 path")).expect("the file opens");

    use crate::gate_editor::plots::data_helpers::cofactors_carried_by;
    let real: Arc<str> = fcs
        .parameters
        .values()
        .next()
        .expect("the panel has channels")
        .channel_name
        .clone();

    let asked = vec![
        (real.clone(), 150.0f32),
        (Arc::from("No Such Channel-A"), 6000.0),
    ];
    let carried = cofactors_carried_by(&fcs, &asked);
    assert_eq!(carried.len(), 1, "one of the two is in this panel");
    assert_eq!(carried[0].0, real);
    assert_eq!(carried[0].1, 150.0, "the cofactor comes through unchanged");
}
