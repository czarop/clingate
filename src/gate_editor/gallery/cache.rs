//! Knowing when a drawn plot is still the right picture.
//!
//! The gallery draws the same twenty files over and over - page forward, page
//! back, pick a different gate and come back to this one - and drawing is the
//! expensive thing in this program. Caching the images makes all of that free,
//! but only if the cache can tell when a picture has gone stale, and the thing
//! that makes it stale is a gate moving. That is precisely what the autogater
//! does, to hundreds of gates at once, while the tab is open.
//!
//! ## How a moved gate is noticed
//!
//! Every resolved gate in this program is an `Arc<dyn DrawableGate>`, and a
//! gate is never mutated in place: moving one inserts a *new* `Arc` into the
//! registry or into an override map. So pointer identity already answers the
//! question - the same `Arc` is the same geometry, and a different `Arc` is a
//! gate that has been written since. `ComparableGate` in the gate store leans
//! on exactly this, and so does the editor's own `chain_gates` memo.
//!
//! A fingerprint is therefore the addresses of every gate the picture depends
//! on: the chain that filters the events, and the gates drawn on top.
//!
//! ## Why the fingerprint holds the gates
//!
//! Hashing an address is only sound while that address cannot be handed to
//! something else. If the last `Arc` to a gate were dropped, the allocator
//! could put an unrelated gate at the same address and a stale image would
//! answer to a fresh fingerprint - a plot showing the wrong position, which is
//! the one failure this tab must not have.
//!
//! So the fingerprint *keeps* the `Arc`s it hashed. While an entry is in the
//! cache its gates are alive, their addresses cannot be reused, and equal
//! fingerprints really are the same gates. The cost is a handful of live gates
//! per cached image, which is nothing beside the image.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::gate_editor::AxisInfo;
use crate::gate_editor::gates::gate_traits::DrawableGate;

use super::render::PlotImage;

/// The address of a gate, as its identity.
fn address(gate: &Arc<dyn DrawableGate>) -> usize {
    Arc::as_ptr(gate) as *const () as usize
}

/// Everything a drawn plot depends on.
///
/// Two plots with equal fingerprints are the same picture, so one may be shown
/// where the other was asked for.
#[derive(Clone)]
pub struct Fingerprint {
    /// The file, by the id the metadata knows it as.
    pub file: Arc<str>,
    pub x: Arc<str>,
    pub y: Arc<str>,
    pub x_axis: AxisInfo,
    pub y_axis: AxisInfo,
    pub size: u32,
    /// A digest of every channel's scaling.
    ///
    /// The axes above cover the two channels this plot is drawn on, and a gate
    /// rescaled by a cofactor change gets a new `Arc`, so most of this is
    /// already covered twice over. What is not is a chain gate whose *other*
    /// axis was rescaled: the events it admits move, the picture changes, and
    /// nothing else here would have noticed. A digest is cheap enough that
    /// relying on a subtle invariant in another module is not worth it.
    pub scaling: u64,
    /// Every gate this picture depends on - the filtering chain first, then the
    /// gates drawn on it. Held, not merely hashed: see the module comment.
    pub gates: Vec<Arc<dyn DrawableGate>>,
}

/// Fold every channel's cofactor into one number.
///
/// Order-independent, because the axis settings are a hash map and iterate in
/// no fixed order: the same settings must digest the same way whichever order
/// they come out in. Exclusive-or of per-entry hashes gives that.
pub fn scaling_digest(cofactors: &[(Arc<str>, f32)]) -> u64 {
    cofactors.iter().fold(0u64, |acc, (channel, cofactor)| {
        let mut hasher = rustc_hash::FxHasher::default();
        channel.hash(&mut hasher);
        cofactor.to_bits().hash(&mut hasher);
        acc ^ hasher.finish()
    })
}

impl PartialEq for Fingerprint {
    fn eq(&self, other: &Self) -> bool {
        self.file == other.file
            && self.x == other.x
            && self.y == other.y
            && self.size == other.size
            && self.scaling == other.scaling
            && self.x_axis == other.x_axis
            && self.y_axis == other.y_axis
            && self.gates.len() == other.gates.len()
            && self
                .gates
                .iter()
                .zip(other.gates.iter())
                .all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

impl Eq for Fingerprint {}

impl Hash for Fingerprint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.file.hash(state);
        self.x.hash(state);
        self.y.hash(state);
        self.size.hash(state);
        self.scaling.hash(state);
        // An axis is floats, which do not hash. The bits do, and two axes with
        // identical bits are the same axis - including the NaN case, where bit
        // equality is stricter than `==` rather than looser, so it can only
        // ever miss the cache.
        for axis in [&self.x_axis, &self.y_axis] {
            axis.axis_lower.to_bits().hash(state);
            axis.axis_upper.to_bits().hash(state);
            axis.param.fluoro.hash(state);
        }
        for gate in &self.gates {
            address(gate).hash(state);
        }
    }
}

impl std::fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fingerprint")
            .field("file", &self.file)
            .field("x", &self.x)
            .field("y", &self.y)
            .field("size", &self.size)
            .field("gates", &self.gates.iter().map(address).collect::<Vec<_>>())
            .finish()
    }
}

/// Drawn plots, by what they were drawn from.
///
/// Bounded, and oldest-first when it has to give something up. A run of a
/// hundred files at twenty plots a page is five pages, so a few hundred entries
/// holds everything a person will page through while keeping the memory a
/// stated number rather than an open question.
pub struct PlotCache {
    entries: FxHashMap<Fingerprint, (u64, Arc<PlotImage>)>,
    /// Bumped on every insert, so the least recently *inserted* entry is known
    /// without keeping a second list in step with the map.
    clock: u64,
    limit: usize,
}

impl Default for PlotCache {
    fn default() -> Self {
        Self::with_limit(400)
    }
}

impl PlotCache {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            entries: FxHashMap::default(),
            clock: 0,
            limit: limit.max(1),
        }
    }

    pub fn get(&self, key: &Fingerprint) -> Option<Arc<PlotImage>> {
        self.entries.get(key).map(|(_, image)| image.clone())
    }

    pub fn insert(&mut self, key: Fingerprint, image: Arc<PlotImage>) {
        self.clock += 1;
        let stamp = self.clock;
        self.entries.insert(key, (stamp, image));
        while self.entries.len() > self.limit {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (stamp, _))| *stamp)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Throw everything away, for when the document underneath changes so
    /// completely that no cached picture can still be right - a fresh gating
    /// file, or a fresh folder of FCS.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
