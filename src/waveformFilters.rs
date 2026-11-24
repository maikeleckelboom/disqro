//! Display band descriptors, never used by the audio callback. Independent
//! second-order Butterworth low/high pass sections; the middle is HP then LP.
//! These overlapping descriptive bands are not a reconstruction crossover.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    // Keep independent channels adjacent so LLVM can evaluate the same section
    // for both lanes. Coefficients, f64 operation order and filter state agree
    // bit for bit with the previous channel-at-a-time implementation.
    z: [[f64; 2]; 2],
}
impl Biquad {
    fn new(rate: u32, hz: f64, high: bool) -> Self {
        let k = (std::f64::consts::PI * hz / f64::from(rate)).tan();
        let norm = 1.0 / (1.0 + std::f64::consts::SQRT_2 * k + k * k);
        let b = if high {
            [norm, -2.0 * norm, norm]
        } else {
            [k * k * norm, 2.0 * k * k * norm, k * k * norm]
        };
        Self {
            b,
            a: [
                2.0 * (k * k - 1.0) * norm,
                (1.0 - std::f64::consts::SQRT_2 * k + k * k) * norm,
            ],
            z: [[0.0; 2]; 2],
        }
    }
    fn push(&mut self, x: [f64; 2]) -> [f64; 2] {
        let y = std::array::from_fn(|c| self.b[0] * x[c] + self.z[0][c]);
        self.z[0] = std::array::from_fn(|c| self.b[1] * x[c] - self.a[0] * y[c] + self.z[1][c]);
        self.z[1] = std::array::from_fn(|c| self.b[2] * x[c] - self.a[1] * y[c]);
        y
    }
}

pub(super) struct Bands {
    filters: [Biquad; 4],
    energy: [f64; 3],
    attack: f64,
}
impl Bands {
    pub fn new(rate: u32) -> Self {
        Self {
            filters: [
                Biquad::new(rate, 200.0, false),
                Biquad::new(rate, 200.0, true),
                Biquad::new(rate, 2500.0, false),
                Biquad::new(rate, 2500.0, true),
            ],
            energy: [0.0; 3],
            // Source-time causal exponential energy, 12 ms time constant.
            // Native detail includes 200 ms context, truncated at source/CUE start.
            attack: 1.0 - (-1.0 / (0.012 * f64::from(rate))).exp(),
        }
    }

    pub fn push(&mut self, frame: [f32; 2]) -> ([f64; 3], [f64; 3]) {
        let mut peaks = [0.0f64; 3];
        let mut power = [0.0; 3];
        let x = frame.map(f64::from);
        let middle = self.filters[1].push(x);
        let bands = [
            self.filters[0].push(x),
            self.filters[2].push(middle),
            self.filters[3].push(x),
        ];
        for i in 0..3 {
            peaks[i] = bands[i][0].abs().max(bands[i][1].abs());
            power[i] = bands[i][0] * bands[i][0] * 0.5 + bands[i][1] * bands[i][1] * 0.5;
        }
        for (energy, power) in self.energy.iter_mut().zip(power) {
            *energy += self.attack * (power - *energy);
        }
        (peaks, self.energy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent scalar recurrence of the persisted v1 analysis. Optimizing
    // the lane layout must not invalidate existing overview derivatives.
    #[test]
    fn paired_channels_match_scalar_facts_bit_for_bit() {
        for rate in [8000, 44100, 48000, 96000, 192000] {
            let mut paired = Bands::new(rate);
            let coefficients = paired.filters;
            let mut states = [[[0.0; 2]; 4]; 2];
            let mut energy = [0.0; 3];
            let mut seed = 7_u32;
            for frame_index in 0..50000 {
                let frame = std::array::from_fn(|_| {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    // Silence/tail, impulses, independent channels and over-range.
                    if frame_index % 10000 > 5000 {
                        0.0
                    } else {
                        (seed as i32 as f64 / i32::MAX as f64 * 1.5) as f32
                    }
                });
                let mut peak = [0.0_f64; 3];
                let mut power = [0.0; 3];
                for c in 0..2 {
                    let x = f64::from(frame[c]);
                    let mut section = |i: usize, x: f64| {
                        let f = coefficients[i];
                        let z = &mut states[c][i];
                        let y = f.b[0] * x + z[0];
                        z[0] = f.b[1] * x - f.a[0] * y + z[1];
                        z[1] = f.b[2] * x - f.a[1] * y;
                        y
                    };
                    let middle = section(1, x);
                    let bands = [section(0, x), section(2, middle), section(3, x)];
                    for i in 0..3 {
                        peak[i] = peak[i].max(bands[i].abs());
                        power[i] += bands[i] * bands[i] * 0.5;
                    }
                }
                for i in 0..3 {
                    energy[i] += paired.attack * (power[i] - energy[i]);
                }
                let (actual_peak, actual_energy) = paired.push(frame);
                assert_eq!(
                    actual_peak.map(f64::to_bits),
                    peak.map(f64::to_bits),
                    "rate {rate}, frame {frame_index}"
                );
                assert_eq!(
                    actual_energy.map(f64::to_bits),
                    energy.map(f64::to_bits),
                    "rate {rate}, frame {frame_index}"
                );
            }
        }
    }
}
