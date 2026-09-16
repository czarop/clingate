#![allow(non_snake_case)]

use clingate::gate_editor::route::Shell;
use dioxus::prelude::*;

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};

static NAV_STYLE: Asset = asset!("assets/navbar.css");
static SPINNER_STYLE: Asset = asset!("assets/spinner.css");
static COMPONENTS_STYLE: Asset = asset!("assets/dx-components-theme.css");
static COMPONENTS_STYLE_2: Asset = asset!("assets/searchable_select.css");

#[component]
fn FPSCounter() -> Element {
    let mut last_measured_time = use_signal(std::time::Instant::now);
    let mut display_fps = use_signal(|| 0.0);
    let mut frame_count = use_signal(|| 0);
    let mut last_ui_update = use_signal(std::time::Instant::now);

    use_future(move || async move {
        loop {
            // High-frequency measurement (approx 60fps check)
            tokio::time::sleep(std::time::Duration::from_millis(16)).await;

            let now = std::time::Instant::now();
            let elapsed = now.duration_since(*last_measured_time.peek()).as_secs_f32();

            if elapsed > 0.0 {
                let current_fps = 1.0 / elapsed;

                // Track frames and time since last UI update
                frame_count.with_mut(|c| *c += 1);

                let ui_elapsed = now.duration_since(*last_ui_update.peek()).as_secs_f32();

                // Only update the visible signal ~5 times per second (every 200ms)
                if ui_elapsed >= 0.2 {
                    display_fps.set(current_fps);
                    last_ui_update.set(now);
                    frame_count.set(0);
                }
            }
            last_measured_time.set(now);
        }
    });

    rsx! {
        div { style: "position: fixed; bottom: 20px; right: 20px;
                    background: rgba(20, 20, 20, 0.9); color: #00ff00; 
                    padding: 10px 15px; font-family: monospace; 
                    border: 2px solid #333; border-radius: 8px; 
                    box-shadow: 0 4px 6px rgba(0,0,0,0.3);
                    pointer-events: none; z-index: 10000;",
            span { style: "color: #888; margin-right: 8px;", "PERF:" }
            b { "{display_fps:.1} FPS" }
        }
    }
}

#[component]
fn App() -> Element {
    rsx! {
        document::Stylesheet { href: COMPONENTS_STYLE }
        document::Stylesheet { href: COMPONENTS_STYLE_2 }
        document::Stylesheet { href: NAV_STYLE }
        document::Stylesheet { href: SPINNER_STYLE }
        div { class: "main_div",
            FPSCounter {}
            Shell {}
        }
    }
}

// #[cfg(feature = "desktop")]
fn main() {
    unsafe { std::env::set_var("RUST_BACKTRACE", "full") };

    dioxus::LaunchBuilder::new()
        .with_cfg(
            Config::new().with_disable_context_menu(true).with_window(
                WindowBuilder::new()
                    .with_title("FCS Plot Viewer")
                    .with_inner_size(LogicalSize::new(1600.0, 950.0)),
            ),
        )
        .launch(App);
}

// #[cfg(not(feature = "desktop"))]
// fn main() {
//     // For Web, we use the simple launch
//     // Dioxus handles the WASM panic hook and browser mounting automatically
//     dioxus::launch(App);
// }
