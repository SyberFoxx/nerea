//! The machine's vitals: what runs, what it eats, what broke.
//!
//! Everything here reads. Nothing starts, stops, signals or changes a process,
//! and no command is assembled from text a model wrote: the binaries and their
//! arguments are constants.
//!
//! `/proc` is read directly instead of shelling out to `ps`: no column parsing,
//! no locale, no subprocess per question, and the numbers are the kernel's own.

use std::path::PathBuf;

/// Kernel clock ticks per second. 100 on every Linux this runs on; if that ever
/// changes, the CPU column is the only thing affected.
const TICKS_PER_SECOND: f64 = 100.0;
/// Page size for the RSS field of `/proc/<pid>/stat`.
const PAGE_KB: u64 = 4;

/// One running process, as the kernel describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub state: char,
    /// Lifetime average: total CPU time over total uptime.
    pub cpu: f64,
    pub rss_kb: u64,
    pub cmd: String,
}

/// Which column sorts the listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Memory,
    Cpu,
    Pid,
}

impl Sort {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "cpu" => Sort::Cpu,
            "pid" => Sort::Pid,
            _ => Sort::Memory,
        }
    }
}

/// Every process that is visible, sorted and cut to `limit`.
pub fn processes(limit: usize, sort: Sort) -> Result<String, String> {
    let uptime = uptime_seconds().unwrap_or(0.0);
    let mut found: Vec<Process> = std::fs::read_dir("/proc")
        .map_err(|err| format!("no puedo leer /proc: {err}"))?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()))
        .filter_map(|pid| read_process(pid, uptime))
        .collect();

    if found.is_empty() {
        return Err("no pude leer ningún proceso".to_owned());
    }

    match sort {
        Sort::Memory => found.sort_by_key(|process| std::cmp::Reverse(process.rss_kb)),
        Sort::Cpu => found.sort_by(|left, right| {
            right.cpu.partial_cmp(&left.cpu).unwrap_or(std::cmp::Ordering::Equal)
        }),
        Sort::Pid => found.sort_by_key(|process| process.pid),
    }

    let total = found.len();
    found.truncate(limit.clamp(1, 5_000));

    let mut out = format!(
        "{total} procesos visibles, ordenados por {}\n{:>7}  {:>6}  {:>9}  {:<6}  {}\n",
        match sort {
            Sort::Memory => "memoria",
            Sort::Cpu => "CPU",
            Sort::Pid => "PID",
        },
        "PID",
        "CPU",
        "RAM",
        "ESTADO",
        "PROCESO"
    );

    for process in &found {
        out.push_str(&format!(
            "{:>7}  {:>5.1}%  {:>6} MB  {:<6}  {}{}\n",
            process.pid,
            process.cpu,
            process.rss_kb / 1024,
            process.state,
            process.name,
            if process.cmd.is_empty() || process.cmd == process.name {
                String::new()
            } else {
                format!("  — {}", process.cmd)
            }
        ));
    }

    if total > found.len() {
        out.push_str(&format!(
            "\n[mostrando {} de {total}: pedime más con `limit`]",
            found.len()
        ));
    }

    Ok(out)
}

fn read_process(pid: u32, uptime: f64) -> Option<Process> {
    let base = PathBuf::from("/proc").join(pid.to_string());
    let stat = std::fs::read_to_string(base.join("stat")).ok()?;
    let (name, fields) = split_stat(&stat)?;

    let state = fields.first()?.chars().next().unwrap_or('?');
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    let rss_pages: u64 = fields.get(21)?.parse().ok()?;

    let cpu = if uptime > 0.0 {
        (utime + stime) / TICKS_PER_SECOND / uptime * 100.0
    } else {
        0.0
    };

    let cmd = std::fs::read(base.join("cmdline"))
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .split('\0')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    Some(Process {
        pid,
        name,
        state,
        cpu: (cpu * 10.0).round() / 10.0,
        rss_kb: rss_pages.saturating_mul(PAGE_KB),
        cmd: clip_text(&cmd, 70),
    })
}

/// `pid (comm) state …`: the name may hold spaces and parentheses, so it is
/// delimited by the first `(` and the **last** `)`.
fn split_stat(line: &str) -> Option<(String, Vec<&str>)> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let name = line.get(open + 1..close)?.to_owned();
    let rest = line.get(close + 1..)?.trim();
    Some((name, rest.split_whitespace().collect()))
}

fn uptime_seconds() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    text.split_whitespace().next()?.parse().ok()
}

/// Memory, swap, load, uptime and the disks worth watching.
pub fn vitals() -> Result<String, String> {
    let meminfo = std::fs::read_to_string("/proc/meminfo")
        .map_err(|err| format!("no puedo leer la memoria: {err}"))?;
    let memory = parse_meminfo(&meminfo);

    let load = std::fs::read_to_string("/proc/loadavg")
        .map(|text| parse_loadavg(&text))
        .unwrap_or_default();

    let uptime = uptime_seconds().unwrap_or(0.0);
    let cores = std::thread::available_parallelism().map(|count| count.get()).unwrap_or(0);

    let mut out = String::new();
    out.push_str(&format!(
        "memoria: {} usados de {} ({} libre), swap {} de {} usada\n",
        human_kb(memory.used()),
        human_kb(memory.total),
        human_kb(memory.available),
        human_kb(memory.swap_total.saturating_sub(memory.swap_free)),
        human_kb(memory.swap_total)
    ));
    out.push_str(&format!(
        "carga: {:.2} / {:.2} / {:.2} sobre {cores} núcleos\n",
        load.0, load.1, load.2
    ));
    out.push_str(&format!("encendida desde hace {}\n", human_uptime(uptime)));

    let disks = std::process::Command::new("df")
        .args(["-B1", "--output=target,size,used,avail", "-x", "tmpfs", "-x", "devtmpfs"])
        .output();

    if let Ok(output) = disks
        && output.status.success()
    {
        out.push_str("\ndiscos:\n");
        for line in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
            let columns: Vec<&str> = line.split_whitespace().collect();
            if columns.len() < 4 {
                continue;
            }
            let (Ok(size), Ok(used)) = (columns[1].parse::<u64>(), columns[2].parse::<u64>()) else {
                continue;
            };
            if size == 0 {
                continue;
            }
            out.push_str(&format!(
                "  {:<24} {} de {} ({}%)\n",
                columns[0],
                human_bytes(used),
                human_bytes(size),
                used * 100 / size
            ));
        }
    }

    Ok(out)
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl Memory {
    fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
}

pub fn parse_meminfo(text: &str) -> Memory {
    let mut memory = Memory::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let Some(number) = value.split_whitespace().next().and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        match key.trim() {
            "MemTotal" => memory.total = number,
            "MemAvailable" => memory.available = number,
            "SwapTotal" => memory.swap_total = number,
            "SwapFree" => memory.swap_free = number,
            _ => {}
        }
    }
    memory
}

pub fn parse_loadavg(text: &str) -> (f64, f64, f64) {
    let mut parts = text.split_whitespace();
    let one = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
    let five = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
    let fifteen = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
    (one, five, fifteen)
}

/// Units that failed, which is the question "¿qué se cayó?" actually asks.
pub fn services() -> Result<String, String> {
    let output = std::process::Command::new("systemctl")
        .args(["--failed", "--no-legend", "--plain", "--no-pager"])
        .output()
        .map_err(|err| format!("no puedo preguntar por los servicios: {err}"))?;

    let text = String::from_utf8_lossy(&output.stdout);
    let units: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();

    if units.is_empty() {
        return Ok("No hay servicios fallidos: todas las unidades están limpias.".to_owned());
    }

    Ok(format!("{} unidades fallidas:\n{}", units.len(), units.join("\n")))
}

/// Recent errors from the journal. The priority and the count are clamped: a
/// journal read is a window, not a dump.
pub fn logs(lines: usize, priority: &str) -> Result<String, String> {
    let lines = lines.clamp(1, 80);
    let priority = match priority.trim().to_lowercase().as_str() {
        "warn" | "warning" => "warning",
        "crit" => "crit",
        "info" => "info",
        _ => "err",
    };

    let output = std::process::Command::new("journalctl")
        .args(["-p", priority, "-n", &lines.to_string(), "--no-pager", "-o", "short-iso"])
        .output()
        .map_err(|err| format!("no puedo leer el journal: {err}"))?;

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "el journal no respondió: {}",
            clip_text(detail.trim(), 200)
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return Ok(format!("Sin entradas de prioridad «{priority}» en el journal."));
    }

    Ok(text.to_owned())
}

fn clip_text(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}…")
}

pub fn human_uptime(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let minutes = (total % 3_600) / 60;

    match (days, hours) {
        (0, 0) => format!("{minutes} minutos"),
        (0, _) => format!("{hours} h {minutes} min"),
        (1, _) => format!("1 día y {hours} h"),
        _ => format!("{days} días y {hours} h"),
    }
}

fn human_kb(kilobytes: u64) -> String {
    human_bytes(kilobytes.saturating_mul(1024))
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Kept for the tests: the process table of a synthetic `/proc`.
#[cfg(test)]
fn read_from(base: &std::path::Path, pid: u32) -> Option<(String, Vec<String>)> {
    let stat = std::fs::read_to_string(base.join(pid.to_string()).join("stat")).ok()?;
    let (name, fields) = split_stat(&stat)?;
    Some((name, fields.iter().map(|field| (*field).to_owned()).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_name_survives_spaces_and_parentheses() {
        let line = "4242 (Web Content (nerea)) S 1 4242 4242 0 -1 4194304 100 0 0 0 5 7 0 0 20 0 1 0 1000 9000 300 0 0 0";
        let (name, fields) = split_stat(line).expect("a process");
        assert_eq!(name, "Web Content (nerea)");
        assert_eq!(fields[0], "S");
        assert_eq!(fields[11], "5", "utime");
        assert_eq!(fields[12], "7", "stime");
        assert_eq!(fields[21], "300", "rss in pages");
    }

    #[test]
    fn memory_reads_the_four_numbers_it_needs() {
        let text = "MemTotal:       32768000 kB\nMemFree:  1000 kB\nMemAvailable:   16384000 kB\n\
SwapTotal:       8388608 kB\nSwapFree:        4194304 kB\n";
        let memory = parse_meminfo(text);

        assert_eq!(memory.total, 32_768_000);
        assert_eq!(memory.available, 16_384_000);
        assert_eq!(memory.swap_total, 8_388_608);
        assert_eq!(memory.swap_free, 4_194_304);
        assert_eq!(memory.used(), 16_384_000);
    }

    #[test]
    fn load_averages_are_three_numbers_in_order() {
        let (one, five, fifteen) = parse_loadavg("1.20 0.98 0.85 2/512 4242");
        assert!((one - 1.20).abs() < 0.001);
        assert!((five - 0.98).abs() < 0.001);
        assert!((fifteen - 0.85).abs() < 0.001);
    }

    #[test]
    fn a_synthetic_process_tree_is_read_without_ps() {
        let dir = std::env::temp_dir().join(format!("nerea-proc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("77")).expect("pid dir");
        std::fs::write(
            dir.join("77/stat"),
            "77 (nerea-test) S 1 77 77 0 -1 0 0 0 0 0 10 5 0 0 20 0 1 0 100 5000 250 0",
        )
        .expect("stat");
        std::fs::write(dir.join("77/cmdline"), b"nerea\x00--test\x00").expect("cmdline");

        let (name, fields) = read_from(&dir, 77).expect("process");
        assert_eq!(name, "nerea-test");
        assert_eq!(fields[21], "250");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_and_ages_read_like_someone_talking() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(human_uptime(90.0), "1 minutos");
        assert_eq!(human_uptime(3_600.0), "1 h 0 min");
        assert_eq!(human_uptime(90_000.0), "1 día y 1 h");
        assert_eq!(human_uptime(200_000.0), "2 días y 7 h");
    }

    #[test]
    fn a_long_command_line_is_cut_not_dumped() {
        let long = "x".repeat(500);
        let cut = clip_text(&long, 70);
        assert!(cut.chars().count() <= 71, "{}", cut.chars().count());
        assert!(cut.ends_with('…'));
    }
}
