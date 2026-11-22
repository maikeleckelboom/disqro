use super::{History, Transforms, measured};
use crate::{
    capture::BLOCK,
    diagnostics, spectral_history as spectral,
    spectrum::{Speed, Stereo},
    stereo_field as field,
};
use async_channel::Sender;
use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering::Relaxed},
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Request {
    Spectral(spectral::Analysis),
    Field(field::Analysis),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    pub speed: Speed,
    pub statistics: bool,
}
pub enum Payload {
    Spectral(spectral::History),
    Field(field::Reading),
}
pub struct VisualFrame {
    pub published: std::time::Instant,
    pub epoch: u64,
    pub continuity: u64,
    pub rate: u32,
    pub data: Payload,
}

enum Analysis {
    Spectral(spectral::Scale, spectral::History),
    Field(field::Field),
}
struct Client {
    id: usize,
    delivery: Delivery,
    tx: Sender<Option<Arc<VisualFrame>>>,
    sent: bool,
    last_end: Option<u64>,
    settled: bool,
}
#[derive(Default)]
struct Group {
    clients: Vec<Client>,
    state: Option<Analysis>,
    /// During activation a reused PCM tail can predate the panel.
    first: u64,
}
#[derive(Default)]
pub(super) struct Streams {
    groups: HashMap<Request, Group>,
}
impl Streams {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
    pub fn hz(&self) -> u32 {
        self.groups
            .values()
            .flat_map(|g| &g.clients)
            .map(|c| c.delivery.speed.hz())
            .max()
            .unwrap_or(0)
    }
    pub fn add(
        &mut self,
        id: usize,
        request: Request,
        delivery: Delivery,
        tx: Sender<Option<Arc<VisualFrame>>>,
        start: u64,
    ) {
        let group = self.groups.entry(request).or_insert_with(|| Group {
            first: start,
            ..Default::default()
        });
        group.clients.push(Client {
            id,
            delivery,
            tx,
            sent: false,
            last_end: None,
            settled: false,
        });
    }
    pub fn set_delivery(&mut self, id: usize, delivery: Delivery) {
        if let Some(client) = self
            .groups
            .values_mut()
            .flat_map(|g| &mut g.clients)
            .find(|c| c.id == id)
        {
            client.delivery = delivery;
        }
    }
    pub fn remove(&mut self, id: usize) {
        self.groups.retain(|_, g| {
            g.clients.retain(|c| c.id != id);
            !g.clients.is_empty()
        });
    }
    pub fn reset(&mut self) {
        for g in self.groups.values_mut() {
            g.state = None;
            g.first = 0;
            for c in &mut g.clients {
                if c.sent {
                    let _ = c.tx.force_send(None);
                    c.sent = false;
                }
            }
        }
    }
    pub fn observe(&mut self, h: &History, transforms: &mut Transforms) {
        for (&request, g) in &mut self.groups {
            if h.end <= g.first {
                continue;
            }
            match request {
                Request::Spectral(a) => {
                    let n = a.resolution.len(h.rate);
                    if h.valid < n
                        || h.end - g.first < n as u64
                        || !h.end.is_multiple_of((n / 4) as u64)
                    {
                        continue;
                    }
                    let Analysis::Spectral(scale, ring) = g.state.get_or_insert_with(|| {
                        Analysis::Spectral(
                            spectral::Scale::new(a, h.rate, n),
                            spectral::History::new(h.rate, n),
                        )
                    }) else {
                        unreachable!()
                    };
                    let silent = h.end - h.audible_end >= n as u64;
                    let column = if silent {
                        [0; spectral::ROWS]
                    } else {
                        let fft = measured(h, n, transforms);
                        let _timer = diagnostics::Timer::measure(|d| &d.history_us);
                        scale.column(fft)
                    };
                    ring.push(h.end - n as u64 / 2, column);
                    if let Some(d) = diagnostics::get() {
                        d.columns.fetch_add(1, Relaxed);
                    }
                }
                Request::Field(a) => {
                    let _timer = diagnostics::Timer::measure(|d| &d.field_us);
                    let Analysis::Field(field) = g
                        .state
                        .get_or_insert_with(|| Analysis::Field(field::Field::new(a, h.rate)))
                    else {
                        unreachable!()
                    };
                    field.push(h.tail(BLOCK));
                }
            }
        }
    }
    pub fn publish(&mut self, h: &History, current: bool, tick: u64) {
        if !current {
            self.reset();
            return;
        }
        let mut stereo = None;
        for g in self.groups.values_mut() {
            let Some(state) = g.state.as_ref() else {
                continue;
            };
            let (end, settled) = match state {
                Analysis::Spectral(_, ring) => (ring.end, false),
                Analysis::Field(f) => (h.end, f.settled),
            };
            let mut frame = None;
            let statistics = g.clients.iter().any(|c| c.delivery.statistics);
            for c in &mut g.clients {
                if !tick.is_multiple_of(60 / u64::from(c.delivery.speed.hz()))
                    || (c.sent && (c.last_end == Some(end) || (settled && c.settled)))
                {
                    continue;
                }
                let frame = frame.get_or_insert_with(|| {
                    let data = match state {
                        Analysis::Spectral(_, ring) => Payload::Spectral(ring.clone()),
                        Analysis::Field(f) => {
                            let stats = if statistics {
                                *stereo.get_or_insert_with(|| {
                                    Stereo::measure(h.tail(h.valid.min((h.rate / 20) as usize)))
                                })
                            } else {
                                Stereo::default()
                            };
                            Payload::Field(f.reading(stats))
                        }
                    };
                    Arc::new(VisualFrame {
                        published: std::time::Instant::now(),
                        epoch: h.epoch,
                        continuity: h.continuity,
                        rate: h.rate,
                        data,
                    })
                });
                let _ = c.tx.force_send(Some(frame.clone()));
                c.sent = true;
                c.last_end = Some(end);
                c.settled = settled;
                if let Some(d) = diagnostics::get() {
                    d.visual_updates.fetch_add(1, Relaxed);
                }
            }
        }
    }
}
