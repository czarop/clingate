//! Writing the gallery out as a PDF.
//!
//! The tab renders a page at a time because that is what a person looks at.
//! The export is the other case: every specimen, at a size worth printing, and
//! nobody watching while it happens. So it does not go through the page's
//! cache or its permits - those exist to keep an interactive page responsive -
//! and runs the whole run through rayon instead, with a count and a Stop.
//!
//! Plots are re-rendered rather than taken from the cache. The cache holds
//! whatever size the person happened to be viewing, which may be 200 pixels
//! across; a QC record that cannot be read when printed is not a record.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use dioxus::prelude::*;
use dioxus::stores::SyncStore;
use rayon::prelude::*;

use crate::components::toast::{say, use_toast, warn};
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::{GateStateStoreExt, ROOTGATE};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::path_picker::{Pick, PickPath};
use crate::gate_editor::plots::axis_store::{AxisStore, AxisStoreStoreExt, Param};
use crate::omiq::metadata::{MetaDataStore, MetaDataStoreStoreExt};

use super::overlay::flatten_gates;
use super::pdf::{Drawn, Sheet, write_pdf};
use super::render::{PlotJob, render_plot};
use super::select;
use super::window::Card;

/// Pixels per plot in the exported file.
///
/// The page gives a plot about 170 points, so this is a little over three times
/// its printed size - around 220 dots per inch, which is where a density plot
/// stops looking like pixels on paper.
const EXPORT_SIZE: u32 = 512;

/// One plot's work, with the pieces the page needs once it is drawn.
struct ExportJob {
    card: usize,
    slot: usize,
    name: String,
    job: PlotJob,
    drawn: Vec<Arc<dyn DrawableGate>>,
    selected: Option<Arc<str>>,
}

#[component]
pub fn ExportPdf(
    cards: Vec<Card>,
    node: Arc<str>,
    x: Param,
    y: Param,
    gate_name: String,
) -> Element {
    let gate_store = use_context::<SyncStore<GateState>>();
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();
    let axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();

    let toasts = use_toast();
    let mut path = use_signal(|| "gate_gallery.pdf".to_string());
    let mut progress = use_signal(|| None::<(usize, usize)>);
    let mut cancel = use_signal(|| None::<Arc<AtomicBool>>);

    let running = cancel.read().is_some();
    // Read again below, after the closure has taken its own copy.
    let at_root = *node == **ROOTGATE;

    let start = move |_| {
        let cards = cards.clone();
        let node = node.clone();
        let (x, y) = (x.clone(), y.clone());
        // ASCII only: a page's strings are WinAnsi, so anything else is
        // written out as a question mark. A dash that survives beats one that
        // turns into punctuation nobody chose.
        let heading = format!("{gate_name}  -  {x} / {y}");
        let target = std::path::PathBuf::from(path());

        // Every store read happens here, on the UI thread, before anything is
        // handed to a worker. A resolver built now is a snapshot: the run
        // exports the gates as they stand at the moment Export was pressed,
        // which is the only answer that makes the file mean anything.
        let mut jobs: Vec<ExportJob> = Vec::new();
        let mut titles: Vec<(String, usize)> = Vec::new();
        {
            let state = gate_store.read();
            let settings = axis_store.settings();
            let settings = settings.read();
            let x_axis = settings.get(&x.fluoro).cloned().unwrap_or_default();
            let y_axis = settings.get(&y.fluoro).cloned().unwrap_or_default();
            let cofactors: Vec<(Arc<str>, f32)> = settings
                .iter()
                .filter_map(|(k, v)| v.get_cofactor().map(|c| (k.clone(), c)))
                .collect();
            let selected = gate_store.selected_gate().read().clone();
            let names = metadata_store.file_name_to_gating_id();
            let names = names.read();
            let metadata = metadata_store.metadata();
            let metadata = metadata.read();

            for (at, card) in cards.iter().enumerate() {
                titles.push((card.title.clone(), card.slots.len()));
                for (slot, filled) in card.slots.iter().enumerate() {
                    let Some((name, file_path)) = filled else {
                        continue;
                    };
                    let key: Arc<str> = Arc::from(
                        file_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or_default(),
                    );
                    let Some(file_id) = names.get(&key).cloned() else {
                        continue;
                    };
                    let Some(groups) = metadata.get(&file_id).cloned() else {
                        continue;
                    };
                    let resolver = state.get_current_sample(file_id, &groups);
                    let drawn = select::matched_to_axes(
                        &select::drawn_on(&state, &node, &resolver),
                        &x.fluoro,
                        &y.fluoro,
                    );
                    jobs.push(ExportJob {
                        card: at,
                        slot,
                        name: name.clone(),
                        job: PlotJob {
                            path: file_path.clone(),
                            cofactors: cofactors.clone(),
                            chain: select::chain_of(&state, &node),
                            resolver,
                            x: x.fluoro.clone(),
                            y: y.fluoro.clone(),
                            x_axis: x_axis.clone(),
                            y_axis: y_axis.clone(),
                            gates: drawn.clone(),
                            size: EXPORT_SIZE,
                        },
                        drawn,
                        selected: selected.clone(),
                    });
                }
            }
        }

        if jobs.is_empty() {
            warn(&toasts, "Nothing to export");
            return;
        }

        let total = jobs.len();
        let stop = Arc::new(AtomicBool::new(false));
        cancel.set(Some(stop.clone()));
        progress.set(Some((0, total)));

        spawn(async move {
            let done = Arc::new(AtomicUsize::new(0));
            let ticker = done.clone();
            let watching = stop.clone();
            // The count is polled rather than pushed: a rayon worker writing a
            // signal per finished plot would wake the renderer hundreds of
            // times to move a number.
            let counter = spawn(async move {
                loop {
                    let seen = ticker.load(Ordering::Relaxed);
                    progress.set(Some((seen, total)));
                    if seen >= total || watching.load(Ordering::Relaxed) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                }
            });

            let stopping = stop.clone();
            let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
                let rendered: Vec<Option<(usize, usize, Drawn)>> = jobs
                    .par_iter()
                    .map(|entry| {
                        if stopping.load(Ordering::Relaxed) {
                            return None;
                        }
                        let image = render_plot(&entry.job).ok();
                        done.fetch_add(1, Ordering::Relaxed);
                        let image = image?;
                        let shapes = flatten_gates(
                            &entry.drawn,
                            &image.stats,
                            entry.selected.as_ref(),
                            &image.mapper,
                        );
                        Some((
                            entry.card,
                            entry.slot,
                            Drawn {
                                name: entry.name.clone(),
                                jpeg: image.jpeg.clone(),
                                shapes,
                                rendered_at: entry.job.size as f32,
                            },
                        ))
                    })
                    .collect();

                if stopping.load(Ordering::Relaxed) {
                    return Err(anyhow::anyhow!("stopped"));
                }

                let mut sheets: Vec<Sheet> = titles
                    .iter()
                    .map(|(title, slots)| Sheet {
                        title: title.clone(),
                        slots: (0..*slots).map(|_| None).collect(),
                    })
                    .collect();
                for (card, slot, drawn) in rendered.into_iter().flatten() {
                    if let Some(sheet) = sheets.get_mut(card)
                        && let Some(place) = sheet.slots.get_mut(slot)
                    {
                        *place = Some(drawn);
                    }
                }
                write_pdf(&heading, &sheets)
            })
            .await;

            counter.cancel();
            cancel.set(None);
            progress.set(None);

            let written = match outcome {
                Ok(Ok(bytes)) => std::fs::write(&target, bytes)
                    .map(|()| target.display().to_string())
                    .map_err(|e| e.to_string()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(format!("export thread failed: {e}")),
            };
            match written {
                Ok(where_to) => say(&toasts, format!("Contact sheet written to {where_to}")),
                Err(why) => warn(&toasts, format!("Could not export: {why}")),
            }
        });
    };

    rsx! {
        div { class: "gallery-export",
            input {
                value: "{path}",
                disabled: running,
                oninput: move |e| path.set(e.value()),
            }
            PickPath {
                path,
                mode: Pick::SaveFile,
                label: "PDF",
                extensions: vec!["pdf".to_string()],
                disabled: running,
            }
            if running {
                button {
                    class: "gallery-export_stop",
                    onclick: move |_| {
                        if let Some(stop) = cancel.read().as_ref() {
                            stop.store(true, Ordering::Relaxed);
                        }
                    },
                    "Stop"
                }
            } else {
                button {
                    class: "gallery-export_go",
                    disabled: at_root,
                    title: "Every specimen in the run, at print size, in the order shown",
                    onclick: start,
                    "Export PDF"
                }
            }
            // Progress stays inline: it describes what is happening right
            // now and has to be watchable, which is the opposite of a message
            // that fades.
            if let Some((done, total)) = progress() {
                span { class: "gallery-export_note", "drawing {done} of {total}" }
            }
        }
    }
}
