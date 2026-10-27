//! NVIDIA GPUs through NVML: per-device utilization, memory, power, temperature, clock and per-process GPU memory.

use std::collections::HashMap;
use std::thread;

use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::enums::device::UsedGpuMemory;
use nvml_wrapper::Nvml;

use crate::config::SharedConfig;
use crate::snapshot::{Gpu, Shared};

pub fn spawn(state: Shared, cfg: SharedConfig) {
    thread::Builder::new()
        .name("hw".into())
        .spawn(move || {
            let nvml = match Nvml::init() {
                Ok(n) => n,
                Err(e) => {
                    state.write().unwrap().errors.push(format!("hw: NVML unavailable: {e}"));
                    return;
                }
            };
            loop {
                let count = nvml.device_count().unwrap_or(0);
                let mut gpus = Vec::with_capacity(count as usize);
                let mut proc_mem: HashMap<u32, u64> = HashMap::new();
                for i in 0..count {
                    let Ok(d) = nvml.device_by_index(i) else { continue };
                    let mem = d.memory_info().ok();
                    gpus.push(Gpu {
                        name: d.name().unwrap_or_else(|_| format!("GPU {i}")),
                        utilization: d.utilization_rates().ok().map(|u| u.gpu as f64),
                        mem_used: mem.as_ref().map(|m| m.used),
                        mem_total: mem.as_ref().map(|m| m.total),
                        power_w: d.power_usage().ok().map(|mw| mw as f64 / 1000.0),
                        power_limit_w: d.enforced_power_limit().ok().map(|mw| mw as f64 / 1000.0),
                        temp_c: d.temperature(TemperatureSensor::Gpu).ok().map(|t| t as f64),
                        freq_mhz: d.clock_info(Clock::Graphics).ok(),
                    });
                    for p in d.running_compute_processes().unwrap_or_default() {
                        if let UsedGpuMemory::Used(b) = p.used_gpu_memory {
                            *proc_mem.entry(p.pid).or_default() += b;
                        }
                    }
                }
                let mut snap = state.write().unwrap();
                snap.gpu = gpus;
                for p in snap.top_processes.iter_mut().chain(snap.process.iter_mut()) {
                    p.gpu_mem = proc_mem.get(&p.pid).copied();
                }
                drop(snap);
                thread::sleep(cfg.interval());
            }
        })
        .expect("spawn hw thread");
}
