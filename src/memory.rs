#[derive(Debug, Clone, Copy)]
pub struct MemoryInfo {
    pub total_physical_bytes: u64,
    pub available_physical_bytes: u64,
    pub total_page_file_bytes: u64,
    pub available_page_file_bytes: u64,
    pub total_virtual_bytes: u64,
    pub available_virtual_bytes: u64,
    pub memory_load_percent: u32,
}

impl MemoryInfo {
    pub fn total_physical_mb(&self) -> u64 {
        self.total_physical_bytes / 1024 / 1024
    }

    pub fn available_physical_mb(&self) -> u64 {
        self.available_physical_bytes / 1024 / 1024
    }

    pub fn used_physical_bytes(&self) -> u64 {
        self.total_physical_bytes
            .saturating_sub(self.available_physical_bytes)
    }
}

pub fn system_memory_info() -> Option<MemoryInfo> {
    system_memory_info_impl().ok()
}

#[cfg(target_os = "windows")]
fn system_memory_info_impl() -> anyhow::Result<MemoryInfo> {
    use std::mem::size_of;
    use windows_sys::Win32::System::SystemInformation::{
        GlobalMemoryStatusEx, MEMORYSTATUSEX,
    };

    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };

    let ok = unsafe { GlobalMemoryStatusEx(&mut status as *mut MEMORYSTATUSEX) };

    if ok == 0 {
        return Err(std::io::Error::last_os_error()).map_err(Into::into);
    }

    Ok(MemoryInfo {
        total_physical_bytes: status.ullTotalPhys,
        available_physical_bytes: status.ullAvailPhys,
        total_page_file_bytes: status.ullTotalPageFile,
        available_page_file_bytes: status.ullAvailPageFile,
        total_virtual_bytes: status.ullTotalVirtual,
        available_virtual_bytes: status.ullAvailVirtual,
        memory_load_percent: status.dwMemoryLoad,
    })
}

#[cfg(target_os = "linux")]
fn system_memory_info_impl() -> anyhow::Result<MemoryInfo> {
    linux_memory_info()
}

#[cfg(target_os = "macos")]
fn system_memory_info_impl() -> anyhow::Result<MemoryInfo> {
    macos_memory_info()
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn system_memory_info_impl() -> anyhow::Result<MemoryInfo> {
    anyhow::bail!("memory detection is not implemented for this OS")
}

#[cfg(target_os = "linux")]
fn linux_memory_info() -> anyhow::Result<MemoryInfo> {
    use std::fs::read_to_string;

    let meminfo = read_to_string("/proc/meminfo")?;
    
    let mut total: Option<u64> = None;
    let mut available: Option<u64> = None;
    let mut free: Option<u64> = None;
    let mut buffers: Option<u64> = None;
    let mut cached: Option<u64> = None;

    for line in meminfo.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let key = parts[0];
            let value: u64 = parts[1].parse().unwrap_or(0) * 1024; // Convert kB to bytes

            match key {
                "MemTotal:" => total = Some(value),
                "MemAvailable:" => available = Some(value),
                "MemFree:" => free = Some(value),
                "Buffers:" => buffers = Some(value),
                "Cached:" => cached = Some(value),
                _ => {}
            }
        }
    }

    let total = total.ok_or_else(|| anyhow::anyhow!("Could not read MemTotal from /proc/meminfo"))?;
    let available = available.or_else(|| {
        free.and_then(|f| {
            buffers.and_then(|b| {
                cached.map(|c| f + b + c)
            })
        })
    }).ok_or_else(|| anyhow::anyhow!("Could not determine available memory"))?;

    let used = total.saturating_sub(available);
    let memory_load_percent = if total > 0 {
        ((used as f64) / (total as f64) * 100.0) as u32
    } else {
        0
    };

    Ok(MemoryInfo {
        total_physical_bytes: total,
        available_physical_bytes: available,
        total_page_file_bytes: 0,
        available_page_file_bytes: 0,
        total_virtual_bytes: total,
        available_virtual_bytes: available,
        memory_load_percent,
    })
}

#[cfg(target_os = "macos")]
fn macos_memory_info() -> anyhow::Result<MemoryInfo> {
    use std::process::Command;

    let output = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("sysctl hw.memsize failed");
    }

    let total = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()?;

    let vm_output = Command::new("vm_stat")
        .output()?;

    if !vm_output.status.success() {
        anyhow::bail!("vm_stat failed");
    }

    let vm_stats = String::from_utf8_lossy(&vm_output.stdout);
    let mut page_size = 4096u64;
    let mut free_pages = 0u64;

    for line in vm_stats.lines() {
        if line.contains("Mach Virtual Memory Statistics") {
            // This line doesn't contain the data we need
            continue;
        }
        if let Some(value_str) = line.split(':').nth(1) {
            let value = value_str.trim().trim_end_matches('.').parse::<u64>().unwrap_or(0);
            
            if line.contains("page size") {
                page_size = value;
            } else if line.contains("Pages free") {
                free_pages = value;
            }
        }
    }

    let available = free_pages * page_size;
    let used = total.saturating_sub(available);
    let memory_load_percent = if total > 0 {
        ((used as f64) / (total as f64) * 100.0) as u32
    } else {
        0
    };

    Ok(MemoryInfo {
        total_physical_bytes: total,
        available_physical_bytes: available,
        total_page_file_bytes: 0,
        available_page_file_bytes: 0,
        total_virtual_bytes: total,
        available_virtual_bytes: available,
        memory_load_percent,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryBudget {
    pub total_physical_bytes: u64,
    pub available_physical_bytes: u64,
    pub requested_max_bytes: Option<u64>,
    pub effective_max_bytes: u64,
}

pub fn compute_memory_budget(
    requested_max_bytes: Option<u64>,
    fraction_of_available: f64,
    hard_cap_bytes: Option<u64>,
) -> MemoryBudget {
    let info = system_memory_info();

    let total = info
        .map(|m| m.total_physical_bytes)
        .unwrap_or(0);

    let available = info
        .map(|m| m.available_physical_bytes)
        .unwrap_or(0);

    let auto_budget = if available > 0 {
        ((available as f64) * fraction_of_available.clamp(0.05, 0.95)) as u64
    } else if total > 0 {
        ((total as f64) * 0.25) as u64
    } else {
        // Last fallback: 2 GiB.
        2 * 1024 * 1024 * 1024
    };

    let mut effective = requested_max_bytes.unwrap_or(auto_budget);

    if let Some(cap) = hard_cap_bytes {
        effective = effective.min(cap);
    }

    MemoryBudget {
        total_physical_bytes: total,
        available_physical_bytes: available,
        requested_max_bytes,
        effective_max_bytes: effective,
    }
}

pub fn parse_memory_size(input: &str) -> anyhow::Result<u64> {
    let value = input.trim().to_ascii_lowercase();

    let (number, multiplier) = if let Some(n) = value.strip_suffix("gib") {
        (n, 1024_u64.pow(3))
    } else if let Some(n) = value.strip_suffix("gb") {
        (n, 1000_u64.pow(3))
    } else if let Some(n) = value.strip_suffix('g') {
        (n, 1024_u64.pow(3))
    } else if let Some(n) = value.strip_suffix("mib") {
        (n, 1024_u64.pow(2))
    } else if let Some(n) = value.strip_suffix("mb") {
        (n, 1000_u64.pow(2))
    } else if let Some(n) = value.strip_suffix('m') {
        (n, 1024_u64.pow(2))
    } else if let Some(n) = value.strip_suffix("kib") {
        (n, 1024)
    } else if let Some(n) = value.strip_suffix("kb") {
        (n, 1000)
    } else if let Some(n) = value.strip_suffix('k') {
        (n, 1024)
    } else {
        (value.as_str(), 1)
    };

    let parsed: f64 = number
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid memory size: {input}"))?;

    if parsed < 0.0 {
        anyhow::bail!("memory size must be non-negative: {input}");
    }

    Ok((parsed * multiplier as f64) as u64)
}

/// Check if system has sufficient memory for parallel operations
/// Returns Ok(()) if sufficient, Err with message if not
pub fn check_memory_available(min_required_gb: f64) -> anyhow::Result<()> {
    let min_required_bytes = (min_required_gb * 1024.0 * 1024.0 * 1024.0) as u64;
    
    if let Some(info) = system_memory_info() {
        let available = info.available_physical_bytes;
        
        if available < min_required_bytes {
            let available_gb = available as f64 / 1024.0 / 1024.0 / 1024.0;
            anyhow::bail!(
                "Insufficient memory: {:.1} GB available, {:.1} GB required. \
                 Reduce RAYON_NUM_THREADS or close other applications.",
                available_gb, min_required_gb
            );
        }
    }
    
    Ok(())
}

/// Get recommended thread count based on available memory
/// Returns a conservative thread count to prevent OOM
pub fn recommended_thread_count(per_thread_memory_gb: f64) -> usize {
    if let Some(info) = system_memory_info() {
        let available_gb = info.available_physical_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
        
        // Reserve 2 GB for system overhead
        let usable_gb = (available_gb - 2.0).max(1.0);
        
        let recommended = (usable_gb / per_thread_memory_gb).floor() as usize;
        
        // Cap at physical CPU count and minimum of 1
        let max_threads = num_cpus::get();
        recommended.min(max_threads).max(1)
    } else {
        // Fallback to CPU count if memory detection fails
        num_cpus::get()
    }
}

/// Pick a phrase / tfidf bucket count for the corpus + memory budget.
///
/// Rough heuristic. Each DF/posting record is ~12–16 bytes. We want
/// per-bucket files small enough to sort in memory during Phase 2:
/// `target_records_per_bucket = budget / 16 / safety`. Total records
/// scales roughly with parquet file count × per-file char volume; we
/// approximate `total = parquet_files × 200_000` (a few hundred MB of
/// CJK text per part file is typical). bucket_count is the next power
/// of two that brings `total / bucket_count` under target. Clamped to
/// `[64, 8192]`.
pub fn bucket_count_for_corpus(parquet_file_count: usize, memory_budget_bytes: Option<u64>) -> usize {
    let budget = memory_budget_bytes
        .map(|b| b as f64)
        .or_else(|| system_memory_info().map(|m| (m.available_physical_bytes as f64) * 0.6))
        .unwrap_or(4.0 * 1024.0 * 1024.0 * 1024.0); // 4 GB fallback

    const BYTES_PER_RECORD: f64 = 16.0;
    const SAFETY: f64 = 4.0; // multiple passes over a bucket during sort
    let target_records_per_bucket = (budget / BYTES_PER_RECORD / SAFETY).max(100_000.0);

    let parquet_files = parquet_file_count.max(1) as f64;
    let estimated_total = parquet_files * 200_000.0; // records/file estimate
    let raw = (estimated_total / target_records_per_bucket).ceil() as usize;
    let pow2 = raw.next_power_of_two().max(64);
    pow2.min(8192)
}
