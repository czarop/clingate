//! The app shell: the tabs, and the document they all share.
//!
//! Every tab is mounted once and hidden with CSS rather than swapped in and
//! out. A router unmounts the route it leaves, and unmounting is what was
//! losing the editor's setup on every tab change - the selected sample, the
//! axis markers, and `upload_succeded`, the flag that stops the gating file
//! being re-imported. Coming back re-imported it, rebuilding `GateState` and
//! discarding any positioning the autogater had done.
//!
//! Hiding keeps every signal, resource and in-flight task alive, so there is no
//! state to lift out and nothing to restore. The cost is that a hidden tab
//! still reacts: a plot whose parent chain contains a gate the autogater moves
//! will re-filter and re-render while nobody is looking at it. If that becomes
//! noticeable, the render is the part worth skipping - the data pipeline is
//! cheap to keep warm, the image is not.

use crate::components::toast::ToastProvider;
use crate::file_load::FcsFiles;
use crate::gate_editor::gallery::window::GalleryWindow;
use crate::gate_editor::gate_rules_window::GateRulesWindow;
use crate::gate_editor::gates::GateState;
use crate::gate_editor::main_window::MainWindow;
use crate::gate_editor::plots::axis_store::AxisStore;
use crate::gate_rules::rule_store::RuleStore;
use crate::omiq::metadata::MetaDataStore;
use dioxus::prelude::*;
use dioxus::stores::use_store_sync;

/// Which tab is in front. Every tab is mounted whatever this says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Editor,
    Rules,
    Gallery,
}

impl Tab {
    fn icon(self) -> &'static str {
        match self {
            Tab::Editor => "🏠",
            Tab::Rules => "📐",
            Tab::Gallery => "🖼",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Tab::Editor => "Gate editor",
            Tab::Rules => "Gate rules",
            Tab::Gallery => "Gate gallery",
        }
    }
}

const TABS: [Tab; 3] = [Tab::Editor, Tab::Rules, Tab::Gallery];

/// Every panel is mounted; only the one in front is displayed.
fn panel_class(active: Tab, tab: Tab) -> &'static str {
    if active == tab {
        "tab-panel"
    } else {
        "tab-panel tab-panel_hidden"
    }
}

#[component]
pub fn Shell() -> Element {
    // The document, shared by every tab. A store here is the document; a signal
    // inside a component is that component's own business.
    let gate_store = use_store_sync(GateState::default);
    use_context_provider(|| gate_store);

    let metadata_store = use_store_sync(MetaDataStore::default);
    use_context_provider(|| metadata_store);

    let axis_store = use_store_sync(AxisStore::default);
    use_context_provider(|| axis_store);

    let rules = use_signal(RuleStore::default);
    use_context_provider(|| rules);

    let filehandler: Signal<Option<FcsFiles>> = use_signal(|| None);
    use_context_provider(|| filehandler);

    let mut active = use_signal(|| Tab::Editor);
    // Offered to the tabs themselves, so an expensive one can tell whether it
    // is worth drawing.
    use_context_provider(|| active);

    rsx! {
        // Every tab is inside the provider, so a toast raised by a run that
        // finishes while you are looking at another tab still arrives. A
        // provider per tab would have swallowed exactly the messages most worth
        // seeing.
        ToastProvider {
        // Hidden by class, not by an inline style. Diffing `style` down to an
        // empty string does not reliably clear what was set before, which left
        // both panels displaying none and the nav bar alone at the top of the
        // window. A class is a value the renderer always replaces wholesale.
        //
        // Hiding, not unmounting: that difference is the whole point.
        div { class: panel_class(active(), Tab::Editor), MainWindow {} }
        div { class: panel_class(active(), Tab::Rules), GateRulesWindow {} }
        div { class: panel_class(active(), Tab::Gallery), GalleryWindow {} }

        div { class: "route-nav_bar",
            nav { aria_label: "main navigation", role: "navigation",
                div { class: "nav_bar-items",
                    for (at , tab) in TABS.iter().enumerate() {
                        if at > 0 {
                            div { class: "nav_bar-spacer", "|" }
                        }
                        div {
                            class: if active() == *tab { "nav_bar-item selected" } else { "nav_bar-item" },
                            title: "{tab.title()}",
                            onclick: {
                                let tab = *tab;
                                move |_| active.set(tab)
                            },
                            "{tab.icon()}"
                        }
                    }
                }
            }
        }
        }
    }
}
