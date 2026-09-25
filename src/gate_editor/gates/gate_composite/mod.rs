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

#[cfg(test)]
mod gate_composite_tests;
