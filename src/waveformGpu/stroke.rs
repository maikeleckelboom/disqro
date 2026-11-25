//! Keep immutable source pages on the shared device. Each draw changes only
//! the viewport's exact source-cell allocation and compact page descriptors.
use super::{Buffer, Evidence, Parameters, Tile, View, Word};
use crate::waveform_render::{Page, fallback_colour, stroke::Partition, stroke::page_slices};
use anyhow::{Context as _, Result, ensure};
use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Weak},
};
use windows::Win32::Graphics::Direct3D11::*;

const RESIDENT_BYTES: u32 = 16 * 1024 * 1024;
const WORD_BYTES: u32 = size_of::<Word>() as u32;
// Two Stroke words, one sample summary and two words for each of at most
// nine neighbouring segments per device column (see MAX_SEGMENTS in HLSL).
const SAMPLE_WORDS_PER_COLUMN: u32 = 21;

struct Resident {
    page: Weak<Page>,
    words: Range<u32>,
    spectrum: Option<(u32, u32, u32)>, // first hop, end hop, starting word
}

struct Samples {
    tile: Weak<Tile>,
    words: Range<u32>,
}

pub(super) struct Detail {
    cores_shader: ID3D11ComputeShader,
    columns_shader: ID3D11ComputeShader,
    evidence: Buffer,
    resident: HashMap<usize, Resident>,
    samples: HashMap<usize, Samples>,
    preview: Option<(Weak<Page>, Buffer)>,
    projection: Buffer,
    cores: Buffer,
    columns: Buffer,
    #[cfg(any(test, feature = "qualification"))]
    pub(super) origin: usize,
}

impl Detail {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let mut cores = None;
        let mut columns = None;
        // SAFETY: these bytecode blobs are compiled for a D3D11 compute shader.
        unsafe {
            device.CreateComputeShader(
                include_bytes!(concat!(env!("OUT_DIR"), "/strokeCores.cs")),
                None,
                Some(&mut cores),
            )?;
            device.CreateComputeShader(
                include_bytes!(concat!(env!("OUT_DIR"), "/strokeColumns.cs")),
                None,
                Some(&mut columns),
            )?;
        }
        Ok(Self {
            cores_shader: cores.context("create source-cell shader")?,
            columns_shader: columns.context("create Stroke column shader")?,
            evidence: Buffer::new(
                device,
                RESIDENT_BYTES,
                D3D11_BIND_SHADER_RESOURCE,
                WORD_BYTES,
            )?,
            resident: HashMap::new(),
            samples: HashMap::new(),
            preview: None,
            projection: Buffer::new(device, 64 * 1024, D3D11_BIND_SHADER_RESOURCE, WORD_BYTES)?,
            cores: Buffer::new(
                device,
                32 * 1024,
                D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS,
                WORD_BYTES,
            )?,
            columns: Buffer::new(
                device,
                64 * 1024,
                D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS,
                WORD_BYTES,
            )?,
            #[cfg(any(test, feature = "qualification"))]
            origin: 0,
        })
    }

    fn upload(&mut self, context: &ID3D11DeviceContext, page: &Arc<Page>) -> Result<()> {
        let id = Arc::as_ptr(page) as usize;
        if self.resident.contains_key(&id) {
            return Ok(());
        }
        let mut data = Vec::<Word>::new();
        if let Some(detail) = &page.source_detail {
            data.reserve(detail.offsets.len());
            for (&[lo, hi], &offset) in detail.bounds.as_chunks::<2>().0.iter().zip(&detail.offsets)
            {
                data.push([
                    lo.to_bits(),
                    hi.to_bits(),
                    u32::from(offset & 31),
                    u32::from((offset >> 5) & 31),
                ]);
            }
        } else {
            data.reserve(page.extrema.len());
            for (f, indices) in page
                .facts
                .as_chunks::<{ super::super::FACTS }>()
                .0
                .iter()
                .zip(page.extrema.as_chunks::<2>().0)
            {
                data.push([
                    f[0].to_bits(),
                    f[1].to_bits(),
                    indices[0] as u32,
                    indices[1] as u32,
                ]);
                data.push([f[3], f[4], f[5], f[6]].map(f32::to_bits));
            }
        }
        let spectrum = page
            .octave
            .as_ref()
            .filter(|o| o.analysis.as_deref() == Some("stroke-hann512"))
            .map(|o| {
                let first = (o.first_cell_frame / 256) as u32;
                let begin = data.len() as u32;
                let scale = 2.8606101989383417_f64.exp();
                let norm = scale.ln_1p();
                // The original transfers this f32 encoding before folding.
                data.extend(o.powers.as_chunks::<4>().0.iter().map(|row| {
                    row.map(|p| (((scale * p.max(0.0).sqrt()).ln_1p() / norm) as f32).to_bits())
                }));
                (first, first + (o.powers.len() / 8) as u32, begin)
            });
        let words = self.upload_words(context, &data)?;
        let first = words.start;
        self.resident.insert(
            id,
            Resident {
                page: Arc::downgrade(page),
                words,
                spectrum: spectrum.map(|(start, end, offset)| (start, end, first + offset)),
            },
        );
        Ok(())
    }

    fn upload_words(&mut self, context: &ID3D11DeviceContext, data: &[Word]) -> Result<Range<u32>> {
        self.resident.retain(|_, item| item.page.strong_count() > 0);
        self.samples.retain(|_, item| item.tile.strong_count() > 0);
        let words = data.len() as u32;
        let mut occupied: Vec<_> = self
            .resident
            .values()
            .map(|p| p.words.clone())
            .chain(self.samples.values().map(|s| s.words.clone()))
            .collect();
        occupied.sort_unstable_by_key(|r| r.start);
        let mut first = 0;
        for used in occupied {
            if first + words <= used.start {
                break;
            }
            first = used.end;
        }
        ensure!(
            (first + words) * WORD_BYTES <= RESIDENT_BYTES,
            "waveform source residency exhausted"
        );
        self.evidence.write(
            context,
            first * WORD_BYTES,
            bytemuck::cast_slice(data),
            false,
        );
        if let Some(log) = crate::diagnostics::get() {
            log.wave_upload_bytes.fetch_add(
                u64::from(words * WORD_BYTES),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        Ok(first..first + words)
    }

    pub fn prepare(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        input: &Evidence,
        view: &View,
        exact_first: f64,
        params: &mut Parameters,
    ) -> Result<()> {
        let pages = &input.pages;
        let rate = f64::from(view.rate);
        let partition = Partition::new(view.rate, view.pixels_per_second);
        let first_cell = partition.cell(view.first) - 1;
        let end_cell = partition.cell(view.first + view.width as i64 - 1) + 2;
        let extent = view.frame_count as f64 * view.pixels_per_second / rate;
        let clip = |f: i64| (f.max(0) as usize).min(view.frame_count);
        let begin = clip(partition.start(first_cell));
        let end = clip(partition.start(end_cell));
        let mut spans = Vec::new();
        let mut spectral = Vec::new();
        for page in pages {
            self.upload(context, page)?;
            let resident = &self.resident[&(Arc::as_ptr(page) as usize)];
            if page.end() > begin
                && page.start_frame < end
                && (page.source_detail.is_some() || !page.facts.is_empty())
            {
                spans.push(page.as_ref());
            }
            if let Some((start, end, word)) = resident.spectrum {
                spectral.push((start, page.frames_per_column, [start, end, word, 0]));
            }
        }
        let origin = spans
            .iter()
            .map(|p| p.start_frame)
            .min()
            .unwrap_or(begin)
            .min(begin);
        #[cfg(any(test, feature = "qualification"))]
        {
            self.origin = view.origin + origin;
        }
        let local = |frame: usize| (frame as i64 - origin as i64) as i32 as u32;
        let mut data = Vec::with_capacity(view.width * 2 + pages.len() * 3 + 4);
        for x in view.first..view.first + view.width as i64 {
            let cell = (partition.cell(x) - first_cell) as u32;
            let colour = partition.colour_frame(x).unwrap_or(0.0) / 256.0;
            let hop = colour.floor();
            let coverage = ((x as f64 + 1.0).min(extent) - (x as f64).max(0.0)).clamp(0.0, 1.0);
            data.push([
                cell,
                hop as i32 as u32,
                ((colour - hop) as f32).to_bits(),
                (coverage as f32).to_bits(),
            ]);
        }
        let mut current = begin;
        for cell in first_cell + 1..=end_cell {
            let next = clip(partition.start(cell));
            data.push([local(current), local(next), 0, 0]);
            current = next;
        }
        let slices = data.len() as u32;
        for (from, to, page) in page_slices(&spans) {
            if to <= begin || from >= end {
                continue;
            }
            let resident = &self.resident[&(std::ptr::from_ref(page) as usize)];
            let (base, stride) =
                page.source_detail
                    .as_ref()
                    .map_or((local(page.start_frame), 2), |detail| {
                        (
                            ((detail.first_cell_frame as i64 - view.origin as i64) - origin as i64)
                                as i32 as u32,
                            1,
                        )
                    });
            data.extend([
                [local(from), local(to), base, page.leaf_width() as u32],
                [
                    resident.words.start,
                    stride,
                    local(page.start_frame),
                    local(page.end()),
                ],
            ]);
        }
        let spectrum = data.len() as u32;
        spectral.sort_by_key(|(start, width, _)| (*start, std::cmp::Reverse(*width)));
        data.extend(spectral.into_iter().map(|(_, _, words)| words));
        let cell_count = (end_cell - first_cell) as u32;
        params.extent[2] = 1;
        params.detail = [cell_count, slices, spectrum, data.len() as u32];
        params.source = [
            view.frame_count.div_ceil(256).saturating_sub(1) as u32,
            u32::from(view.style.light),
            0,
            0,
        ];
        if partition.colour_frame(view.first).is_none() {
            params.source[3] = data.len() as u32;
            let references: Vec<_> = pages.iter().map(Arc::as_ref).collect();
            let mut colour = fallback_colour(&references, view);
            for pixel in view.first..view.first + view.width as i64 {
                let [r, g, b] = colour(pixel).unwrap_or([-1.0; 3]);
                data.push([r, g, b, 0.0].map(|v| (v as f32).to_bits()));
            }
        }
        self.prepare_samples(context, input, view, exact_first, params, &mut data)?;
        self.prepare_preview(device, context, input, view, params)?;
        self.projection
            .ensure(device, (data.len() as u32) * WORD_BYTES)?;
        self.projection
            .write(context, 0, bytemuck::cast_slice(&data), false);
        self.cores.ensure(device, cell_count * WORD_BYTES)?;
        let words = if params.sample_tiles[0] == params.sample_tiles[1] {
            2
        } else {
            SAMPLE_WORDS_PER_COLUMN
        };
        self.columns
            .ensure(device, view.width as u32 * words * WORD_BYTES)?;
        Ok(())
    }

    pub fn dispatch(&self, context: &ID3D11DeviceContext, params: &Parameters, uniform: &Buffer) {
        let inputs = [self.projection.srv.clone(), self.evidence.srv.clone(), None];
        // SAFETY: all views are on the same device as the context. Each kernel
        // bounds its dispatch to the number of elements prepared for it.
        unsafe {
            let constants = [Some(uniform.buffer.clone())];
            context.CSSetConstantBuffers(0, Some(&constants));
            context.CSSetShaderResources(1, Some(&inputs));
            context.CSSetShader(&self.cores_shader, None);
            context.CSSetUnorderedAccessViews(0, 1, Some(&self.cores.uav), None);
            context.Dispatch(params.detail[0].div_ceil(64), 1, 1);
            context.CSSetUnorderedAccessViews(0, 1, Some(&self.columns.uav), None);
            context.CSSetShaderResources(3, Some(std::slice::from_ref(&self.cores.srv)));
            context.CSSetShader(&self.columns_shader, None);
            context.Dispatch(params.extent[0].div_ceil(64), 1, 1);
            context.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
            context.CSSetShaderResources(1, Some(&[None, None, None]));
        }
    }

    pub fn bindings(&self) -> [Option<ID3D11ShaderResourceView>; 5] {
        [
            self.columns.srv.clone(),
            None,
            None,
            None,
            self.preview
                .as_ref()
                .and_then(|(_, buffer)| buffer.srv.clone()),
        ]
    }

    #[cfg(any(test, feature = "qualification"))]
    pub(super) fn readback_buffers(&self) -> (&Buffer, &Buffer) {
        (&self.cores, &self.columns)
    }

    fn prepare_preview(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        input: &Evidence,
        view: &View,
        params: &mut Parameters,
    ) -> Result<()> {
        let Some(page) = input.overview.as_ref().filter(|p| !p.facts.is_empty()) else {
            return Ok(());
        };
        let columns = page.facts.as_chunks::<{ super::super::FACTS }>().0;
        if self
            .preview
            .as_ref()
            .is_none_or(|(held, _)| held.as_ptr() != Arc::as_ptr(page))
        {
            let data: Vec<_> = columns
                .iter()
                .map(|f| [f[0], f[1], f[2], 0.0].map(f32::to_bits))
                .collect();
            let bytes = (data.len() as u32) * WORD_BYTES;
            if self.preview.is_none() {
                self.preview = Some((
                    Weak::new(),
                    Buffer::new(
                        device,
                        bytes.next_power_of_two(),
                        D3D11_BIND_SHADER_RESOURCE,
                        WORD_BYTES,
                    )?,
                ));
            }
            let (held, buffer) = self.preview.as_mut().unwrap();
            buffer.ensure(device, bytes)?;
            buffer.write(context, 0, bytemuck::cast_slice(&data), false);
            *held = Arc::downgrade(page);
        }
        let frames_per_pixel = f64::from(view.rate) / view.pixels_per_second;
        let bin = page.frames_per_column as f64;
        params.source[2] = columns.len() as u32;
        params.preview = [
            ((view.first as f64 * frames_per_pixel) / bin) as f32,
            (frames_per_pixel / bin) as f32,
            (page.frame_count as f64 / bin) as f32,
            0.0,
        ];
        Ok(())
    }

    fn prepare_samples(
        &mut self,
        context: &ID3D11DeviceContext,
        input: &Evidence,
        view: &View,
        exact_first: f64,
        params: &mut Parameters,
        data: &mut Vec<Word>,
    ) -> Result<()> {
        let step = f64::from(view.rate) / view.pixels_per_second;
        if step >= 3.0 || input.samples.is_empty() {
            return Ok(());
        }
        let start = view.origin + (exact_first.floor() as usize).saturating_sub(4);
        let end = view.origin
            + (((exact_first + step * view.width as f64).ceil() as usize) + 5)
                .min(view.frame_count);
        let mut tiles = Vec::new();
        for tile in &input.samples {
            let Tile::Samples { first, samples } = tile.as_ref() else {
                continue;
            };
            let from = (*first).max(start);
            let until = (first + samples.len()).min(end);
            if from >= until {
                continue;
            }
            let id = Arc::as_ptr(tile) as usize;
            if !self.samples.contains_key(&id) {
                let packed: Vec<_> = samples
                    .iter()
                    .map(|f| [f.left.to_bits(), f.right.to_bits(), 0, 0])
                    .collect();
                let words = self.upload_words(context, &packed)?;
                self.samples.insert(
                    id,
                    Samples {
                        tile: Arc::downgrade(tile),
                        words,
                    },
                );
            }
            let offset = self.samples[&id].words.start + (from - first) as u32;
            tiles.push([(from - start) as u32, (until - start) as u32, offset, 0]);
        }
        let first_tile = data.len() as u32;
        data.extend(tiles);
        params.sample_tiles = [first_tile, data.len() as u32, 0, 0];
        let blend = ((3.0 - step) / 2.25).clamp(0.0, 1.0);
        let phase = exact_first + (view.origin as i128 - start as i128) as f64;
        params.samples = [
            phase as f32,
            step as f32,
            (blend * blend * (3.0 - 2.0 * blend)) as f32,
            0.0,
        ];
        Ok(())
    }
}
