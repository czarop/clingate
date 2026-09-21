//! Tests for drawing a boundary round a population.

#![cfg(test)]

use super::shape_fit::*;

/// Deterministic scatter, so a failure is reproducible.
struct Cloud(u64);

impl Cloud {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((self.0 >> 32) as u32) as f64 / u32::MAX as f64
    }
    /// Roughly normal, by the central limit theorem on four uniforms.
    fn normal(&mut self) -> f64 {
        (0..4).map(|_| self.unit()).sum::<f64>() - 2.0
    }
    fn blob(&mut self, centre: (f64, f64), spread: (f64, f64), n: usize) -> Vec<(f64, f64)> {
        (0..n)
            .map(|_| {
                (
                    centre.0 + self.normal() * spread.0,
                    centre.1 + self.normal() * spread.1,
                )
            })
            .collect()
    }
}

fn fitted(points: &[(f64, f64)]) -> Fitted {
    fit(points, &[], 0.95, 1.0, 24).expect("a boundary can be drawn")
}

#[test]
fn a_round_blob_gets_a_round_boundary() {
    let mut rng = Cloud(1);
    let points = rng.blob((0.0, 0.0), (1.0, 1.0), 4000);
    let drawn = fitted(&points);

    assert_eq!(drawn.pieces, 1, "a single blob is one piece");
    // Roughly circular: the outline's extent should be similar on both axes.
    let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
    for (x, y) in &drawn.outline.0 {
        min_x = min_x.min(*x);
        max_x = max_x.max(*x);
        min_y = min_y.min(*y);
        max_y = max_y.max(*y);
    }
    let ratio = (max_x - min_x) / (max_y - min_y);
    assert!(
        (0.75..1.35).contains(&ratio),
        "the boundary is {ratio:.2} times as wide as it is tall"
    );
}

#[test]
fn an_elongated_blob_gets_an_elongated_boundary() {
    // The point of fitting rather than translating: the shape follows the
    // cells, so a population that is wide in one sample and narrow in the next
    // gets a gate of the right proportions either way.
    let mut rng = Cloud(2);
    let points = rng.blob((0.0, 0.0), (3.0, 0.5), 4000);
    let drawn = fitted(&points);
    let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
    for (x, y) in &drawn.outline.0 {
        min_x = min_x.min(*x);
        max_x = max_x.max(*x);
        min_y = min_y.min(*y);
        max_y = max_y.max(*y);
    }
    let ratio = (max_x - min_x) / (max_y - min_y);
    assert!(ratio > 3.0, "the boundary is only {ratio:.2} times as wide");
}

#[test]
fn the_boundary_holds_most_of_the_population() {
    let mut rng = Cloud(3);
    let points = rng.blob((2.0, -1.0), (1.0, 1.0), 3000);
    let drawn = fitted(&points);
    assert!(
        drawn.caught > 0.85,
        "it caught only {:.0}%",
        drawn.caught * 100.0
    );
}

#[test]
fn a_straggler_does_not_drag_the_boundary_out() {
    // What a convex hull cannot do: one misassigned cell far away would be a
    // vertex of the hull and would take the gate with it.
    let mut rng = Cloud(4);
    let mut points = rng.blob((0.0, 0.0), (1.0, 1.0), 3000);
    let tight = fitted(&points).outline.area();
    points.push((60.0, 60.0));
    let with_straggler = fitted(&points).outline.area();
    assert!(
        with_straggler < tight * 1.5,
        "the area went from {tight:.1} to {with_straggler:.1}"
    );
}

#[test]
fn two_separate_clouds_are_reported_as_two_pieces() {
    // The signal that a phenotype has matched cells in two places on this
    // plot: either the population really is split, or the signature is
    // catching something else too.
    let mut rng = Cloud(5);
    let mut points = rng.blob((0.0, 0.0), (0.6, 0.6), 2000);
    points.extend(rng.blob((12.0, 12.0), (0.6, 0.6), 2000));
    let drawn = fit(&points, &[], 0.95, 1.0, 24).expect("a boundary can be drawn");
    assert!(
        drawn.pieces >= 2,
        "two clouds came back as {} piece(s)",
        drawn.pieces
    );
    // And the outline is one of them, not a blanket over both.
    assert!(
        drawn.caught < 0.75,
        "the outline caught {:.0}% - it has spanned the gap",
        drawn.caught * 100.0
    );
}

#[test]
fn purity_says_when_the_population_is_not_separated_here() {
    // The honest limit of the whole approach: the gate is two-dimensional, so
    // where the population overlaps its neighbours on these axes, no boundary
    // round it can exclude them.
    let mut rng = Cloud(6);
    let population = rng.blob((0.0, 0.0), (1.0, 1.0), 2000);

    let apart = rng.blob((15.0, 15.0), (1.0, 1.0), 2000);
    let separated = fit(&population, &apart, 0.95, 1.0, 24).expect("drawn");
    assert!(
        separated.purity > 0.95,
        "a separated population read {:.2}",
        separated.purity
    );

    let on_top = rng.blob((0.0, 0.0), (1.0, 1.0), 2000);
    let overlapping = fit(&population, &on_top, 0.95, 1.0, 24).expect("drawn");
    assert!(
        overlapping.purity < 0.65,
        "an overlapping population read {:.2} - purity is not reporting the overlap",
        overlapping.purity
    );
}

#[test]
fn the_outline_is_kept_to_the_vertex_count_asked_for() {
    // A gate with two hundred points is a different kind of object from one a
    // person drew, however well it fits.
    let mut rng = Cloud(7);
    let points = rng.blob((0.0, 0.0), (1.0, 1.0), 4000);
    for target in [8, 16, 32] {
        let drawn = fit(&points, &[], 0.95, 1.0, target).expect("drawn");
        assert!(
            drawn.outline.0.len() <= target,
            "asked for {target}, got {}",
            drawn.outline.0.len()
        );
        assert!(drawn.outline.0.len() >= 3);
    }
}

#[test]
fn the_outline_is_wound_counter_clockwise() {
    let mut rng = Cloud(8);
    let points = rng.blob((0.0, 0.0), (1.0, 1.0), 2000);
    let drawn = fitted(&points);
    // Shoelace: positive means counter-clockwise, which every consumer can
    // then assume rather than test for.
    let ring = &drawn.outline.0;
    let mut twice = 0.0;
    for i in 0..ring.len() {
        let (x1, y1) = ring[i];
        let (x2, y2) = ring[(i + 1) % ring.len()];
        twice += x1 * y2 - x2 * y1;
    }
    assert!(twice > 0.0, "the ring is wound the other way");
}

#[test]
fn too_few_events_is_refused_rather_than_guessed() {
    let mut rng = Cloud(9);
    let points = rng.blob((0.0, 0.0), (1.0, 1.0), 5);
    match fit(&points, &[], 0.95, 1.0, 24) {
        Err(NoShape::TooFew { events, .. }) => assert_eq!(events, 5),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_population_with_no_extent_is_refused() {
    let points = vec![(1.0, 1.0); 200];
    assert!(matches!(
        fit(&points, &[], 0.95, 1.0, 24),
        Err(NoShape::NoContour)
    ));
}

#[test]
fn a_point_inside_the_outline_is_held_and_one_outside_is_not() {
    let square = Outline(vec![(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]);
    assert!(square.holds((1.0, 1.0)));
    assert!(!square.holds((3.0, 1.0)));
    assert!(!square.holds((1.0, -1.0)));
    assert!((square.area() - 4.0).abs() < 1e-9);
}

/// Dump a fitted boundary and the cells it was fitted to, for looking at.
/// Set SHAPE_FIT_OUT to a path; skipped otherwise.
#[test]
fn a_fitted_shape_can_be_looked_at() {
    let Ok(out) = std::env::var("SHAPE_FIT_OUT") else {
        return;
    };
    let mut rng = Cloud(42);
    // A crescent: concave, which is the case a convex hull cannot express and
    // an ellipse fit would smear across the hollow.
    let mut points = Vec::new();
    for _ in 0..6000 {
        let angle = rng.unit() * std::f64::consts::PI * 1.25;
        let radius = 5.0 + rng.normal() * 0.6;
        points.push((angle.cos() * radius, angle.sin() * radius));
    }
    let drawn = fit(&points, &[], 0.95, 1.0, 28).expect("drawn");
    let mut text = String::from("kind,x,y\n");
    for (x, y) in &points {
        text.push_str(&format!("point,{x},{y}\n"));
    }
    for (x, y) in &drawn.outline.0 {
        text.push_str(&format!("outline,{x},{y}\n"));
    }
    std::fs::write(&out, text).expect("written");
    eprintln!(
        "wrote {out}: {} vertices, {} piece(s), caught {:.0}%",
        drawn.outline.0.len(),
        drawn.pieces,
        drawn.caught * 100.0
    );
}

// ── keeping the shape ────────────────────────────────────────────────────

#[test]
fn a_shape_follows_its_population_across() {
    let mut rng = Cloud(11);
    let from = rng.blob((0.0, 0.0), (1.0, 1.0), 2000);
    let to = rng.blob((5.0, -3.0), (1.0, 1.0), 2000);
    let moved = Reshape::between(&from, &to);
    // Same size, new place.
    assert!(
        (moved.scale.0 - 1.0).abs() < 0.15,
        "x scale {}",
        moved.scale.0
    );
    assert!(
        (moved.scale.1 - 1.0).abs() < 0.15,
        "y scale {}",
        moved.scale.1
    );
    let centre = moved.moved((0.0, 0.0));
    assert!((centre.0 - 5.0).abs() < 0.3, "x centre {}", centre.0);
    assert!((centre.1 + 3.0).abs() < 0.3, "y centre {}", centre.1);
}

#[test]
fn a_shape_grows_with_a_population_that_spread_out() {
    let mut rng = Cloud(12);
    let from = rng.blob((0.0, 0.0), (1.0, 1.0), 3000);
    let to = rng.blob((0.0, 0.0), (2.5, 1.0), 3000);
    let moved = Reshape::between(&from, &to);
    assert!(
        (moved.scale.0 - 2.5).abs() < 0.4,
        "x should have grown 2.5x, got {}",
        moved.scale.0
    );
    assert!(
        (moved.scale.1 - 1.0).abs() < 0.2,
        "y should not have changed, got {}",
        moved.scale.1
    );
}

#[test]
fn the_shape_is_kept_even_as_it_is_resized() {
    // The whole point of this mode: proportions within the outline survive.
    // A square stays square-cornered; only its size and place change.
    let mut rng = Cloud(13);
    let from = rng.blob((0.0, 0.0), (1.0, 1.0), 2000);
    let to = rng.blob((10.0, 10.0), (2.0, 2.0), 2000);
    let moved = Reshape::between(&from, &to);

    let square = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let out = moved.apply(&square);
    // Still a rectangle: opposite sides equal, and the corners still square.
    let width_bottom = out[1].0 - out[0].0;
    let width_top = out[2].0 - out[3].0;
    let height_left = out[3].1 - out[0].1;
    let height_right = out[2].1 - out[1].1;
    assert!((width_bottom - width_top).abs() < 1e-9);
    assert!((height_left - height_right).abs() < 1e-9);
    assert!((out[0].1 - out[1].1).abs() < 1e-9, "the bottom edge tilted");
}

#[test]
fn a_shape_off_to_one_side_does_not_fly_away_when_resized() {
    // Scaling about the origin rather than the population's centre would send
    // a gate drawn far from zero a long way off when its population widened.
    let mut rng = Cloud(14);
    let from = rng.blob((100.0, 100.0), (1.0, 1.0), 2000);
    let to = rng.blob((100.0, 100.0), (3.0, 3.0), 2000);
    let moved = Reshape::between(&from, &to);
    let centre = moved.moved((100.0, 100.0));
    assert!(
        (centre.0 - 100.0).abs() < 1.0 && (centre.1 - 100.0).abs() < 1.0,
        "the centre moved to {centre:?}"
    );
}

#[test]
fn an_absurd_stretch_is_clamped_and_said_so() {
    let mut rng = Cloud(15);
    let from = rng.blob((0.0, 0.0), (0.01, 1.0), 2000);
    let to = rng.blob((0.0, 0.0), (10.0, 1.0), 2000);
    let moved = Reshape::between(&from, &to);
    assert!(moved.clamped, "a 1000x stretch was not flagged");
    assert!(
        moved.scale.0 <= MAX_STRETCH + 1e-9,
        "scale {}",
        moved.scale.0
    );
}

#[test]
fn an_ordinary_change_of_size_is_not_flagged() {
    let mut rng = Cloud(16);
    let from = rng.blob((0.0, 0.0), (1.0, 1.0), 2000);
    let to = rng.blob((0.0, 0.0), (1.4, 0.8), 2000);
    assert!(!Reshape::between(&from, &to).clamped);
}

#[test]
fn a_stray_cell_does_not_set_the_size_of_the_gate() {
    // Median and MAD, not mean and standard deviation: the signature will
    // occasionally catch something far away, and it must not inflate the gate.
    let mut rng = Cloud(17);
    let from = rng.blob((0.0, 0.0), (1.0, 1.0), 2000);
    let mut to = rng.blob((0.0, 0.0), (1.0, 1.0), 2000);
    let honest = Reshape::between(&from, &to).scale;
    to.push((500.0, 500.0));
    to.push((-500.0, -500.0));
    let with_strays = Reshape::between(&from, &to).scale;
    assert!(
        (with_strays.0 - honest.0).abs() < 0.05,
        "two strays moved the scale from {} to {}",
        honest.0,
        with_strays.0
    );
}
