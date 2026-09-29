use dioxus::prelude::*;

use clingate_core::gates::gate_types::PrimaryGateType;

const GATE_CONFIG: &[(PrimaryGateType, &str)] = &[
    (PrimaryGateType::Polygon, "P"),
    (PrimaryGateType::Ellipse, "E"),
    (PrimaryGateType::Rectangle, "R"),
    (PrimaryGateType::Line(None), "L"),
    (PrimaryGateType::Bisector, "B"),
    (PrimaryGateType::Quadrant, "Q"),
    (PrimaryGateType::SkewedQuadrant, "S"),
];

#[component]
pub fn NewGateButtons(callback: EventHandler<PrimaryGateType>) -> Element {
    let mut selected_index = use_signal(|| 0);
    let selected_style = "background-color: orange";

    rsx! {
        for (i , (t , d_text)) in GATE_CONFIG.iter().enumerate() {
            button {
                style: if selected_index() == i { Some(selected_style) } else { None },
                onclick: move |_| {
                    selected_index.set(i);
                    callback.call(*t);

                },
                "{d_text}"
            }
        }

    }
}
