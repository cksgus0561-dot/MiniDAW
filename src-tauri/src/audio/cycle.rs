//! Cycle mapping in output frames. The existing read-ahead worker prepares wraps;
//! the callback reads a monotonically increasing stream index, never seeks at a wrap.
use super::{reader::Cancel, streaming::FrameReader};
use crate::{
    error::AppResult,
    project::{
        schema::{invalid, Position, Project},
        time::time,
    },
};

#[derive(Clone, Copy, Debug)]
pub struct CycleFrames {
    pub start: usize,
    pub end: usize,
}
impl CycleFrames {
    pub fn compile(project: &Project, rate: u32) -> AppResult<Option<Self>> {
        let Some(c) = project.cycle.as_ref().filter(|c| c.enabled) else {
            return Ok(None);
        };
        let frame = |ticks| {
            usize::try_from(time(&Position::Ticks { ticks }, &project.musical_time).frame(rate))
                .map_err(|_| invalid("Cycle frame 범위"))
        };
        let result = Self {
            start: frame(c.start_tick)?,
            end: frame(c.end_tick)?,
        };
        if result.start >= result.end {
            return Err(invalid("Cycle은 출력 1 sample 이상이어야 합니다."));
        }
        Ok(Some(result))
    }
    pub fn position(self, frame: usize) -> usize {
        if frame < self.end {
            frame
        } else {
            self.start + (frame - self.end) % (self.end - self.start)
        }
    }
}

pub struct CycleReader<R> {
    reader: R,
    cycle: Option<CycleFrames>,
    position: usize,
    cancel: Cancel,
}
impl<R: FrameReader> CycleReader<R> {
    pub fn new(mut reader: R, cycle: Option<CycleFrames>) -> Self {
        reader.set_cycle(cycle);
        Self {
            reader,
            cycle,
            position: 0,
            cancel: std::sync::Arc::new(|| false),
        }
    }
}
impl<R: FrameReader> FrameReader for CycleReader<R> {
    fn lookahead(&self) -> usize {
        self.reader.lookahead()
    }
    fn seek(&mut self, target: usize, cancel: Cancel) -> AppResult<()> {
        self.cancel = cancel.clone();
        self.position = self.cycle.map_or(target, |c| c.position(target));
        self.reader.seek(self.position, cancel)
    }
    fn read_frame(&mut self) -> AppResult<[f32; 2]> {
        if let Some(c) = self.cycle {
            if self.position == c.end {
                self.position = c.start;
                self.reader.wrap(c.start, self.cancel.clone())?;
            }
        }
        let result = self.reader.read_frame()?;
        self.position += 1;
        Ok(result)
    }
}
