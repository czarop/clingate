use dioxus::prelude::*;

/// Whether an item is shown for what has been typed: its displayed text
/// contains the search, ignoring case. `lowered` is the search already
/// lowercased, since it is compared against every item.
pub(crate) fn matches_search(item: &impl std::fmt::Display, lowered: &str) -> bool {
    item.to_string().to_lowercase().contains(lowered)
}

#[cfg(test)]
mod tests {
    use super::matches_search;

    #[test]
    fn an_empty_search_shows_everything() {
        assert!(matches_search(&"CD3", ""));
        assert!(matches_search(&"", ""));
    }

    #[test]
    fn a_search_ignores_case_and_matches_anywhere_in_the_text() {
        assert!(matches_search(&"BV421-A :: CD3", "cd3"));
        assert!(matches_search(&"BV421-A :: CD3", "421"));
        assert!(!matches_search(&"BV421-A :: CD3", "cd4"));
    }

    #[test]
    fn a_search_is_matched_against_what_is_displayed() {
        struct Shown;
        impl std::fmt::Display for Shown {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "Lymphocytes")
            }
        }
        assert!(matches_search(&Shown, "lymph"));
    }
}

#[component]
pub fn SearchableSelectList<T: Clone + PartialEq + std::fmt::Display + 'static>(
    items: ReadSignal<Vec<T>>,
    on_select: EventHandler<(usize, T)>,
    placeholder: Option<String>,
    selected_index: Option<ReadSignal<usize>>,
) -> Element {
    // In your component
    let mut is_open = use_signal(|| false);
    let mut search = use_signal(String::new);
    let mut local_selected_value: Signal<Option<(usize, T)>> = use_signal(|| None);

    let filtered = use_memo(move || {
        let q = search.read().to_lowercase();
        items
            .read()
            .iter()
            .enumerate() // <--- Get the original index FIRST
            .filter(|(_, item)| matches_search(*item, &q))
            .map(|(i, item)| (i, item.clone())) // Store both
            .collect::<Vec<(usize, T)>>()
    });

    let mut display_text = use_signal(String::new);

    use_effect(move || {
        let text = local_selected_value
            .read()
            .as_ref()
            .map(|(_, v)| v.to_string())
            .or(placeholder.clone())
            .unwrap_or_else(|| "Select an item...".to_string());
        display_text.set(text);
    });

    use_effect(move || {
        if let Some((i, val)) = local_selected_value() {
            on_select((i, val));
        };
    });

    use_effect(move || {
        if let Some(sig) = selected_index {
            let i = sig();
            let items = &*items.peek();
            if i < items.len() {
                display_text.set(items[i].to_string())
            }
        }
    });

    rsx! {
        div {
            class: "combobox-container",
            // Close when clicking outside (you might need a window listener for robust closing)
            onmouseleave: move |_| is_open.set(false),

            // The Trigger / Input
            div { class: "combobox-input-wrapper",
                input {
                    class: "combobox-input",
                    value: "{search}",
                    placeholder: "{display_text}", // Show selected if empty

                    autocorrect: "off",
                    autocapitalize: "none",
                    autocomplete: "off",
                    spellcheck: "false",

                    // Open on click or focus
                    onfocus: move |_| is_open.set(true),
                    onclick: move |_| is_open.set(true),

                    // Handle typing
                    oninput: move |e| {
                        search.set(e.value());
                        is_open.set(true);
                    },
                }
                // Optional: Arrow icon
                div {
                    class: "combobox-arrow",
                    onclick: move |_| is_open.toggle(),
                    "▼"
                }
            }

            // The Dropdown List
            // Only render if open
            if is_open() {
                div { class: "combobox-list scrollable",
                    // Iterate directly over items. We use enumerate just for the key.
                    for (original_i , item) in filtered().into_iter() {
                        // Capture the item for the closure
                        {
                            let current_item = item.clone();
                            rsx! {
                                div {
                                    key: "{original_i}", // React/Dioxus likes keys
                                    class: "combobox-option", // Use the Display implementation
                                    onclick: move |_| {
                                        // Set the actual item, not the index
                                        local_selected_value.set(Some((original_i, current_item.clone())));

                                        // Reset search so the placeholder (selected value) shows again
                                        search.set(String::new());
                                        is_open.set(false);
                                    },
                                    // Use the Display implementation
                                    "{item}"
                                }
                            }
                        }
                    }
                    // Handle empty state
                    if filtered.is_empty() {
                        div { class: "combobox-empty", "No results" }
                    }
                }
            }
        }
    }
}

use crate::FxIndexMap;

#[component]
pub fn SearchableSelectMap<
    T: Clone + PartialEq + std::fmt::Display + 'static,
    S: Clone + PartialEq + Eq + std::fmt::Display + std::hash::Hash + 'static,
>(
    items: ReadSignal<FxIndexMap<S, T>>,
    on_select: EventHandler<(usize, T)>,
    placeholder: Option<String>,
    selected_index: Option<ReadSignal<usize>>,
) -> Element {
    // In your component
    let mut is_open = use_signal(|| false);
    let mut search = use_signal(String::new);
    let mut local_selected_value: Signal<Option<(usize, T)>> = use_signal(|| None);

    let filtered = use_memo(move || {
        let q = search.read().to_lowercase();
        items
            .read()
            .iter()
            .enumerate() // Capture the REAL index in the master map
            .filter(|(_, (_, v))| matches_search(v, &q))
            .map(|(i, (k, v))| (i, k.clone(), v.clone())) // Store (Index, Key, Value)
            .collect::<Vec<(usize, S, T)>>()
    });

    let mut display_text = use_signal(String::new);

    use_effect(move || {
        let text = local_selected_value
            .read()
            .as_ref()
            .map(|(_, v)| v.to_string())
            .or(placeholder.clone())
            .unwrap_or_else(|| "Select an item...".to_string());
        display_text.set(text);
    });

    use_effect(move || {
        if let Some((i, val)) = local_selected_value() {
            on_select((i, val));
        };
    });

    use_effect(move || {
        if let Some(sig) = selected_index {
            let i = sig();
            let items = &*items.peek();
            if i < items.len() {
                display_text.set(items[i].to_string())
            }
        }
    });

    rsx! {
        div {
            class: "combobox-container",
            // Close when clicking outside (you might need a window listener for robust closing)
            onmouseleave: move |_| is_open.set(false),

            // The Trigger / Input
            div { class: "combobox-input-wrapper",
                input {
                    class: "combobox-input",
                    value: "{search}",
                    placeholder: "{display_text}", // Show selected if empty

                    autocorrect: "off",
                    autocapitalize: "none",
                    autocomplete: "off",
                    spellcheck: "false",

                    // Open on click or focus
                    onfocus: move |_| is_open.set(true),
                    onclick: move |_| is_open.set(true),

                    // Handle typing
                    oninput: move |e| {
                        search.set(e.value());
                        is_open.set(true);
                    },
                }
                // Optional: Arrow icon
                div {
                    class: "combobox-arrow",
                    onclick: move |_| is_open.toggle(),
                    "▼"
                }
            }

            // The Dropdown List
            // Only render if open
            if is_open() {
                div { class: "combobox-list scrollable",
                    // Iterate directly over items. We use enumerate just for the key.
                    for (original_i , _k , item) in filtered().into_iter() {
                        // Capture the item for the closure
                        {
                            let current_item = item.clone();
                            rsx! {
                                div {
                                    key: "{original_i}", // React/Dioxus likes keys
                                    class: "combobox-option", // Use the Display implementation
                                    onclick: move |_| {
                                        // Set the actual item, not the index
                                        local_selected_value.set(Some((original_i, current_item.clone())));

                                        // Reset search so the placeholder (selected value) shows again
                                        search.set(String::new());
                                        is_open.set(false);
                                    },
                                    // Use the Display implementation
                                    "{item}"
                                }
                            }
                        }
                    }
                    // Handle empty state
                    if filtered.is_empty() {
                        div { class: "combobox-empty", "No results" }
                    }
                }
            }
        }
    }
}

#[component]
pub fn SearchableSelectSet<
    T: Clone + PartialEq + Eq + std::fmt::Display + std::hash::Hash + 'static,
>(
    items: ReadSignal<indexmap::IndexSet<T, rustc_hash::FxBuildHasher>>,
    on_select: EventHandler<(usize, T)>,
    placeholder: Option<String>,
    selected_index: Option<ReadSignal<usize>>,
) -> Element {
    // In your component
    let mut is_open = use_signal(|| false);
    let mut search = use_signal(String::new);
    let mut local_selected_value: Signal<Option<(usize, T)>> = use_signal(|| None);

    let filtered = use_memo(move || {
        let q = search.read().to_lowercase();
        items
            .read()
            .iter()
            .enumerate()
            .filter(|(_, v)| matches_search(*v, &q))
            .map(|(i, v)| (i, v.clone()))
            .collect::<Vec<(usize, T)>>()
    });

    let mut display_text = use_signal(String::new);

    use_effect(move || {
        let text = local_selected_value
            .read()
            .as_ref()
            .map(|(_, v)| v.to_string())
            .or(placeholder.clone())
            .unwrap_or_else(|| "Select an item...".to_string());
        display_text.set(text);
    });

    use_effect(move || {
        if let Some((i, val)) = local_selected_value() {
            on_select((i, val));
        };
    });

    use_effect(move || {
        if let Some(sig) = selected_index {
            let i = sig();
            let items = &*items.peek();
            if i < items.len() {
                display_text.set(items[i].to_string())
            }
        }
    });

    rsx! {
        div {
            class: "combobox-container",
            // Close when clicking outside (you might need a window listener for robust closing)
            onmouseleave: move |_| is_open.set(false),

            // The Trigger / Input
            div { class: "combobox-input-wrapper",
                input {
                    class: "combobox-input",
                    value: "{search}",
                    placeholder: "{display_text}", // Show selected if empty

                    autocorrect: "off",
                    autocapitalize: "none",
                    autocomplete: "off",
                    spellcheck: "false",

                    // Open on click or focus
                    onfocus: move |_| is_open.set(true),
                    onclick: move |_| is_open.set(true),

                    // Handle typing
                    oninput: move |e| {
                        search.set(e.value());
                        is_open.set(true);
                    },
                }
                // Optional: Arrow icon
                div {
                    class: "combobox-arrow",
                    onclick: move |_| is_open.toggle(),
                    "▼"
                }
            }

            // The Dropdown List
            // Only render if open
            if is_open() {
                div { class: "combobox-list scrollable",
                    // Iterate directly over items. We use enumerate just for the key.
                    for (original_i , item) in filtered().into_iter() {
                        // Capture the item for the closure
                        {
                            let current_item = item.clone();
                            rsx! {
                                div {
                                    key: "{original_i}", // React/Dioxus likes keys
                                    class: "combobox-option", // Use the Display implementation
                                    onclick: move |_| {
                                        // Set the actual item, not the index
                                        local_selected_value.set(Some((original_i, current_item.clone())));

                                        // Reset search so the placeholder (selected value) shows again
                                        search.set(String::new());
                                        is_open.set(false);
                                    },
                                    // Use the Display implementation
                                    "{item}"
                                }
                            }
                        }
                    }
                    // Handle empty state
                    if filtered.is_empty() {
                        div { class: "combobox-empty", "No results" }
                    }
                }
            }
        }
    }
}
