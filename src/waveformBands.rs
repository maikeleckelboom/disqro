use kira::Frame;

/// Three descriptive bands. Their powers do not reconstruct the input.
/// All four sections keep independent state for each channel.
pub(super) struct Bands {
    low: Biquad,
    middle_high_pass: Biquad,
    middle_low_pass: Biquad,
    high: Biquad,
}

impl Bands {
    pub(super) fn new(rate: u32) -> Self {
        Self {
            low: Biquad::butterworth(rate, 200., false),
            middle_high_pass: Biquad::butterworth(rate, 200., true),
            middle_low_pass: Biquad::butterworth(rate, 2500., false),
            high: Biquad::butterworth(rate, 2500., true),
        }
    }

    pub(super) fn power(&mut self, frame: Frame) -> [f64; 3] {
        let input = [f64::from(frame.left), f64::from(frame.right)];
        let low = self.low.sample(input);
        let middle = self
            .middle_low_pass
            .sample(self.middle_high_pass.sample(input));
        let high = self.high.sample(input);
        [low, middle, high].map(|[left, right]| (left * left + right * right) * 0.5)
    }
}

struct Biquad {
    feed: [f64; 3],
    feedback: [f64; 2],
    previous: [[f64; 2]; 2],
}

impl Biquad {
    fn butterworth(rate: u32, hz: f64, high_pass: bool) -> Self {
        let k = (std::f64::consts::PI * hz / f64::from(rate)).tan();
        let squared = k * k;
        let damped = std::f64::consts::SQRT_2 * k;
        let scale = 1. / (1. + damped + squared);
        let feed = if high_pass {
            [scale, -2. * scale, scale]
        } else {
            let value = squared * scale;
            [value, 2. * value, value]
        };
        Self {
            feed,
            feedback: [2. * (squared - 1.) * scale, (1. - damped + squared) * scale],
            previous: [[0.; 2]; 2],
        }
    }

    fn sample(&mut self, [left, right]: [f64; 2]) -> [f64; 2] {
        let [b0, b1, b2] = self.feed;
        let [a1, a2] = self.feedback;
        let [[l1, r1], [l2, r2]] = self.previous;
        let l = b0 * left + l1;
        let r = b0 * right + r1;
        self.previous = [
            [b1 * left - a1 * l + l2, b1 * right - a1 * r + r2],
            [b2 * left - a2 * l, b2 * right - a2 * r],
        ];
        [l, r]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_tones_keep_the_same_band_dominance_under_polarity_reversal() {
        for rate in [8000, 44100, 48000, 96000, 384000] {
            for (dominant, hz) in [64., 800., 3300.].into_iter().enumerate() {
                let (mut in_phase, mut opposite_phase) = (Bands::new(rate), Bands::new(rate));
                let mut energy = [0.; 3];
                for index in 0..rate / 10 {
                    let t = f64::from(index) / f64::from(rate);
                    let value = (std::f64::consts::TAU * hz * t).sin() as f32 * 0.5;
                    let even = in_phase.power(Frame::new(value, value));
                    assert_eq!(even, opposite_phase.power(Frame::new(value, -value)));
                    for (sum, power) in energy.iter_mut().zip(even) {
                        *sum += power;
                    }
                }
                for (band, power) in energy.iter().enumerate() {
                    if band != dominant {
                        assert!(
                            energy[dominant] > power * 2.,
                            "{rate} Hz, {hz} Hz: {energy:?}"
                        );
                    }
                }
            }
        }
    }
}
