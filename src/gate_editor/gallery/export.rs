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
use super::pdf::{Cell, Drawn, Sheet, write_pdf};
use super::render::{PlotJob, render_plot};
use super::select;
use super::window::Card;

/// Pixels per plot in the exported file.
///
/// The page gives a plot about 170 points, so this is a little over three times
/// its printed size - around 220 dots per inch, which is where a density plot
/// stops looking like pixels on paper.
const EXPORT_SIZE: u32 = 512;

/// One file's place on the sheet, and what to put there.
pub(super) struct ExportJob {
    /// Which specimen, and which of its files: where the plot goes on the sheet.
    pub(super) card: usize,
    pub(super) slot: usize,
    pub(super) name: String,
    /// The plot to draw, or why there is none to draw. A paired file that
    /// cannot be drawn is still on the sheet, saying why, rather than being
    /// left as an empty slot - which the sheet prints as "no paired file".
    pub(super) plot: Result<ToDraw, String>,
}

/// What rendering one plot needs.
pub(super) struct ToDraw {
    pub(super) job: PlotJob,
    pub(super) drawn: Vec<Arc<dyn DrawableGate>>,
    pub(super) selected: Option<Arc<str>>,
}

/// A written contact sheet, and the plots it could not draw.
pub(super) struct ContactSheet {
    pub(super) pdf: Vec<u8>,
    /// Each file that could not be drawn, with why - for the message that
    /// says the sheet was written, which must not read as a clean run.
    pub(super) failed: Vec<(String, String)>,
}

/// What the on-screen gallery says for a paired file with no metadata row,
/// said the same way on paper.
pub(super) const NO_METADATA: &str = "no metadata for this file";

/// Render every job and lay the plots out as the contact sheet.
///
/// `titles` is one entry per specimen: its title and how many files it has.
/// Blocking, and parallel across the jobs; `done` counts finished plots for
/// the progress bar. A run stopped part-way is an error - a sheet with holes
/// where the stop landed is not a record of anything.
pub(super) fn contact_sheet(
    heading: &str,
    titles: &[(String, usize)],
    jobs: &[ExportJob],
    stop: &AtomicBool,
    done: &AtomicUsize,
) -> anyhow::Result<ContactSheet> {
    let rendered: Vec<Option<(usize, usize, Cell)>> = jobs
        .par_iter()
        .map(|entry| {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            let cell = match &entry.plot {
                Ok(plot) => match render_plot(&plot.job) {
                    Ok(image) => Cell::Drawn(Drawn {
                        name: entry.name.clone(),
                        png: image.png.clone(),
                        shapes: flatten_gates(
                            &plot.drawn,
                            &image.stats,
                            plot.selected.as_ref(),
                            &image.mapper,
                        ),
                        rendered_at: plot.job.size as f32,
                    }),
                    Err(e) => Cell::Failed {
                        name: entry.name.clone(),
                        reason: e.to_string(),
                    },
                },
                Err(reason) => Cell::Failed {
                    name: entry.name.clone(),
                    reason: reason.clone(),
                },
            };
            done.fetch_add(1, Ordering::Relaxed);
            Some((entry.card, entry.slot, cell))
        })
        .collect();

    if stop.load(Ordering::Relaxed) {
        return Err(anyhow::anyhow!("stopped"));
    }

    let mut sheets: Vec<Sheet> = titles
        .iter()
        .map(|(title, slots)| Sheet {
            title: title.clone(),
            slots: (0..*slots).map(|_| Cell::NoFile).collect(),
        })
        .collect();
    let mut failed = Vec::new();
    for (card, slot, cell) in rendered.into_iter().flatten() {
        if let Cell::Failed { name, reason } = &cell {
            failed.push((name.clone(), reason.clone()));
        }
        if let Some(sheet) = sheets.get_mut(card)
            && let Some(place) = sheet.slots.get_mut(slot)
        {
            *place = cell;
        }
    }
    Ok(ContactSheet {
        pdf: write_pdf(heading, &sheets)?,
        failed,
    })
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
                    let Some(file) = filled else {
                        continue;
                    };
                    let Some((file_id, groups)) = names.get(&file.name).and_then(|file_id| {
                        Some((file_id.clone(), metadata.get(file_id)?.clone()))
                    }) else {
                        jobs.push(ExportJob {
                            card: at,
                            slot,
                            name: file.label().to_string(),
                            plot: Err(NO_METADATA.to_string()),
                        });
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
                        name: file.label().to_string(),
                        plot: Ok(ToDraw {
                            job: PlotJob {
                                path: file.path.clone(),
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
                        }),
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
            let outcome = tokio::task::spawn_blocking(move || {
                contact_sheet(&heading, &titles, &jobs, &stopping, &done)
            })
            .await;

            counter.cancel();
            cancel.set(None);
            progress.set(None);

            let written = match outcome {
                Ok(Ok(sheet)) => std::fs::write(&target, &sheet.pdf)
                    .map(|()| (target.display().to_string(), sheet.failed))
                    .map_err(|e| e.to_string()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(format!("export thread failed: {e}")),
            };
            match written {
                Ok((where_to, failed)) if failed.is_empty() => {
                    say(&toasts, format!("Contact sheet written to {where_to}"))
                }
                // Written, but not the clean record the plain message would
                // claim: say which files are on it without a plot.
                Ok((where_to, failed)) => warn(
                    &toasts,
                    format!(
                        "Contact sheet written to {where_to} - {} could not be drawn: {}",
                        match failed.len() {
                            1 => "1 plot".to_string(),
                            n => format!("{n} plots"),
                        },
                        failed
                            .iter()
                            .map(|(name, reason)| format!("{name} ({reason})"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ),
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
