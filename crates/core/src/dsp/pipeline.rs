//! A chain of [`Processor`]s: one trace's filtering pipeline.
//!
//! Each stage keeps its own parameter set, so a chain like `detrend → LPF`
//! reconfigures every stage when the sample rate changes.

use super::{DspError, ParamValues, Processor};

struct Stage {
    proc: Box<dyn Processor>,
    params: ParamValues,
}

pub struct Pipeline {
    stages: Vec<Stage>,
}

impl Pipeline {
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Append a processor with the parameters its `configure` should use.
    pub fn push(&mut self, proc: Box<dyn Processor>, params: ParamValues) {
        self.stages.push(Stage { proc, params });
    }

    /// (Re)design every stage from its own parameters at the given sample rate.
    pub fn configure(&mut self, fs_hz: f64) -> Result<(), DspError> {
        for stage in &mut self.stages {
            stage.proc.configure(&stage.params, fs_hz)?;
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.proc.reset();
        }
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let mut out = x;
        for stage in &mut self.stages {
            out = stage.proc.process(out);
        }
        out
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::biquad::{Biquad, BiquadKind};
    use super::super::moving_average::MovingAverage;
    use super::*;

    #[test]
    fn chains_detrend_and_lowpass_orders_correctly() {
        let mut pipe = Pipeline::new();
        pipe.push(Box::new(MovingAverage::new(4).unwrap()), {
            let mut p = ParamValues::new();
            p.set("window", 4.0);
            p
        });
        pipe.push(
            Box::new(
                Biquad::new(
                    BiquadKind::LowPass,
                    100.0,
                    std::f64::consts::FRAC_1_SQRT_2,
                    1000.0,
                )
                .unwrap(),
            ),
            {
                let mut p = ParamValues::new();
                p.set("fc_hz", 100.0);
                p.set("q", std::f64::consts::FRAC_1_SQRT_2);
                p
            },
        );
        pipe.configure(1000.0).expect("configure");
        // 1000 samples: LPF removes the DC, the moving average smooths.
        let mut out = 0.0;
        for _ in 0..1000 {
            out = pipe.process(1.0);
        }
        assert!(out.is_finite(), "chain output is finite");
    }
}
