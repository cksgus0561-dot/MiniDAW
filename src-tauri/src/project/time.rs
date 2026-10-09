//! Exact editing arithmetic; output-frame quantization happens only in plan compilation.
use super::schema::{invalid, MusicalTime, Position, Signed, MAX_TIME_DENOMINATOR};
use crate::error::AppResult;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Time {
    pub n: i128,
    pub d: i128,
}
impl Time {
    pub fn new(n: i128, d: i128) -> Self {
        let (mut a, mut b) = (n.abs(), d);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let g = a.max(1);
        Self { n: n / g, d: d / g }
    }
    pub fn frames(n: u64, rate: u32) -> Self {
        Self::new(n as i128, rate as i128)
    }
    pub fn seconds(self) -> f64 {
        self.n as f64 / self.d as f64
    }
    pub fn plus(self, b: Self) -> Self {
        Self::new(self.n * b.d + b.n * self.d, self.d * b.d)
    }
    pub fn minus(self, b: Self) -> Self {
        Self::new(self.n * b.d - b.n * self.d, self.d * b.d)
    }
    pub fn cmp_time(self, b: Self) -> std::cmp::Ordering {
        (self.n * b.d).cmp(&(b.n * self.d))
    }
    pub fn frame(self, rate: u32) -> i128 {
        let n = self.n * rate as i128;
        (n + self.d / 2).div_euclid(self.d)
    }
    pub fn ceil_frame(self, rate: u32) -> i128 {
        let n = self.n * rate as i128;
        -(-n).div_euclid(self.d)
    }
    pub fn position(self) -> AppResult<Position> {
        if self.d <= 0 || self.d > MAX_TIME_DENOMINATOR as i128 {
            return Err(invalid("timeline 분모 범위 초과"));
        }
        Ok(Position::Seconds {
            numerator: Signed(
                i64::try_from(self.n).map_err(|_| invalid("timeline 정밀도 범위 초과"))?,
            ),
            denominator: self.d as u64,
        })
    }
}
pub fn time(p: &Position, m: &MusicalTime) -> Time {
    match p {
        Position::Seconds {
            numerator,
            denominator,
        } => Time::new(numerator.0 as i128, *denominator as i128),
        Position::Ticks { ticks } => {
            // Fixed decimal BPM has an exact rational clock. Rounding to ns
            // before ceil_frame could add a sample at exact beat boundaries
            // (e.g. 2 beats at 180 BPM / 48 kHz became 32001, not 32000).
            let bpm = m.tempo_map[0].bpm;
            let micros = (bpm * 1_000_000.).round();
            if (micros / 1_000_000. - bpm).abs() < 1e-12 && m.tempo_map.iter().all(|p| p.bpm == bpm)
            {
                return Time::new(
                    ticks.0 as i128 * 60_000_000,
                    micros as i128 * m.ticks_per_quarter as i128,
                );
            }
            let mut seconds = 0.0;
            let mut previous = 0;
            let mut bpm = m.tempo_map[0].bpm;
            for point in m.tempo_map.iter().skip(1) {
                if point.tick.0 >= ticks.0 {
                    break;
                }
                seconds +=
                    (point.tick.0 - previous) as f64 * 60.0 / (bpm * m.ticks_per_quarter as f64);
                previous = point.tick.0;
                bpm = point.bpm;
            }
            seconds += (ticks.0 - previous) as f64 * 60.0 / (bpm * m.ticks_per_quarter as f64);
            Time::new((seconds * 1_000_000_000.0).round() as i128, 1_000_000_000)
        }
    }
}
