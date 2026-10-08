//! Windows: how busy the graphics card is, for JBD's title-bar GPU meter. Ported from the first
//! JBD (`src-tauri/src/system.rs`).
//!
//! It reads Windows' own performance counters, the ones Task Manager's GPU graphs use, so it works
//! for NVIDIA, AMD and Intel graphics alike: the 3D engine's utilisation summed over every process
//! (`\GPU Engine(*engtype_3D)\Utilization Percentage`) and the dedicated video memory in use.

use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};

/// An open counter query. Sample it about once a second: each percentage is measured over the
/// time since the previous sample.
pub struct GpuCounters {
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    memory: PDH_HCOUNTER,
}

// SAFETY: PDH query and counter handles are plain handles, not tied to the thread that opened
// them; `GpuCounters` is only used through `&mut self`, one call at a time.
unsafe impl Send for GpuCounters {}

/// A wide, NUL-terminated string for the counter paths.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl GpuCounters {
    /// Opens the counters; None when Windows has no GPU counters (old drivers, remote sessions).
    pub fn open() -> Option<Self> {
        let mut query: PDH_HQUERY = std::ptr::null_mut();
        // SAFETY: a null data source means "live counters"; `query` is a valid out pointer.
        if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) } != 0 || query.is_null() {
            return None;
        }
        let engine_path = wide("\\GPU Engine(*engtype_3D)\\Utilization Percentage");
        let memory_path = wide("\\GPU Adapter Memory(*)\\Dedicated Usage");
        let mut engine: PDH_HCOUNTER = std::ptr::null_mut();
        let mut memory: PDH_HCOUNTER = std::ptr::null_mut();
        // SAFETY: `query` is open; the paths are NUL-terminated UTF-16 that live across the calls;
        // the out pointers are valid.
        let ok_engine = unsafe { PdhAddEnglishCounterW(query, engine_path.as_ptr(), 0, &mut engine) } == 0;
        // SAFETY: as above.
        let ok_memory = unsafe { PdhAddEnglishCounterW(query, memory_path.as_ptr(), 0, &mut memory) } == 0;
        if !ok_engine && !ok_memory {
            // SAFETY: `query` is open and closed exactly once here.
            unsafe { PdhCloseQuery(query) };
            return None;
        }
        // the first sample: percentages need two
        // SAFETY: `query` is open.
        unsafe { PdhCollectQueryData(query) };
        Some(GpuCounters {
            query,
            engine: if ok_engine { engine } else { std::ptr::null_mut() },
            memory: if ok_memory { memory } else { std::ptr::null_mut() },
        })
    }

    /// (GPU load 0–100, dedicated video memory in use in bytes) since the previous sample.
    pub fn sample(&mut self) -> (Option<f64>, Option<u64>) {
        // SAFETY: `self.query` stays open until drop.
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return (None, None);
        }
        let load = sum(self.engine).map(|v| v.clamp(0.0, 100.0));
        let memory = sum(self.memory).map(|v| v.max(0.0) as u64);
        (load, memory)
    }
}

impl Drop for GpuCounters {
    fn drop(&mut self) {
        // SAFETY: the query was opened in `open` and is closed only here.
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// Adds up every instance of a wildcard counter (one per process and engine).
fn sum(counter: PDH_HCOUNTER) -> Option<f64> {
    if counter.is_null() {
        return None;
    }
    let (mut size, mut count) = (0u32, 0u32);
    // SAFETY: the first call with a null buffer only reports the size needed.
    unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut()) };
    if size == 0 || size > 16 << 20 {
        return None;
    }
    // u64 elements keep the buffer aligned for the item structs
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    let items = buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    // SAFETY: `buffer` holds at least `size` bytes, 8-byte aligned, as PDH asked for.
    if unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, items) } != 0 {
        return None;
    }
    let max_items = (buffer.len() * 8) / std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    let n = (count as usize).min(max_items);
    // SAFETY: PDH wrote `count` items at the start of `buffer` (capped to what fits); each item's
    // value was requested as a double (PDH_FMT_DOUBLE), so reading `doubleValue` is the field it
    // filled. The item names point inside `buffer` and are not read.
    let total = unsafe { std::slice::from_raw_parts(items, n) }.iter().map(|item| unsafe { item.FmtValue.Anonymous.doubleValue }).sum();
    Some(total)
}
