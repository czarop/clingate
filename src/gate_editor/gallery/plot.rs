//! One plot on the gallery page: look it up, draw it if it is not there.
//!
//! Every plot on a page is its own component with its own resource, which is
//! what gives the page its progressive fill - each picture appears as it is
//! finished rather than the page arriving all at once. It is also what makes
//! paging cheap: leaving a page drops its components, and dropping a component
//! drops the future, so a render nobody is waiting for stops at its next await
//! instead of holding a permit.
//!
//! The permits are the other half. Twenty resources firing at once would open
//! twenty FCS files and hold twenty scaled DataFrames, which is the memory
//! spike, not the throughput win - the work is single-threaded per plot and
//! there are not twenty cores. Four at a time keeps every core busy with a
//! bounded working set, and the queue drains in the order the page reads.

use std::path::PathBuf;
use std::sync::Arc;

use dioxus::prelude::*;
use dioxus::stores::SyncStore;
use tokio::sync::Semaphore;

use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::GateState;
use crate::gate_editor::gates::gate_store::{GateId, GateOverrideResolver, GateStateStoreExt};
use crate::gate_editor::gates::gate_traits::DrawableGate;
use crate::gate_editor::plots::axis_store::{AxisStore, AxisStoreStoreExt, Param};
use crate::omiq::metadata::{MetaDataStore, MetaDataStoreStoreExt};

use super::cache::{Fingerprint, PlotCache, scaling_digest};
use super::overlay::{Flat, StaticGates, flatten_gates};
use super::render::{PlotImage, PlotJob, render_plot};
use super::select;

/// How many plots may be in flight at once, shared by the whole page.
#[derive(Clone)]
pub struct Permits(pub Arc<Semaphore>);

impl Permits {
    pub fn new(at_once: usize) -> Self {
        Self(Arc::new(Semaphore::new(at_once)))
    }
}

impl PartialEq for Permits {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Everything needed to draw one plot, worked out from the stores.
///
/// Built in one memo rather than several because it is one question - what does
/// this plot show - and because the fingerprint has to be taken from the same
/// read of the store as the gates it fingerprints.
#[derive(Clone)]
pub struct Setup {
    pub fingerprint: Fingerprint,
    pub chain: Vec<GateId>,
    pub resolver: GateOverrideResolver,
    pub cofactors: Vec<(Arc<str>, f32)>,
    /// The drawn gates, rewritten for this plot's axes.
    pub drawn: Vec<Arc<dyn DrawableGate>>,
    pub x_axis: AxisInfo,
    pub y_axis: AxisInfo,
    pub path: PathBuf,
    pub size: u32,
}

impl PartialEq for Setup {
    /// By fingerprint alone.
    ///
    /// Not a shortcut: the fingerprint is, by construction, every input the
    /// rest of this struct is derived from - the file, the axes, the size and
    /// the identity of every gate involved. Two setups with the same
    /// fingerprint draw the same picture, so comparing the derived fields as
    /// well could only ever report a difference that is not one, and would
    /// re-render on it.
    fn eq(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint
    }
}

#[component]
pub fn GalleryPlot(path: PathBuf, node: Arc<str>, x: Param, y: Param, size: u32) -> Element {
    let gate_store = use_context::<SyncStore<GateState>>();
    let metadata_store =
        use_context::<Store<MetaDataStore, CopyValue<MetaDataStore, SyncStorage>>>();
    let axis_store = use_context::<Store<AxisStore, CopyValue<AxisStore, SyncStorage>>>();
    let mut cache = use_context::<SyncSignal<PlotCache>>();
    let permits = use_context::<Permits>();

    let setup = use_memo({
        let path = path.clone();
        let node = node.clone();
        let x = x.clone();
        let y = y.clone();
        move || {
            let file_name: Arc<str> = Arc::from(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default(),
            );
            let file_id = metadata_store
                .file_name_to_gating_id()
                .read()
                .get(&file_name)
                .cloned()?;
            let groups = metadata_store.metadata().read().get(&file_id).cloned()?;

            // Tracked, so a gate moving anywhere re-runs this. The fingerprint
            // then decides whether it changed anything *here* - a memo only
            // notifies when its value differs, and the value is compared by
            // fingerprint, so a gate edited off this plot's chain stops at this
            // line.
            let state = gate_store.read();
            let resolver = state.get_current_sample(file_id.clone(), &groups);

            let gates = select::dependencies(&state, &node, &resolver);
            let chain = select::chain_of(&state, &node);
            let drawn = select::matched_to_axes(
                &select::drawn_on(&state, &node, &resolver),
                &x.fluoro,
                &y.fluoro,
            );

            let settings = axis_store.settings();
            let settings = settings.read();
            let x_axis = settings.get(&x.fluoro).cloned().unwrap_or_default();
            let y_axis = settings.get(&y.fluoro).cloned().unwrap_or_default();
            let cofactors: Vec<(Arc<str>, f32)> = settings
                .iter()
                .filter_map(|(k, v)| v.get_cofactor().map(|c| (k.clone(), c)))
                .collect();

            Some(Setup {
                fingerprint: Fingerprint {
                    scaling: scaling_digest(&cofactors),
                    file: file_id,
                    x: x.fluoro.clone(),
                    y: y.fluoro.clone(),
                    x_axis: x_axis.clone(),
                    y_axis: y_axis.clone(),
                    size,
                    gates,
                },
                chain,
                resolver,
                cofactors,
                drawn,
                x_axis,
                y_axis,
                path: path.clone(),
                size,
            })
        }
    });

    let image = use_resource(move || {
        let setup = setup();
        let permits = permits.0.clone();
        async move {
            let Some(setup) = setup else {
                return Err("no metadata for this file".to_string());
            };
            if let Some(hit) = cache.peek().get(&setup.fingerprint) {
                return Ok(hit);
            }
            // Dropped here if the page moves on, which releases the permit and
            // lets the page being looked at have it.
            let _permit = permits
                .acquire_owned()
                .await
                .map_err(|_| "render queue closed".to_string())?;
            // Re-checked: while this was queued, a neighbouring plot of the
            // same file and gate may have finished the identical picture. On a
            // page where the FMX and the FS of a specimen share a position this
            // is not a rare case.
            if let Some(hit) = cache.peek().get(&setup.fingerprint) {
                return Ok(hit);
            }

            let key = setup.fingerprint.clone();
            let job = PlotJob {
                path: setup.path.clone(),
                cofactors: setup.cofactors.clone(),
                chain: setup.chain.clone(),
                resolver: setup.resolver.clone(),
                x: setup.fingerprint.x.clone(),
                y: setup.fingerprint.y.clone(),
                x_axis: setup.x_axis.clone(),
                y_axis: setup.y_axis.clone(),
                gates: setup.drawn.clone(),
                size: setup.size,
            };
            let drawn = tokio::task::spawn_blocking(move || render_plot(&job))
                .await
                .map_err(|e| format!("render thread failed: {e}"))?
                .map_err(|e| e.to_string())?;
            let drawn = Arc::new(drawn);
            cache.write().insert(key, drawn.clone());
            Ok(drawn)
        }
    });

    // The outline, over the picture. Kept out of the render so that selecting a
    // different gate redraws the lines without redrawing the bitmap.
    let shapes = use_memo(move || {
        let picture: Arc<PlotImage> = match &*image.read() {
            Some(Ok(image)) => image.clone(),
            _ => return Vec::<Flat>::new(),
        };
        let Some(setup) = setup() else {
            return Vec::new();
        };
        let selected = gate_store.selected_gate().read().clone();
        flatten_gates(
            &setup.drawn,
            &picture.stats,
            selected.as_ref(),
            &picture.mapper,
        )
    });

    let side = size;
    match &*image.read() {
        Some(Ok(picture)) => {
            let src = picture.src.clone();
            rsx! {
                div {
                    class: "gallery-plot_frame",
                    style: "width: {side}px; height: {side}px;",
                    img { src: "{src}", width: "{side}", height: "{side}", draggable: false }
                    StaticGates { shapes, size: side }
                }
            }
        }
        Some(Err(why)) => rsx! {
            div {
                class: "gallery-plot_frame gallery-plot_failed",
                style: "width: {side}px; height: {side}px;",
                span { "{why}" }
            }
        },
        None => rsx! {
            div {
                class: "gallery-plot_frame gallery-plot_waiting",
                style: "width: {side}px; height: {side}px;",
                div { class: "spinner" }
            }
        },
    }
}
