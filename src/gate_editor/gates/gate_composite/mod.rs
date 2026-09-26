pub mod bisector_gate;
pub mod quadrant_gate;
pub mod skewed_quadrant_gate;

/// Refuse an axis range a composite cannot be laid out in: one that is not a
/// pair of numbers with the lower below the upper.
///
/// A quadrant keeps its centre inside the axis by clamping into the range,
/// and `f32::clamp` panics on a range the wrong way round - which is how an
/// upper limit typed below the lower one took the app down (B-AX-1). The
/// callers check an axis before it gets here; this is so a range that gets
/// past them anyway is an error, not a crash.
pub fn usable_range(lower: f32, upper: f32) -> anyhow::Result<()> {
    if lower.is_finite() && upper.is_finite() && lower < upper {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "an axis from {lower} to {upper} cannot hold a gate - the lower limit must be below the upper"
        ))
    }
}

/// Where an arm of a quadrant ends once it is carried along its own line to
/// `edge` on one axis - X if `along_x` - keeping its direction from the
/// centre exactly.
///
/// A quadrant's quarters depend only on its centre and the direction of each
/// arm: the arms are projected out to the transform's infinite bounds before
/// the quarters are built. Where an arm's end sits along its line matters only
/// for drawing, which is why a change of axis range may move it and must not
/// move the centre or turn the arm. Snapping one coordinate of a slanted arm's
/// end to the new edge, as the range change used to, turned it.
///
/// An arm whose line does not reach `edge` on its own side of the centre - a
/// centre beyond the edge, which a narrowed axis can leave - keeps its end.
pub fn arm_to_edge(centre: (f32, f32), arm: (f32, f32), along_x: bool, edge: f32) -> (f32, f32) {
    let (c, a) = if along_x {
        (centre.0, arm.0)
    } else {
        (centre.1, arm.1)
    };
    let (run, to_edge) = (a - c, edge - c);
    if run == 0.0 || to_edge == 0.0 || run.signum() != to_edge.signum() {
        return arm;
    }
    let t = to_edge / run;
    let mut end = (
        centre.0 + t * (arm.0 - centre.0),
        centre.1 + t * (arm.1 - centre.1),
    );
    // Exactly on the edge, not a rounding error either side of it.
    if along_x {
        end.0 = edge;
    } else {
        end.1 = edge;
    }
    end
}

/// Where the centre of a quadrant is drawn: the real centre, or the nearest
/// point of the plot when a narrowed axis has left the real one off it.
///
/// For drawing only. The centre the quarters are built from is never moved
/// by a change of axis - that clamped the gate itself, and a quadrant near the
/// end of an axis changed its cells on a cofactor change (B-AX-4).
pub fn drawn_centre(centre: (f32, f32), x: (f32, f32), y: (f32, f32)) -> (f32, f32) {
    (centre.0.clamp(x.0, x.1), centre.1.clamp(y.0, y.1))
}

#[cfg(test)]
mod gate_composite_tests;
