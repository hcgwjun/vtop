//! Apple Silicon power, frequency and temperature through the macmon crate (IOReport + SMC, no sudo).
//! GPU memory is not in IOReport, so it comes from `ioreg`'s IOAccelerator statistics.

use std::process::Command;
use std::thread;

use crate::config::SharedConfig;
use crate::snapshot::{Gpu, Shared};

/// `(in use, allocated)` bytes from `ioreg -r -d 1 -c IOAccelerator`; `None` for any key this chip does not report.
/// Measured with vllm-metal: "Alloc system memory" moves by the model's size (5.1 → 13.3 GB), "In use" does not.
pub fn parse_ioreg(text: &str) -> (Option<u64>, Option<u64>) {
    let field = |key: &str| -> Option<u64> {
        let rest = text.split_once(&format!("\"{key}\"="))?.1;
        rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    };
    (field("In use system memory"), field("Alloc system memory"))
}

fn gpu_memory() -> (Option<u64>, Option<u64>) {
    Command::new("ioreg")
        .args(["-r", "-d", "1", "-c", "IOAccelerator"])
        .output()
        .map(|o| parse_ioreg(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or((None, None))
}

pub fn spawn(state: Shared, cfg: SharedConfig) {
    thread::Builder::new()
        .name("hw".into())
        .spawn(move || {
            let mut sampler = match macmon::Sampler::new() {
                Ok(s) => s,
                Err(e) => {
                    state.write().unwrap().errors.push(format!("hw: {e}"));
                    return;
                }
            };
            let name = sampler.get_soc_info().chip_name.clone();
            let mut mem = (None, None);
            let mut tick = 0u32;
            loop {
                // get_metrics blocks for the interval, so this thread needs no sleep
                let m = match sampler.get_metrics(cfg.interval().as_millis() as u32) {
                    Ok(m) => m,
                    Err(e) => {
                        let mut snap = state.write().unwrap();
                        snap.errors.retain(|x| !x.starts_with("hw:"));
                        snap.errors.push(format!("hw: {e}"));
                        thread::sleep(cfg.interval());
                        continue;
                    }
                };
                // ioreg is a process spawn (~2-3% CPU per second), so refresh it every 5th sample
                if tick % 5 == 0 {
                    mem = gpu_memory();
                }
                tick += 1;
                let (_, mem_used) = mem; // Alloc = what Metal/MLX holds; In use ignores it
                let mem_total = Some(m.memory.ram_total); // unified memory: the GPU can use all of it
                let mut snap = state.write().unwrap();
                snap.gpu = vec![Gpu {
                    name: name.clone(),
                    utilization: Some(m.gpu_active_ratio as f64 * 100.0),
                    mem_used,
                    mem_total,
                    power_w: Some(m.gpu_power as f64),
                    power_limit_w: None,
                    temp_c: m.temp.gpu_temp_avg.map(|t| t as f64),
                    freq_mhz: Some(m.gpu_freq_mhz),
                }];
                snap.system.cpu_power_w = Some(m.cpu_power as f64);
                snap.system.ane_power_w = Some(m.ane_power as f64);
                snap.system.fans = m.fans.iter().map(|f| (f.name.clone(), f.rpm)).collect();
                snap.errors.retain(|x| !x.starts_with("hw:"));
            }
        })
        .expect("spawn hw thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macmon_samples_once() {
        let mut sampler = macmon::Sampler::new().expect("IOReport/SMC available");
        let m = sampler.get_metrics(200).expect("sample");
        assert!(m.cpu_power >= 0.0 && m.gpu_power >= 0.0);
    }

    #[test]
    fn ioreg_parses_two_fields() {
        let text = r#"    "PerformanceStatistics" = {"In use system memory (driver)"=0,"Alloc system memory"=3999416320,"Tiler Utilization %"=2,"In use system memory"=1003257856}"#;
        assert_eq!(parse_ioreg(text), (Some(1003257856), Some(3999416320)));
    }

    #[test]
    fn ioreg_missing_keys_gives_none() {
        assert_eq!(parse_ioreg(""), (None, None));
    }
}
