//! Output-only transition smoothing. No source edits, lookahead, or delay line.
//! Weights are allocated/calculated before the callback. Steady state and OFF
//! return the input unchanged; a transition blends the last output into new audio.
pub const RAMP_MS: f64 = 2.0;

pub struct Declick {
    enabled: bool,
    weights: Box<[f64]>,
    cursor: usize,
    anchor: [f64; 2],
    last: [f64; 2],
    available: bool,
}

impl Declick {
    pub fn new(rate: u32, milliseconds: f64) -> Self {
        let frames = ((rate as f64 * milliseconds / 1000.0).round() as usize).max(2);
        let weights = (0..frames)
            .map(|n| (1.0 - (std::f64::consts::PI * n as f64 / (frames - 1) as f64).cos()) * 0.5)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            enabled: false,
            cursor: frames,
            weights,
            anchor: [0.0; 2],
            last: [0.0; 2],
            available: false,
        }
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.cursor = self.weights.len();
        }
    }
    pub fn transition(&mut self) {
        if self.enabled {
            self.anchor = self.last;
            self.cursor = 0;
        }
    }
    pub fn active(&self) -> bool {
        self.enabled && self.cursor < self.weights.len()
    }
    pub fn process(&mut self, input: Option<[f64; 2]>) -> [f64; 2] {
        let available = input.is_some();
        if available != self.available {
            self.transition();
        }
        self.available = available;
        let mut output = input.unwrap_or([0.0; 2]);
        if self.active() {
            let weight = self.weights[self.cursor];
            for (out, anchor) in output.iter_mut().zip(self.anchor) {
                *out = anchor * (1.0 - weight) + *out * weight;
            }
            self.cursor += 1;
        }
        self.last = output;
        output
    }
}
