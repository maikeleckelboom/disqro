use crate::library::Track;
use anyhow::{Context, Result, ensure};
use kira::{Frame, sound::streaming::Decoder};
use std::{fs::File, io, sync::OnceLock};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::{
    codecs::audio::{
        AudioDecoder,
        well_known::{CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_VORBIS},
    },
    formats::{FormatReader, SeekMode, SeekTo, TrackType, probe::Hint},
    io::MediaSourceStream,
    units::{TimeBase, Timestamp},
};

pub struct Recording {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    timebase: TimeBase,
    origin: i64,
    rate: u32,
    start: usize,
    length: usize,
    cursor: usize,
    preroll: usize,
    samples: Vec<f32>,
    gain: f32,
}
pub fn disc_frame(tick: u64, rate: u32) -> usize {
    ((u128::from(tick) * u128::from(rate) + 37) / 75) as usize
}
impl Recording {
    pub fn open(track: &Track, end: Option<u64>) -> Result<Self> {
        let file = Box::new(File::open(&track.source)?);
        let reader = symphonia::default::get_probe().probe(
            &Hint::default(),
            MediaSourceStream::new(file, Default::default()),
            Default::default(),
            Default::default(),
        )?;
        let stream = reader
            .default_track(TrackType::Audio)
            .context("No audio stream")?;
        let params = stream
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .context("Missing audio properties")?;
        let rate = params.sample_rate.context("Unknown sample rate")?;
        ensure!((8000..=384000).contains(&rate), "Unsupported sample rate");
        let timebase = stream.time_base.context("Unknown source timing")?;
        let mut total = usize::try_from(stream.num_frames.context("Unknown recording length")?)?;
        let mut origin = stream.start_ts.get();
        if matches!(params.codec, CODEC_ID_AAC | CODEC_ID_ALAC)
            && let Some((first, frames)) = crate::movie::edit(&track.source, stream.id, rate)?
        {
            ensure!(
                timebase.numer.get() == 1 && timebase.denom.get() == rate,
                "Unsupported MP4 sample timebase"
            );
            origin = first;
            total = frames.min(
                total
                    .checked_sub(usize::try_from(first)?)
                    .context("MP4 edit starts outside recording")?,
            );
        }
        if params.codec == CODEC_ID_VORBIS {
            origin += i64::from(stream.delay.unwrap_or(0));
            total = total
                .checked_sub(usize::try_from(origin)?)
                .context("Invalid Vorbis duration")?;
        }
        let mut gain = 1.;
        let mut params = params.clone();
        if params.codec == CODEC_ID_OPUS {
            let header = params
                .extra_data
                .as_deref()
                .context("Missing Opus identification")?;
            ensure!(
                header.len() >= 19 && header.starts_with(b"OpusHead") && header[18] == 0,
                "Unsupported Opus channel mapping"
            );
            let skip = u16::from_le_bytes([header[10], header[11]]);
            let db = i16::from_le_bytes([header[16], header[17]]) as f32 / 256.;
            gain = 10f32.powf(db / 20.);
            // Ogg granules include encoder pre-skip. Own this offset on the
            // source timeline so opening partway through a file uses it once.
            params.extra_data = None;
            origin += i64::from(skip);
            total = total
                .checked_sub(usize::try_from(origin)?)
                .context("Invalid Opus duration")?;
        }
        let start = track.disc.as_ref().map_or(0, |d| disc_frame(d.start, rate));
        let end = end.map_or(total, |d| disc_frame(d, rate));
        ensure!(
            start < end && end <= total,
            "Track boundaries exceed the recording"
        );
        static CODECS: OnceLock<CodecRegistry> = OnceLock::new();
        let codecs = CODECS.get_or_init(|| {
            let mut registry = CodecRegistry::new();
            symphonia::default::register_enabled_codecs(&mut registry);
            registry.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
            registry
        });
        let decoder = codecs.make_audio_decoder(&params, &Default::default())?;
        let preroll = if matches!(
            params.codec,
            CODEC_ID_AAC | CODEC_ID_MP3 | CODEC_ID_OPUS | CODEC_ID_VORBIS
        ) {
            16384
        } else {
            0
        };
        let track_id = stream.id;

        Ok(Self {
            reader,
            decoder,
            track_id,
            timebase,
            origin,
            rate,
            start,
            length: end - start,
            cursor: 0,
            preroll,
            samples: Vec::new(),
            gain,
        })
    }
    fn timestamp(&self, frame: usize) -> io::Result<i64> {
        let scale = i128::from(self.timebase.numer.get()) * i128::from(self.rate);
        i64::try_from(
            frame as i128 * i128::from(self.timebase.denom.get()) / scale + i128::from(self.origin),
        )
        .map_err(io::Error::other)
    }
    fn frame(&self, ts: i64) -> i128 {
        (i128::from(ts) - i128::from(self.origin))
            * i128::from(self.timebase.numer.get())
            * i128::from(self.rate)
            / i128::from(self.timebase.denom.get())
    }
}
impl Decoder for Recording {
    type Error = io::Error;
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn num_frames(&self) -> usize {
        self.length
    }
    fn seek(&mut self, index: usize) -> io::Result<usize> {
        let local = index.min(self.length);
        let target = self.start + local;
        if target != 0 || self.cursor != 0 {
            // Warm compressed decoder history before the requested sample.
            let ts = self.timestamp(target.saturating_sub(self.preroll))?;
            self.reader
                .seek(
                    SeekMode::Accurate,
                    SeekTo::Timestamp {
                        ts: Timestamp::new(ts),
                        track_id: self.track_id,
                    },
                )
                .map_err(io::Error::other)?;
            self.decoder.reset();
        }
        self.cursor = local;
        Ok(local)
    }
    fn decode(&mut self) -> io::Result<Vec<Frame>> {
        if self.cursor == self.length {
            return Ok(vec![]);
        }
        loop {
            let packet = self
                .reader
                .next_packet()
                .map_err(io::Error::other)?
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Recording ended before its declared length",
                    )
                })?;
            if packet.track_id != self.track_id {
                continue;
            }
            let first = self.frame(packet.pts.get()) + i128::from(packet.trim_start.get());
            let block = self.decoder.decode(&packet).map_err(io::Error::other)?;
            let channels = block.spec().channels().count();
            if !(1..=2).contains(&channels)
                || block.spec().rate() != self.rate
                || block.frames() > 1_048_576
            {
                return Err(io::Error::other(
                    "Unsupported channel layout, sample rate change or oversized audio block",
                ));
            }
            if block.frames() == 0 {
                continue;
            }
            let wanted = (self.start + self.cursor) as i128;
            if first > wanted {
                return Err(io::Error::other(format!(
                    "Discontinuous audio timestamps: expected {wanted}, got {first}"
                )));
            }
            let skip = (wanted - first) as usize;
            if skip >= block.frames() {
                continue;
            }
            let count = (block.frames() - skip).min(self.length - self.cursor);
            self.samples.resize(block.samples_interleaved(), 0.);
            block.copy_to_slice_interleaved(&mut self.samples);
            self.cursor += count;
            return Ok(self.samples[skip * channels..(skip + count) * channels]
                .chunks_exact(channels)
                .map(|s| Frame::new(s[0] * self.gain, s[channels - 1] * self.gain))
                .collect());
        }
    }
}
