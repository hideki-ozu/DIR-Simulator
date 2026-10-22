//! Stable physical ports (including parallel size-two torus ports) and escape tree.
use crate::types::noc_flit::{TopologyConfig, TopologyKind};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub source: usize,
    pub destination: usize,
    pub port: usize,
    pub reverse: usize,
}
#[derive(Clone, Debug)]
pub struct Topology {
    pub nodes: usize,
    pub links: Vec<Link>,
    pub outgoing: Vec<Vec<usize>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routing {
    pub distances: Vec<Vec<Option<usize>>>,
    pub parent: Vec<Option<usize>>,
    pub tree_links: Vec<bool>,
}
impl Topology {
    pub fn new(c: &TopologyConfig) -> Self {
        let n = c.width * c.height;
        let mut links = Vec::new();
        let mut outgoing = vec![Vec::new(); n];
        for (r, ports) in outgoing.iter_mut().enumerate() {
            let (x, y) = (r % c.width, r / c.width);
            for port in 0..4 {
                let xy = match port {
                    0 if x + 1 < c.width => Some((x + 1, y)),
                    1 if x > 0 => Some((x - 1, y)),
                    2 if y + 1 < c.height => Some((x, y + 1)),
                    3 if y > 0 => Some((x, y - 1)),
                    _ if c.kind == TopologyKind::Torus => Some(match port {
                        0 => (0, y),
                        1 => (c.width - 1, y),
                        2 => (x, 0),
                        _ => (x, c.height - 1),
                    }),
                    _ => None,
                };
                if let Some((a, b)) = xy {
                    ports.push(links.len());
                    links.push(Link {
                        source: r,
                        destination: b * c.width + a,
                        port,
                        reverse: 0,
                    });
                }
            }
        }
        for i in 0..links.len() {
            let a = &links[i];
            let opposite = a.port ^ 1;
            let reverse = outgoing[a.destination]
                .iter()
                .copied()
                .find(|j| links[*j].port == opposite)
                .expect("paired topology");
            links[i].reverse = reverse;
        }
        Self {
            nodes: n,
            links,
            outgoing,
        }
    }
    pub fn routing(&self, healthy: &[bool]) -> Routing {
        let usable = |l: usize| healthy[l] && healthy[self.links[l].reverse];
        let mut distances = vec![vec![None; self.nodes]; self.nodes];
        for (dst, d) in distances.iter_mut().enumerate() {
            let mut q = VecDeque::from([dst]);
            d[dst] = Some(0);
            while let Some(r) = q.pop_front() {
                for &l in &self.outgoing[r] {
                    let next = self.links[l].destination;
                    if usable(l) && d[next].is_none() {
                        d[next] = Some(d[r].unwrap() + 1);
                        q.push_back(next);
                    }
                }
            }
        }
        let mut parent = vec![None; self.nodes];
        let mut seen = vec![false; self.nodes];
        let mut tree_links = vec![false; self.links.len()];
        for root in 0..self.nodes {
            if seen[root] {
                continue;
            }
            seen[root] = true;
            let mut q = VecDeque::from([root]);
            while let Some(r) = q.pop_front() {
                for &l in &self.outgoing[r] {
                    let next = self.links[l].destination;
                    if usable(l) && !seen[next] {
                        seen[next] = true;
                        parent[next] = Some(self.links[l].reverse);
                        tree_links[l] = true;
                        tree_links[self.links[l].reverse] = true;
                        q.push_back(next);
                    }
                }
            }
        }
        Routing {
            distances,
            parent,
            tree_links,
        }
    }
    /// Unique tree path, including up then down and excluding all non-tree links.
    pub fn escape_next(&self, routing: &Routing, here: usize, dst: usize) -> Option<usize> {
        if here == dst || routing.distances[dst][here].is_none() {
            return None;
        }
        let mut q = VecDeque::from([here]);
        let mut previous = vec![None; self.nodes];
        previous[here] = Some(usize::MAX);
        while let Some(r) = q.pop_front() {
            for &l in &self.outgoing[r] {
                let next = self.links[l].destination;
                if routing.tree_links[l] && previous[next].is_none() {
                    previous[next] = Some(l);
                    q.push_back(next);
                }
            }
        }
        let mut at = dst;
        loop {
            let l = previous[at]?;
            if self.links[l].source == here {
                return Some(l);
            }
            at = self.links[l].source;
        }
    }
}
