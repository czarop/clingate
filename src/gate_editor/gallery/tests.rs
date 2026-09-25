//! What can be tested without a renderer: the cache's staleness rule, the
//! flattening, and the PDF's structure.

#![cfg(test)]

use std::sync::Arc;

use super::cache::{Fingerprint, PlotCache};
use super::overlay::{Flat, flatten};
use super::pdf::{Cell, Drawn, Sheet, write_pdf};
use super::render::PlotImage;
use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::ROOTGATE;
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
            Cell::Drawn(Drawn {
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
            Cell::NoFile,
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
        slots: vec![Cell::NoFile, Cell::NoFile],
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
            Cell::Drawn(Drawn {
                name: "FMX".to_string(),
                jpeg: image.jpeg.clone(),
                shapes: Vec::new(),
                rendered_at: 320.0,
            }),
            Cell::NoFile,
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

// ── what a picture depends on ────────────────────────────────────────────

/// A parent gate under the root and a child under it; returns the state and
/// the parent's node.
fn parent_and_child() -> (GateState, Arc<str>) {
    let mut state = GateState::default();
    for (name, parent) in [("parent", None), ("child", Some("parent"))] {
        let parent = parent.map(|p| {
            state
                .registered_ids()
                .into_iter()
                .find(|id| state.registered_gate(id).is_some_and(|g| g.get_name() == p))
                .unwrap()
        });
        state
            .add_gate(
                &mapper(),
                300.0,
                300.0,
                Arc::from("FSC-A"),
                Arc::from("SSC-A"),
                None,
                parent.or(Some(ROOTGATE.clone())),
                PrimaryGateType::Rectangle,
                Some(name.to_string()),
            )
            .unwrap();
    }
    let parent = state
        .registered_ids()
        .into_iter()
        .find(|id| {
            state
                .registered_gate(id)
                .is_some_and(|g| g.get_name() == "parent")
        })
        .unwrap();
    let node = state
        .primary_node_for_gate(&parent)
        .unwrap()
        .as_arc()
        .clone();
    (state, node)
}

#[test]
fn a_picture_depends_on_its_filters_and_on_what_is_drawn_over_it() {
    let (state, node) = parent_and_child();
    let resolver = state.get_current_sample(Arc::from("f1"), &Default::default());
    let names: Vec<String> = super::select::dependencies(&state, &node, &resolver)
        .iter()
        .map(|g| g.get_name().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["parent", "child"],
        "the chain, then the drawing"
    );
}

#[test]
fn moving_a_gate_drawn_over_a_picture_changes_what_it_depends_on() {
    // The cache asks "has anything this picture depends on changed", so a
    // child moved on one file must show up in that file's dependencies -
    // and only that file's.
    use crate::gate_editor::gates::gate_store::GateSource;
    let (mut state, node) = parent_and_child();
    let child = state
        .registered_ids()
        .into_iter()
        .find(|id| {
            state
                .registered_gate(id)
                .is_some_and(|g| g.get_name() == "child")
        })
        .unwrap();
    let before_f1 = {
        let r = state.get_current_sample(Arc::from("f1"), &Default::default());
        super::select::dependencies(&state, &node, &r)
    };
    let moved = gate("child");
    state.place_gate(
        &[child.clone()],
        &moved,
        &GateSource::Sample((child, Arc::from("f1"))),
    );

    let after_f1 = {
        let r = state.get_current_sample(Arc::from("f1"), &Default::default());
        super::select::dependencies(&state, &node, &r)
    };
    let after_f2 = {
        let r = state.get_current_sample(Arc::from("f2"), &Default::default());
        super::select::dependencies(&state, &node, &r)
    };
    let same = |a: &[Arc<dyn DrawableGate>], b: &[Arc<dyn DrawableGate>]| {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| Arc::ptr_eq(x, y))
    };
    assert!(!same(&before_f1, &after_f1), "f1's picture must go stale");
    assert!(same(&before_f1, &after_f2), "f2's picture must not");
}

#[test]
fn a_whole_plot_flattens_every_gate_on_it() {
    use super::overlay::flatten_gates;
    let gates = vec![gate("a"), gate("b")];
    let one = |g: &Arc<dyn DrawableGate>| {
        flatten_gates(
            std::slice::from_ref(g),
            &Default::default(),
            None,
            &mapper(),
        )
        .len()
    };
    let both = flatten_gates(&gates, &Default::default(), None, &mapper());
    assert_eq!(both.len(), one(&gates[0]) + one(&gates[1]));
    assert!(flatten_gates(&[], &Default::default(), None, &mapper()).is_empty());
}

// ── the file's own structure ─────────────────────────────────────────────

/// Check what a PDF reader relies on before it draws anything: the
/// cross-reference table points at each object's first byte, every stream is
/// exactly as long as it says, and no number in a content stream is one a
/// reader cannot parse. The other tests look for strings in the output; a
/// file with every string present and one offset wrong still opens as
/// "damaged" in most readers.
fn assert_well_formed(bytes: &[u8]) {
    // Byte offsets, so bytes throughout: the file holds a binary comment and
    // raw JPEGs, and any conversion to text would move every offset after them.
    let find = |needle: &[u8], from: usize| -> Option<usize> {
        bytes[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|at| from + at)
    };
    let rfind =
        |needle: &[u8]| -> Option<usize> { bytes.windows(needle.len()).rposition(|w| w == needle) };
    let number_at = |at: usize| -> usize {
        let digits: Vec<u8> = bytes[at..]
            .iter()
            .copied()
            .take_while(u8::is_ascii_digit)
            .collect();
        String::from_utf8(digits)
            .unwrap()
            .parse()
            .unwrap_or_else(|_| panic!("a number at byte {at}"))
    };

    let startxref = rfind(b"startxref\n").expect("a startxref line");
    let xref_at = number_at(startxref + b"startxref\n".len());
    assert!(
        bytes[xref_at..].starts_with(b"xref\n0 "),
        "startxref does not point at the table"
    );
    let count = number_at(xref_at + b"xref\n0 ".len());
    let entries = find(b"\n", xref_at + b"xref\n".len()).unwrap() + 1;
    // Each entry is exactly twenty bytes, the free one first.
    assert!(bytes[entries..].starts_with(b"0000000000 65535 f \n"));
    for id in 1..count {
        let entry = entries + 20 * id;
        assert!(
            bytes[entry + 10..].starts_with(b" 00000 n \n"),
            "entry {id} is not twenty bytes"
        );
        let offset = number_at(entry);
        assert!(
            bytes[offset..].starts_with(format!("{id} 0 obj\n").as_bytes()),
            "object {id}'s entry points at {offset}, which is not its start"
        );
    }
    let trailer = find(b"trailer\n", entries).expect("a trailer");
    assert_eq!(
        number_at(find(b"/Size ", trailer).expect("a /Size") + b"/Size ".len()),
        count,
        "the trailer's /Size disagrees with the table"
    );

    // Every stream's /Length is its byte count.
    let mut from = 0;
    let mut streams = 0;
    while let Some(found) = find(b"/Length ", from) {
        let length = number_at(found + b"/Length ".len());
        let start = find(b"stream\n", found).expect("the stream") + b"stream\n".len();
        assert!(
            bytes[start + length..].starts_with(b"\nendstream"),
            "a stream of stated length {length} does not end there"
        );
        from = start + length;
        streams += 1;
    }
    assert!(streams > 0, "every page has a content stream");

    // The content streams are ASCII, so these searches cannot be fooled by
    // the conversion; a JPEG could only produce a false alarm, never hide one.
    let text = String::from_utf8_lossy(bytes);
    for bad in ["NaN", "inf"] {
        assert!(
            !text.contains(&format!(" {bad} ")) && !text.contains(&format!("-{bad} ")),
            "a content stream holds {bad}, which no reader can parse"
        );
    }
}

#[test]
fn the_cross_reference_table_and_stream_lengths_are_exact() {
    let sheets: Vec<Sheet> = (0..7).map(|i| sheet(&format!("D{i}"))).collect();
    assert_well_formed(&write_pdf("heading", &sheets).expect("wrote"));
}

#[test]
fn any_name_leaves_the_file_well_formed() {
    // Names come from folders and metadata: brackets, backslashes, accents
    // and the odd control character all reach the page. Each has to be
    // escaped or replaced without moving a byte the table counts.
    use rand::prelude::*;
    let alphabet: Vec<char> = "abcXYZ019 ()\\/%<>[]{}+-_.éµ→\t\n\r€'\"".chars().collect();
    for seed in 0..200 {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut name = || -> String {
            (0..rng.random_range(0..24))
                .map(|_| alphabet[rng.random_range(0..alphabet.len())])
                .collect()
        };
        let heading = name();
        let sheets: Vec<Sheet> = (0..3)
            .map(|_| {
                let mut s = sheet(&name());
                s.title = name();
                s
            })
            .collect();
        let bytes = write_pdf(&heading, &sheets).expect("wrote");
        assert_well_formed(&bytes);
        // Every string opened in a content stream is closed on the same line:
        // an unescaped bracket would leave one running into the operators.
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines().filter(|l| l.starts_with("BT ")) {
            assert!(line.ends_with(") Tj ET"), "seed {seed}: {line:?}");
            let body = &line[line.find('(').unwrap() + 1..line.rfind(") Tj").unwrap()];
            let mut depth = 0i32;
            let mut escaped = false;
            for c in body.chars() {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '(') => depth += 1,
                    (false, ')') => {
                        depth -= 1;
                        assert!(
                            depth >= 0,
                            "seed {seed}: a bracket ends the string: {line:?}"
                        );
                    }
                    _ => {}
                }
                assert!(
                    c.is_ascii() && c >= ' ',
                    "seed {seed}: {c:?} reached the page"
                );
            }
            assert!(
                !escaped,
                "seed {seed}: a trailing backslash escapes the close: {line:?}"
            );
        }
    }
}

// ── the export: plots from files on disk to a contact sheet ──────────────

mod the_export {
    use super::*;
    use crate::file_load_tests::{scratch, write_fcs_rows};
    use crate::gate_editor::gallery::export::{
        ContactSheet, ExportJob, NO_METADATA, ToDraw, contact_sheet,
    };
    use crate::gate_editor::gallery::render::PlotJob;
    use crate::gate_editor::gates::gate_store::GateOverrideResolver;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn small(name: &str) -> AxisInfo {
        AxisInfo {
            param: Param {
                marker: Arc::from(name),
                fluoro: Arc::from(name),
            },
            axis_lower: 0.0,
            axis_upper: 1000.0,
            transform: TransformType::Linear,
        }
    }

    /// A file of events spread over the plot, some inside `gate`'s square.
    fn file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(format!("{name}.fcs"));
        let rows: Vec<Vec<f32>> = (0..400)
            .map(|i| vec![(i % 20) as f32 * 50.0 + 5.0, (i / 20) as f32 * 50.0 + 5.0])
            .collect();
        write_fcs_rows(&path, &[("FSC-A", None), ("SSC-A", None)], &rows, &[]);
        path
    }

    fn job(
        card: usize,
        slot: usize,
        name: &str,
        path: PathBuf,
        drawn: Vec<Arc<dyn DrawableGate>>,
    ) -> ExportJob {
        ExportJob {
            card,
            slot,
            name: name.to_string(),
            plot: Ok(ToDraw {
                job: PlotJob {
                    path,
                    cofactors: Vec::new(),
                    chain: Vec::new(),
                    resolver: GateOverrideResolver {
                        active_gates: im::HashMap::with_hasher(rustc_hash::FxBuildHasher),
                        gate_origins: im::HashMap::with_hasher(rustc_hash::FxBuildHasher),
                    },
                    x: Arc::from("FSC-A"),
                    y: Arc::from("SSC-A"),
                    x_axis: small("FSC-A"),
                    y_axis: small("SSC-A"),
                    gates: drawn.clone(),
                    size: 160,
                },
                drawn,
                selected: None,
            }),
        }
    }

    fn run(
        titles: &[(String, usize)],
        jobs: &[ExportJob],
    ) -> (anyhow::Result<ContactSheet>, usize) {
        let done = AtomicUsize::new(0);
        let out = contact_sheet("heading", titles, jobs, &AtomicBool::new(false), &done);
        (out, done.load(Ordering::Relaxed))
    }

    #[test]
    fn each_plot_lands_in_its_own_specimens_slot() {
        let dir = scratch("export-slots");
        // Two specimens of two files; the second has no second file.
        let titles = vec![("first".to_string(), 2), ("second".to_string(), 2)];
        let jobs = vec![
            job(1, 0, "second-a", file(&dir, "s2a"), Vec::new()),
            job(0, 1, "first-b", file(&dir, "s1b"), Vec::new()),
            job(0, 0, "first-a", file(&dir, "s1a"), Vec::new()),
        ];
        let (pdf, done) = run(&titles, &jobs);
        let pdf = pdf.expect("the sheet is written").pdf;
        assert_eq!(done, 3, "every plot is counted");
        assert_well_formed(&pdf);

        let text = String::from_utf8_lossy(&pdf);
        assert_eq!(text.matches("/Filter /DCTDecode").count(), 3);
        assert_eq!(text.matches("(no paired file) Tj").count(), 1);
        // Placed by card and slot, not by the order the jobs came in: each
        // name is drawn in the column its slot gives it, under its specimen.
        let x_of = |name: &str| -> f32 {
            let line = text
                .lines()
                .find(|l| l.ends_with(&format!("({name}) Tj ET")))
                .unwrap_or_else(|| panic!("{name} is on the page"));
            let td: Vec<&str> = line.split_whitespace().collect();
            let at = td.iter().position(|t| *t == "Td").unwrap();
            td[at - 2].parse().unwrap()
        };
        assert!(
            x_of("first-a") < x_of("first-b"),
            "slot 0 is left of slot 1"
        );
        assert!(
            (x_of("first-a") - x_of("second-a")).abs() > 100.0,
            "the second specimen is in the next cell across"
        );
    }

    #[test]
    fn the_gates_drawn_come_with_their_percentages() {
        let dir = scratch("export-gates");
        let titles = vec![("only".to_string(), 1)];
        let jobs = vec![job(0, 0, "f", file(&dir, "f"), vec![gate("drawn")])];
        let (pdf, _) = run(&titles, &jobs);
        let pdf = pdf.expect("the sheet is written").pdf;
        assert_well_formed(&pdf);
        let text = String::from_utf8_lossy(&pdf);
        assert!(
            text.lines()
                .any(|l| l.starts_with("BT ") && l.contains("%) Tj")),
            "the gate's percentage is written as text on the page"
        );
        assert!(text.contains(" RG "), "the gate's outline is stroked");
    }

    #[test]
    fn a_stopped_export_writes_nothing() {
        let dir = scratch("export-stop");
        let titles = vec![("only".to_string(), 1)];
        let jobs = vec![job(0, 0, "f", file(&dir, "f"), Vec::new())];
        let done = AtomicUsize::new(0);
        let out = contact_sheet("h", &titles, &jobs, &AtomicBool::new(true), &done);
        assert!(out.is_err(), "a stopped run is not a record");
        assert_eq!(done.load(Ordering::Relaxed), 0, "nothing was rendered");
    }

    /// Was B-PDF-1. On screen a plot that cannot be drawn shows why, in its
    /// own frame. The export turned the same failure into an empty slot,
    /// printed "no paired file", and reported the sheet written as if clean.
    #[test]
    fn a_plot_that_fails_to_render_is_not_called_a_missing_file() {
        let dir = scratch("export-fail");
        let damaged = dir.join("damaged.fcs");
        std::fs::write(&damaged, b"not an FCS file").unwrap();
        let titles = vec![("only".to_string(), 2)];
        let jobs = vec![
            job(0, 0, "good", file(&dir, "good"), Vec::new()),
            job(0, 1, "damaged", damaged, Vec::new()),
        ];
        let (sheet, done) = run(&titles, &jobs);
        let sheet = sheet.expect("one bad file does not stop the sheet");
        assert_eq!(done, 2, "a failure is counted as done");
        assert_well_formed(&sheet.pdf);
        let text = String::from_utf8_lossy(&sheet.pdf);
        assert!(
            !text.contains("(no paired file) Tj"),
            "the damaged file was paired; the record says it was not"
        );
        assert!(
            text.contains("(damaged) Tj"),
            "the slot names the file that could not be drawn"
        );
        assert!(text.contains("(could not be drawn:) Tj"));
        assert_eq!(
            text.matches("/Filter /DCTDecode").count(),
            1,
            "the good plot is drawn"
        );
        assert_eq!(
            sheet.failed.len(),
            1,
            "the failure is reported: {:?}",
            sheet.failed
        );
        assert_eq!(sheet.failed[0].0, "damaged");
    }

    #[test]
    fn a_paired_file_with_no_metadata_says_so_on_the_sheet() {
        // Skipped before rendering, as the on-screen gallery does, and said in
        // the same words.
        let dir = scratch("export-no-metadata");
        let titles = vec![("only".to_string(), 2)];
        let jobs = vec![
            job(0, 0, "good", file(&dir, "good"), Vec::new()),
            ExportJob {
                card: 0,
                slot: 1,
                name: "unlisted".to_string(),
                plot: Err(NO_METADATA.to_string()),
            },
        ];
        let (sheet, done) = run(&titles, &jobs);
        let sheet = sheet.expect("written");
        assert_eq!(done, 2);
        let text = String::from_utf8_lossy(&sheet.pdf);
        assert!(text.contains("(unlisted) Tj"));
        assert!(text.contains(&format!("({NO_METADATA}) Tj")));
        assert!(!text.contains("(no paired file) Tj"));
        assert_eq!(
            sheet.failed,
            vec![("unlisted".to_string(), NO_METADATA.to_string())]
        );
    }

    #[test]
    fn a_long_reason_is_wrapped_inside_the_frame() {
        let reason = "Parameter BUV805-A not found: column BUV805-A not found in the frame \
                      read from /data/plate_12/a_very_long_folder_name_without_spaces_at_all.fcs";
        let sheets = vec![Sheet {
            title: "D1".to_string(),
            slots: vec![Cell::Failed {
                name: "D1 FS".to_string(),
                reason: reason.to_string(),
            }],
        }];
        let pdf = write_pdf("x", &sheets).expect("written");
        assert_well_formed(&pdf);
        let text = String::from_utf8_lossy(&pdf);
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| l.contains(" 6.50 Tf") && !l.contains("(D1 FS)"))
            .collect();
        assert!(lines.len() > 1, "wrapped over several lines: {lines:?}");
        for line in &lines {
            let body = &line[line.find('(').unwrap() + 1..line.rfind(") Tj").unwrap()];
            assert!(
                body.chars().count() <= 44,
                "{body:?} is wider than the frame"
            );
        }
        // Nothing lost in the wrapping.
        let joined: String = lines
            .iter()
            .map(|l| &l[l.find('(').unwrap() + 1..l.rfind(") Tj").unwrap()])
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(joined.replace(' ', ""), reason.replace(' ', ""));
    }
}
