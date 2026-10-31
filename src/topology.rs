//! CPU topology: which logical processors are P-cores / E-cores, and the CPU's marketing name.

use std::collections::BTreeSet;

use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows_sys::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, RelationProcessorCore,
};

/// Kind of core a logical processor belongs to, derived from its EfficiencyClass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreKind {
    /// Single-class CPU (no hybrid architecture).
    Uniform,
    Performance,
    Efficient,
    LowPowerEfficient,
}

impl CoreKind {
    pub fn prefix(self) -> &'static str {
        match self {
            CoreKind::Uniform => "C",
            CoreKind::Performance => "P",
            CoreKind::Efficient => "E",
            CoreKind::LowPowerEfficient => "LP",
        }
    }
}

/// One logical processor, as PDH names it (`group,number`).
#[derive(Clone, Debug)]
pub struct LogicalCpu {
    pub group: u16,
    pub number: u8,
    pub kind: CoreKind,
    /// Display label, e.g. `P0`, `E3`.
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct Topology {
    pub cpu_name: String,
    /// Sorted by (group, number) — the same order PDH instances are matched against.
    pub cpus: Vec<LogicalCpu>,
}

impl Topology {
    pub fn detect() -> Topology {
        let mut raw = read_cores();
        raw.sort_by_key(|c| (c.group, c.number));

        let classes: BTreeSet<u8> = raw.iter().map(|c| c.class).collect();
        let max = classes.iter().next_back().copied().unwrap_or(0);
        let min = classes.iter().next().copied().unwrap_or(0);
        let kind_of = |class: u8| match classes.len() {
            0 | 1 => CoreKind::Uniform,
            2 if class == max => CoreKind::Performance,
            2 => CoreKind::Efficient,
            _ if class == max => CoreKind::Performance,
            _ if class == min => CoreKind::LowPowerEfficient,
            _ => CoreKind::Efficient,
        };

        // P-cores first, then E, then LP — the order people read a hybrid CPU in.
        let mut cpus: Vec<LogicalCpu> = raw
            .iter()
            .map(|c| LogicalCpu { group: c.group, number: c.number, kind: kind_of(c.class), label: String::new() })
            .collect();
        cpus.sort_by_key(|c| (rank(c.kind), c.group, c.number));
        let mut counters = [0usize; 4];
        for cpu in &mut cpus {
            let n = &mut counters[rank(cpu.kind)];
            cpu.label = format!("{}{}", cpu.kind.prefix(), n);
            *n += 1;
        }

        Topology { cpu_name: cpu_name().unwrap_or_else(|| "Unknown CPU".into()), cpus }
    }

    pub fn count(&self, kind: CoreKind) -> usize {
        self.cpus.iter().filter(|c| c.kind == kind).count()
    }

    /// Index into `cpus` for the PDH instance name `group,number`.
    pub fn index_of(&self, instance: &str) -> Option<usize> {
        let (g, n) = instance.split_once(',')?;
        let (g, n): (u16, u8) = (g.trim().parse().ok()?, n.trim().parse().ok()?);
        self.cpus.iter().position(|c| c.group == g && c.number == n)
    }
}

fn rank(kind: CoreKind) -> usize {
    match kind {
        CoreKind::Performance => 0,
        CoreKind::Uniform => 1,
        CoreKind::Efficient => 2,
        CoreKind::LowPowerEfficient => 3,
    }
}

struct RawCpu {
    group: u16,
    number: u8,
    class: u8,
}

/// Walks SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX records for RelationProcessorCore.
/// Parsed by offset because the record is variable-length (GroupCount masks follow the header).
fn read_cores() -> Vec<RawCpu> {
    let mut len: u32 = 0;
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len) };
    if len == 0 {
        return fallback_cores();
    }
    // u64 storage keeps the buffer 8-byte aligned for the masks.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let ok = unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, buf.as_mut_ptr().cast(), &mut len) };
    if ok == 0 {
        return fallback_cores();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), len as usize) };

    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 32 <= bytes.len() {
        let rec = &bytes[off..];
        let size = u32::from_le_bytes(rec[4..8].try_into().unwrap()) as usize;
        if size == 0 {
            break;
        }
        let class = rec[9];
        let group_count = u16::from_le_bytes(rec[30..32].try_into().unwrap()) as usize;
        for g in 0..group_count {
            let at = 32 + g * 16;
            if at + 10 > size {
                break;
            }
            let mask = u64::from_le_bytes(rec[at..at + 8].try_into().unwrap());
            let group = u16::from_le_bytes(rec[at + 8..at + 10].try_into().unwrap());
            for bit in 0..64u8 {
                if mask & (1u64 << bit) != 0 {
                    out.push(RawCpu { group, number: bit, class });
                }
            }
        }
        off += size;
    }
    if out.is_empty() {
        fallback_cores()
    } else {
        out
    }
}

fn fallback_cores() -> Vec<RawCpu> {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    (0..n).map(|i| RawCpu { group: (i / 64) as u16, number: (i % 64) as u8, class: 0 }).collect()
}

fn cpu_name() -> Option<String> {
    let key = wide(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
    let value = wide("ProcessorNameString");
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..end]).replace("(R)", "").replace("(TM)", "");
    // Some vendors pad the string; collapse runs of spaces.
    Some(name.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(group: u16, number: u8) -> LogicalCpu {
        LogicalCpu { group, number, kind: CoreKind::Uniform, label: String::new() }
    }

    #[test]
    fn index_of_matches_pdh_instance_names() {
        let topo = Topology { cpu_name: String::new(), cpus: vec![cpu(0, 4), cpu(0, 0), cpu(1, 0)] };
        assert_eq!(topo.index_of("0,0"), Some(1));
        assert_eq!(topo.index_of("1,0"), Some(2));
        assert_eq!(topo.index_of("0,_Total"), None);
        assert_eq!(topo.index_of("_Total"), None);
    }
}
