use std::sync::Arc;

use dioxus::prelude::*;
use flow_fcs::TransformType;

use crate::gate_editor::plots::axis_store::Param;

pub fn asinh_transform_f32(value: f32, cofactor: f32) -> anyhow::Result<f32> {
    if value.is_nan() || value.is_infinite() {
        return Err(anyhow::anyhow!("Value {value} cannot be arcsinh transform"));
    }
    if cofactor == 0_f32 {
        return Err(anyhow::anyhow!(
            "Cofactor {cofactor} cannot be used for arcsinh transform"
        ));
    }
    Ok((value / cofactor).asinh())
}

pub fn asinh_reverse_f32(transformed_value: f32, cofactor: f32) -> anyhow::Result<f32> {
    if transformed_value.is_nan() || transformed_value.is_infinite() {
        return Err(anyhow::anyhow!(
            "Transformed value {transformed_value} is invalid"
        ));
    }
    if cofactor == 0_f32 {
        return Err(anyhow::anyhow!("Cofactor {cofactor} cannot be zero"));
    }
    Ok(transformed_value.sinh() * cofactor)
}

pub fn asinh_to_asinh(value: f32, old_cofactor: f32, new_cofactor: f32) -> anyhow::Result<f32> {
    let untransformed = asinh_reverse_f32(value, old_cofactor)?;
    asinh_transform_f32(untransformed, new_cofactor)
}

#[derive(Debug, Clone, PartialEq, Props)]
pub struct AxisInfo {
    pub param: Param,
    pub axis_lower: f32,
    pub axis_upper: f32,
    // pub data_lower: f32,
    // pub data_upper: f32,
    pub transform: flow_fcs::TransformType,
}

impl Default for AxisInfo {
    fn default() -> Self {
        Self {
            param: Param {
                marker: Arc::from(""),
                fluoro: Arc::from(""),
            },
            axis_lower: 0_f32,
            axis_upper: 4194304_f32,
            // data_lower: 0_f32,
            // data_upper: 4194304_f32,
            transform: flow_fcs::TransformType::Linear,
        }
    }
}

impl AxisInfo {
    pub fn new_from_raw(
        param: Param,
        lower_raw: f32,
        upper_raw: f32,
        // data_lower: f32,
        // data_upper: f32,
        transform: TransformType,
    ) -> Self {
        match transform {
            TransformType::Linear => Self {
                param,
                axis_lower: lower_raw,
                axis_upper: upper_raw,
                // data_lower,
                // data_upper,
                transform,
            },
            TransformType::Arcsinh { cofactor } => {
                let lower = asinh_transform_f32(lower_raw, cofactor).unwrap_or(0f32);
                let upper = asinh_transform_f32(upper_raw, cofactor).unwrap_or(f32::INFINITY);
                // let data_lower = asinh_transform_f32(data_lower, cofactor).unwrap_or(0f32);
                // let data_upper = asinh_transform_f32(data_upper, cofactor).unwrap_or(f32::INFINITY);
                Self {
                    param,
                    axis_lower: lower,
                    axis_upper: upper,
                    // data_lower,
                    // data_upper,
                    transform,
                }
            }
            TransformType::Biexponential {
                top_of_scale: _,
                positive_decades: _,
                negative_decades: _,
                width: _,
            } => todo!(),
        }
    }

    pub fn into_archsinh(&self, cofactor: f32) -> anyhow::Result<Self> {
        let old_lower = self.axis_lower;
        let old_upper = self.axis_upper;
        // let old_dl = self.data_lower;
        // let old_du = self.data_upper;
        let transform = TransformType::Arcsinh { cofactor };
        let new_self = match self.transform {
            flow_fcs::TransformType::Arcsinh {
                cofactor: old_cofactor,
            } => {
                let lower = asinh_to_asinh(old_lower, old_cofactor, cofactor)?;
                let upper = asinh_to_asinh(old_upper, old_cofactor, cofactor)?;
                // let data_lower = asinh_to_asinh(old_dl, old_cofactor, cofactor)?;
                // let data_upper = asinh_to_asinh(old_du, old_cofactor, cofactor)?;
                Self {
                    param: self.param.clone(),
                    axis_lower: lower,
                    axis_upper: upper,
                    // data_lower,
                    // data_upper,
                    transform,
                }
            }
            _ => {
                let lower = asinh_transform_f32(old_lower, cofactor)?;
                let upper = asinh_transform_f32(old_upper, cofactor)?;
                // let data_lower = asinh_transform_f32(old_dl, cofactor)?;
                // let data_upper = asinh_transform_f32(old_du, cofactor)?;
                Self {
                    param: self.param.clone(),
                    axis_lower: lower,
                    axis_upper: upper,
                    // data_lower,
                    // data_upper,
                    transform,
                }
            }
        };
        Ok(new_self)
    }

    pub fn into_linear(&self) -> anyhow::Result<Self> {
        let old_lower = self.axis_lower;
        let old_upper = self.axis_upper;
        // let old_dl = self.data_lower;
        // let old_du = self.data_upper;
        let transform = TransformType::Linear;
        let new_self = match self.transform {
            TransformType::Linear => self.clone(),

            TransformType::Arcsinh {
                cofactor: old_cofactor,
            } => {
                let upper_untransformed = asinh_reverse_f32(old_upper, old_cofactor)?;
                let lower_untransformed = asinh_reverse_f32(old_lower, old_cofactor)?;
                // let data_lower = asinh_reverse_f32(old_dl, old_cofactor)?;
                // let data_upper = asinh_reverse_f32(old_du, old_cofactor)?;
                Self {
                    param: self.param.clone(),
                    axis_lower: lower_untransformed,
                    axis_upper: upper_untransformed,
                    // data_lower,
                    // data_upper,
                    transform,
                }
            }
            TransformType::Biexponential { .. } => Self {
                param: self.param.clone(),
                axis_lower: old_lower,
                axis_upper: old_upper,
                // data_lower: old_dl,
                // data_upper: old_du,
                transform,
            },
        };
        Ok(new_self)
    }

    pub fn is_linear(&self) -> bool {
        matches!(self.transform, TransformType::Linear)
    }

    pub fn is_arcsinh(&self) -> bool {
        matches!(self.transform, TransformType::Arcsinh { .. })
    }

    pub fn get_untransformed_bounds(&self) -> (f32, f32) {
        match self.transform {
            TransformType::Arcsinh { cofactor } => (
                asinh_reverse_f32(self.axis_lower, cofactor).unwrap_or_default(),
                asinh_reverse_f32(self.axis_upper, cofactor).unwrap_or_default(),
            ),
            _ => (self.axis_lower, self.axis_upper),
        }
    }

    pub fn get_untransformed_lower(&self) -> f32 {
        match self.transform {
            TransformType::Arcsinh { cofactor } => {
                asinh_reverse_f32(self.axis_lower, cofactor).unwrap_or_default()
            }
            _ => self.axis_lower,
        }
    }

    pub fn get_untransformed_upper(&self) -> f32 {
        match self.transform {
            TransformType::Arcsinh { cofactor } => {
                asinh_reverse_f32(self.axis_upper, cofactor).unwrap_or_default()
            }
            _ => self.axis_upper,
        }
    }

    pub fn into_new_lower(&self, lower_raw: f32) -> Self {
        match self.transform {
            TransformType::Linear => Self {
                param: self.param.clone(),
                axis_lower: lower_raw,
                axis_upper: self.axis_upper,
                // data_lower: self.data_lower,
                // data_upper: self.data_upper,
                transform: self.transform.clone(),
            },
            TransformType::Arcsinh { cofactor } => {
                let new_lower = asinh_transform_f32(lower_raw, cofactor).unwrap_or(self.axis_lower);
                Self {
                    param: self.param.clone(),
                    axis_lower: new_lower,
                    axis_upper: self.axis_upper,
                    // data_lower: self.data_lower,
                    // data_upper: self.data_upper,
                    transform: self.transform.clone(),
                }
            }
            TransformType::Biexponential { .. } => todo!(),
        }
    }

    pub fn into_new_upper(&self, upper_raw: f32) -> Self {
        match self.transform {
            TransformType::Linear => Self {
                param: self.param.clone(),
                axis_lower: self.axis_lower,
                axis_upper: upper_raw,
                // data_lower: self.data_lower,
                // data_upper: self.data_upper,
                transform: self.transform.clone(),
            },
            TransformType::Arcsinh { cofactor } => {
                let new_upper = asinh_transform_f32(upper_raw, cofactor).unwrap_or(self.axis_upper);
                Self {
                    param: self.param.clone(),
                    axis_lower: self.axis_lower,
                    axis_upper: new_upper,
                    // data_lower: self.data_lower,
                    // data_upper: self.data_upper,
                    transform: self.transform.clone(),
                }
            }
            TransformType::Biexponential { .. } => todo!(),
        }
    }

    pub fn get_cofactor(&self) -> Option<f32> {
        match self.transform {
            TransformType::Linear => None,
            TransformType::Arcsinh { cofactor } => Some(cofactor),
            TransformType::Biexponential { .. } => None,
        }
    }
}

//cargo test axis_info_tests -- --nocapture
// ─── Tests ────────────────────────────────────────────────────────────────────
//
// NOTE: written without a compiler - the flow-fcs/flow-gates git dependencies
// were not reachable in the environment these were authored in, so this module
// has never been built or run. Treat a failure here as suspect-the-test first.

#[cfg(test)]
mod axis_info_tests {
    use super::*;

    const COFACTOR: f32 = 6000.0;

    fn param(name: &str) -> Param {
        Param {
            marker: Arc::from(name),
            fluoro: Arc::from(name),
        }
    }

    // ── Raw transform helpers ─────────────────────────────────────────────────

    #[test]
    fn arcsinh_round_trips_through_its_inverse() {
        for raw in [0.0f32, 1.0, 500.0, 12_345.0, 4_194_304.0, -2_000.0] {
            let t = asinh_transform_f32(raw, COFACTOR).unwrap();
            let back = asinh_reverse_f32(t, COFACTOR).unwrap();
            let tolerance = (raw.abs() * 1e-4).max(1e-2);
            assert!(
                (back - raw).abs() <= tolerance,
                "{raw} round-tripped to {back}"
            );
        }
    }

    #[test]
    fn arcsinh_of_zero_is_zero() {
        assert_eq!(asinh_transform_f32(0.0, COFACTOR).unwrap(), 0.0);
        assert_eq!(asinh_reverse_f32(0.0, COFACTOR).unwrap(), 0.0);
    }

    #[test]
    fn arcsinh_is_monotonic() {
        let a = asinh_transform_f32(100.0, COFACTOR).unwrap();
        let b = asinh_transform_f32(1_000.0, COFACTOR).unwrap();
        let c = asinh_transform_f32(10_000.0, COFACTOR).unwrap();
        assert!(a < b && b < c, "transform must preserve ordering");
    }

    #[test]
    fn arcsinh_handles_negative_values() {
        // Unlike a log scale, arcsinh is defined below zero - this is why it is
        // used for compensated flow data.
        let t = asinh_transform_f32(-5_000.0, COFACTOR).unwrap();
        assert!(t < 0.0);
        assert!((asinh_reverse_f32(t, COFACTOR).unwrap() - -5_000.0).abs() < 1.0);
    }

    #[test]
    fn a_zero_cofactor_is_rejected_rather_than_dividing_by_zero() {
        assert!(asinh_transform_f32(100.0, 0.0).is_err());
        assert!(asinh_reverse_f32(1.0, 0.0).is_err());
    }

    #[test]
    fn non_finite_inputs_are_rejected() {
        assert!(asinh_transform_f32(f32::NAN, COFACTOR).is_err());
        assert!(asinh_transform_f32(f32::INFINITY, COFACTOR).is_err());
        assert!(asinh_reverse_f32(f32::NAN, COFACTOR).is_err());
        assert!(asinh_reverse_f32(f32::INFINITY, COFACTOR).is_err());
    }

    #[test]
    fn changing_cofactor_preserves_the_underlying_raw_value() {
        let raw = 25_000.0f32;
        let at_6000 = asinh_transform_f32(raw, 6000.0).unwrap();
        let at_1000 = asinh_to_asinh(at_6000, 6000.0, 1000.0).unwrap();

        let expected = asinh_transform_f32(raw, 1000.0).unwrap();
        assert!(
            (at_1000 - expected).abs() < 1e-3,
            "expected {expected}, got {at_1000}"
        );
    }

    #[test]
    fn a_smaller_cofactor_spreads_the_scale_further() {
        let raw = 10_000.0f32;
        let wide = asinh_transform_f32(raw, 100.0).unwrap();
        let narrow = asinh_transform_f32(raw, 10_000.0).unwrap();
        assert!(wide > narrow);
    }

    // ── AxisInfo construction ─────────────────────────────────────────────────

    #[test]
    fn linear_axes_keep_their_raw_bounds() {
        let a = AxisInfo::new_from_raw(param("FSC-A"), 0.0, 4_194_304.0, TransformType::Linear);

        assert!(a.is_linear());
        assert!(!a.is_arcsinh());
        assert_eq!(a.axis_lower, 0.0);
        assert_eq!(a.axis_upper, 4_194_304.0);
        assert_eq!(a.get_cofactor(), None);
    }

    #[test]
    fn arcsinh_axes_store_transformed_bounds_but_report_raw_ones() {
        let a = AxisInfo::new_from_raw(
            param("CD3"),
            -10_000.0,
            4_194_304.0,
            TransformType::Arcsinh { cofactor: COFACTOR },
        );

        assert!(a.is_arcsinh());
        assert_eq!(a.get_cofactor(), Some(COFACTOR));
        // Stored in transformed space...
        assert!(a.axis_upper < 100.0, "axis_upper should be transformed");
        // ...but reported back in raw space.
        let (lower, upper) = a.get_untransformed_bounds();
        assert!((lower - -10_000.0).abs() < 1.0, "lower was {lower}");
        assert!((upper - 4_194_304.0).abs() < 500.0, "upper was {upper}");
    }

    #[test]
    fn untransformed_accessors_agree_with_the_bounds_pair() {
        let a = AxisInfo::new_from_raw(
            param("CD4"),
            -1_000.0,
            1_000_000.0,
            TransformType::Arcsinh { cofactor: COFACTOR },
        );
        let (lower, upper) = a.get_untransformed_bounds();

        assert_eq!(a.get_untransformed_lower(), lower);
        assert_eq!(a.get_untransformed_upper(), upper);
    }

    #[test]
    fn a_linear_axis_reports_its_bounds_unchanged() {
        let a = AxisInfo::new_from_raw(param("SSC-A"), 10.0, 500.0, TransformType::Linear);
        assert_eq!(a.get_untransformed_bounds(), (10.0, 500.0));
    }

    // ── Rescaling ─────────────────────────────────────────────────────────────

    /// Changing the cofactor must not move the gate in raw space - this is what
    /// GateState::rescale_gates depends on when it rewrites every stored gate.
    #[test]
    fn into_arcsinh_preserves_raw_bounds_across_a_cofactor_change() {
        let original = AxisInfo::new_from_raw(
            param("CD8"),
            -10_000.0,
            4_194_304.0,
            TransformType::Arcsinh { cofactor: 6000.0 },
        );
        let (raw_lower, raw_upper) = original.get_untransformed_bounds();

        let rescaled = original.into_archsinh(1000.0).unwrap();
        let (new_lower, new_upper) = rescaled.get_untransformed_bounds();

        assert_eq!(rescaled.get_cofactor(), Some(1000.0));
        assert!((new_lower - raw_lower).abs() < 1.0, "lower drifted");
        assert!(
            (new_upper - raw_upper).abs() / raw_upper.abs() < 1e-3,
            "upper drifted from {raw_upper} to {new_upper}"
        );
    }

    #[test]
    fn a_linear_axis_converted_to_arcsinh_transforms_its_bounds() {
        let linear = AxisInfo::new_from_raw(param("CD19"), 0.0, 100_000.0, TransformType::Linear);
        let arcsinh = linear.into_archsinh(COFACTOR).unwrap();

        assert!(arcsinh.is_arcsinh());
        let (_, upper) = arcsinh.get_untransformed_bounds();
        assert!((upper - 100_000.0).abs() / 100_000.0 < 1e-3);
    }

    #[test]
    fn into_linear_undoes_the_transform() {
        let arcsinh = AxisInfo::new_from_raw(
            param("CD45"),
            0.0,
            262_144.0,
            TransformType::Arcsinh { cofactor: COFACTOR },
        );
        let linear = arcsinh.into_linear().unwrap();

        assert!(linear.is_linear());
        assert!((linear.axis_upper - 262_144.0).abs() / 262_144.0 < 1e-3);
    }

    #[test]
    fn into_linear_on_an_already_linear_axis_is_a_no_op() {
        let linear = AxisInfo::new_from_raw(param("Time"), 0.0, 1000.0, TransformType::Linear);
        assert_eq!(linear.into_linear().unwrap(), linear);
    }

    // ── Limit edits ───────────────────────────────────────────────────────────

    #[test]
    fn setting_a_new_lower_limit_takes_a_raw_value() {
        let a = AxisInfo::new_from_raw(
            param("CD3"),
            -10_000.0,
            4_194_304.0,
            TransformType::Arcsinh { cofactor: COFACTOR },
        );
        let updated = a.into_new_lower(-1_000.0);

        assert!((updated.get_untransformed_lower() - -1_000.0).abs() < 1.0);
        // The upper bound must be left exactly as it was.
        assert_eq!(updated.axis_upper, a.axis_upper);
    }

    #[test]
    fn setting_a_new_upper_limit_takes_a_raw_value() {
        let a = AxisInfo::new_from_raw(
            param("CD3"),
            -10_000.0,
            4_194_304.0,
            TransformType::Arcsinh { cofactor: COFACTOR },
        );
        let updated = a.into_new_upper(100_000.0);

        assert!((updated.get_untransformed_upper() - 100_000.0).abs() / 100_000.0 < 1e-3);
        assert_eq!(updated.axis_lower, a.axis_lower);
    }

    #[test]
    fn limit_edits_on_a_linear_axis_are_stored_verbatim() {
        let a = AxisInfo::new_from_raw(param("FSC-A"), 0.0, 4_194_304.0, TransformType::Linear);

        assert_eq!(a.into_new_lower(500.0).axis_lower, 500.0);
        assert_eq!(a.into_new_upper(1000.0).axis_upper, 1000.0);
    }

    #[test]
    fn the_cofactor_is_preserved_across_a_limit_edit() {
        let a = AxisInfo::new_from_raw(
            param("CD4"),
            0.0,
            262_144.0,
            TransformType::Arcsinh { cofactor: 1234.0 },
        );
        assert_eq!(a.into_new_lower(-500.0).get_cofactor(), Some(1234.0));
        assert_eq!(a.into_new_upper(500_000.0).get_cofactor(), Some(1234.0));
    }

    // ── Param display ─────────────────────────────────────────────────────────

    #[test]
    fn a_param_with_no_separate_marker_shows_just_the_channel() {
        assert_eq!(param("FSC-A").to_string(), "FSC-A");
    }

    #[test]
    fn a_labelled_param_shows_marker_and_trimmed_channel() {
        let p = Param {
            marker: Arc::from("CD3"),
            fluoro: Arc::from("BV421-A"),
        };
        assert_eq!(p.to_string(), "CD3-BV421");
    }

    #[test]
    fn a_channel_without_the_area_suffix_is_left_alone() {
        let p = Param {
            marker: Arc::from("CD3"),
            fluoro: Arc::from("BV421"),
        };
        assert_eq!(p.to_string(), "CD3-BV421");
    }

    #[test]
    fn the_default_axis_is_linear_over_the_full_range() {
        let d = AxisInfo::default();
        assert!(d.is_linear());
        assert_eq!(d.axis_lower, 0.0);
        assert_eq!(d.axis_upper, 4_194_304.0);
    }
}
