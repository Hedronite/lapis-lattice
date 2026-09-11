//! 2D force-directed layout for the whole-vault graph.
//!
//! Ported from the forces two open graph views agree on — Quartz's d3 stack
//! (`charge`, `center`, `link`, link distance) and the Barnes-Hut Rust sim in
//! `obsidian-3d-graph`, flattened to two dimensions. Neither library is a
//! dependency: there is no Pixi, no d3, no Cytoscape here, only the arithmetic.
//!
//! The sim is deliberately headless. It owns positions and velocities, ticks
//! them, and reports kinetic energy; drawing belongs to the window and storage
//! belongs to the engine. That is what makes it testable without a GPU.
//!
//! Repulsion is all-pairs below [`BARNES_HUT_THRESHOLD`] nodes and a Barnes-Hut
//! quadtree above it. The exact sum is cheaper than a tree on a small vault; on
//! a big one the tree is the difference between 2 000 nodes at 60 fps and 2 000
//! nodes at 30.

use lapis_lattice::GraphSnapshot;

/// Above this many nodes, repulsion goes through the quadtree.
pub const BARNES_HUT_THRESHOLD: usize = 64;
/// A cell smaller than this stops subdividing. Without it, two nodes at the
/// same point would split forever.
const MIN_CELL: f32 = 1e-3;
const EMPTY: u32 = u32::MAX;

/// Barnes-Hut quadtree over the current positions, stored as parallel arrays
/// and rebuilt every tick.
///
/// Flattened from the octree in `obsidian-3d-graph`'s `physics.rs`: same
/// centre-of-mass accumulation on insert, same `size / distance < theta`
/// opening angle on traversal, two dimensions instead of three.
#[derive(Debug, Default, Clone)]
struct QuadTree {
    com: Vec<[f32; 2]>,
    mass: Vec<f32>,
    /// Cell width, for the opening-angle test.
    size: Vec<f32>,
    center: Vec<[f32; 2]>,
    children: Vec<[u32; 4]>,
    /// The single body in a leaf, or `EMPTY`.
    body: Vec<u32>,
    child_count: Vec<u8>,
    insert_stack: Vec<(u32, u32, [f32; 2])>,
    walk_stack: Vec<u32>,
}

impl QuadTree {
    fn clear(&mut self) {
        self.com.clear();
        self.mass.clear();
        self.size.clear();
        self.center.clear();
        self.children.clear();
        self.body.clear();
        self.child_count.clear();
    }

    fn push_cell(&mut self, center: [f32; 2], size: f32) -> u32 {
        let i = self.mass.len() as u32;
        self.com.push([0.0; 2]);
        self.mass.push(0.0);
        self.size.push(size);
        self.center.push(center);
        self.children.push([EMPTY; 4]);
        self.body.push(EMPTY);
        self.child_count.push(0);
        i
    }

    #[inline]
    fn quadrant(&self, cell: u32, p: [f32; 2]) -> usize {
        let c = self.center[cell as usize];
        usize::from(p[0] >= c[0]) | (usize::from(p[1] >= c[1]) << 1)
    }

    fn child(&mut self, cell: u32, q: usize) -> u32 {
        let i = cell as usize;
        if self.children[i][q] != EMPTY {
            return self.children[i][q];
        }
        let half = self.size[i] * 0.5;
        let c = self.center[i];
        let quarter = half * 0.5;
        let center = [
            if q & 1 != 0 { c[0] + quarter } else { c[0] - quarter },
            if q & 2 != 0 { c[1] + quarter } else { c[1] - quarter },
        ];
        let new = self.push_cell(center, half);
        self.children[i][q] = new;
        self.child_count[i] += 1;
        new
    }

    fn build(&mut self, pos: &[[f32; 2]]) {
        self.clear();
        if pos.is_empty() {
            return;
        }
        let (mut lo, mut hi) = (pos[0], pos[0]);
        for p in &pos[1..] {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        let center = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
        // Square root cell, with slack so a body on the boundary still lands
        // inside it.
        let size = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.0) * 1.01;
        self.push_cell(center, size);
        for (i, &p) in pos.iter().enumerate() {
            self.insert(i as u32, p, pos);
        }
    }

    fn insert(&mut self, body: u32, p: [f32; 2], pos: &[[f32; 2]]) {
        self.insert_stack.clear();
        self.insert_stack.push((0, body, p));
        while let Some((cell, body, p)) = self.insert_stack.pop() {
            let i = cell as usize;
            let m = self.mass[i];
            let next = m + 1.0;
            self.com[i] = [(self.com[i][0] * m + p[0]) / next, (self.com[i][1] * m + p[1]) / next];
            self.mass[i] = next;

            let held = self.body[i];
            if held == EMPTY && self.child_count[i] == 0 {
                self.body[i] = body;
                continue;
            }
            // Two bodies on top of each other: stop splitting and let the mass
            // speak for both. Their mutual force is softened to nothing anyway.
            if self.size[i] < MIN_CELL {
                continue;
            }
            if held != EMPTY {
                self.body[i] = EMPTY;
                let hp = pos[held as usize];
                let q = self.quadrant(cell, hp);
                let c = self.child(cell, q);
                self.insert_stack.push((c, held, hp));
            }
            let q = self.quadrant(cell, p);
            let c = self.child(cell, q);
            self.insert_stack.push((c, body, p));
        }
    }

    /// Repulsion on `body` at `p`, opening a cell only when it subtends more
    /// than `theta`.
    fn repulsion(&mut self, p: [f32; 2], body: u32, strength: f32, theta: f32) -> [f32; 2] {
        let mut acc = [0.0f32; 2];
        if self.mass.is_empty() {
            return acc;
        }
        self.walk_stack.clear();
        self.walk_stack.push(0);
        while let Some(cell) = self.walk_stack.pop() {
            let i = cell as usize;
            let m = self.mass[i];
            if m == 0.0 {
                continue;
            }
            let d = [p[0] - self.com[i][0], p[1] - self.com[i][1]];
            let d2 = (d[0] * d[0] + d[1] * d[1]).max(1.0);
            let dist = d2.sqrt();
            if self.child_count[i] == 0 || (self.size[i] / dist) < theta {
                if self.body[i] != body {
                    let f = strength * m / (d2 * dist);
                    acc[0] += d[0] * f;
                    acc[1] += d[1] * f;
                }
            } else {
                for &c in &self.children[i] {
                    if c != EMPTY {
                        self.walk_stack.push(c);
                    }
                }
            }
        }
        acc
    }
}

/// Force constants. Names follow the settings both references expose, so a
/// later settings panel has somewhere to land.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForceParams {
    /// Pull toward the origin. Quartz's `centerForce`.
    pub center: f32,
    /// All-pairs charge. Quartz's `repelForce`.
    pub repel: f32,
    /// Spring constant on a link. Quartz's `linkForce`.
    pub link: f32,
    /// Rest length of a link.
    pub link_distance: f32,
    /// Velocity retained per tick. Under 1.0 this is what makes the settle
    /// damped instead of a big bang that never stops.
    pub damping: f32,
    pub max_velocity: f32,
    pub dt: f32,
    /// Energy below which the graph is at rest and ticking stops.
    pub alpha_min: f32,
    /// Barnes-Hut opening angle. Smaller is more exact and slower.
    pub theta: f32,
}

impl Default for ForceParams {
    fn default() -> Self {
        Self {
            center: 0.05,
            repel: 400.0,
            link: 0.02,
            link_distance: 30.0,
            damping: 0.85,
            max_velocity: 40.0,
            dt: 0.3,
            alpha_min: 0.001,
            // 0.8 is the reference value and it converges. A coarser angle
            // saves a little tick time and leaves a residual energy that never
            // falls under `alpha_min`, which `the_settle_is_damped_and_ends`
            // catches: a graph that jitters forever is not a faster graph.
            theta: 0.8,
        }
    }
}

/// A ticking 2D layout. Positions start on a deterministic spiral, never on
/// hardcoded coordinates, so the settled shape comes from the forces.
#[derive(Debug, Clone)]
pub struct ForceSim {
    pub params: ForceParams,
    pos: Vec<[f32; 2]>,
    vel: Vec<[f32; 2]>,
    force: Vec<[f32; 2]>,
    edges: Vec<(usize, usize)>,
    /// Nodes the operator is holding. They push on everyone else and are never
    /// pushed themselves, which is d3's `fx`/`fy`.
    pinned: Vec<bool>,
    energy: f32,
    tree: QuadTree,
}

impl ForceSim {
    /// `edges` are index pairs into the node list; out-of-range pairs and
    /// self-loops are dropped, because a snapshot may be filtered after it is
    /// built and a spring on one node is not a force.
    pub fn new(node_count: usize, edges: &[(usize, usize)], params: ForceParams) -> Self {
        let golden = (1.0 + 5.0_f32.sqrt()) / 2.0;
        let scale = params.link_distance * (node_count as f32).sqrt().max(1.0);
        let pos: Vec<[f32; 2]> = (0..node_count)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / golden;
                let r = scale * ((i as f32 + 1.0) / node_count.max(1) as f32).sqrt();
                [r * a.cos(), r * a.sin()]
            })
            .collect();
        let edges =
            edges.iter().copied().filter(|(a, b)| a != b && *a < node_count && *b < node_count).collect();
        Self {
            params,
            vel: vec![[0.0; 2]; node_count],
            force: vec![[0.0; 2]; node_count],
            pinned: vec![false; node_count],
            pos,
            edges,
            energy: 1.0,
            tree: QuadTree::default(),
        }
    }

    /// A sim with nothing in it, for a layout that does not use forces.
    pub fn empty() -> Self {
        Self::new(0, &[], ForceParams::default())
    }

    /// Build straight from an engine snapshot. Node order is the snapshot's, so
    /// `positions()[i]` belongs to `snapshot.nodes[i]`.
    pub fn from_snapshot(snap: &GraphSnapshot, params: ForceParams) -> Self {
        let index: std::collections::HashMap<&str, usize> =
            snap.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let edges: Vec<(usize, usize)> = snap
            .edges
            .iter()
            .filter_map(|e| Some((*index.get(e.src.as_str())?, *index.get(e.dst.as_str())?)))
            .collect();
        Self::new(snap.nodes.len(), &edges, params)
    }

    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    pub fn positions(&self) -> &[[f32; 2]] {
        &self.pos
    }

    /// Mean kinetic energy over the nodes. Falling energy is a settling graph.
    pub fn energy(&self) -> f32 {
        self.energy
    }

    /// Move a node and wake the sim, so the rest of the graph settles around it.
    pub fn place(&mut self, i: usize, x: f32, y: f32) {
        if let Some(p) = self.pos.get_mut(i) {
            *p = [x, y];
            self.vel[i] = [0.0; 2];
            self.wake();
        }
    }

    /// Hold a node still. It keeps pushing on its neighbours; nothing pushes it.
    pub fn set_pinned(&mut self, i: usize, pinned: bool) {
        if let Some(p) = self.pinned.get_mut(i) {
            *p = pinned;
            if !pinned {
                self.wake();
            }
        }
    }

    pub fn is_pinned(&self, i: usize) -> bool {
        self.pinned.get(i).copied().unwrap_or(false)
    }

    pub fn unpin_all(&mut self) {
        self.pinned.iter_mut().for_each(|p| *p = false);
        self.wake();
    }

    /// Put enough energy back to restart ticking after an interaction.
    pub fn wake(&mut self) {
        self.energy = self.energy.max(self.params.alpha_min * 50.0);
    }

    /// True while the sim still has energy to spend, so a caller can tell
    /// whether the next frame is already scheduled.
    pub fn is_running(&self) -> bool {
        !self.pos.is_empty() && self.energy >= self.params.alpha_min
    }

    /// Bounding box of the current positions, for fitting a camera to it.
    pub fn extent(&self) -> ([f32; 2], [f32; 2]) {
        let mut lo = [0.0f32; 2];
        let mut hi = [0.0f32; 2];
        for (k, p) in self.pos.iter().enumerate() {
            if k == 0 {
                lo = *p;
                hi = *p;
            } else {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
        }
        (lo, hi)
    }

    /// One step. Returns false once the graph is at rest, so a render loop can
    /// stop asking.
    pub fn tick(&mut self) -> bool {
        let n = self.pos.len();
        if n == 0 || self.energy < self.params.alpha_min {
            return false;
        }
        self.force.fill([0.0; 2]);

        // charge: every node pushes every other apart, softened near zero so
        // coincident nodes get a finite shove instead of an infinity. Exact
        // while the vault is small, Barnes-Hut once it is not.
        let repel = self.params.repel;
        if n > BARNES_HUT_THRESHOLD {
            self.tree.build(&self.pos);
            let theta = self.params.theta;
            for i in 0..n {
                let f = self.tree.repulsion(self.pos[i], i as u32, repel, theta);
                self.force[i][0] += f[0];
                self.force[i][1] += f[1];
            }
        } else {
            for i in 0..n {
                let pi = self.pos[i];
                let mut fi = [0.0f32; 2];
                for j in (i + 1)..n {
                    let dx = pi[0] - self.pos[j][0];
                    let dy = pi[1] - self.pos[j][1];
                    let d2 = (dx * dx + dy * dy).max(1.0);
                    let k = repel / (d2 * d2.sqrt());
                    fi[0] += dx * k;
                    fi[1] += dy * k;
                    self.force[j][0] -= dx * k;
                    self.force[j][1] -= dy * k;
                }
                self.force[i][0] += fi[0];
                self.force[i][1] += fi[1];
            }
        }

        // link: a spring toward the rest length, pulling when long and pushing
        // when short.
        let (k, rest) = (self.params.link, self.params.link_distance);
        for &(a, b) in &self.edges {
            let dx = self.pos[b][0] - self.pos[a][0];
            let dy = self.pos[b][1] - self.pos[a][1];
            let d = (dx * dx + dy * dy).sqrt().max(0.1);
            let f = k * (d - rest) / d;
            self.force[a][0] += dx * f;
            self.force[a][1] += dy * f;
            self.force[b][0] -= dx * f;
            self.force[b][1] -= dy * f;
        }

        // center: gravity toward the origin, which is what keeps disconnected
        // components on screen instead of drifting out of the void.
        let g = self.params.center;
        for (f, p) in self.force.iter_mut().zip(self.pos.iter()) {
            f[0] -= p[0] * g;
            f[1] -= p[1] * g;
        }

        let (dt, damping, max_v) = (self.params.dt, self.params.damping, self.params.max_velocity);
        let max_v2 = max_v * max_v;
        let mut total = 0.0f32;
        for (((p, v), f), held) in
            self.pos.iter_mut().zip(self.vel.iter_mut()).zip(self.force.iter()).zip(self.pinned.iter())
        {
            if *held {
                *v = [0.0; 2];
                continue;
            }
            let mut vx = (v[0] + f[0] * dt) * damping;
            let mut vy = (v[1] + f[1] * dt) * damping;
            let v2 = vx * vx + vy * vy;
            if v2 > max_v2 {
                let s = max_v / v2.sqrt();
                vx *= s;
                vy *= s;
            }
            total += vx * vx + vy * vy;
            *v = [vx, vy];
            p[0] += vx * dt;
            p[1] += vy * dt;
        }
        self.energy = total / n as f32;
        true
    }

    /// Tick until the graph is at rest or `max_ticks` is spent. Returns the
    /// number of ticks actually run.
    pub fn settle(&mut self, max_ticks: usize) -> usize {
        let mut run = 0;
        while run < max_ticks && self.tick() {
            run += 1;
        }
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture with a hub, a chain and a disconnected pair, so the sim has
    /// both springs and free bodies to settle.
    fn fixture() -> ForceSim {
        let edges = [(0, 1), (0, 2), (0, 3), (1, 2), (3, 4), (4, 5), (6, 7)];
        ForceSim::new(9, &edges, ForceParams::default())
    }

    #[test]
    fn positions_move_then_energy_falls() {
        let mut sim = fixture();
        let start = sim.positions().to_vec();

        assert!(sim.tick(), "a fresh graph is not at rest");
        let after_one = sim.positions().to_vec();
        let moved = start.iter().zip(&after_one).filter(|(a, b)| a != b).count();
        assert_eq!(moved, start.len(), "every node moves on the first tick");

        // Energy climbs while repulsion unwinds the seed spiral, so the early
        // reading is taken after the graph has opened up, not at t=0.
        for _ in 0..9 {
            sim.tick();
        }
        let early = sim.energy();
        assert!(early > 0.0 && early.is_finite());

        let ran = sim.settle(4_000);
        let late = sim.energy();
        assert!(late < early, "energy falls: {early} -> {late} over {ran} ticks");
        assert!(late < sim.params.alpha_min, "the settle is damped, not endless jitter");
        assert!(!sim.tick(), "a graph at rest stops asking to be ticked");
        assert!(
            sim.positions().iter().all(|p| p[0].is_finite() && p[1].is_finite()),
            "no NaN escapes the integrator"
        );
    }

    #[test]
    fn seed_layout_is_derived_not_hardcoded() {
        let a = ForceSim::new(9, &[], ForceParams::default());
        let wide = ForceSim::new(9, &[], ForceParams { link_distance: 300.0, ..ForceParams::default() });
        assert_ne!(a.positions()[3], wide.positions()[3], "the spiral scales with link distance");
        assert!(a.positions().iter().all(|p| p[0].is_finite()));
        let mut ids: Vec<String> = a.positions().iter().map(|p| format!("{p:?}")).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 9, "no two nodes start on top of each other");
    }

    /// The link force has to actually pull, or the layout is just repulsion in a
    /// gravity well and the edges mean nothing. Measured on a crowd, because on
    /// three nodes gravity alone already sets the spacing near the rest length.
    #[test]
    fn linked_neighbours_settle_closer_than_the_crowd() {
        let chain: Vec<(usize, usize)> = (0..9).map(|i| (i, i + 1)).collect();
        let mut sim = ForceSim::new(40, &chain, ForceParams::default());
        sim.settle(6_000);
        let p = sim.positions();
        let d = |a: [f32; 2], b: [f32; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();

        let linked: f32 = chain.iter().map(|&(a, b)| d(p[a], p[b])).sum::<f32>() / chain.len() as f32;
        let mut all = Vec::new();
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                all.push(d(p[i], p[j]));
            }
        }
        let crowd: f32 = all.iter().sum::<f32>() / all.len() as f32;
        assert!(linked < crowd, "linked mean {linked} should sit under crowd mean {crowd}");
    }

    #[test]
    fn no_javascript_graph_library_is_a_dependency() {
        for toml in [include_str!("../Cargo.toml"), include_str!("../../../Cargo.toml")] {
            let lower = toml.to_lowercase();
            for banned in ["pixi", "cytoscape", "d3-force", "juggl", "electron", "webview"] {
                assert!(!lower.contains(banned), "`{banned}` must not be a dependency");
            }
        }
    }

    /// G3d: the settle is damped, and it stops. A graph that never falls under
    /// `alpha_min` is the "big bang then jitter forever" the visual bar rules out.
    #[test]
    fn the_settle_is_damped_and_ends() {
        for n in [9usize, 200] {
            let edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
            let mut sim = ForceSim::new(n, &edges, ForceParams::default());
            let ticks = sim.settle(20_000);
            assert!(ticks < 20_000, "{n} nodes never came to rest");
            assert!(sim.energy() < sim.params.alpha_min, "{n} nodes still jittering");

            // At rest, another thousand ticks move nothing.
            let before = sim.positions().to_vec();
            assert_eq!(sim.settle(1_000), 0, "a graph at rest does not ask to tick");
            assert_eq!(sim.positions(), before.as_slice());

            // And an interaction wakes it again rather than leaving it dead.
            sim.place(0, 900.0, 900.0);
            assert!(sim.tick(), "moving a node wakes the sim");
            assert!(sim.settle(20_000) < 20_000, "and it settles again");
        }
    }

    /// The tree is an optimisation, not different physics. `theta = 0` never
    /// satisfies the opening test, so it walks to the leaves and is the exact
    /// sum through the same code path.
    #[test]
    fn barnes_hut_agrees_with_the_exact_sum() {
        let n = BARNES_HUT_THRESHOLD * 2;
        let edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        let exact_params = ForceParams { theta: 0.0, ..ForceParams::default() };
        let mut tree_sim = ForceSim::new(n, &edges, ForceParams::default());
        let mut exact_sim = ForceSim::new(n, &edges, exact_params);
        assert_eq!(tree_sim.positions(), exact_sim.positions(), "same start");

        // One tick isolates the force error from the chaos a long run adds.
        tree_sim.tick();
        exact_sim.tick();
        let worst = tree_sim
            .positions()
            .iter()
            .zip(exact_sim.positions())
            .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.25, "one tick of tree repulsion drifted {worst} units");

        // And over a full settle it lands on the same shape, judged by the
        // measures that describe a layout rather than by node coordinates,
        // which a force sim is free to permute.
        tree_sim.settle(3_000);
        exact_sim.settle(3_000);
        let mean_link = |s: &ForceSim| {
            let p = s.positions();
            edges
                .iter()
                .map(|&(a, b)| ((p[a][0] - p[b][0]).powi(2) + (p[a][1] - p[b][1]).powi(2)).sqrt())
                .sum::<f32>()
                / edges.len() as f32
        };
        let span = |s: &ForceSim| {
            let (lo, hi) = s.extent();
            (hi[0] - lo[0]).max(hi[1] - lo[1])
        };
        let (lt, le) = (mean_link(&tree_sim), mean_link(&exact_sim));
        let (st, se) = (span(&tree_sim), span(&exact_sim));
        assert!((lt - le).abs() / le < 0.2, "mean link {lt} vs {le}");
        assert!((st - se).abs() / se < 0.3, "span {st} vs {se}");
        assert!(tree_sim.positions().iter().all(|p| p[0].is_finite() && p[1].is_finite()));
    }

    /// A deterministic 2 000-node / 4 000-edge graph, the size the perf bar names.
    fn big(n: usize, m: usize) -> ForceSim {
        // A cheap reproducible spread: a spanning chain so nothing is isolated,
        // then extra chords from a linear congruential sequence.
        let mut edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        let mut r: u64 = 0x2545_F491_4F6C_DD1D;
        while edges.len() < m {
            r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let a = (r >> 33) as usize % n;
            r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let b = (r >> 33) as usize % n;
            if a != b {
                edges.push((a, b));
            }
        }
        ForceSim::new(n, &edges, ForceParams::default())
    }

    /// G3c's budget, measured rather than felt. Timing depends on the machine,
    /// so it is `--ignored`: run it with `--release --ignored` to get a number,
    /// and read the on-screen overlay for the number that actually counts.
    #[test]
    #[ignore = "timing; run with --release -- --ignored --nocapture"]
    fn two_thousand_nodes_tick_inside_a_frame() {
        let mut sim = big(2_000, 4_000);
        // warm the tree and get past the opening burst
        for _ in 0..20 {
            sim.tick();
        }
        let runs = 120;
        let t0 = std::time::Instant::now();
        for _ in 0..runs {
            sim.tick();
        }
        let per_tick = t0.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        println!("2000 nodes / 4000 edges: {per_tick:.3} ms per tick");
        assert!(per_tick < 16.6, "{per_tick:.3} ms per tick misses the 60 fps budget");
    }

    #[test]
    fn a_pinned_node_holds_still_and_the_rest_moves_around_it() {
        let mut sim = fixture();
        sim.settle(4_000);
        sim.place(0, 120.0, -60.0);
        sim.set_pinned(0, true);
        assert!(sim.is_pinned(0));
        let others = sim.positions().to_vec();
        sim.settle(4_000);
        assert_eq!(sim.positions()[0], [120.0, -60.0], "the held node does not drift");
        assert!(
            sim.positions().iter().skip(1).zip(others.iter().skip(1)).any(|(a, b)| a != b),
            "everything else settles around it"
        );
        sim.unpin_all();
        assert!(!sim.is_pinned(0));
        sim.settle(4_000);
        assert_ne!(sim.positions()[0], [120.0, -60.0], "released, it rejoins the layout");
    }
}
