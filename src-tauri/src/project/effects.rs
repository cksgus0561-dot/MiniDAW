//! Persisted insert settings only. DSP buffers/meters never enter the document.
use super::{edit::EditRequest, schema::*};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Band {
    pub frequency: f64,
    pub gain_db: f64,
    pub q: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Processor {
    External { plugin: crate::plugins::Selection },
    Eq {
        bands: [Band; 3],
    },
    Compressor {
        threshold_db: f64,
        ratio: f64,
        attack_ms: f64,
        release_ms: f64,
        makeup_db: f64,
    },
    Limiter {
        ceiling_db: f64,
        input_db: f64,
    },
    Reverb {
        decay: f64,
        wet: f64,
    },
    Delay {
        time_ms: f64,
        feedback: f64,
        wet: f64,
        sync_beats: Option<f64>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Effect {
    pub effect_id: String,
    pub enabled: bool,
    #[serde(flatten)]
    pub processor: Processor,
}
impl Effect {
    pub fn new(kind: &str) -> AppResult<Self> {
        let processor = match kind {
            "eq" => Processor::Eq {
                bands: [120., 1000., 8000.].map(|frequency| Band {
                    frequency,
                    gain_db: 0.,
                    q: 0.707,
                }),
            },
            "compressor" => Processor::Compressor {
                threshold_db: -18.,
                ratio: 4.,
                attack_ms: 10.,
                release_ms: 100.,
                makeup_db: 0.,
            },
            "limiter" => Processor::Limiter {
                ceiling_db: -1.,
                input_db: 0.,
            },
            "reverb" => Processor::Reverb {
                decay: 1.8,
                wet: 0.2,
            },
            "delay" => Processor::Delay {
                time_ms: 250.,
                feedback: 0.3,
                wet: 0.2,
                sync_beats: None,
            },
            _ => return Err(invalid("Effect 종류")),
        };
        Ok(Self {
            effect_id: id(),
            enabled: true,
            processor,
        })
    }
    pub fn validate(&self) -> AppResult<()> {
        fn range(v: f64, a: f64, b: f64) -> bool {
            v.is_finite() && (a..=b).contains(&v)
        }
        let valid = !self.effect_id.is_empty()
            && self.effect_id.len() <= 128
            && match &self.processor {
                Processor::External { plugin } => plugin.validate().is_ok() && !plugin.descriptor.instrument,
                Processor::Eq { bands } => bands.iter().all(|b| {
                    range(b.frequency, 20., 20000.)
                        && range(b.gain_db, -24., 24.)
                        && range(b.q, 0.1, 20.)
                }),
                Processor::Compressor {
                    threshold_db,
                    ratio,
                    attack_ms,
                    release_ms,
                    makeup_db,
                } => {
                    range(*threshold_db, -60., 0.)
                        && range(*ratio, 1., 20.)
                        && range(*attack_ms, 0.1, 200.)
                        && range(*release_ms, 10., 2000.)
                        && range(*makeup_db, -12., 24.)
                }
                Processor::Limiter {
                    ceiling_db,
                    input_db,
                } => range(*ceiling_db, -24., 0.) && range(*input_db, -24., 24.),
                Processor::Reverb { decay, wet } => range(*decay, 0.2, 8.) && range(*wet, 0., 1.),
                Processor::Delay {
                    time_ms,
                    feedback,
                    wet,
                    sync_beats,
                } => {
                    range(*time_ms, 1., 2000.)
                        && range(*feedback, 0., 0.85)
                        && range(*wet, 0., 1.)
                        && sync_beats.is_none_or(|b| [0.125, 0.25, 0.5, 1., 2., 4.].contains(&b))
                }
            };
        if valid {
            Ok(())
        } else {
            Err(invalid("Effect parameter 범위"))
        }
    }
}
pub fn validate(p: &Project) -> AppResult<()> {
    let mut ids = std::collections::HashSet::new();
    let mut active = 0;
    let mut delays = 0;
    let mut delay_seconds = 0.;
    for chain in std::iter::once(&p.master.inserts).chain(p.tracks.iter().map(|t| &t.inserts)) {
        if chain.len() > 8 {
            return Err(invalid("Channel당 Insert는 최대 8개입니다."));
        }
        for e in chain {
            e.validate()?;
            if !ids.insert(&e.effect_id) {
                return Err(invalid("중복 Effect ID"));
            }
            let automated = p.automation.iter().any(|c| {
                c.read
                    && !c.lanes.is_empty()
                    && (if c.track_id == "master" {
                        std::ptr::eq(chain, &p.master.inserts)
                    } else {
                        p.tracks
                            .iter()
                            .any(|t| t.track_id == c.track_id && std::ptr::eq(chain, &t.inserts))
                    })
            });
            if e.enabled || automated {
                if let Processor::Delay {
                    time_ms,
                    sync_beats,
                    ..
                } = e.processor
                {
                    delay_seconds += if automated {
                        2f64.max(240. / p.musical_time.tempo_map[0].bpm)
                    } else {
                        sync_beats.map_or(time_ms / 1000., |beats| {
                            beats * 60. / p.musical_time.tempo_map.first().map_or(120., |t| t.bpm)
                        })
                    };
                }
                active += 1;
                if matches!(
                    e.processor,
                    Processor::Delay { .. } | Processor::Reverb { .. }
                ) {
                    delays += 1;
                }
            }
        }
    }
    // Bound DSP/memory independently of the existing Track count. The summed
    // delay-line duration cap also applies to Tempo-synced delays.
    if active > 64 || delays > 16 || !delay_seconds.is_finite() || delay_seconds > 32. {
        return Err(invalid(
            "활성 Insert 한도: 전체 64개, Reverb/Delay 합계 16개, Delay 시간 합계 32초",
        ));
    }
    Ok(())
}
pub fn apply(p: &Project, r: &EditRequest) -> AppResult<Project> {
    let mut next = p.clone();
    let chain = if r.track_ids.is_empty() {
        &mut next.master.inserts
    } else {
        if r.track_ids.len() != 1 {
            return Err(invalid("Effect Track 선택"));
        }
        &mut next
            .tracks
            .iter_mut()
            .find(|t| t.track_id == r.track_ids[0])
            .ok_or_else(|| invalid("Effect Track 없음"))?
            .inserts
    };
    if r.command == "effect.add" {
        if let Some(plugin)=&r.plugin { plugin.validate()?; chain.push(Effect{effect_id:id(),enabled:true,processor:Processor::External{plugin:plugin.clone()}}); } else { chain.push(Effect::new(
            r.effect_kind
                .as_deref()
                .ok_or_else(|| invalid("Effect 종류"))?,
        )?); }
    } else {
        let i = chain
            .iter()
            .position(|e| Some(&e.effect_id) == r.effect_id.as_ref())
            .ok_or_else(|| invalid("Effect가 변경되었습니다."))?;
        match r.command.as_str() {
            "effect.remove" => {
                chain.remove(i);
            }
            "effect.move" => {
                let d = r
                    .direction
                    .filter(|d| matches!(d, -1 | 1))
                    .ok_or_else(|| invalid("Effect 순서"))?;
                let j = (i as isize + d as isize).clamp(0, chain.len() as isize - 1) as usize;
                chain.swap(i, j);
            }
            "effect.set" => {
                let e = r.effect.as_ref().ok_or_else(|| invalid("Effect 설정"))?;
                if e.effect_id != chain[i].effect_id
                    || std::mem::discriminant(&e.processor)
                        != std::mem::discriminant(&chain[i].processor)
                {
                    return Err(invalid("Effect ID/종류"));
                }
                chain[i] = e.clone();
            }
            _ => return Err(invalid("Effect command")),
        }
    }
    super::automation::prune(&mut next);
    next.validate()?;
    Ok(next)
}
