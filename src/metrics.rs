//! Collects one snapshot of every metric per tick.

use std::collections::HashMap;

use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use crate::pdh::{Counter, Query};
use crate::topology::Topology;

#[derive(Clone, Copy, Default)]
pub struct CoreSample {
    /// % Processor Utility, clamped to 0..=100 (it can exceed 100 while turbo boosting).
    pub util: f64,
    /// Effective clock in MHz (nominal frequency × % Processor Performance).
    pub mhz: f64,
}

/// Power rails from the `Energy Meter` counter set (Intel RAPL via the Energy Meter Interface).
#[derive(Clone, Copy, Default)]
pub struct Power {
    pub package: Option<f64>,
    pub cores: Option<f64>,
    pub gpu: Option<f64>,
    pub dram: Option<f64>,
}

#[derive(Clone, Copy, Default)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    pub commit_used: u64,
    pub commit_limit: u64,
    pub pagefile_pct: Option<f64>,
}

/// GPU/NPU engine utilisation in percent, the same aggregation Task Manager uses.
#[derive(Clone, Copy, Default)]
pub struct Engines {
    pub gpu_3d: Option<f64>,
    pub compute: Option<f64>,
    pub video: Option<f64>,
    pub npu: Option<f64>,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    /// Same order as `Topology::cpus`.
    pub cores: Vec<CoreSample>,
    pub power: Power,
    pub engines: Engines,
    pub memory: Memory,
    /// (zone name, °C), hottest first.
    pub temps: Vec<(String, f64)>,
}

pub struct Sampler {
    query: Query,
    utility: Option<Counter>,
    perf: Option<Counter>,
    freq: Option<Counter>,
    energy: Option<Counter>,
    gpu: Option<Counter>,
    temp_hp: Option<Counter>,
    temp: Option<Counter>,
    pagefile: Option<Counter>,
}

impl Sampler {
    pub fn new() -> Option<Sampler> {
        let mut query = Query::open()?;
        let utility = query.add(r"\Processor Information(*)\% Processor Utility");
        let perf = query.add(r"\Processor Information(*)\% Processor Performance");
        let freq = query.add(r"\Processor Information(*)\Processor Frequency");
        let energy = query.add(r"\Energy Meter(*)\Power");
        let gpu = query.add(r"\GPU Engine(*)\Utilization Percentage");
        let temp_hp = query.add(r"\Thermal Zone Information(*)\High Precision Temperature");
        let temp = query.add(r"\Thermal Zone Information(*)\Temperature");
        let pagefile = query.add(r"\Paging File(_Total)\% Usage");
        let mut sampler = Sampler { query, utility, perf, freq, energy, gpu, temp_hp, temp, pagefile };
        // Rate counters need a baseline collection before they report valid data.
        sampler.query.collect();
        Some(sampler)
    }

    pub fn sample(&mut self, topo: &Topology) -> Snapshot {
        self.query.collect();
        Snapshot {
            cores: self.cores(topo),
            power: self.power(),
            engines: self.engines(),
            memory: self.memory(),
            temps: self.temps(),
        }
    }

    fn per_cpu(&self, counter: Option<Counter>, topo: &Topology) -> Vec<Option<f64>> {
        let mut out = vec![None; topo.cpus.len()];
        if let Some(c) = counter {
            for (name, v) in self.query.values(c) {
                if let Some(i) = topo.index_of(&name) {
                    out[i] = Some(v);
                }
            }
        }
        out
    }

    fn cores(&self, topo: &Topology) -> Vec<CoreSample> {
        let util = self.per_cpu(self.utility, topo);
        let perf = self.per_cpu(self.perf, topo);
        let freq = self.per_cpu(self.freq, topo);
        (0..topo.cpus.len())
            .map(|i| CoreSample {
                util: util[i].unwrap_or(0.0).clamp(0.0, 100.0),
                mhz: match (freq[i], perf[i]) {
                    (Some(f), Some(p)) => f * p / 100.0,
                    (Some(f), None) => f,
                    _ => 0.0,
                },
            })
            .collect()
    }

    fn power(&self) -> Power {
        let mut p = Power::default();
        let Some(c) = self.energy else { return p };
        for (name, mw) in self.query.values(c) {
            let w = Some(mw / 1000.0);
            let name = name.to_ascii_lowercase();
            if name.ends_with("_pkg") {
                p.package = w;
            } else if name.ends_with("_pp0") {
                p.cores = w;
            } else if name.ends_with("_pp1") {
                p.gpu = w;
            } else if name.ends_with("_dram") {
                p.dram = w;
            }
        }
        p
    }

    /// Instances look like `pid_1234_luid_0x0_0xD1F2_phys_0_eng_3_engtype_3D`. Utilisation is
    /// summed per physical engine across processes, then the busiest engine of each type is shown.
    fn engines(&self) -> Engines {
        let mut e = Engines::default();
        let Some(c) = self.gpu else { return e };
        let mut per_engine: HashMap<String, (String, f64)> = HashMap::new();
        for (name, v) in self.query.values(c) {
            let name = name.to_ascii_lowercase();
            let Some((head, engtype)) = name.rsplit_once("_engtype_") else { continue };
            let engine = head.split_once("_luid_").map_or(head, |(_, rest)| rest).to_string();
            per_engine.entry(engine).or_insert_with(|| (engtype.to_string(), 0.0)).1 += v;
        }
        let bump = |slot: &mut Option<f64>, v: f64| *slot = Some(slot.unwrap_or(0.0).max(v.min(100.0)));
        for (engtype, v) in per_engine.into_values() {
            match engtype.as_str() {
                "3d" => bump(&mut e.gpu_3d, v),
                "compute" => bump(&mut e.compute, v),
                t if t.starts_with("video") => bump(&mut e.video, v),
                "neural" => bump(&mut e.npu, v),
                _ => {}
            }
        }
        e
    }

    fn memory(&self) -> Memory {
        let mut st: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let mut m = Memory::default();
        if unsafe { GlobalMemoryStatusEx(&mut st) } != 0 {
            m.total = st.ullTotalPhys;
            m.used = st.ullTotalPhys - st.ullAvailPhys;
            m.commit_limit = st.ullTotalPageFile;
            m.commit_used = st.ullTotalPageFile - st.ullAvailPageFile;
        }
        m.pagefile_pct = self.pagefile.and_then(|c| self.query.values(c).first().map(|v| v.1));
        m
    }

    /// ACPI thermal zones. These are firmware-defined sensors (CPU, skin, charger…), not per-core.
    fn temps(&self) -> Vec<(String, f64)> {
        let (counter, scale) = match (self.temp_hp, self.temp) {
            (Some(c), _) if !self.query.values(c).is_empty() => (c, 10.0),
            (_, Some(c)) => (c, 1.0),
            _ => return Vec::new(),
        };
        let mut temps: Vec<(String, f64)> = self
            .query
            .values(counter)
            .into_iter()
            .map(|(name, k)| (zone_name(&name), k / scale - 273.15))
            // Unpopulated zones report ~0 K or ~273 K (0 °C); no real sensor reads that low indoors.
            .filter(|(_, c)| *c > 5.0 && *c < 150.0)
            .collect();
        temps.sort_by(|a, b| b.1.total_cmp(&a.1));
        temps
    }
}

/// `\_TZ.CPUZ` → `CPUZ`.
fn zone_name(raw: &str) -> String {
    raw.rsplit(['.', '\\']).next().unwrap_or(raw).to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::zone_name;

    #[test]
    fn zone_name_strips_the_acpi_prefix() {
        assert_eq!(zone_name(r"\_tz.cpuz"), "CPUZ");
        assert_eq!(zone_name("TZ00"), "TZ00");
    }
}
