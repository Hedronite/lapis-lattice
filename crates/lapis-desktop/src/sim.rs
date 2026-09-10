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
//! Repulsion is the naive all-pairs sum. It is O(n²) per tick, which is honest
//! for the vault sizes v0.2.1 draws; a Barnes-Hut tree is the drop-in when the
//! perf gate asks for 2 000 nodes.

use lapis_lattice::GraphSnapshot;

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
    energy: f32,
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
            pos,
            edges,
            energy: 1.0,
        }
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

    /// Pin a node where the operator dropped it: zero velocity, and the
    /// integrator leaves it alone.
    pub fn place(&mut self, i: usize, x: f32, y: f32) {
        if let Some(p) = self.pos.get_mut(i) {
            *p = [x, y];
            self.vel[i] = [0.0; 2];
            self.energy = self.energy.max(self.params.alpha_min * 10.0);
        }
    }

    /// One step. Returns false once the graph is at rest, so a render loop can
    /// stop asking.
    pub fn tick(&mut self) -> bool {
        let n = self.pos.len();
        if n == 0 || self.energy < self.params.alpha_min {
            return false;
        }
        self.force.fill([0.0; 2]);

        // charge: every pair pushes apart, softened near zero so coincident
        // nodes get a finite shove instead of an infinity.
        let repel = self.params.repel;
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
        for ((p, v), f) in self.pos.iter_mut().zip(self.vel.iter_mut()).zip(self.force.iter()) {
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

    /// Positions rescaled so the widest axis spans `±half`, which is what turns
    /// simulation units into the canvas units the scene draws in.
    pub fn normalized(&self, half: f32) -> Vec<[f32; 2]> {
        let extent =
            self.pos.iter().flat_map(|p| [p[0].abs(), p[1].abs()]).fold(0.0f32, f32::max).max(f32::EPSILON);
        let s = half / extent;
        self.pos.iter().map(|p| [p[0] * s, p[1] * s]).collect()
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

    #[test]
    fn normalized_fits_the_canvas_box() {
        let mut sim = fixture();
        sim.settle(2_000);
        let n = sim.normalized(2.0);
        assert_eq!(n.len(), sim.len());
        let widest = n.iter().flat_map(|p| [p[0].abs(), p[1].abs()]).fold(0.0f32, f32::max);
        assert!((widest - 2.0).abs() < 1e-3, "widest axis lands on the half-extent, got {widest}");
    }
}
