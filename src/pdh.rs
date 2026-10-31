//! Thin safe wrapper over the Performance Data Helper (PDH) API.
//!
//! Counters are added by their English path (`PdhAddEnglishCounterW`) so the same paths work on
//! localized Windows (e.g. Korean, where the display names are translated).

use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
};

use crate::topology::wide;

/// Not exported by windows-sys. Lets `% Processor Utility` report its real value above 100.
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
const PDH_CSTATUS_NEW_DATA: u32 = 1;

pub struct Query {
    handle: isize,
}

#[derive(Clone, Copy)]
pub struct Counter(isize);

impl Query {
    pub fn open() -> Option<Query> {
        let mut handle: isize = 0;
        let status = unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut handle) };
        (status == 0).then_some(Query { handle })
    }

    /// Adds a (usually wildcard) counter. `None` when the counter set does not exist on this machine,
    /// e.g. no `Energy Meter` on hardware without RAPL/EMI support.
    pub fn add(&mut self, path: &str) -> Option<Counter> {
        let path = wide(path);
        let mut counter: isize = 0;
        let status = unsafe { PdhAddEnglishCounterW(self.handle, path.as_ptr(), 0, &mut counter) };
        (status == 0).then_some(Counter(counter))
    }

    pub fn collect(&mut self) -> bool {
        unsafe { PdhCollectQueryData(self.handle) == 0 }
    }

    /// Current value of every instance of `counter`. Instances whose value is not valid yet
    /// (rate counters need two collections) are skipped.
    pub fn values(&self, counter: Counter) -> Vec<(String, f64)> {
        let format = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
        let mut size: u32 = 0;
        let mut count: u32 = 0;
        let status = unsafe {
            PdhGetFormattedCounterArrayW(counter.0, format, &mut size, &mut count, std::ptr::null_mut())
        };
        if status != PDH_MORE_DATA || size == 0 {
            return Vec::new();
        }

        // u64 storage keeps the item array 8-byte aligned.
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let status = unsafe { PdhGetFormattedCounterArrayW(counter.0, format, &mut size, &mut count, items) };
        if status != 0 {
            return Vec::new();
        }

        let items = unsafe { std::slice::from_raw_parts(items, count as usize) };
        items
            .iter()
            .filter(|it| matches!(it.FmtValue.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA))
            .map(|it| {
                let name = unsafe { from_wide_ptr(it.szName) };
                let value = unsafe { it.FmtValue.Anonymous.doubleValue };
                (name, value)
            })
            .collect()
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.handle) };
    }
}

unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}
