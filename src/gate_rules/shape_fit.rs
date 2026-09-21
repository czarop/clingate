//! Drawing a boundary round a population.
//!
//! The other half of the phenotype rule. [`phenotype`](super::phenotype) says
//! *which* events are the population; this says what shape holds them on the
//! two axes the gate is drawn on.
//!
//! A shape rather than a nudge, because the gate has to fit whatever those
//! cells look like in this sample - a population can be wider, narrower or a
//! different shape entirely in the next donor, and sliding the reference's
//! outline over it would keep the drawing and lose the cells.
//!
//! ## How the boundary is chosen
//!
//! The obvious answer, a convex hull, is the wrong one: every vertex of a hull
//! is the single most extreme event in some direction, so one misassigned cell
//! moves the boundary, and a population with a concave waist comes back as a
//! blob spanning the gap.
//!
//! Instead the population's density is estimated on a grid and the boundary is
//! the contour at the level that holds [`keep`](contour_around) of the events -
//! the highest-density region. Stragglers fall outside it without moving it,
//! concave shapes are traced as they are, and the result looks like a gate a
//! person would draw, because it follows the cloud.
//!
//! ## What it cannot do
//!
//! The boundary is two-dimensional and the population was identified across
//! the whole panel. Where those cells are not actually separated on these two
//! axes, no outline round them excludes their neighbours - anything drawn will
//! admit whatever else sits in the same place. That is a fact about the plot
//! rather than a failure of the fit, and it is why [`Fitted::purity`] is
//! reported: it is the number that distinguishes "this gate holds the
//! population" from "this gate holds the population and a great deal else".

use std::collections::HashMap;

/// A closed boundary in plot coordinates, wound counter-clockwise and not
/// repeating its first point.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline(pub Vec<(f64, f64)>);

/// Why no boundary could be drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum NoShape {
    /// Fewer events than a density could be estimated from.
    TooFew { events: usize, needed: usize },
    /// The density never crossed the level, or there was nothing for it to
    /// cross over - every event on one spot, or one axis with no spread at all.
    NoContour,
}

impl std::fmt::Display for NoShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoShape::TooFew { events, needed } => write!(
                f,
                "only {events} events matched; {needed} are needed to draw a boundary"
            ),
            NoShape::NoContour => {
                write!(
                    f,
                    "the matched events have no extent to draw a boundary round"
                )
            }
        }
    }
}

/// The fewest events a density estimate is worth making from.
pub const MIN_EVENTS: usize = 20;

/// A drawn boundary and what it turned out to hold.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub outline: Outline,
    /// How many separate pieces the contour came out in, before the largest
    /// was taken.
    ///
    /// More than one means the phenotype matched cells sitting in two places
    /// on this plot. That is worth seeing: either the population really is
    /// split here, or the signature is catching something else as well, and
    /// either way a single outline is not the whole story.
    pub pieces: usize,
    /// The fraction of the events inside the outline that are the population.
    ///
    /// One means the outline holds the matched cells and nothing else. A low
    /// value means the population is not separated on these two axes - see the
    /// module comment.
    pub purity: f64,
    /// The fraction of the population that ended up inside the outline.
    pub caught: f64,
}

/// Draw a boundary round `population` that holds `keep` of it.
///
/// `others` are the rest of the parent, used only to report purity - they
/// never move the boundary.
pub fn fit(
    population: &[(f64, f64)],
    others: &[(f64, f64)],
    keep: f64,
    smoothing: f64,
    vertices: usize,
) -> Result<Fitted, NoShape> {
    let (outline, pieces) = contour_around(population, keep, smoothing, vertices)?;
    let inside_population = population.iter().filter(|p| outline.holds(**p)).count();
    let inside_others = others.iter().filter(|p| outline.holds(**p)).count();
    let inside = inside_population + inside_others;
    Ok(Fitted {
        outline,
        pieces,
        purity: if inside == 0 {
            0.0
        } else {
            inside_population as f64 / inside as f64
        },
        caught: if population.is_empty() {
            0.0
        } else {
            inside_population as f64 / population.len() as f64
        },
    })
}

/// The contour holding `keep` of the points, and how many pieces it had.
pub fn contour_around(
    points: &[(f64, f64)],
    keep: f64,
    smoothing: f64,
    vertices: usize,
) -> Result<(Outline, usize), NoShape> {
    if points.len() < MIN_EVENTS {
        return Err(NoShape::TooFew {
            events: points.len(),
            needed: MIN_EVENTS,
        });
    }
    // Refused up front rather than left to produce a boundary of no size. A
    // population with no spread on one axis - a saturated detector, a channel
    // that was not collected - would otherwise come back as a sliver, which is
    // a valid polygon and a useless gate.
    let (width, height) = extent(points);
    if !(width > 0.0) || !(height > 0.0) {
        return Err(NoShape::NoContour);
    }
    let grid = Grid::of(points, smoothing);
    // The level that holds `keep` of the events is read off the events
    // themselves - the density under each one, and the quantile of those - so
    // it is the highest-density region by construction rather than a guess at
    // a contour height.
    let mut under: Vec<f64> = points.iter().map(|p| grid.at(*p)).collect();
    under.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = (((under.len() - 1) as f64) * (1.0 - keep.clamp(0.0, 1.0))).round() as usize;
    let level = under[at];
    if !(level > 0.0) {
        return Err(NoShape::NoContour);
    }

    let rings = grid.trace(level);
    let pieces = rings.len();
    let biggest = rings
        .into_iter()
        .max_by(|a, b| {
            area(a)
                .abs()
                .partial_cmp(&area(b).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or(NoShape::NoContour)?;

    let mut simple = simplify(&biggest, vertices);
    if area(&simple) < 0.0 {
        simple.reverse();
    }
    if simple.len() < 3 {
        return Err(NoShape::NoContour);
    }
    Ok((Outline(simple), pieces))
}

impl Outline {
    /// Whether a point is inside, by the crossing-number rule.
    pub fn holds(&self, (x, y): (f64, f64)) -> bool {
        let n = self.0.len();
        if n < 3 {
            return false;
        }
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = self.0[i];
            let (xj, yj) = self.0[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    pub fn area(&self) -> f64 {
        area(&self.0).abs()
    }
}

/// The bounding box's width and height.
fn extent(points: &[(f64, f64)]) -> (f64, f64) {
    let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
    for (x, y) in points {
        min_x = min_x.min(*x);
        max_x = max_x.max(*x);
        min_y = min_y.min(*y);
        max_y = max_y.max(*y);
    }
    (max_x - min_x, max_y - min_y)
}

/// Twice the signed area, positive counter-clockwise.
fn area(ring: &[(f64, f64)]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut total = 0.0;
    for i in 0..n {
        let (x1, y1) = ring[i];
        let (x2, y2) = ring[(i + 1) % n];
        total += x1 * y2 - x2 * y1;
    }
    total / 2.0
}

/// How many cells the density is estimated on, per axis.
const CELLS: usize = 128;

/// A smoothed density over a bounding box.
struct Grid {
    values: Vec<f64>,
    n: usize,
    x0: f64,
    y0: f64,
    dx: f64,
    dy: f64,
}

impl Grid {
    /// Estimate the density of `points`, on a box that leaves room for the
    /// contour to close outside the outermost events.
    fn of(points: &[(f64, f64)], smoothing: f64) -> Self {
        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        for (x, y) in points {
            min_x = min_x.min(*x);
            max_x = max_x.max(*x);
            min_y = min_y.min(*y);
            max_y = max_y.max(*y);
        }
        // A margin, so a contour round the outermost events has somewhere to
        // close. Without it the ring runs off the edge of the grid and is left
        // open, which marching squares cannot stitch.
        let pad_x = ((max_x - min_x) * 0.15).max(1e-9);
        let pad_y = ((max_y - min_y) * 0.15).max(1e-9);
        let (x0, x1) = (min_x - pad_x, max_x + pad_x);
        let (y0, y1) = (min_y - pad_y, max_y + pad_y);

        let n = CELLS;
        let dx = (x1 - x0) / (n - 1) as f64;
        let dy = (y1 - y0) / (n - 1) as f64;

        // Bandwidth per axis, Silverman against the points' own spread, then
        // whatever the rule asks for on top.
        let hx = (bandwidth(&points.iter().map(|p| p.0).collect::<Vec<_>>()) * smoothing).max(dx);
        let hy = (bandwidth(&points.iter().map(|p| p.1).collect::<Vec<_>>()) * smoothing).max(dy);

        // Bin first, then blur. Summing a Gaussian per event over the whole
        // grid is events x cells work; binning and blurring is events + cells,
        // and a separable blur makes the second term linear in the side.
        let mut values = vec![0.0f64; n * n];
        for (x, y) in points {
            let col = ((x - x0) / dx).round();
            let row = ((y - y0) / dy).round();
            if col >= 0.0 && row >= 0.0 && (col as usize) < n && (row as usize) < n {
                values[row as usize * n + col as usize] += 1.0;
            }
        }
        blur(&mut values, n, hx / dx, hy / dy);
        Self {
            values,
            n,
            x0,
            y0,
            dx,
            dy,
        }
    }

    /// The density at a point, by bilinear interpolation.
    fn at(&self, (x, y): (f64, f64)) -> f64 {
        let cx = ((x - self.x0) / self.dx).clamp(0.0, (self.n - 1) as f64);
        let cy = ((y - self.y0) / self.dy).clamp(0.0, (self.n - 1) as f64);
        let (i, j) = (cx.floor() as usize, cy.floor() as usize);
        let (i1, j1) = ((i + 1).min(self.n - 1), (j + 1).min(self.n - 1));
        let (fx, fy) = (cx - i as f64, cy - j as f64);
        let v = |a: usize, b: usize| self.values[b * self.n + a];
        v(i, j) * (1.0 - fx) * (1.0 - fy)
            + v(i1, j) * fx * (1.0 - fy)
            + v(i, j1) * (1.0 - fx) * fy
            + v(i1, j1) * fx * fy
    }

    fn point(&self, col: f64, row: f64) -> (f64, f64) {
        (self.x0 + col * self.dx, self.y0 + row * self.dy)
    }

    /// Every closed ring where the density crosses `level`.
    ///
    /// Marching squares. Crossings are keyed by the grid edge they lie on
    /// rather than by their coordinates, so stitching segments into rings is
    /// integer matching and cannot be defeated by two crossings that round to
    /// the same float.
    fn trace(&self, level: f64) -> Vec<Vec<(f64, f64)>> {
        let n = self.n;
        let mut links: HashMap<Edge, Vec<Edge>> = HashMap::new();
        let mut where_at: HashMap<Edge, (f64, f64)> = HashMap::new();

        for row in 0..n - 1 {
            for col in 0..n - 1 {
                let v = |a: usize, b: usize| self.values[b * n + a];
                let (bl, br, tr, tl) = (
                    v(col, row),
                    v(col + 1, row),
                    v(col + 1, row + 1),
                    v(col, row + 1),
                );
                let mut case = 0u8;
                if bl >= level {
                    case |= 1;
                }
                if br >= level {
                    case |= 2;
                }
                if tr >= level {
                    case |= 4;
                }
                if tl >= level {
                    case |= 8;
                }
                if case == 0 || case == 15 {
                    continue;
                }

                let bottom = Edge::horizontal(col, row);
                let right = Edge::vertical(col + 1, row);
                let top = Edge::horizontal(col, row + 1);
                let left = Edge::vertical(col, row);

                let mut place = |edge: Edge| {
                    where_at.entry(edge).or_insert_with(|| match edge.kind {
                        Kind::Horizontal => {
                            let (a, b) = (v(edge.col, edge.row), v(edge.col + 1, edge.row));
                            self.point(edge.col as f64 + cross(a, b, level), edge.row as f64)
                        }
                        Kind::Vertical => {
                            let (a, b) = (v(edge.col, edge.row), v(edge.col, edge.row + 1));
                            self.point(edge.col as f64, edge.row as f64 + cross(a, b, level))
                        }
                    });
                };

                // The centre decides the two ambiguous cases, so a saddle is
                // resolved the same way on both sides and the rings close.
                let centre = (bl + br + tr + tl) / 4.0;
                let pairs: &[(Edge, Edge)] = &match case {
                    1 | 14 => vec![(left, bottom)],
                    2 | 13 => vec![(bottom, right)],
                    3 | 12 => vec![(left, right)],
                    4 | 11 => vec![(right, top)],
                    6 | 9 => vec![(bottom, top)],
                    7 | 8 => vec![(left, top)],
                    5 => {
                        if centre >= level {
                            vec![(left, top), (bottom, right)]
                        } else {
                            vec![(left, bottom), (right, top)]
                        }
                    }
                    10 => {
                        if centre >= level {
                            vec![(left, bottom), (right, top)]
                        } else {
                            vec![(left, top), (bottom, right)]
                        }
                    }
                    _ => vec![],
                };
                for (a, b) in pairs {
                    place(*a);
                    place(*b);
                    links.entry(*a).or_default().push(*b);
                    links.entry(*b).or_default().push(*a);
                }
            }
        }

        // Walk each connected run of crossings into a ring.
        let mut seen: HashMap<Edge, bool> = HashMap::new();
        let mut rings = Vec::new();
        let starts: Vec<Edge> = links.keys().copied().collect();
        for start in starts {
            if seen.contains_key(&start) {
                continue;
            }
            let mut ring = Vec::new();
            let mut at = start;
            let mut from: Option<Edge> = None;
            loop {
                seen.insert(at, true);
                if let Some(p) = where_at.get(&at) {
                    ring.push(*p);
                }
                let next = links.get(&at).and_then(|nexts| {
                    nexts
                        .iter()
                        .copied()
                        .find(|e| Some(*e) != from && !seen.contains_key(e))
                });
                match next {
                    Some(next) => {
                        from = Some(at);
                        at = next;
                    }
                    None => break,
                }
            }
            if ring.len() >= 3 {
                rings.push(ring);
            }
        }
        rings
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Edge {
    kind: Kind,
    col: usize,
    row: usize,
}

impl Edge {
    fn horizontal(col: usize, row: usize) -> Self {
        Self {
            kind: Kind::Horizontal,
            col,
            row,
        }
    }
    fn vertical(col: usize, row: usize) -> Self {
        Self {
            kind: Kind::Vertical,
            col,
            row,
        }
    }
}

/// Where between two corners the level falls.
fn cross(a: f64, b: f64, level: f64) -> f64 {
    let span = b - a;
    if span.abs() < f64::EPSILON {
        0.5
    } else {
        ((level - a) / span).clamp(0.0, 1.0)
    }
}

/// Silverman's rule, on the spread of one axis.
fn bandwidth(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 2 {
        return 1.0;
    }
    let mean = values.iter().sum::<f64>() / n as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    let sd = variance.sqrt();
    if sd <= 0.0 {
        return 1e-9;
    }
    1.06 * sd * (n as f64).powf(-0.2)
}

/// A separable Gaussian blur, in cells.
fn blur(values: &mut [f64], n: usize, sigma_x: f64, sigma_y: f64) {
    for (sigma, horizontal) in [(sigma_x, true), (sigma_y, false)] {
        if sigma <= 0.0 {
            continue;
        }
        let radius = (sigma * 3.0).ceil() as isize;
        let kernel: Vec<f64> = (-radius..=radius)
            .map(|d| (-(d as f64).powi(2) / (2.0 * sigma * sigma)).exp())
            .collect();
        let total: f64 = kernel.iter().sum();
        let mut out = vec![0.0; n * n];
        for row in 0..n {
            for col in 0..n {
                let mut sum = 0.0;
                for (at, weight) in kernel.iter().enumerate() {
                    let step = at as isize - radius;
                    let (c, r) = if horizontal {
                        (col as isize + step, row as isize)
                    } else {
                        (col as isize, row as isize + step)
                    };
                    if c >= 0 && r >= 0 && (c as usize) < n && (r as usize) < n {
                        sum += values[r as usize * n + c as usize] * weight;
                    }
                }
                out[row * n + col] = sum / total;
            }
        }
        values.copy_from_slice(&out);
    }
}

/// Thin a ring down to about `target` points, keeping the ones that carry the
/// shape.
///
/// Douglas-Peucker with the tolerance found by bisection, because what is
/// wanted here is a vertex count - a gate with two hundred points is a
/// different kind of object from one a person drew, however well it fits.
pub fn simplify(ring: &[(f64, f64)], target: usize) -> Vec<(f64, f64)> {
    if ring.len() <= target.max(3) {
        return ring.to_vec();
    }
    let extent = {
        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        for (x, y) in ring {
            min_x = min_x.min(*x);
            max_x = max_x.max(*x);
            min_y = min_y.min(*y);
            max_y = max_y.max(*y);
        }
        ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt()
    };
    let (mut lo, mut hi) = (0.0, extent.max(1e-9));
    let mut best = ring.to_vec();
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        let candidate = douglas_peucker(ring, mid);
        if candidate.len() > target {
            lo = mid;
        } else {
            best = candidate;
            hi = mid;
        }
    }
    if best.len() < 3 { ring.to_vec() } else { best }
}

fn douglas_peucker(points: &[(f64, f64)], tolerance: f64) -> Vec<(f64, f64)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    let last = points.len() - 1;
    keep[last] = true;
    let mut stack = vec![(0usize, last)];
    while let Some((start, end)) = stack.pop() {
        let mut worst = 0.0;
        let mut at = start;
        for i in start + 1..end {
            let d = perpendicular(points[i], points[start], points[end]);
            if d > worst {
                worst = d;
                at = i;
            }
        }
        if worst > tolerance && at > start {
            keep[at] = true;
            stack.push((start, at));
            stack.push((at, end));
        }
    }
    points
        .iter()
        .zip(keep.iter())
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect()
}

fn perpendicular((x, y): (f64, f64), (x1, y1): (f64, f64), (x2, y2): (f64, f64)) -> f64 {
    let (dx, dy) = (x2 - x1, y2 - y1);
    let length = (dx * dx + dy * dy).sqrt();
    if length < f64::EPSILON {
        return ((x - x1).powi(2) + (y - y1).powi(2)).sqrt();
    }
    ((x - x1) * dy - (y - y1) * dx).abs() / length
}
