pub use reactive_state;
pub use wm_proto;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use wm_proto::{
    ActionCommand, AppsAction, NotifAction, ReactiveStore, StateAction, StatePatch, StatePath,
    StateProvider, StateValue, SysAction,
};

fn round_1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round_2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ============================================================================
// 1. CpuProvider ("sys.cpu")
// ============================================================================

#[derive(Clone, Debug)]
pub struct CpuProvider {
    prev_total: u64,
    prev_idle: u64,
    usage_pct: f64,
    cores: usize,
    load_avg: f64,
    temp_c: f64,
}

impl Default for CpuProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuProvider {
    pub fn new() -> Self {
        let stat_text = fs::read_to_string("/proc/stat").unwrap_or_default();
        let (prev_total, prev_idle) = Self::parse_proc_stat(&stat_text).unwrap_or((0, 0));
        let cores = Self::count_cores(&stat_text);
        let load_avg = Self::read_loadavg();
        let temp_c = Self::read_cpu_temp();
        let usage_pct = if prev_total > 0 {
            round_1(
                ((prev_total.saturating_sub(prev_idle)) as f64 / prev_total as f64 * 100.0)
                    .clamp(0.0, 100.0),
            )
        } else {
            0.0
        };

        Self {
            prev_total,
            prev_idle,
            usage_pct,
            cores,
            load_avg,
            temp_c,
        }
    }

    /// Parses the aggregate `cpu ...` line from `/proc/stat`, returning `(total_ticks, idle_ticks)`.
    pub fn parse_proc_stat(stat_text: &str) -> Option<(u64, u64)> {
        let line = stat_text
            .lines()
            .find(|l| l.starts_with("cpu ") || l.starts_with("cpu\t"))?;
        let nums: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .filter_map(| tok| tok.parse::<u64>().ok())
            .collect();
        if nums.len() < 4 {
            return None;
        }
        let total: u64 = nums.iter().sum();
        let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
        Some((total, idle))
    }

    pub fn compute_usage_delta(
        prev_total: u64,
        prev_idle: u64,
        cur_total: u64,
        cur_idle: u64,
    ) -> Option<f64> {
        let delta_total = cur_total.saturating_sub(prev_total);
        let delta_idle = cur_idle.saturating_sub(prev_idle);
        if delta_total == 0 {
            return None;
        }
        let busy = delta_total.saturating_sub(delta_idle);
        Some(round_1(
            (busy as f64 / delta_total as f64 * 100.0).clamp(0.0, 100.0),
        ))
    }

    fn count_cores(stat_text: &str) -> usize {
        let count = stat_text
            .lines()
            .filter(|l| {
                if let Some(rest) = l.strip_prefix("cpu") {
                    rest.chars().next().is_some_and(|c| c.is_ascii_digit())
                } else {
                    false
                }
            })
            .count();
        if count > 0 {
            count
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        }
    }

    fn read_loadavg() -> f64 {
        fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
            .map(round_2)
            .unwrap_or(0.0)
    }

    fn read_cpu_temp() -> f64 {
        if let Ok(entries) = fs::read_dir("/sys/class/thermal") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("thermal_zone") {
                    let temp_path = entry.path().join("temp");
                    if let Ok(raw) = fs::read_to_string(temp_path) {
                        if let Ok(milli) = raw.trim().parse::<f64>() {
                            let c = if milli > 1000.0 {
                                milli / 1000.0
                            } else {
                                milli
                            };
                            if (0.0..=150.0).contains(&c) {
                                return round_1(c);
                            }
                        }
                    }
                }
            }
        }
        42.0
    }

    fn sample(&mut self) {
        if let Ok(stat_text) = fs::read_to_string("/proc/stat") {
            if let Some((cur_total, cur_idle)) = Self::parse_proc_stat(&stat_text) {
                if let Some(delta_pct) =
                    Self::compute_usage_delta(self.prev_total, self.prev_idle, cur_total, cur_idle)
                {
                    self.usage_pct = delta_pct;
                }
                self.prev_total = cur_total;
                self.prev_idle = cur_idle;
            }
            self.cores = Self::count_cores(&stat_text);
        }
        self.load_avg = Self::read_loadavg();
        self.temp_c = Self::read_cpu_temp();
    }

    fn build_patches(&self) -> Vec<StatePatch> {
        let mut fields = BTreeMap::new();
        fields.insert("usage_pct".to_string(), StateValue::Float(self.usage_pct));
        fields.insert("cores".to_string(), StateValue::Int(self.cores as i64));
        fields.insert("load_avg".to_string(), StateValue::Float(self.load_avg));
        fields.insert("temp_c".to_string(), StateValue::Float(self.temp_c));
        fields.insert("temp".to_string(), StateValue::Float(self.temp_c));
        vec![StatePatch::Merge {
            path: self.namespace().to_string(),
            fields,
        }]
    }
}

impl StateProvider for CpuProvider {
    fn namespace(&self) -> &'static str {
        "sys.cpu"
    }

    fn initial_state(&mut self) -> Vec<StatePatch> {
        self.sample();
        self.build_patches()
    }

    fn poll_updates(&mut self) -> Vec<StatePatch> {
        self.sample();
        self.build_patches()
    }

    fn handle_action(&mut self, _action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        Ok(Vec::new())
    }
}

// ============================================================================
// 2. RamProvider ("sys.ram")
// ============================================================================

#[derive(Clone, Debug, Default)]
pub struct RamProvider {
    percent: f64,
    used_gb: f64,
    total_gb: f64,
    swap_used_gb: f64,
}

impl RamProvider {
    pub fn new() -> Self {
        let mut p = Self::default();
        p.sample();
        p
    }

    pub fn parse_meminfo(meminfo_text: &str) -> (f64, f64, f64, f64) {
        let mut mem_total_kb = 0u64;
        let mut mem_avail_kb = None;
        let mut mem_free_kb = 0u64;
        let mut buffers_kb = 0u64;
        let mut cached_kb = 0u64;
        let mut swap_total_kb = 0u64;
        let mut swap_free_kb = 0u64;

        for line in meminfo_text.lines() {
            let Some((key, rest)) = line.split_once(':') else {
                continue;
            };
            let val_kb = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            match key.trim() {
                "MemTotal" => mem_total_kb = val_kb,
                "MemAvailable" => mem_avail_kb = Some(val_kb),
                "MemFree" => mem_free_kb = val_kb,
                "Buffers" => buffers_kb = val_kb,
                "Cached" => cached_kb = val_kb,
                "SwapTotal" => swap_total_kb = val_kb,
                "SwapFree" => swap_free_kb = val_kb,
                _ => {}
            }
        }

        let avail_kb = mem_avail_kb.unwrap_or(mem_free_kb + buffers_kb + cached_kb);
        let used_kb = mem_total_kb.saturating_sub(avail_kb);
        let swap_used_kb = swap_total_kb.saturating_sub(swap_free_kb);

        let kib_per_gib = 1024.0 * 1024.0;
        let total_gb = round_2(mem_total_kb as f64 / kib_per_gib);
        let used_gb = round_2(used_kb as f64 / kib_per_gib);
        let swap_used_gb = round_2(swap_used_kb as f64 / kib_per_gib);
        let percent = if mem_total_kb > 0 {
            round_1((used_kb as f64 / mem_total_kb as f64 * 100.0).clamp(0.0, 100.0))
        } else {
            0.0
        };
        (percent, used_gb, total_gb, swap_used_gb)
    }

    fn sample(&mut self) {
        if let Ok(text) = fs::read_to_string("/proc/meminfo") {
            let (percent, used_gb, total_gb, swap_used_gb) = Self::parse_meminfo(&text);
            self.percent = percent;
            self.used_gb = used_gb;
            self.total_gb = total_gb;
            self.swap_used_gb = swap_used_gb;
        }
    }

    fn build_patches(&self) -> Vec<StatePatch> {
        let mut fields = BTreeMap::new();
        fields.insert("percent".to_string(), StateValue::Float(self.percent));
        fields.insert("used_gb".to_string(), StateValue::Float(self.used_gb));
        fields.insert("total_gb".to_string(), StateValue::Float(self.total_gb));
        fields.insert(
            "swap_used_gb".to_string(),
            StateValue::Float(self.swap_used_gb),
        );
        vec![StatePatch::Merge {
            path: self.namespace().to_string(),
            fields,
        }]
    }
}

impl StateProvider for RamProvider {
    fn namespace(&self) -> &'static str {
        "sys.ram"
    }

    fn initial_state(&mut self) -> Vec<StatePatch> {
        self.sample();
        self.build_patches()
    }

    fn poll_updates(&mut self) -> Vec<StatePatch> {
        self.sample();
        self.build_patches()
    }

    fn handle_action(&mut self, _action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        Ok(Vec::new())
    }
}

// ============================================================================
// 3. BatteryProvider ("sys.battery")
// ============================================================================

#[derive(Clone, Debug)]
pub struct BatteryProvider {
    present: bool,
    capacity: i64,
    status: String,
    charging: bool,
}

impl Default for BatteryProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl BatteryProvider {
    pub fn new() -> Self {
        let mut p = Self {
            present: false,
            capacity: 100,
            status: "AC".to_string(),
            charging: false,
        };
        p.sample();
        p
    }

    fn sample(&mut self) {
        if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("BAT") {
                    let path = entry.path();
                    let capacity = fs::read_to_string(path.join("capacity"))
                        .ok()
                        .and_then(|s| s.trim().parse::<i64>().ok())
                        .unwrap_or(100)
                        .clamp(0, 100);
                    let status = fs::read_to_string(path.join("status"))
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "Discharging".to_string());
                    let charging = status.eq_ignore_ascii_case("charging");
                    self.present = true;
                    self.capacity = capacity;
                    self.status = status;
                    self.charging = charging;
                    return;
                }
            }
        }
        // Desktop / AC fallback
        self.present = false;
        self.capacity = 100;
        self.status = "AC".to_string();
        self.charging = false;
    }

    fn build_patches(&self) -> Vec<StatePatch> {
        let mut fields = BTreeMap::new();
        fields.insert("present".to_string(), StateValue::Bool(self.present));
        fields.insert("capacity".to_string(), StateValue::Int(self.capacity));
        fields.insert("percent".to_string(), StateValue::Int(self.capacity));
        fields.insert("status".to_string(), StateValue::String(self.status.clone()));
        fields.insert("charging".to_string(), StateValue::Bool(self.charging));
        vec![StatePatch::Merge {
            path: self.namespace().to_string(),
            fields,
        }]
    }
}

impl StateProvider for BatteryProvider {
    fn namespace(&self) -> &'static str {
        "sys.battery"
    }

    fn initial_state(&mut self) -> Vec<StatePatch> {
        self.sample();
        self.build_patches()
    }

    fn poll_updates(&mut self) -> Vec<StatePatch> {
        self.sample();
        self.build_patches()
    }

    fn handle_action(&mut self, _action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        Ok(Vec::new())
    }
}

// ============================================================================
// 4. ClockProvider ("sys.clock")
// ============================================================================

#[derive(Clone, Debug)]
pub struct ClockProvider {
    tz_offset_secs: i64,
    time: String,
    time_sec: String,
    date: String,
    weekday: String,
    weekday_short: String,
    month_name: String,
    year: i32,
    month: u32,
    day: u32,
    calendar: Vec<StateValue>,
}

impl Default for ClockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ClockProvider {
    pub fn new() -> Self {
        let now_utc = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let tz_offset_secs = detect_local_tz_offset(now_utc);
        let mut p = Self {
            tz_offset_secs,
            time: String::new(),
            time_sec: String::new(),
            date: String::new(),
            weekday: String::new(),
            weekday_short: String::new(),
            month_name: String::new(),
            year: 1970,
            month: 1,
            day: 1,
            calendar: Vec::new(),
        };
        p.sample_at(now_utc);
        p
    }

    pub fn sample_at(&mut self, unix_utc_secs: i64) {
        let local_secs = unix_utc_secs + self.tz_offset_secs;
        let days = local_secs.div_euclid(86_400);
        let rem = local_secs.rem_euclid(86_400);
        let hour = (rem / 3600) as u32;
        let minute = ((rem % 3600) / 60) as u32;
        let second = (rem % 60) as u32;

        let (year, month, day) = civil_from_days(days);
        // 1970-01-01 was Thursday (4 if Sunday=0, or 3 if Monday=0).
        // Let's use Sunday = 0 .. Saturday = 6:
        let wday_sun0 = (days + 4).rem_euclid(7) as usize;
        let weekdays = [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ];
        let weekdays_short = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        let months = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];

        self.year = year;
        self.month = month;
        self.day = day;
        self.time = format!("{hour:02}:{minute:02}");
        self.time_sec = format!("{hour:02}:{minute:02}:{second:02}");
        self.date = format!("{year:04}-{month:02}-{day:02}");
        self.weekday = weekdays[wday_sun0].to_string();
        self.weekday_short = weekdays_short[wday_sun0].to_string();
        self.month_name = months[(month.saturating_sub(1) as usize).min(11)].to_string();
        self.calendar = build_calendar_grid(year, month, day);
    }

    fn build_patches(&self) -> Vec<StatePatch> {
        let mut fields = BTreeMap::new();
        fields.insert("time".to_string(), StateValue::String(self.time.clone()));
        fields.insert(
            "time_sec".to_string(),
            StateValue::String(self.time_sec.clone()),
        );
        fields.insert("date".to_string(), StateValue::String(self.date.clone()));
        fields.insert(
            "weekday".to_string(),
            StateValue::String(self.weekday.clone()),
        );
        fields.insert(
            "weekday_short".to_string(),
            StateValue::String(self.weekday_short.clone()),
        );
        fields.insert(
            "month_name".to_string(),
            StateValue::String(self.month_name.clone()),
        );
        fields.insert("year".to_string(), StateValue::Int(self.year as i64));
        fields.insert("month".to_string(), StateValue::Int(self.month as i64));
        fields.insert("day".to_string(), StateValue::Int(self.day as i64));
        fields.insert(
            "calendar".to_string(),
            StateValue::List(self.calendar.clone()),
        );
        fields.insert(
            "calendar_days".to_string(),
            StateValue::List(self.calendar.clone()),
        );
        vec![StatePatch::Merge {
            path: self.namespace().to_string(),
            fields,
        }]
    }
}

impl StateProvider for ClockProvider {
    fn namespace(&self) -> &'static str {
        "sys.clock"
    }

    fn initial_state(&mut self) -> Vec<StatePatch> {
        let now_utc = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.sample_at(now_utc);
        self.build_patches()
    }

    fn poll_updates(&mut self) -> Vec<StatePatch> {
        let now_utc = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.sample_at(now_utc);
        self.build_patches()
    }

    fn handle_action(&mut self, _action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        Ok(Vec::new())
    }
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = year as i64 - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let m = month;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn build_calendar_grid(year: i32, month: u32, today: u32) -> Vec<StateValue> {
    let first_days = days_from_civil(year, month, 1);
    let first_wday_sun0 = (first_days + 4).rem_euclid(7) as u32;
    let cur_month_days = days_in_month(year, month);
    let (prev_year, prev_month) = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    let prev_month_days = days_in_month(prev_year, prev_month);

    let mut cells = Vec::with_capacity(42);
    for idx in 0..42u32 {
        let (cell_day, in_month) = if idx < first_wday_sun0 {
            let d = prev_month_days - (first_wday_sun0 - 1 - idx);
            (d, false)
        } else if idx < first_wday_sun0 + cur_month_days {
            let d = idx - first_wday_sun0 + 1;
            (d, true)
        } else {
            let d = idx - (first_wday_sun0 + cur_month_days) + 1;
            (d, false)
        };
        let is_today = in_month && cell_day == today;
        let mut cell = BTreeMap::new();
        cell.insert("day".to_string(), StateValue::Int(cell_day as i64));
        cell.insert("label".to_string(), StateValue::String(cell_day.to_string()));
        cell.insert("in_month".to_string(), StateValue::Bool(in_month));
        cell.insert("is_today".to_string(), StateValue::Bool(is_today));
        cells.push(StateValue::Map(cell));
    }
    cells
}

/// Parses `/etc/localtime` (RFC 8536 TZif format) to extract the current UTC offset in seconds.
fn detect_local_tz_offset(now_utc: i64) -> i64 {
    let Ok(bytes) = fs::read("/etc/localtime") else {
        return 0;
    };
    parse_tzif_offset(&bytes, now_utc).unwrap_or(0)
}

fn parse_tzif_offset(bytes: &[u8], now_utc: i64) -> Option<i64> {
    if bytes.len() < 44 || &bytes[0..4] != b"TZif" {
        return None;
    }
    let version = bytes[4];
    let read_u32 = |off: usize| -> Option<usize> {
        let arr: [u8; 4] = bytes.get(off..off + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(arr) as usize)
    };
    let read_i32 = |off: usize| -> Option<i32> {
        let arr: [u8; 4] = bytes.get(off..off + 4)?.try_into().ok()?;
        Some(i32::from_be_bytes(arr))
    };
    let read_i64 = |off: usize| -> Option<i64> {
        let arr: [u8; 8] = bytes.get(off..off + 8)?.try_into().ok()?;
        Some(i64::from_be_bytes(arr))
    };

    let isutcnt1 = read_u32(20)?;
    let isstdcnt1 = read_u32(24)?;
    let leapcnt1 = read_u32(28)?;
    let timecnt1 = read_u32(32)?;
    let typecnt1 = read_u32(36)?;
    let charcnt1 = read_u32(40)?;

    let v1_data_len =
        timecnt1 * 5 + typecnt1 * 6 + charcnt1 + leapcnt1 * 8 + isstdcnt1 + isutcnt1;
    let v2_hdr_off = 44 + v1_data_len;

    if (version == b'2' || version == b'3') && bytes.len() >= v2_hdr_off + 44 {
        if &bytes[v2_hdr_off..v2_hdr_off + 4] == b"TZif" {
            let timecnt2 = read_u32(v2_hdr_off + 32)?;
            let typecnt2 = read_u32(v2_hdr_off + 36)?;
            let data_off = v2_hdr_off + 44;
            let types_off = data_off + timecnt2 * 8;
            let ttinfo_off = types_off + timecnt2;

            let mut chosen_type = 0usize;
            for idx in 0..timecnt2 {
                let tr = read_i64(data_off + idx * 8)?;
                if tr <= now_utc {
                    chosen_type = *bytes.get(types_off + idx)? as usize;
                } else {
                    break;
                }
            }
            if chosen_type < typecnt2 {
                let utoff = read_i32(ttinfo_off + chosen_type * 6)?;
                return Some(utoff as i64);
            }
        }
    }

    if typecnt1 > 0 {
        let data_off = 44;
        let types_off = data_off + timecnt1 * 4;
        let ttinfo_off = types_off + timecnt1;
        let mut chosen_type = 0usize;
        for idx in 0..timecnt1 {
            let tr = read_i32(data_off + idx * 4)? as i64;
            if tr <= now_utc {
                chosen_type = *bytes.get(types_off + idx)? as usize;
            } else {
                break;
            }
        }
        if chosen_type < typecnt1 {
            let utoff = read_i32(ttinfo_off + chosen_type * 6)?;
            return Some(utoff as i64);
        }
    }

    None
}

// ============================================================================
// 5. AudioProvider ("sys.audio")
// ============================================================================

#[derive(Clone, Debug)]
pub struct AudioProvider {
    percent: u8,
    muted: bool,
    sink_name: String,
    brightness_percent: u8,
    external_sync_enabled: bool,
}

impl Default for AudioProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioProvider {
    pub fn new() -> Self {
        let external_sync_enabled = Self::pipewire_socket_available();
        let mut p = Self {
            percent: 65,
            muted: false,
            sink_name: "Built-in Audio Analog Stereo".to_string(),
            brightness_percent: 80,
            external_sync_enabled,
        };
        if p.external_sync_enabled {
            p.refresh_from_wpctl();
        }
        p
    }

    fn pipewire_socket_available() -> bool {
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            Path::new(&runtime_dir).join("pipewire-0").exists()
        } else {
            false
        }
    }

    /// Parses output of `wpctl get-volume @DEFAULT_AUDIO_SINK@`, e.g. `Volume: 0.45 [MUTED]`.
    pub fn parse_wpctl_volume(output: &str) -> Option<(u8, bool)> {
        let line = output.lines().find(|l| l.contains("Volume:"))?;
        let after = line.split("Volume:").nth(1)?.trim();
        let num_str = after.split_whitespace().next()?;
        let val = num_str.parse::<f64>().ok()?;
        let pct = (val * 100.0).round().clamp(0.0, 100.0) as u8;
        let muted = line.contains("[MUTED]");
        Some((pct, muted))
    }

    fn refresh_from_wpctl(&mut self) {
        if !self.external_sync_enabled {
            return;
        }
        if let Ok(output) = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if let Some((pct, muted)) = Self::parse_wpctl_volume(&text) {
                    self.percent = pct;
                    self.muted = muted;
                }
            }
        }
    }

    fn build_patches(&self) -> Vec<StatePatch> {
        let mut fields = BTreeMap::new();
        fields.insert("percent".to_string(), StateValue::Int(self.percent as i64));
        fields.insert("muted".to_string(), StateValue::Bool(self.muted));
        fields.insert(
            "sink_name".to_string(),
            StateValue::String(self.sink_name.clone()),
        );
        vec![
            StatePatch::Merge {
                path: self.namespace().to_string(),
                fields,
            },
            StatePatch::Set {
                path: "sys.brightness.percent".to_string(),
                value: StateValue::Int(self.brightness_percent as i64),
            },
        ]
    }
}

impl StateProvider for AudioProvider {
    fn namespace(&self) -> &'static str {
        "sys.audio"
    }

    fn initial_state(&mut self) -> Vec<StatePatch> {
        self.build_patches()
    }

    fn poll_updates(&mut self) -> Vec<StatePatch> {
        self.refresh_from_wpctl();
        self.build_patches()
    }

    fn handle_action(&mut self, action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        let ActionCommand::Sys(sys_action) = action else {
            return Ok(Vec::new());
        };
        match sys_action {
            SysAction::SetVolume { percent } => {
                self.percent = (*percent).min(100);
                if self.external_sync_enabled {
                    let _ = Command::new("wpctl")
                        .args([
                            "set-volume",
                            "@DEFAULT_AUDIO_SINK@",
                            &format!("{}%", self.percent),
                        ])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                Ok(self.build_patches())
            }
            SysAction::ChangeVolume { delta } => {
                let next = (self.percent as i16 + *delta as i16).clamp(0, 100) as u8;
                self.percent = next;
                if self.external_sync_enabled {
                    let arg = if *delta >= 0 {
                        format!("{}%+", delta)
                    } else {
                        format!("{}%-", delta.unsigned_abs())
                    };
                    let _ = Command::new("wpctl")
                        .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &arg])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                Ok(self.build_patches())
            }
            SysAction::ToggleMute => {
                self.muted = !self.muted;
                if self.external_sync_enabled {
                    let _ = Command::new("wpctl")
                        .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                Ok(self.build_patches())
            }
            SysAction::SetBrightness { percent } => {
                self.brightness_percent = (*percent).min(100);
                Ok(vec![StatePatch::Set {
                    path: "sys.brightness.percent".to_string(),
                    value: StateValue::Int(self.brightness_percent as i64),
                }])
            }
            SysAction::ChangeBrightness { delta } => {
                self.brightness_percent =
                    (self.brightness_percent as i16 + *delta as i16).clamp(10, 100) as u8;
                Ok(self.build_patches())
            }
            SysAction::SetResolution { width, height } => Ok(vec![StatePatch::Set {
                path: "sys.display.resolution".to_string(),
                value: StateValue::String(format!("{width}x{height}")),
            }]),
            SysAction::Logout => Ok(vec![StatePatch::Set {
                path: "sys.session.exit_requested".to_string(),
                value: StateValue::Bool(true),
            }]),
            SysAction::Reboot => Ok(vec![StatePatch::Set {
                path: "sys.session.power_action".to_string(),
                value: StateValue::String("reboot".to_string()),
            }]),
            SysAction::PowerOff => Ok(vec![StatePatch::Set {
                path: "sys.session.power_action".to_string(),
                value: StateValue::String("poweroff".to_string()),
            }]),
        }
    }
}

// ============================================================================
// 6. NetworkProvider ("sys.network")
// ============================================================================

#[derive(Clone, Debug)]
pub struct NetworkProvider {
    connected: bool,
    interface: String,
    kind: String,
    rx_kbps: f64,
    tx_kbps: f64,
    prev_rx_bytes: u64,
    prev_tx_bytes: u64,
    prev_sample_time: Instant,
}

impl Default for NetworkProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkProvider {
    pub fn new() -> Self {
        let mut p = Self {
            connected: false,
            interface: "none".to_string(),
            kind: "offline".to_string(),
            rx_kbps: 0.0,
            tx_kbps: 0.0,
            prev_rx_bytes: 0,
            prev_tx_bytes: 0,
            prev_sample_time: Instant::now(),
        };
        p.sample();
        p
    }

    /// Parses `/proc/net/dev` and returns `Vec<(iface_name, rx_bytes, tx_bytes)>` excluding `lo`.
    pub fn parse_proc_net_dev(text: &str) -> Vec<(String, u64, u64)> {
        let mut out = Vec::new();
        for line in text.lines().skip(2) {
            let Some((iface_raw, stats_raw)) = line.split_once(':') else {
                continue;
            };
            let iface = iface_raw.trim();
            if iface.is_empty() || iface == "lo" {
                continue;
            }
            let cols: Vec<u64> = stats_raw
                .split_whitespace()
                .filter_map(|s| s.parse::<u64>().ok())
                .collect();
            if cols.len() >= 9 {
                let rx_bytes = cols[0];
                let tx_bytes = cols[8];
                out.push((iface.to_string(), rx_bytes, tx_bytes));
            }
        }
        out
    }

    fn detect_iface_kind(iface: &str) -> &'static str {
        let sys_path = Path::new("/sys/class/net").join(iface);
        if sys_path.join("wireless").exists()
            || sys_path.join("phy80211").exists()
            || iface.starts_with("wl")
            || iface.starts_with("wlan")
            || iface.starts_with("wifi")
        {
            "wifi"
        } else {
            "ethernet"
        }
    }

    fn is_iface_up(iface: &str) -> bool {
        let oper_path = Path::new("/sys/class/net").join(iface).join("operstate");
        if let Ok(state) = fs::read_to_string(oper_path) {
            let s = state.trim();
            s == "up" || s == "unknown"
        } else {
            false
        }
    }

    fn sample(&mut self) {
        let now = Instant::now();
        let elapsed_secs = now.duration_since(self.prev_sample_time).as_secs_f64();
        let net_dev = fs::read_to_string("/proc/net/dev").unwrap_or_default();
        let entries = Self::parse_proc_net_dev(&net_dev);

        let chosen = entries
            .iter()
            .find(|(iface, _, _)| Self::is_iface_up(iface))
            .or_else(|| entries.iter().max_by_key(|(_, rx, tx)| rx + tx));

        if let Some((iface, rx_bytes, tx_bytes)) = chosen {
            if elapsed_secs > 1e-3 && self.prev_rx_bytes > 0 && self.interface == *iface {
                let delta_rx = rx_bytes.saturating_sub(self.prev_rx_bytes);
                let delta_tx = tx_bytes.saturating_sub(self.prev_tx_bytes);
                self.rx_kbps = round_1((delta_rx as f64 / 1024.0) / elapsed_secs);
                self.tx_kbps = round_1((delta_tx as f64 / 1024.0) / elapsed_secs);
            } else {
                self.rx_kbps = 0.0;
                self.tx_kbps = 0.0;
            }
            self.connected = true;
            self.interface = iface.clone();
            self.kind = Self::detect_iface_kind(iface).to_string();
            self.prev_rx_bytes = *rx_bytes;
            self.prev_tx_bytes = *tx_bytes;
        } else {
            self.connected = false;
            self.interface = "none".to_string();
            self.kind = "offline".to_string();
            self.rx_kbps = 0.0;
            self.tx_kbps = 0.0;
        }
        self.prev_sample_time = now;
    }

    fn build_patches(&self) -> Vec<StatePatch> {
        let mut fields = BTreeMap::new();
        fields.insert("connected".to_string(), StateValue::Bool(self.connected));
        fields.insert(
            "interface".to_string(),
            StateValue::String(self.interface.clone()),
        );
        let ssid = if self.connected && self.interface != "none" {
            self.interface.clone()
        } else {
            "Offline".to_string()
        };
        fields.insert("ssid".to_string(), StateValue::String(ssid));
        fields.insert("kind".to_string(), StateValue::String(self.kind.clone()));
        fields.insert("rx_kbps".to_string(), StateValue::Float(self.rx_kbps));
        fields.insert("tx_kbps".to_string(), StateValue::Float(self.tx_kbps));
        vec![StatePatch::Merge {
            path: self.namespace().to_string(),
            fields,
        }]
    }
}

impl StateProvider for NetworkProvider {
    fn namespace(&self) -> &'static str {
        "sys.network"
    }

    fn initial_state(&mut self) -> Vec<StatePatch> {
        self.sample();
        self.build_patches()
    }

    fn poll_updates(&mut self) -> Vec<StatePatch> {
        self.sample();
        self.build_patches()
    }

    fn handle_action(&mut self, _action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        Ok(Vec::new())
    }
}

// ============================================================================
// 7. AppsProvider ("apps")
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopAppEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub exec: String,
    pub icon: String,
    pub categories: String,
}

impl DesktopAppEntry {
    pub fn to_state_value(&self) -> StateValue {
        let mut m = BTreeMap::new();
        m.insert("id".to_string(), StateValue::String(self.id.clone()));
        m.insert("name".to_string(), StateValue::String(self.name.clone()));
        m.insert(
            "description".to_string(),
            StateValue::String(self.description.clone()),
        );
        m.insert("exec".to_string(), StateValue::String(self.exec.clone()));
        m.insert("icon".to_string(), StateValue::String(self.icon.clone()));
        m.insert(
            "categories".to_string(),
            StateValue::String(self.categories.clone()),
        );
        let primary_cat = self
            .categories
            .split(';')
            .find(|c| !c.trim().is_empty())
            .unwrap_or("App")
            .to_string();
        m.insert("category".to_string(), StateValue::String(primary_cat));
        StateValue::Map(m)
    }
}

#[derive(Clone, Debug)]
pub struct AppsProvider {
    all: Vec<DesktopAppEntry>,
    filtered: Vec<DesktopAppEntry>,
    query: String,
    selected_idx: usize,
    last_launched: Option<String>,
    spawn_processes: bool,
}

impl Default for AppsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AppsProvider {
    pub fn new() -> Self {
        let all = Self::discover_desktop_apps();
        let filtered = all.clone();
        Self {
            all,
            filtered,
            query: String::new(),
            selected_idx: 0,
            last_launched: None,
            spawn_processes: std::env::var("LUMINA_SPAWN_APPS")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }

    pub fn from_entries(entries: Vec<DesktopAppEntry>) -> Self {
        let filtered = entries.clone();
        Self {
            all: entries,
            filtered,
            query: String::new(),
            selected_idx: 0,
            last_launched: None,
            spawn_processes: false,
        }
    }

    pub fn set_spawn_processes(&mut self, enabled: bool) {
        self.spawn_processes = enabled;
    }

    pub fn last_launched(&self) -> Option<&str> {
        self.last_launched.as_deref()
    }

    pub fn discover_desktop_apps() -> Vec<DesktopAppEntry> {
        let mut dirs = vec![PathBuf::from("/usr/share/applications")];
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(Path::new(&home).join(".local/share/applications"));
        }

        let mut by_id: BTreeMap<String, DesktopAppEntry> = BTreeMap::new();
        for dir in dirs {
            if let Ok(read_dir) = fs::read_dir(&dir) {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                        continue;
                    }
                    let id = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("app")
                        .to_string();
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Some(parsed) = Self::parse_desktop_entry(&id, &content) {
                            by_id.insert(id, parsed);
                        }
                    }
                }
            }
        }

        if by_id.is_empty() {
            for fallback in Self::default_fallback_apps() {
                by_id.entry(fallback.id.clone()).or_insert(fallback);
            }
        }

        let mut list: Vec<DesktopAppEntry> = by_id.into_values().collect();
        list.sort_by(|a, b| {
            app_priority_rank(&a.id)
                .cmp(&app_priority_rank(&b.id))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        list
    }

    pub fn parse_desktop_entry(id: &str, content: &str) -> Option<DesktopAppEntry> {
        if is_unwanted_desktop_id(id) {
            return None;
        }

        let mut in_desktop_entry = false;
        let mut name = None;
        let mut generic_name = None;
        let mut comment = None;
        let mut exec = None;
        let mut icon = String::from("application-x-executable");
        let mut categories = String::new();
        let mut no_display = false;
        let mut hidden = false;
        let mut is_terminal_cli = false;

        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                in_desktop_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_desktop_entry {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim();
            let val = v.trim();
            match key {
                "Name" if name.is_none() => name = Some(val.to_string()),
                "GenericName" if generic_name.is_none() => generic_name = Some(val.to_string()),
                "Comment" if comment.is_none() => comment = Some(val.to_string()),
                "Exec" if exec.is_none() => exec = Some(clean_exec_command(val)),
                "Icon" => icon = val.to_string(),
                "Categories" => categories = val.to_string(),
                "NoDisplay" => no_display = val.eq_ignore_ascii_case("true"),
                "Hidden" => hidden = val.eq_ignore_ascii_case("true"),
                "Terminal" => is_terminal_cli = val.eq_ignore_ascii_case("true"),
                _ => {}
            }
        }

        if no_display || hidden || is_terminal_cli {
            return None;
        }
        let name = name?;
        let exec = exec.filter(|e| !e.is_empty())?;
        let description = comment.or(generic_name).unwrap_or_default();

        Some(DesktopAppEntry {
            id: id.to_string(),
            name,
            description,
            exec,
            icon,
            categories,
        })
    }

    pub fn default_fallback_apps() -> Vec<DesktopAppEntry> {
        vec![
            DesktopAppEntry {
                id: "org.mozilla.firefox".to_string(),
                name: "Firefox Web Browser".to_string(),
                description: "Browse the World Wide Web".to_string(),
                exec: "firefox".to_string(),
                icon: "firefox".to_string(),
                categories: "Network;WebBrowser;".to_string(),
            },
            DesktopAppEntry {
                id: "org.alacritty.Alacritty".to_string(),
                name: "Alacritty Terminal".to_string(),
                description: "Fast GPU-accelerated terminal emulator".to_string(),
                exec: "alacritty".to_string(),
                icon: "utilities-terminal".to_string(),
                categories: "System;TerminalEmulator;".to_string(),
            },
            DesktopAppEntry {
                id: "org.gnome.Nautilus".to_string(),
                name: "Files".to_string(),
                description: "Access and organize files".to_string(),
                exec: "nautilus".to_string(),
                icon: "system-file-manager".to_string(),
                categories: "System;FileTools;FileManager;".to_string(),
            },
            DesktopAppEntry {
                id: "com.visualstudio.code".to_string(),
                name: "Visual Studio Code".to_string(),
                description: "Code Editing. Redefined.".to_string(),
                exec: "code".to_string(),
                icon: "vscode".to_string(),
                categories: "Development;IDE;TextEditor;".to_string(),
            },
            DesktopAppEntry {
                id: "htop".to_string(),
                name: "System Monitor (htop)".to_string(),
                description: "Interactive process viewer".to_string(),
                exec: "htop".to_string(),
                icon: "utilities-system-monitor".to_string(),
                categories: "System;Monitor;".to_string(),
            },
        ]
    }

    fn recompute_filtered(&mut self) {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            self.filtered = self.all.clone();
            self.selected_idx = 0;
            return;
        }

        let mut scored: Vec<(i32, DesktopAppEntry)> = self
            .all
            .iter()
            .filter_map(|app| {
                let score = fuzzy_score_app(app, &q)?;
                Some((score, app.clone()))
            })
            .collect();

        scored.sort_by(|(sa, a), (sb, b)| {
            sb.cmp(sa)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        self.filtered = scored.into_iter().map(|(_, app)| app).collect();
        self.selected_idx = 0;
    }

    fn launch_app_entry(&mut self, entry: &DesktopAppEntry) {
        self.last_launched = Some(entry.id.clone());
        if self.spawn_processes && !entry.exec.is_empty() {
            let _ = Command::new("sh")
                .args(["-c", &entry.exec])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }

    fn build_patches(&self) -> Vec<StatePatch> {
        let all_vals: Vec<StateValue> = self.all.iter().map(|a| a.to_state_value()).collect();
        let filtered_vals: Vec<StateValue> = self
            .filtered
            .iter()
            .enumerate()
            .map(|(idx, a)| {
                let mut val = a.to_state_value();
                if let StateValue::Map(ref mut m) = val {
                    m.insert(
                        "is_selected".to_string(),
                        StateValue::Bool(idx == self.selected_idx),
                    );
                }
                val
            })
            .collect();
        let mut fields = BTreeMap::new();
        fields.insert("query".to_string(), StateValue::String(self.query.clone()));
        fields.insert(
            "selected_idx".to_string(),
            StateValue::Int(self.selected_idx as i64),
        );
        fields.insert("all".to_string(), StateValue::List(all_vals));
        fields.insert("filtered".to_string(), StateValue::List(filtered_vals));
        fields.insert(
            "last_launched".to_string(),
            self.last_launched
                .as_ref()
                .map(|s| StateValue::String(s.clone()))
                .unwrap_or(StateValue::Null),
        );
        vec![StatePatch::Merge {
            path: self.namespace().to_string(),
            fields,
        }]
    }

    pub fn find_app(&self, app_id_or_name: &str) -> Option<&DesktopAppEntry> {
        let key = app_id_or_name.trim().trim_end_matches(".desktop");
        self.all.iter().find(|a| {
            a.id.eq_ignore_ascii_case(key)
                || a.exec.eq_ignore_ascii_case(key)
                || a.name.eq_ignore_ascii_case(key)
        })
    }
}

fn is_unwanted_desktop_id(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    if lower.starts_with("waydroid.")
        || lower.starts_with("wine-extension-")
        || lower == "wine"
        || lower.starts_with("org.freedesktop.ibus")
        || lower.starts_with("xdg-desktop-portal")
        || lower.ends_with("-handler")
        || lower.ends_with("-previewer")
        || lower.ends_with("-panel")
        || lower.contains("autorun")
        || lower.contains("url-handler")
    {
        return true;
    }
    matches!(
        lower.as_str(),
        "avahi-discover"
            | "bssh"
            | "bvnc"
            | "lstopo"
            | "qv4l2"
            | "qvidcap"
            | "xgps"
            | "xgpsspeed"
            | "java-java-openjdk"
            | "user-dirs-update-gtk"
            | "gcm-import"
            | "gcm-picker"
            | "gcr-prompter"
            | "gcr-viewer"
            | "geoclue-demo-agent"
            | "geoclue-where-am-i"
            | "org.freedesktop.xwayland"
            | "org.gnome.zenty"
            | "org.gnome.zenity"
            | "org.gnupg.pinentry-qt"
            | "polkit-gnome-authentication-agent-1"
            | "bluetooth-sendto"
            | "arch-update-tray"
            | "com.cloudflare.warptaskbar"
            | "com.shellyorg.shelly-notifications"
            | "org.gnome.shell.calendarserver"
            | "org.gnome.shell.portalhelper"
            | "org.gnome.onlineaccounts.oauth2"
            | "org.gnome.goa-daemon"
    )
}

fn app_priority_rank(id: &str) -> u8 {
    let lower = id.to_ascii_lowercase();
    if lower.contains("antigravity")
        || lower.contains("console")
        || lower.contains("kgx")
        || lower.contains("alacritty")
        || lower.contains("ptyxis")
        || lower.contains("terminal")
    {
        0
    } else if lower.contains("nautilus") || lower.contains("texteditor") || lower.contains("calculator") {
        1
    } else if lower.contains("resources") || lower.contains("decibels") || lower.contains("loupe") || lower.contains("papers") || lower.contains("evince") {
        2
    } else if lower.contains("firefox") || lower.contains("brave") {
        3
    } else {
        5
    }
}

fn clean_exec_command(raw: &str) -> String {
    raw.split_whitespace()
        .filter(|tok| !tok.starts_with('%'))
        .collect::<Vec<&str>>()
        .join(" ")
}


fn fuzzy_score_app(app: &DesktopAppEntry, q: &str) -> Option<i32> {
    let name_l = app.name.to_lowercase();
    let desc_l = app.description.to_lowercase();
    let exec_l = app.exec.to_lowercase();
    let cat_l = app.categories.to_lowercase();
    let id_l = app.id.to_lowercase();

    if name_l == q || exec_l == q || id_l == q {
        return Some(1000);
    }
    if name_l.starts_with(q) {
        return Some(800);
    }
    if exec_l.starts_with(q) {
        return Some(750);
    }
    if name_l.contains(q) {
        return Some(600);
    }
    if exec_l.contains(q) || id_l.contains(q) {
        return Some(500);
    }
    if desc_l.contains(q) {
        return Some(350);
    }
    if cat_l.contains(q) {
        return Some(250);
    }
    if is_subsequence(q, &name_l) || is_subsequence(q, &exec_l) {
        return Some(100);
    }
    None
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut n_chars = needle.chars().peekable();
    for ch in haystack.chars() {
        if n_chars.peek() == Some(&ch) {
            n_chars.next();
        }
    }
    n_chars.peek().is_none()
}

impl StateProvider for AppsProvider {
    fn namespace(&self) -> &'static str {
        "apps"
    }

    fn initial_state(&mut self) -> Vec<StatePatch> {
        self.build_patches()
    }

    fn poll_updates(&mut self) -> Vec<StatePatch> {
        Vec::new()
    }

    fn handle_action(&mut self, action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        let ActionCommand::Apps(apps_action) = action else {
            return Ok(Vec::new());
        };
        match apps_action {
            AppsAction::SetQuery { query } => {
                self.query = query.clone();
                self.recompute_filtered();
                Ok(self.build_patches())
            }
            AppsAction::SelectRelative { delta } => {
                if self.filtered.is_empty() {
                    self.selected_idx = 0;
                } else {
                    let len = self.filtered.len() as i32;
                    let next = (self.selected_idx as i32 + delta).rem_euclid(len) as usize;
                    self.selected_idx = next;
                }
                Ok(self.build_patches())
            }
            AppsAction::Launch { app_id } => {
                let target = self
                    .all
                    .iter()
                    .find(|a| {
                        a.id.eq_ignore_ascii_case(app_id)
                            || a.exec.eq_ignore_ascii_case(app_id)
                            || a.name.eq_ignore_ascii_case(app_id)
                    })
                    .cloned();
                if let Some(entry) = target {
                    self.launch_app_entry(&entry);
                } else {
                    self.last_launched = Some(app_id.clone());
                }
                self.query.clear();
                self.recompute_filtered();
                Ok(self.build_patches())
            }
            AppsAction::LaunchSelected => {
                if let Some(entry) = self.filtered.get(self.selected_idx).cloned() {
                    self.launch_app_entry(&entry);
                    self.query.clear();
                    self.recompute_filtered();
                }
                Ok(self.build_patches())
            }
        }
    }
}

// ============================================================================
// 8. NotifProvider ("notif")
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationItem {
    pub id: u64,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub urgency: String,
}

impl NotificationItem {
    pub fn to_state_value(&self) -> StateValue {
        let mut m = BTreeMap::new();
        m.insert("id".to_string(), StateValue::Int(self.id as i64));
        m.insert(
            "app_name".to_string(),
            StateValue::String(self.app_name.clone()),
        );
        m.insert(
            "summary".to_string(),
            StateValue::String(self.summary.clone()),
        );
        m.insert("body".to_string(), StateValue::String(self.body.clone()));
        m.insert(
            "urgency".to_string(),
            StateValue::String(self.urgency.clone()),
        );
        StateValue::Map(m)
    }
}

#[derive(Clone, Debug)]
pub struct NotifProvider {
    next_id: u64,
    list: Vec<NotificationItem>,
    dnd: bool,
}

impl Default for NotifProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl NotifProvider {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            list: Vec::new(),
            dnd: false,
        }
    }

    pub fn push_notification(
        &mut self,
        app_name: impl Into<String>,
        summary: impl Into<String>,
        body: impl Into<String>,
        urgency: impl Into<String>,
    ) -> Vec<StatePatch> {
        let id = self.next_id;
        self.next_id += 1;
        let item = NotificationItem {
            id,
            app_name: app_name.into(),
            summary: summary.into(),
            body: body.into(),
            urgency: urgency.into(),
        };
        self.list.push(item);
        self.build_patches()
    }

    pub fn notifications(&self) -> &[NotificationItem] {
        &self.list
    }

    pub fn dnd(&self) -> bool {
        self.dnd
    }

    fn build_patches(&self) -> Vec<StatePatch> {
        let list_vals: Vec<StateValue> = self.list.iter().map(|n| n.to_state_value()).collect();
        let unread_count = self.list.len() as i64;
        let mut fields = BTreeMap::new();
        fields.insert("list".to_string(), StateValue::List(list_vals));
        fields.insert("unread_count".to_string(), StateValue::Int(unread_count));
        fields.insert("dnd".to_string(), StateValue::Bool(self.dnd));
        vec![StatePatch::Merge {
            path: self.namespace().to_string(),
            fields,
        }]
    }
}

impl StateProvider for NotifProvider {
    fn namespace(&self) -> &'static str {
        "notif"
    }

    fn initial_state(&mut self) -> Vec<StatePatch> {
        self.build_patches()
    }

    fn poll_updates(&mut self) -> Vec<StatePatch> {
        Vec::new()
    }

    fn handle_action(&mut self, action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        let ActionCommand::Notif(notif_action) = action else {
            return Ok(Vec::new());
        };
        match notif_action {
            NotifAction::Dismiss { id } => {
                self.list.retain(|n| n.id != *id);
                Ok(self.build_patches())
            }
            NotifAction::ClearAll => {
                self.list.clear();
                Ok(self.build_patches())
            }
            NotifAction::ToggleDnd => {
                self.dnd = !self.dnd;
                Ok(self.build_patches())
            }
        }
    }
}

// ============================================================================
// 9. SystemProviderManager
// ============================================================================

/// Aggregates all system and desktop state providers, initializes a `ReactiveStore`,
/// polls periodic updates, and dispatches `ActionCommand`s.
pub struct SystemProviderManager {
    pub cpu: CpuProvider,
    pub ram: RamProvider,
    pub battery: BatteryProvider,
    pub clock: ClockProvider,
    pub audio: AudioProvider,
    pub network: NetworkProvider,
    pub apps: AppsProvider,
    pub notif: NotifProvider,
    extra_providers: Vec<Box<dyn StateProvider>>,
}

impl Default for SystemProviderManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemProviderManager {
    pub fn new() -> Self {
        Self {
            cpu: CpuProvider::new(),
            ram: RamProvider::new(),
            battery: BatteryProvider::new(),
            clock: ClockProvider::new(),
            audio: AudioProvider::new(),
            network: NetworkProvider::new(),
            apps: AppsProvider::new(),
            notif: NotifProvider::new(),
            extra_providers: Vec::new(),
        }
    }

    pub fn register_provider(&mut self, provider: Box<dyn StateProvider>) {
        self.extra_providers.push(provider);
    }

    /// Collects initial state patches from all providers.
    pub fn initial_patches(&mut self) -> Vec<StatePatch> {
        let mut patches = Vec::new();
        patches.extend(self.cpu.initial_state());
        patches.extend(self.ram.initial_state());
        patches.extend(self.battery.initial_state());
        patches.extend(self.clock.initial_state());
        patches.extend(self.audio.initial_state());
        patches.extend(self.network.initial_state());
        patches.extend(self.apps.initial_state());
        patches.extend(self.notif.initial_state());
        for extra in &mut self.extra_providers {
            patches.extend(extra.initial_state());
        }
        patches
    }

    /// Initializes `store` with the initial state of all registered providers.
    pub fn init_store(&mut self, store: &mut dyn ReactiveStore) -> Vec<StatePath> {
        let patches = self.initial_patches();
        store.apply_patches(patches)
    }

    /// Alias for [`Self::init_store`].
    pub fn initialize(&mut self, store: &mut dyn ReactiveStore) -> Vec<StatePath> {
        self.init_store(store)
    }

    /// Polls all providers and returns their state patches.
    pub fn poll_patches(&mut self) -> Vec<StatePatch> {
        let mut patches = Vec::new();
        patches.extend(self.cpu.poll_updates());
        patches.extend(self.ram.poll_updates());
        patches.extend(self.battery.poll_updates());
        patches.extend(self.clock.poll_updates());
        patches.extend(self.audio.poll_updates());
        patches.extend(self.network.poll_updates());
        patches.extend(self.apps.poll_updates());
        patches.extend(self.notif.poll_updates());
        for extra in &mut self.extra_providers {
            patches.extend(extra.poll_updates());
        }
        patches
    }

    /// Polls all providers and applies their updates directly to `store`.
    pub fn poll_store(&mut self, store: &mut dyn ReactiveStore) -> Vec<StatePath> {
        let patches = self.poll_patches();
        store.apply_patches(patches)
    }

    /// Dispatches an `ActionCommand` across all providers and returns resulting `StatePatch`es.
    pub fn handle_action(&mut self, action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        let mut patches = Vec::new();
        patches.extend(self.cpu.handle_action(action)?);
        patches.extend(self.ram.handle_action(action)?);
        patches.extend(self.battery.handle_action(action)?);
        patches.extend(self.clock.handle_action(action)?);
        patches.extend(self.audio.handle_action(action)?);
        patches.extend(self.network.handle_action(action)?);
        patches.extend(self.apps.handle_action(action)?);
        patches.extend(self.notif.handle_action(action)?);
        for extra in &mut self.extra_providers {
            patches.extend(extra.handle_action(action)?);
        }
        Ok(patches)
    }

    /// Dispatches an `ActionCommand` and applies resulting state mutations to `store`.
    pub fn dispatch_action(
        &mut self,
        action: &ActionCommand,
        store: &mut dyn ReactiveStore,
    ) -> Result<Vec<StatePath>, String> {
        if let ActionCommand::State(state_action) = action {
            let changed = match state_action {
                StateAction::Set { path, value } => store.set(path, value.clone()),
                StateAction::ToggleBool { path } => {
                    let cur = store.get(path).map(|v| v.as_bool()).unwrap_or(false);
                    store.set(path, StateValue::Bool(!cur))
                }
            };
            return Ok(changed);
        }
        let patches = self.handle_action(action)?;
        Ok(store.apply_patches(patches))
    }

    /// Pushes a desktop notification into `NotifProvider` and returns the state patches.
    pub fn push_notification(
        &mut self,
        app_name: impl Into<String>,
        summary: impl Into<String>,
        body: impl Into<String>,
        urgency: impl Into<String>,
    ) -> Vec<StatePatch> {
        self.notif
            .push_notification(app_name, summary, body, urgency)
    }
}

impl StateProvider for SystemProviderManager {
    fn namespace(&self) -> &'static str {
        "sys"
    }

    fn initial_state(&mut self) -> Vec<StatePatch> {
        self.initial_patches()
    }

    fn poll_updates(&mut self) -> Vec<StatePatch> {
        self.poll_patches()
    }

    fn handle_action(&mut self, action: &ActionCommand) -> Result<Vec<StatePatch>, String> {
        SystemProviderManager::handle_action(self, action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactive_state::StateStore;

    #[test]
    fn test_cpu_and_ram_providers_populate_real_linux_metrics() {
        let mut store = StateStore::new();
        let mut cpu = CpuProvider::new();
        let mut ram = RamProvider::new();

        store.apply_patches(cpu.initial_state());
        store.apply_patches(ram.initial_state());

        let usage = store
            .get("sys.cpu.usage_pct")
            .and_then(|v| v.as_f64())
            .expect("sys.cpu.usage_pct must exist");
        assert!((0.0..=100.0).contains(&usage));

        let cores = store
            .get("sys.cpu.cores")
            .and_then(|v| v.as_i64())
            .expect("sys.cpu.cores must exist");
        assert!(cores >= 1);

        let total_gb = store
            .get("sys.ram.total_gb")
            .and_then(|v| v.as_f64())
            .expect("sys.ram.total_gb must exist");
        assert!(total_gb > 0.0);

        let ram_pct = store
            .get("sys.ram.percent")
            .and_then(|v| v.as_f64())
            .expect("sys.ram.percent must exist");
        assert!((0.0..=100.0).contains(&ram_pct));
    }

    #[test]
    fn test_clock_provider_formats_and_calendar_grid() {
        let mut clock = ClockProvider::new();
        // Test deterministic timestamp: 2026-10-16 12:34:56 UTC
        clock.tz_offset_secs = 0;
        let ts = days_from_civil(2026, 10, 16) * 86_400 + 12 * 3600 + 34 * 60 + 56;
        clock.sample_at(ts);

        let mut store = StateStore::new();
        store.apply_patches(clock.build_patches());

        assert_eq!(
            store.get("sys.clock.time"),
            Some(StateValue::String("12:34".to_string()))
        );
        assert_eq!(
            store.get("sys.clock.time_sec"),
            Some(StateValue::String("12:34:56".to_string()))
        );
        assert_eq!(
            store.get("sys.clock.date"),
            Some(StateValue::String("2026-10-16".to_string()))
        );
        assert_eq!(
            store.get("sys.clock.weekday"),
            Some(StateValue::String("Friday".to_string()))
        );
        assert_eq!(store.get("sys.clock.calendar.len"), Some(StateValue::Int(42)));
    }

    #[test]
    fn test_audio_provider_actions() {
        let mut store = StateStore::new();
        let mut audio = AudioProvider::new();
        audio.external_sync_enabled = false;
        store.apply_patches(audio.initial_state());

        let patches = audio
            .handle_action(&ActionCommand::Sys(SysAction::SetVolume { percent: 80 }))
            .unwrap();
        store.apply_patches(patches);
        assert_eq!(store.get("sys.audio.percent"), Some(StateValue::Int(80)));

        let patches = audio
            .handle_action(&ActionCommand::Sys(SysAction::ChangeVolume { delta: -15 }))
            .unwrap();
        store.apply_patches(patches);
        assert_eq!(store.get("sys.audio.percent"), Some(StateValue::Int(65)));

        let patches = audio
            .handle_action(&ActionCommand::Sys(SysAction::ToggleMute))
            .unwrap();
        store.apply_patches(patches);
        assert_eq!(store.get("sys.audio.muted"), Some(StateValue::Bool(true)));

        let patches = audio
            .handle_action(&ActionCommand::Sys(SysAction::ChangeBrightness {
                delta: -10,
            }))
            .unwrap();
        store.apply_patches(patches);
        assert_eq!(
            store.get("sys.brightness.percent"),
            Some(StateValue::Int(70))
        );

        let patches = audio
            .handle_action(&ActionCommand::Sys(SysAction::Logout))
            .unwrap();
        store.apply_patches(patches);
        assert_eq!(
            store.get("sys.session.exit_requested"),
            Some(StateValue::Bool(true))
        );
    }

    #[test]
    fn test_apps_provider_search_select_and_launch() {
        let mut store = StateStore::new();
        let mut apps = AppsProvider::from_entries(AppsProvider::default_fallback_apps());
        store.apply_patches(apps.initial_state());

        let total = store.get("apps.all.len").and_then(|v| v.as_i64()).unwrap();
        assert!(total >= 5);
        assert_eq!(
            store.get("apps.filtered.0.is_selected"),
            Some(StateValue::Bool(true))
        );
        assert_eq!(
            store.get("apps.filtered.1.is_selected"),
            Some(StateValue::Bool(false))
        );

        let patches = apps
            .handle_action(&ActionCommand::Apps(AppsAction::SelectRelative {
                delta: 1,
            }))
            .unwrap();
        store.apply_patches(patches);
        assert_eq!(
            store.get("apps.filtered.0.is_selected"),
            Some(StateValue::Bool(false))
        );
        assert_eq!(
            store.get("apps.filtered.1.is_selected"),
            Some(StateValue::Bool(true))
        );

        // Fuzzy query for firefox
        let patches = apps
            .handle_action(&ActionCommand::Apps(AppsAction::SetQuery {
                query: "fire".to_string(),
            }))
            .unwrap();
        store.apply_patches(patches);

        assert_eq!(store.get("apps.filtered.len"), Some(StateValue::Int(1)));
        assert_eq!(
            store.get("apps.filtered.0.id"),
            Some(StateValue::String("org.mozilla.firefox".to_string()))
        );

        // Launch selected
        let patches = apps
            .handle_action(&ActionCommand::Apps(AppsAction::LaunchSelected))
            .unwrap();
        store.apply_patches(patches);
        assert_eq!(
            store.get("apps.last_launched"),
            Some(StateValue::String("org.mozilla.firefox".to_string()))
        );
        assert_eq!(
            store.get("apps.query"),
            Some(StateValue::String(String::new()))
        );
    }

    #[test]
    fn test_notif_provider_push_dismiss_clear_and_dnd() {
        let mut store = StateStore::new();
        let mut notif = NotifProvider::new();
        store.apply_patches(notif.initial_state());

        let p1 = notif.push_notification("Volume", "Audio 70%", "Unmuted", "low");
        store.apply_patches(p1);
        let p2 = notif.push_notification("Battery", "Low Battery", "15% remaining", "critical");
        store.apply_patches(p2);

        assert_eq!(store.get("notif.unread_count"), Some(StateValue::Int(2)));
        assert_eq!(
            store.get("notif.list.1.summary"),
            Some(StateValue::String("Low Battery".to_string()))
        );

        // Dismiss first notification (id = 1)
        let dismiss = notif
            .handle_action(&ActionCommand::Notif(NotifAction::Dismiss { id: 1 }))
            .unwrap();
        store.apply_patches(dismiss);
        assert_eq!(store.get("notif.unread_count"), Some(StateValue::Int(1)));
        assert_eq!(
            store.get("notif.list.0.id"),
            Some(StateValue::Int(2))
        );

        // Toggle DND
        let dnd = notif
            .handle_action(&ActionCommand::Notif(NotifAction::ToggleDnd))
            .unwrap();
        store.apply_patches(dnd);
        assert_eq!(store.get("notif.dnd"), Some(StateValue::Bool(true)));

        // Clear all
        let clear = notif
            .handle_action(&ActionCommand::Notif(NotifAction::ClearAll))
            .unwrap();
        store.apply_patches(clear);
        assert_eq!(store.get("notif.unread_count"), Some(StateValue::Int(0)));
        assert_eq!(store.get("notif.list.len"), Some(StateValue::Int(0)));
    }

    #[test]
    fn test_system_provider_manager_end_to_end() {
        let mut store = StateStore::new();
        let mut mgr = SystemProviderManager::new();
        mgr.audio.external_sync_enabled = false;

        let initial_paths = mgr.init_store(&mut store);
        assert!(!initial_paths.is_empty());

        assert!(store.get("sys.cpu.usage_pct").is_some());
        assert!(store.get("sys.ram.percent").is_some());
        assert!(store.get("sys.battery.capacity").is_some());
        assert!(store.get("sys.clock.time").is_some());
        assert!(store.get("sys.audio.percent").is_some());
        assert!(store.get("sys.network.kind").is_some());
        assert!(store.get("apps.all").is_some());
        assert!(store.get("notif.unread_count").is_some());

        let cmd = ActionCommand::parse_dsl("sys:set-volume(42)").unwrap();
        mgr.dispatch_action(&cmd, &mut store).unwrap();
        assert_eq!(store.get("sys.audio.percent"), Some(StateValue::Int(42)));

        let patches = mgr.push_notification("System", "Welcome", "Lumina WM ready", "normal");
        store.apply_patches(patches);
        assert_eq!(store.get("notif.unread_count"), Some(StateValue::Int(1)));
    }
}
