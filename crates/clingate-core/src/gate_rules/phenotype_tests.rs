//! Tests for describing a population by its phenotype and finding it again.

#![cfg(test)]

use super::phenotype::*;
use std::sync::Arc;

fn markers(names: &[&str]) -> Vec<Arc<str>> {
    names.iter().map(|n| Arc::from(*n)).collect()
}

/// A population scattered round a centre, deterministic so a failure is
/// reproducible. A plain linear congruential generator: the distribution only
/// has to be a cloud, not a good normal.
struct Cloud(u64);

impl Cloud {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        // Two uniforms averaged, centred: enough of a bell for these tests.
        let a = ((self.0 >> 32) as u32) as f64 / u32::MAX as f64;
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let b = ((self.0 >> 32) as u32) as f64 / u32::MAX as f64;
        a + b - 1.0
    }

    /// A cloud as the flat row-major matrix the code takes.
    fn around(&mut self, centre: &[f64], spread: f64, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n * centre.len());
        for _ in 0..n {
            for c in centre {
                out.push((c + self.next() * spread) as f32);
            }
        }
        out
    }
}

/// A matrix view over a flat cloud.
fn rows(values: &[f32], markers: usize) -> Rows<'_> {
    Rows::new(values, markers).expect("a whole number of rows")
}

/// One marker's column, for the tests that read values back.
fn column(values: &[f32], markers: usize, marker: usize) -> Vec<f64> {
    rows(values, markers).column(marker).collect()
}

// ── the arithmetic ───────────────────────────────────────────────────────

#[test]
fn a_robust_baseline_ignores_the_bright_tail() {
    // The parent contains the very population being described. A mean and a
    // standard deviation would be dragged by it; that is the whole reason for
    // median and MAD.
    let mut values: Vec<f64> = (0..95).map(|i| i as f64 / 95.0).collect();
    values.extend((0..5).map(|_| 1000.0));
    let base = Baseline::of(&values);
    assert!(
        base.median < 1.0,
        "the median sits in the bulk, got {}",
        base.median
    );
    assert!(
        base.spread < 2.0,
        "the spread is the bulk's, got {}",
        base.spread
    );
}

#[test]
fn a_marker_with_no_spread_does_not_make_every_cell_infinite() {
    // An empty or saturated detector has a MAD of zero. Dividing by it would
    // put every cell infinitely far from the middle.
    let base = Baseline::of(&[3.0; 200]);
    assert!(base.spread > 0.0);
    assert!(base.z(3.0).is_finite());
    assert!(base.z(4.0).is_finite());
}

// ── describing and finding a population ──────────────────────────────────

#[test]
fn a_population_finds_itself() {
    let mut rng = Cloud(1);
    let mut parent = rng.around(&[0.0, 0.0, 0.0], 1.0, 800);
    let population = rng.around(&[6.0, 0.0, -4.0], 0.4, 120);
    parent.extend_from_slice(&population);

    let signature = Signature::describe(
        markers(&["a", "b", "c"]),
        rows(&population, 3),
        rows(&parent, 3),
    )
    .expect("a signature can be described");
    let found = signature.find_in(rows(&parent, 3));

    // Every member should be found, and few others.
    assert!(
        found.members.len() >= 110,
        "found only {} of 120",
        found.members.len()
    );
    assert!(
        found.members.len() < 160,
        "found {} - it is taking in the background",
        found.members.len()
    );
}

#[test]
fn a_population_is_found_where_the_whole_panel_moved() {
    // The donor case: the same cells, every marker shifted, and the spread
    // changed too. Because each sample is baselined against its own parent,
    // none of that should matter.
    let mut rng = Cloud(2);
    let mut reference = rng.around(&[0.0, 0.0, 0.0], 1.0, 800);
    let population = rng.around(&[6.0, 0.0, -4.0], 0.4, 120);
    reference.extend_from_slice(&population);
    let signature = Signature::describe(
        markers(&["a", "b", "c"]),
        rows(&population, 3),
        rows(&reference, 3),
    )
    .expect("described");

    // The same structure, shifted and stretched as a different donor's would be.
    let shift = [3.0f32, -2.0, 1.5];
    let stretch = 1.6f32;
    let moved = |flat: &[f32]| -> Vec<f32> {
        flat.chunks_exact(3)
            .flat_map(|row| {
                row.iter()
                    .zip(shift.iter())
                    .map(|(v, s)| v * stretch + s)
                    .collect::<Vec<f32>>()
            })
            .collect()
    };
    let sample = moved(&reference);
    let expected = moved(&population);

    let found = signature.find_in(rows(&sample, 3));
    assert!(
        found.members.len() >= 100,
        "found only {} of 120 after the panel moved",
        found.members.len()
    );
    // And they are the right cells: the members sit where the moved population
    // does, not where the background does.
    let centre = expected[0] as f64;
    let sample_rows = rows(&sample, 3);
    let matched_first: Vec<f64> = found
        .members
        .iter()
        .map(|at| sample_rows.row(*at)[0] as f64)
        .collect();
    let mean = matched_first.iter().sum::<f64>() / matched_first.len() as f64;
    assert!(
        (mean - centre).abs() < 3.0,
        "matched cells centre on {mean}, the population is near {centre}"
    );
}

#[test]
fn a_rarer_population_is_still_found() {
    // 7% in the reference against 1% in the sample: the question that defeats
    // anything matching on density. A phenotype does not care how many there
    // are.
    let mut rng = Cloud(3);
    let mut reference = rng.around(&[0.0, 0.0, 0.0], 1.0, 930);
    let population = rng.around(&[6.0, 0.0, -4.0], 0.4, 70);
    reference.extend_from_slice(&population);
    let signature = Signature::describe(
        markers(&["a", "b", "c"]),
        rows(&population, 3),
        rows(&reference, 3),
    )
    .expect("described");

    let mut sample = rng.around(&[0.0, 0.0, 0.0], 1.0, 990);
    let rare = rng.around(&[6.0, 0.0, -4.0], 0.4, 10);
    sample.extend_from_slice(&rare);

    let found = signature.find_in(rows(&sample, 3));
    assert!(
        found.members.len() >= 8,
        "found only {} of 10",
        found.members.len()
    );
    assert!(
        found.fraction() < 0.05,
        "it took in {:.1}% of the parent",
        found.fraction() * 100.0
    );
}

#[test]
fn a_population_that_is_not_there_matches_almost_nothing() {
    // The failure that has to be visible. When the population is absent the
    // nearest cells are still *something*, so the count is what says so - and
    // it must not quietly come back full.
    let mut rng = Cloud(4);
    let mut reference = rng.around(&[0.0, 0.0, 0.0], 1.0, 800);
    let population = rng.around(&[6.0, 0.0, -4.0], 0.4, 120);
    reference.extend_from_slice(&population);
    let signature = Signature::describe(
        markers(&["a", "b", "c"]),
        rows(&population, 3),
        rows(&reference, 3),
    )
    .expect("described");

    // Background only.
    let sample = rng.around(&[0.0, 0.0, 0.0], 1.0, 900);
    let found = signature.find_in(rows(&sample, 3));
    assert!(
        found.fraction() < 0.02,
        "matched {:.1}% of a sample with no such population",
        found.fraction() * 100.0
    );
}

#[test]
fn a_signature_needs_members() {
    let parent = vec![0.0f32; 20];
    assert!(Signature::describe(markers(&["a", "b"]), rows(&[], 2), rows(&parent, 2)).is_none());
}

#[test]
fn a_signature_remembers_how_many_cells_described_it() {
    let mut rng = Cloud(5);
    let parent = rng.around(&[0.0, 0.0], 1.0, 200);
    let population = rng.around(&[3.0, 3.0], 0.3, 17);
    let signature =
        Signature::describe(markers(&["a", "b"]), rows(&population, 2), rows(&parent, 2))
            .expect("described");
    assert_eq!(signature.members, 17);
}

// ── the headline claim ───────────────────────────────────────────────────

/// How common a population is must not change where it is found.
///
/// This is the whole reason the baseline is taken with the tail cut off. The
/// parent contains the population, so a plain robust spread is inflated by it
/// in proportion to how much of it there is - and that is exactly what differs
/// between one donor and the next. Untreated, the same cells read several
/// widths lower in a sample that has more of them.
#[test]
fn how_common_the_population_is_does_not_move_where_it_sits() {
    let centre_at = |fraction: f64| -> f64 {
        let mut rng = Cloud(9);
        let total = 2000;
        let members = (total as f64 * fraction) as usize;
        let mut parent = rng.around(&[0.0], 1.0, total - members);
        parent.extend_from_slice(&rng.around(&[6.0], 0.4, members));
        let base = Baseline::of(&column(&parent, 1, 0));
        base.z(6.0)
    };

    let rare = centre_at(0.01);
    let common = centre_at(0.20);
    assert!(
        (rare - common).abs() / rare < 0.08,
        "the same cells read {rare:.2} at 1% and {common:.2} at 20%"
    );
}

#[test]
fn an_untrimmed_baseline_would_have_moved_it() {
    // The control for the test above: without cutting the tail off, the same
    // measurement drifts with abundance. If this ever stops being true the
    // trimming has become unnecessary - and the test above would no longer be
    // evidence of anything.
    let untrimmed_z = |fraction: f64| -> f64 {
        let mut rng = Cloud(9);
        let total = 2000;
        let members = (total as f64 * fraction) as usize;
        let mut parent = rng.around(&[0.0], 1.0, total - members);
        parent.extend_from_slice(&rng.around(&[6.0], 0.4, members));
        let mut column = column(&parent, 1, 0);
        column.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = column[column.len() / 2];
        let mut deviations: Vec<f64> = column.iter().map(|v| (v - median).abs()).collect();
        deviations.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let spread = deviations[deviations.len() / 2] * 1.4826;
        (6.0 - median) / spread
    };
    let rare = untrimmed_z(0.01);
    let common = untrimmed_z(0.20);
    assert!(
        (rare - common).abs() / rare > 0.15,
        "untrimmed read {rare:.2} at 1% and {common:.2} at 20% - too close for this to be the reason trimming exists"
    );
}

// ── values a real file can hold ───────────────────────────────────────────

/// Was B-PHEN-1: `Baseline::of` did not drop non-finite values. The first,
/// untrimmed median was sorted with the NaNs in it (by
/// `partial_cmp().unwrap_or(Equal)`, which is not an order) and landed on
/// one; every z was then NaN, the trimming pass kept nothing, and the NaN
/// estimate was returned - `median: NaN, spread: 1e-6`. One corrupt event
/// disabled that marker for the whole phenotype match.
#[test]
fn a_nan_among_the_values_does_not_move_the_baseline() {
    // A corrupt event or a transform of a negative that went wrong: one NaN
    // in a marker's column. It is not a value, so it should not count - and
    // it must not take the sort down with it.
    let clean: Vec<f64> = (0..1_001).map(|i| (i as f64) / 100.0).collect();
    let mut dirty = clean.clone();
    dirty.insert(500, f64::NAN);
    dirty.insert(0, f64::NAN);

    let (a, b) = (Baseline::of(&clean), Baseline::of(&dirty));
    assert!(b.median.is_finite() && b.spread.is_finite(), "{b:?}");
    assert!(
        (a.median - b.median).abs() < 0.02,
        "{} vs {}",
        a.median,
        b.median
    );
    assert!(
        (a.spread - b.spread).abs() / a.spread < 0.02,
        "{} vs {}",
        a.spread,
        b.spread
    );
}

/// Left out, not merely outvoted: with NaN and infinite values mixed in, the
/// baseline is exactly the one of the finite values alone.
#[test]
fn values_that_are_not_numbers_are_left_out_of_the_baseline_exactly() {
    let clean: Vec<f64> = (0..997).map(|i| ((i * 37) % 997) as f64 / 10.0).collect();
    let mut dirty = clean.clone();
    for (at, bad) in [
        (0, f64::NAN),
        (300, f64::INFINITY),
        (600, f64::NEG_INFINITY),
        (997, f64::NAN),
    ] {
        dirty.insert(at, bad);
    }
    let (a, b) = (Baseline::of(&clean), Baseline::of(&dirty));
    assert_eq!((a.median, a.spread), (b.median, b.spread));
}

/// A marker with no numbers at all reads as an empty one does.
#[test]
fn a_marker_with_no_numbers_reads_as_an_empty_one() {
    let (none, empty) = (Baseline::of(&[f64::NAN, f64::INFINITY]), Baseline::of(&[]));
    assert_eq!((none.median, none.spread), (empty.median, empty.spread));
}

#[test]
fn rows_are_copied_out_in_the_order_asked_for() {
    let values = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let rows = Rows::new(&values, 2).expect("whole rows");
    assert_eq!(rows.select(&[2, 0]), vec![5.0, 6.0, 1.0, 2.0]);
    assert!(rows.select(&[]).is_empty());
}

// ── each marker on its own landmarks ─────────────────────────────────────

/// A negative of `negatives` at 0 and a positive of `positives` at 10, both
/// spread 1, on one marker.
fn two_peaks(rng: &mut Cloud, negatives: usize, positives: usize) -> Vec<f32> {
    let mut values = rng.around(&[0.0], 1.0, negatives);
    values.extend_from_slice(&rng.around(&[10.0], 1.0, positives));
    values
}

/// The clouds end a spread from their centre, so the valley is the middle of
/// the empty stretch between 1 and 9: 0 at the negative's peak, 5 between.
#[test]
fn a_marker_with_a_negative_and_a_positive_is_read_from_its_landmarks() {
    let values = column(&two_peaks(&mut Cloud(11), 700, 300), 1, 0);
    let landmarks = Frames::of(&values).landmarks.expect("there is a valley");
    assert!(landmarks.by_landmarks);
    assert!(landmarks.origin.abs() < 0.3, "{landmarks:?}");
    assert!((landmarks.unit - 5.0).abs() < 0.5, "{landmarks:?}");
}

#[test]
fn a_marker_with_one_population_has_no_landmarks() {
    let values = column(&Cloud(12).around(&[0.0], 1.0, 1000), 1, 0);
    assert_eq!(Frames::of(&values).landmarks, None);
}

/// Landmarks only where both samples have them: a marker read one way on
/// the reference and another here would compare two different scales.
#[test]
fn two_samples_share_landmarks_only_when_both_have_them() {
    let both = Frames::of(&column(&two_peaks(&mut Cloud(13), 700, 300), 1, 0));
    let one = Frames::of(&column(&Cloud(14).around(&[0.0], 1.0, 1000), 1, 0));
    let (mine, theirs) = both.shared_with(&both);
    assert!(mine.by_landmarks && theirs.by_landmarks);
    let (mine, theirs) = both.shared_with(&one);
    assert!(!mine.by_landmarks && !theirs.by_landmarks);
    assert_eq!((mine, theirs), (both.spread, one.spread));
}

/// The positives are half the parent on the reference and a tenth here. Read
/// against the parent's middle they would sit in different places - the
/// middle of a parent half positive is between the peaks - but the negative's
/// peak and the valley do not move, so the same cells are found.
#[test]
fn a_population_is_found_however_much_of_its_parent_it_is() {
    let mut rng = Cloud(15);
    let reference = two_peaks(&mut rng, 500, 500);
    let positives: Vec<f32> = reference[500..].to_vec();
    let signature = Signature::describe(markers(&["a"]), rows(&positives, 1), rows(&reference, 1))
        .expect("described");
    let sample = two_peaks(&mut rng, 900, 100);
    let found = signature.find_in(rows(&sample, 1));
    assert!(
        (95..=100).contains(&found.members.len()) && found.members.iter().all(|at| *at >= 900),
        "found {} cells, the positives are 900..1000",
        found.members.len()
    );
    assert!(found.reads[0].by_landmarks && !found.reads[0].drifted());
}

/// Seven markers, and cells that are the population on six and out on one -
/// CD8 T cells that are CD4-positive. They are not the population. Out by
/// 0.6 where the population spreads 0.4 at most: one combined distance over
/// all seven would have let them in (3.7 of the population's standard
/// deviations, under the 3.75 seven markers allow), so no marker may be
/// outvoted by the rest.
#[test]
fn a_cell_out_on_one_marker_is_not_a_match_however_well_the_rest_agree() {
    let centre = [6.0, 0.0, -4.0, 2.0, 0.0, 3.0, -2.0];
    let mut rng = Cloud(16);
    let mut parent = rng.around(&[0.0; 7], 1.0, 1000);
    let population = rng.around(&centre, 0.4, 300);
    parent.extend_from_slice(&population);
    let signature = Signature::describe(
        markers(&["CD3", "CD4", "CD8", "gdTCR", "CD56", "CD161", "Va7.2"]),
        rows(&population, 7),
        rows(&parent, 7),
    )
    .expect("described");

    let mut impostor = centre.map(|v| v as f32);
    impostor[1] += 0.6;
    let mut sample = parent.clone();
    let first_impostor = sample.len() / 7;
    for _ in 0..100 {
        sample.extend_from_slice(&impostor);
    }
    let found = signature.find_in(rows(&sample, 7));
    assert!(
        found.members.iter().all(|at| *at < first_impostor),
        "an impostor was matched"
    );
    assert!(found.members.len() >= 280, "found {}", found.members.len());
}

/// The ranges hold the reference's own cells - all markers at once - and no
/// more than they need to.
#[test]
fn the_ranges_hold_the_share_of_the_reference_population_asked_for() {
    let mut rng = Cloud(17);
    let parent = rng.around(&[0.0, 0.0, 0.0], 1.0, 600);
    let population = rng.around(&[5.0, -3.0, 2.0], 0.5, 400);
    let signature = Signature::describe(
        markers(&["a", "b", "c"]),
        rows(&population, 3),
        rows(&parent, 3),
    )
    .expect("described");
    let held = rows(&population, 3)
        .rows()
        .filter(|row| {
            row.iter()
                .zip(&signature.profiles)
                .all(|(v, profile)| (profile.low..=profile.high).contains(&f64::from(*v)))
        })
        .count() as f64
        / 400.0;
    assert!((KEEP..KEEP + 0.02).contains(&held), "held {held}");
}

/// A marker read across the valley - the population dim - so held to its
/// spread.
fn read(reference_middle: f64, reference_spread: f64, middle: f64) -> MarkerRead {
    MarkerRead {
        marker: Arc::from("CD4"),
        by_landmarks: true,
        identity: Identity::Between(0.5, 3.5),
        reference_middle,
        reference_spread,
        middle,
    }
}

/// Middle 2, spread 0.5: allowed 0.5 + a tenth of 2 = 0.7 either way.
#[test]
fn matched_cells_of_a_dim_population_have_drifted_when_further_than_its_spread() {
    assert!(!read(2.0, 0.5, 2.69).drifted());
    assert!(!read(2.0, 0.5, 1.31).drifted());
    assert!(read(2.0, 0.5, 2.71).drifted());
    assert!(read(2.0, 0.5, 1.29).drifted());
    assert!(read(2.0, 0.5, f64::NAN).drifted(), "nothing matched");
}

fn matched(members: usize, parent: usize, reads: Vec<MarkerRead>) -> Matched {
    Matched {
        members: (0..members).collect(),
        parent,
        reads,
    }
}

#[test]
fn fewer_than_fifty_matched_cells_are_a_weak_match() {
    assert!(
        matched(49, 1000, vec![])
            .weak(0.05)
            .unwrap()
            .contains("only 49 cells")
    );
    assert_eq!(matched(50, 1000, vec![]).weak(0.05), None);
}

/// The reference's population was half its parent: 10% here is a fifth.
#[test]
fn a_population_under_a_fifth_as_common_as_the_reference_is_a_weak_match() {
    assert_eq!(matched(100, 1000, vec![]).weak(0.5), None);
    let weak = matched(99, 1000, vec![]).weak(0.5).expect("weak");
    assert!(
        weak.contains("9.900% of the parent matched against 50.000%"),
        "{weak}"
    );
}

#[test]
fn a_marker_the_matched_cells_drifted_on_is_named() {
    let reads = vec![read(2.0, 0.5, 2.1), read(0.0, 0.2, 1.4)];
    let weak = matched(500, 1000, reads).weak(0.5).expect("weak");
    assert!(
        weak.contains("on CD4 the matched cells sit at 1.40 against 0.00"),
        "{weak}"
    );
}

// ── what a cell must be on each marker ───────────────────────────────────

#[test]
fn on_its_landmarks_a_population_above_or_below_the_valley_is_positive_or_negative() {
    assert_eq!(Identity::of(1.3, 3.0, true), Identity::Above(1.0));
    assert_eq!(Identity::of(-0.4, 0.6, true), Identity::Below(1.0));
    assert_eq!(Identity::of(0.2, 1.5, true), Identity::Between(0.2, 1.5));
}

/// With no valley there is no line between negative and positive: a
/// population above the parent's middle may be brighter, one below it
/// dimmer, but not the other way.
#[test]
fn without_landmarks_only_the_far_side_of_a_population_is_let_go() {
    assert_eq!(Identity::of(2.0, 9.0, false), Identity::Above(2.0));
    assert_eq!(Identity::of(-9.0, -2.0, false), Identity::Below(-2.0));
    assert_eq!(Identity::of(-1.0, 1.0, false), Identity::Between(-1.0, 1.0));
}

#[test]
fn a_value_is_one_of_the_population_on_the_right_side_of_its_line() {
    assert!(Identity::Above(1.0).holds(1.01) && !Identity::Above(1.0).holds(1.0));
    assert!(Identity::Below(1.0).holds(0.99) && !Identity::Below(1.0).holds(1.0));
    assert!(Identity::Between(0.0, 2.0).holds(2.0) && !Identity::Between(0.0, 2.0).holds(2.01));
}

/// The run that was too strict: CD161 at 1.43 against 2.36 on the reference,
/// both above the valley - dimmer, still positive; and a marker with no
/// valley at 14.42 against 10.80, both far above the parent's middle -
/// brighter, still the same cells.
#[test]
fn a_population_dimmer_or_brighter_on_the_same_side_has_not_drifted() {
    let dimmer = MarkerRead {
        identity: Identity::Above(1.0),
        ..read(2.36, 0.3, 1.43)
    };
    assert!(!dimmer.drifted());
    let brighter = MarkerRead {
        by_landmarks: false,
        identity: Identity::Above(8.0),
        ..read(10.8, 1.0, 14.42)
    };
    assert!(!brighter.drifted());
    let crossed = MarkerRead {
        identity: Identity::Above(1.0),
        ..read(2.36, 0.3, 0.8)
    };
    assert!(crossed.drifted(), "below the valley is not positive");
}

/// A reference population at 12 to 14 on a marker whose valley sits at 5:
/// 2.4 to 2.8 on its landmarks, positive. In the sample its cells sit at 9
/// to 11, the valley at 5.5 - 1.6 to 2.0, dimmer than any on the reference
/// but above the valley. Held to the reference's range they would all be
/// lost; as positive cells they are all found, and the negatives are not.
#[test]
fn a_positive_population_dimmer_than_on_the_reference_is_still_found() {
    let landmarks = Frame {
        origin: 0.0,
        unit: 5.0,
        by_landmarks: true,
    };
    let signature = Signature {
        markers: markers(&["CD161"]),
        profiles: vec![Profile {
            frames: Frames {
                landmarks: Some(landmarks),
                spread: Frame {
                    origin: 0.0,
                    unit: 1.0,
                    by_landmarks: false,
                },
            },
            low: 12.0,
            high: 14.0,
            middle: 13.0,
            spread: 0.5,
        }],
        members: 200,
    };
    let mut rng = Cloud(21);
    let mut sample = rng.around(&[0.0], 1.0, 800);
    sample.extend_from_slice(&rng.around(&[10.0], 1.0, 200));
    let found = signature.find_in(rows(&sample, 1));
    assert_eq!(found.members, (800..1000).collect::<Vec<_>>());
    assert_eq!(found.reads[0].identity, Identity::Above(1.0));
    assert!(!found.reads[0].drifted());
}
