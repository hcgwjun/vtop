//! Apple Silicon power, frequency and temperature through the macmon crate (IOReport + SMC, no sudo).

#[cfg(test)]
mod tests {
    #[test]
    fn macmon_samples_once() {
        let mut sampler = macmon::Sampler::new().expect("IOReport/SMC available");
        let m = sampler.get_metrics(200).expect("sample");
        assert!(m.cpu_power >= 0.0 && m.gpu_power >= 0.0);
    }
}
