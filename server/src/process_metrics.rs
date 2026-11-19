//! Standard Prometheus process metrics (`process_cpu_seconds_total`,
//! `process_resident_memory_bytes`, …), read from `/proc/self` when `/metrics`
//! is scraped, and never otherwise.
//!
//! Container platforms usually export CPU and memory per container (cAdvisor,
//! `docker stats`), but a plain Docker or systemd deployment scraping only
//! Checkgate had no CPU or memory signal at all. These are the names every
//! Prometheus client library uses, so existing dashboards and alerts work
//! unchanged. Linux only: on other systems no process metrics are emitted.

use prometheus_client::collector::Collector;
use prometheus_client::encoding::{DescriptorEncoder, EncodeMetric};
use prometheus_client::metrics::counter::ConstCounter;
use prometheus_client::metrics::gauge::ConstGauge;
use prometheus_client::registry::Unit;

/// Clock ticks per second in `/proc/<pid>/stat` (`USER_HZ`). It is 100 on
/// every Linux architecture: the kernel scales to it for userspace, so it is
/// part of the ABI and does not follow the kernel's internal `HZ`.
#[cfg_attr(
    not(target_os = "linux"),
    allow(
        dead_code,
        reason = "read by `sample()`, which is Linux only; still unit-tested everywhere"
    )
)]
const USER_HZ: f64 = 100.0;

#[derive(Debug, Default)]
pub struct ProcessCollector;

#[derive(Debug, Default, PartialEq)]
struct Sample {
    cpu_seconds: f64,
    start_time_seconds: Option<f64>,
    virtual_bytes: Option<i64>,
    resident_bytes: Option<i64>,
    threads: Option<i64>,
    open_fds: Option<i64>,
}

/// Fields of `/proc/self/stat` after the command name. The name is in
/// parentheses and may itself contain spaces and `)`, so parsing starts after
/// the *last* `)`. Returns (utime + stime in seconds, start time in ticks since
/// boot, virtual size in bytes).
#[cfg_attr(
    not(target_os = "linux"),
    allow(
        dead_code,
        reason = "read by `sample()`, which is Linux only; still unit-tested everywhere"
    )
)]
fn parse_stat(stat: &str) -> Option<(f64, u64, i64)> {
    let (_, after_comm) = stat.rsplit_once(')')?;
    // `fields[0]` is field 3 (state); field N is at index N - 3.
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let num = |n: usize| -> Option<u64> { fields.get(n.checked_sub(3)?)?.parse().ok() };
    let utime = num(14)?;
    let stime = num(15)?;
    let start_ticks = num(22)?;
    let vsize = i64::try_from(num(23)?).ok()?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "CPU ticks of one process stay far below 2^53"
    )]
    let cpu_seconds = utime.saturating_add(stime) as f64 / USER_HZ;
    Some((cpu_seconds, start_ticks, vsize))
}

/// `VmRSS` (bytes) and `Threads` from `/proc/self/status`.
#[cfg_attr(
    not(target_os = "linux"),
    allow(
        dead_code,
        reason = "read by `sample()`, which is Linux only; still unit-tested everywhere"
    )
)]
fn parse_status(status: &str) -> (Option<i64>, Option<i64>) {
    let field = |name: &str| -> Option<i64> {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    };
    let rss = field("VmRSS:").and_then(|kib| kib.checked_mul(1024));
    (rss, field("Threads:"))
}

/// System boot time (`btime`, seconds since the epoch) from `/proc/stat`.
#[cfg_attr(
    not(target_os = "linux"),
    allow(
        dead_code,
        reason = "read by `sample()`, which is Linux only; still unit-tested everywhere"
    )
)]
fn parse_boot_time(proc_stat: &str) -> Option<u64> {
    proc_stat
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn sample() -> Option<Sample> {
    let (cpu_seconds, start_ticks, virtual_bytes) =
        parse_stat(&std::fs::read_to_string("/proc/self/stat").ok()?)?;
    let (resident_bytes, threads) =
        parse_status(&std::fs::read_to_string("/proc/self/status").unwrap_or_default());
    #[allow(
        clippy::cast_precision_loss,
        reason = "epoch seconds and boot ticks stay far below 2^53"
    )]
    let start_time_seconds = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| parse_boot_time(&s))
        .map(|boot| boot as f64 + start_ticks as f64 / USER_HZ);
    let open_fds = std::fs::read_dir("/proc/self/fd")
        .ok()
        .and_then(|d| i64::try_from(d.count()).ok());
    Some(Sample {
        cpu_seconds,
        start_time_seconds,
        virtual_bytes: Some(virtual_bytes),
        resident_bytes,
        threads,
        open_fds,
    })
}

#[cfg(not(target_os = "linux"))]
fn sample() -> Option<Sample> {
    None
}

impl Collector for ProcessCollector {
    fn encode(&self, mut encoder: DescriptorEncoder<'_>) -> Result<(), std::fmt::Error> {
        let Some(s) = sample() else {
            return Ok(());
        };
        let cpu = ConstCounter::new(s.cpu_seconds);
        cpu.encode(encoder.encode_descriptor(
            "process_cpu",
            "Total user and system CPU time spent",
            Some(&Unit::Seconds),
            cpu.metric_type(),
        )?)?;
        let gauges: [(&str, &str, Option<Unit>, Option<i64>); 4] = [
            (
                "process_resident_memory",
                "Resident memory size",
                Some(Unit::Bytes),
                s.resident_bytes,
            ),
            (
                "process_virtual_memory",
                "Virtual memory size",
                Some(Unit::Bytes),
                s.virtual_bytes,
            ),
            (
                "process_open_fds",
                "Open file descriptors",
                None,
                s.open_fds,
            ),
            ("process_threads", "OS threads", None, s.threads),
        ];
        for (name, help, unit, value) in gauges {
            if let Some(value) = value {
                let gauge = ConstGauge::new(value);
                gauge.encode(encoder.encode_descriptor(
                    name,
                    help,
                    unit.as_ref(),
                    gauge.metric_type(),
                )?)?;
            }
        }
        if let Some(start) = s.start_time_seconds {
            let gauge = ConstGauge::new(start);
            gauge.encode(encoder.encode_descriptor(
                "process_start_time",
                "Start time of the process since the Unix epoch",
                Some(&Unit::Seconds),
                gauge.metric_type(),
            )?)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real /proc/self/stat line, with a command name containing ") " to
    // check that parsing starts after the last parenthesis.
    const STAT: &str = "4242 (checkgate) srv) S 1 4242 4242 0 -1 4194560 2047 0 0 0 \
        1234 566 0 0 20 0 9 0 31337 2203648000 3500 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0";

    #[test]
    fn stat_fields_are_read_after_the_command_name() {
        let (cpu, start, vsize) = parse_stat(STAT).unwrap();
        assert!(
            (cpu - 18.0).abs() < 1e-9,
            "utime 1234 + stime 566 ticks = 18 s"
        );
        assert_eq!(start, 31337);
        assert_eq!(vsize, 2_203_648_000);
    }

    #[test]
    fn truncated_stat_is_rejected() {
        assert_eq!(parse_stat("4242 (checkgate) S 1 2 3"), None);
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn status_and_boot_time() {
        let status = "Name:\tserver\nVmRSS:\t   13340 kB\nThreads:\t9\n";
        assert_eq!(parse_status(status), (Some(13_340 * 1024), Some(9)));
        assert_eq!(parse_status(""), (None, None));
        assert_eq!(
            parse_boot_time("cpu  1 2 3\nbtime 1763452800\nprocesses 9\n"),
            Some(1_763_452_800)
        );
    }

    #[test]
    fn encodes_only_on_linux() {
        let mut registry = prometheus_client::registry::Registry::default();
        registry.register_collector(Box::new(ProcessCollector));
        let mut out = String::new();
        prometheus_client::encoding::text::encode(&mut out, &registry).unwrap();
        if cfg!(target_os = "linux") {
            assert!(out.contains("process_cpu_seconds_total "), "{out}");
            assert!(out.contains("process_resident_memory_bytes "), "{out}");
        } else {
            assert!(!out.contains("process_"), "{out}");
        }
    }
}
