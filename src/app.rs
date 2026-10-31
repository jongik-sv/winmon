//! Rolling history of samples that the UI draws graphs from.

use std::collections::VecDeque;

use crate::metrics::Snapshot;
use crate::topology::Topology;

/// Enough for a very wide terminal; older samples scroll off.
const HISTORY: usize = 1024;

#[derive(Default)]
pub struct Series {
    values: VecDeque<f64>,
    /// A real reading has arrived at least once; series never seen are hidden (e.g. no NPU).
    pub seen: bool,
}

impl Series {
    pub fn push(&mut self, v: f64) {
        self.seen = true;
        self.push_idle(v);
    }

    /// Appends without marking the series as present on this machine.
    pub fn push_idle(&mut self, v: f64) {
        if self.values.len() == HISTORY {
            self.values.pop_front();
        }
        self.values.push_back(v);
    }

    /// The newest `width` values, left-padded with zeros so the graph always ends at the right edge.
    pub fn tail(&self, width: usize, scale: f64) -> Vec<u64> {
        let take = self.values.len().min(width);
        let mut out = vec![0u64; width - take];
        out.extend(self.values.iter().skip(self.values.len() - take).map(|v| (v * scale).round().max(0.0) as u64));
        out
    }

    /// Largest of the newest `width` values (for auto-scaled graphs such as watts).
    pub fn max_tail(&self, width: usize) -> f64 {
        self.values.iter().rev().take(width).copied().fold(0.0, f64::max)
    }
}

pub struct App {
    pub topo: Topology,
    pub interval_ms: u64,
    pub last: Snapshot,
    pub cores: Vec<Series>,
    pub package_w: Series,
    pub cores_w: Series,
    pub gpu_w: Series,
    pub dram_w: Series,
    pub gpu_3d: Series,
    pub compute: Series,
    pub video: Series,
    pub npu: Series,
}

impl App {
    pub fn new(topo: Topology, interval_ms: u64) -> App {
        let cores = topo.cpus.iter().map(|_| Series::default()).collect();
        App {
            topo,
            interval_ms,
            last: Snapshot::default(),
            cores,
            package_w: Series::default(),
            cores_w: Series::default(),
            gpu_w: Series::default(),
            dram_w: Series::default(),
            gpu_3d: Series::default(),
            compute: Series::default(),
            video: Series::default(),
            npu: Series::default(),
        }
    }

    pub fn push(&mut self, snap: Snapshot) {
        for (series, core) in self.cores.iter_mut().zip(&snap.cores) {
            series.push(core.util);
        }
        let p = snap.power;
        for (series, v) in [
            (&mut self.package_w, p.package),
            (&mut self.cores_w, p.cores),
            (&mut self.gpu_w, p.gpu),
            (&mut self.dram_w, p.dram),
        ] {
            if let Some(v) = v {
                series.push(v);
            }
        }
        let e = snap.engines;
        for (series, v) in [
            (&mut self.gpu_3d, e.gpu_3d),
            (&mut self.compute, e.compute),
            (&mut self.video, e.video),
            (&mut self.npu, e.npu),
        ] {
            // Engine instances come and go with the processes using them; a gap means idle.
            match v {
                Some(v) => series.push(v),
                None => series.push_idle(0.0),
            }
        }
        self.last = snap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_left_pads_so_newest_is_at_the_right_edge() {
        let mut s = Series::default();
        s.push(10.0);
        s.push(20.5);
        assert_eq!(s.tail(4, 10.0), vec![0, 0, 100, 205]);
    }

    #[test]
    fn tail_keeps_only_the_newest_values() {
        let mut s = Series::default();
        for v in [1.0, 2.0, 3.0, 4.0] {
            s.push(v);
        }
        assert_eq!(s.tail(2, 1.0), vec![3, 4]);
        assert_eq!(s.max_tail(2), 4.0);
    }

    #[test]
    fn idle_pushes_do_not_mark_a_series_as_present() {
        let mut s = Series::default();
        s.push_idle(0.0);
        assert!(!s.seen);
        s.push(5.0);
        assert!(s.seen);
    }
}
