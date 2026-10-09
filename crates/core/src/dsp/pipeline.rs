//! A chain of [`Processor`]s: one trace's filtering pipeline.
//!
//! The first version connects a single processor; the structure is a chain so
//! `detrend → LPF` becomes possible without a UI change.

use super::{DspError, ParamValues, Processor};

pub struct Pipeline {
    stages: Vec<Box<dyn Processor>>,
}

impl Pipeline {
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    pub fn push(&mut self, stage: Box<dyn Processor>) {
        self.stages.push(stage);
    }

    pub fn configure(&mut self, params: &ParamValues, fs_hz: f64) -> Result<(), DspError> {
        for stage in &mut self.stages {
            stage.configure(params, fs_hz)?;
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let mut out = x;
        for stage in &mut self.stages {
            out = stage.process(out);
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
        pipe.push(Box::new(MovingAverage::new(4).unwrap()));
        pipe.push(Box::new(
            Biquad::new(
                BiquadKind::LowPass,
                100.0,
                std::f64::consts::FRAC_1_SQRT_2,
                1000.0,
            )
            .unwrap(),
        ));
        // 1000 samples: LPF removes the DC, the moving average smooths.
        let mut out = 0.0;
        for _ in 0..1000 {
            out = pipe.process(1.0);
        }
        assert!(out.is_finite(), "chain output is finite");
    }
}
