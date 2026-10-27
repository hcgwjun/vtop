//! CPU, RAM, disk/net rates and the serving process, through sysinfo.

use std::thread;
use std::time::Instant;

use sysinfo::{Disks, Networks, ProcessRefreshKind, ProcessesToUpdate, System as Sys, UpdateKind};

use crate::config::SharedConfig;
use crate::snapshot::{Process, Shared};

pub struct ProcInfo {
    pub pid: u32,
    pub cmd: String,
    pub start_time: u64,
}

/// The vLLM parent: the earliest-started process whose command line mentions vllm.
pub fn find_serving<'a, I: Iterator<Item = &'a ProcInfo>>(procs: I) -> Option<&'a ProcInfo> {
    procs.filter(|p| p.cmd.contains("vllm")).min_by_key(|p| p.start_time)
}

/// cmd is only fetched when asked for, so new processes (a restarted vLLM) need `with_cmd`.
pub fn refresh(sys: &mut Sys) {
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory().with_tasks().with_cmd(UpdateKind::OnlyIfNotSet),
    );
}

/// Every process but vtop itself (its own `--url vllm-host` argument would otherwise match).
pub fn proc_infos(sys: &Sys) -> Vec<ProcInfo> {
    let me = sysinfo::get_current_pid().ok();
    sys.processes()
        .values()
        .filter(|p| Some(p.pid()) != me)
        .map(|p| ProcInfo {
            pid: p.pid().as_u32(),
            cmd: p.cmd().iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" "),
            start_time: p.start_time(),
        })
        .collect()
}

fn to_process(p: &sysinfo::Process) -> Process {
    Process {
        pid: p.pid().as_u32(),
        name: p.name().to_string_lossy().into_owned(),
        uptime_s: p.run_time(),
        cpu_pct: p.cpu_usage() as f64,
        rss: p.memory(),
        threads: p.tasks().map(|t| t.len() as u64),
        gpu_mem: None,
    }
}

pub fn spawn(state: Shared, cfg: SharedConfig) {
    thread::Builder::new()
        .name("system".into())
        .spawn(move || {
            let mut sys = Sys::new();
            let (mut disks, mut nets) = (Disks::new(), Networks::new());
            let mut first = true;
            let mut last = Instant::now();
            loop {
                thread::sleep(cfg.interval());
                refresh(&mut sys);
                disks.refresh(true);
                nets.refresh(true);
                let secs = last.elapsed().as_secs_f64();
                last = Instant::now();

                let (mut rd, mut wr) = (0u64, 0u64);
                for d in disks.list() {
                    let u = d.usage();
                    rd += u.read_bytes;
                    wr += u.written_bytes;
                }
                let (rx, tx) = nets.list().values().fold((0u64, 0u64), |(r, t), n| (r + n.received(), t + n.transmitted()));

                let serving = find_serving(proc_infos(&sys).iter()).map(|i| i.pid);
                let mut top: Vec<Process> = sys.processes().values().map(to_process).collect();
                top.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
                top.truncate(5);

                let mut snap = state.write().unwrap();
                let s = &mut snap.system;
                // sysinfo reports 0% CPU on its first refresh, so the first sample is n/a
                s.cpu_pct = (!first).then(|| sys.global_cpu_usage() as f64);
                s.mem_used = Some(sys.used_memory());
                s.mem_total = Some(sys.total_memory());
                s.swap_used = Some(sys.used_swap());
                if !first && secs > 0.0 {
                    s.disk_read_bps = Some(rd as f64 / secs);
                    s.disk_write_bps = Some(wr as f64 / secs);
                    s.net_rx_bps = Some(rx as f64 / secs);
                    s.net_tx_bps = Some(tx as f64 / secs);
                }
                snap.process = serving.and_then(|pid| sys.process(sysinfo::Pid::from_u32(pid))).map(to_process);
                snap.top_processes = top;
                first = false;
            }
        })
        .expect("spawn system thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_serving_picks_oldest_vllm() {
        let procs = [
            ProcInfo { pid: 1, cmd: "python -m vllm.entrypoints.openai.api_server".into(), start_time: 50 },
            ProcInfo { pid: 2, cmd: "vllm serve x".into(), start_time: 10 },
            ProcInfo { pid: 3, cmd: "bash".into(), start_time: 1 },
        ];
        assert_eq!(find_serving(procs.iter()).map(|p| p.pid), Some(2));
        assert_eq!(find_serving([].iter()).map(|p| p.pid), None);
    }

    #[test]
    fn finds_vllm_process_started_after_vtop() {
        let mut sys = Sys::new();
        refresh(&mut sys);
        let mut child = std::process::Command::new("sh").args(["-c", "sleep 5; echo vtop-cmd-MARKER"]).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        refresh(&mut sys);
        let infos = proc_infos(&sys);
        let found = infos.iter().any(|p| p.pid == child.id() && p.cmd.contains("MARKER"));
        child.kill().ok();
        assert!(found, "cmd of a process started after the first refresh must be populated");
    }
}
