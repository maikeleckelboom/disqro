//! Readback is confined to the headless regression runner and tests.
use super::*;
use crate::waveform_render::{
    Geometry,
    stroke::{Columns, Core, Partition},
};
use windows::{
    Win32::Graphics::{
        Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0},
        Dxgi::{
            Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
            IDXGIAdapter, IDXGIDevice,
        },
    },
    core::Interface,
};

pub struct Renderer {
    pipeline: Pipeline,
    context: ID3D11DeviceContext,
    pub adapter: String,
    pub software: bool,
}

#[cfg(feature = "qualification")]
pub fn start_presentation_trace() {
    use std::{
        fs::OpenOptions,
        io::{BufWriter, Write},
        sync::mpsc,
    };
    let Some(path) = std::env::var_os("DISQRO_DXGI_TRACE") else {
        return;
    };
    let Ok(file) = OpenOptions::new().write(true).create_new(true).open(path) else {
        return;
    };
    let (send, receive) =
        mpsc::sync_channel::<gpui_windows_gpu::frame_statistics::FrameStatistics>(2048);
    gpui_windows_gpu::frame_statistics::listen(move |frame| {
        let _ = send.try_send(frame);
    });
    let _ = std::thread::Builder::new().name("disqro-dxgi-trace".into()).spawn(move || -> std::io::Result<()> {
        let mut out = BufWriter::new(file);
        writeln!(out, "qpc,foreground,queued,present_status,statistics_status,displayed,present_refresh,sync_refresh,sync_qpc")?;
        for frame in receive {
            let d = frame.display;
            writeln!(out, "{},{},{},{},{},{},{},{},{}", frame.qpc, u8::from(frame.foreground), frame.queued,
                frame.present_status, frame.statistics_status, d.PresentCount, d.PresentRefreshCount, d.SyncRefreshCount, d.SyncQPCTime)?;
            if frame.queued.is_multiple_of(32) { out.flush()?; }
        }
        out.flush()
    });
}

#[cfg(feature = "qualification")]
#[derive(serde::Serialize)]
pub struct Timing {
    pub draws: usize,
    pub cpu_ms: [f64; 4],
    pub gpu_ms: [f64; 4],
}

impl Renderer {
    pub fn new() -> Result<Self> {
        let mut device = None;
        let mut context = None;
        let mut software = false;
        // SAFETY: D3D11 populates the device and context output slots. No
        // native window or presentation surface is created by either driver.
        unsafe {
            let mut create = |driver| {
                D3D11CreateDevice(
                    None::<&IDXGIAdapter>,
                    driver,
                    Default::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&[D3D_FEATURE_LEVEL_11_0]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
            };
            if create(D3D_DRIVER_TYPE_HARDWARE).is_err() {
                software = true;
                create(D3D_DRIVER_TYPE_WARP)?;
            }
        }
        let device = device.context("headless D3D11 device")?;
        let adapter = unsafe { device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()? };
        let description = adapter
            .Description
            .split(|&value| value == 0)
            .next()
            .unwrap_or(&[]);
        Ok(Self {
            pipeline: Pipeline::new(&device)?,
            context: context.context("headless D3D11 context")?,
            adapter: String::from_utf16_lossy(description),
            software,
        })
    }

    pub fn render(
        &mut self,
        evidence: &Evidence,
        view: &View,
        first: f64,
    ) -> Result<image::RgbaImage> {
        let (desc, render, target) = self.target(view)?;
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..desc
        };
        let mut staging = None;
        // SAFETY: descriptors live until D3D11 finishes creating each texture.
        unsafe {
            self.pipeline
                .device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        }
        let staging = staging.context("headless staging texture")?;
        self.pipeline
            .draw(&self.context, &target, evidence, view, first)?;
        let mut pixels = image::RgbaImage::new(desc.Width, desc.Height);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: staging has CPU read access and the same dimensions as the
        // source. The mapped storage includes RowPitch bytes per row.
        unsafe {
            self.context.CopyResource(&staging, &render);
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let bytes = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * view.height);
            let output: &mut [u8] = pixels.as_mut();
            for (row, raster) in output
                .chunks_exact_mut(view.width * 4)
                .zip(bytes.chunks_exact(pitch))
            {
                for (pixel, [b, g, r, a]) in row
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(raster.as_chunks::<4>().0)
                {
                    *pixel = [*r, *g, *b, *a];
                }
            }
            self.context.Unmap(&staging, 0);
        }
        Ok(pixels)
    }

    fn target(
        &self,
        view: &View,
    ) -> Result<(
        D3D11_TEXTURE2D_DESC,
        ID3D11Texture2D,
        ID3D11RenderTargetView,
    )> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: view.width as u32,
            Height: view.height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let device = &self.pipeline.device;
        let mut texture = None;
        let mut target = None;
        // SAFETY: D3D11 writes owned interfaces to the output slots.
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
        let texture = texture.context("headless waveform target")?;
        unsafe { device.CreateRenderTargetView(&texture, None, Some(&mut target))? };
        Ok((desc, texture, target.context("headless render view")?))
    }

    #[cfg(feature = "qualification")]
    pub fn benchmark(&mut self, evidence: &Evidence, view: &View) -> Result<Timing> {
        use std::time::{Duration, Instant};
        const FRAMES: usize = 180;
        let (_, _texture, target) = self.target(view)?;
        let rate = f64::from(view.rate);
        let centre = (view.first as f64 + view.width as f64 * 0.5) / view.pixels_per_second;
        self.pipeline.draw(
            &self.context,
            &target,
            evidence,
            view,
            view.first as f64 * rate / view.pixels_per_second,
        )?;
        let query = |kind| -> Result<ID3D11Query> {
            let mut query = None;
            let desc = D3D11_QUERY_DESC {
                Query: kind,
                MiscFlags: 0,
            };
            // SAFETY: creation only writes an owned query interface.
            unsafe { self.pipeline.device.CreateQuery(&desc, Some(&mut query))? };
            query.context("waveform timestamp query")
        };
        let disjoint = query(D3D11_QUERY_TIMESTAMP_DISJOINT)?;
        let timestamps = (0..FRAMES)
            .map(|_| Ok((query(D3D11_QUERY_TIMESTAMP)?, query(D3D11_QUERY_TIMESTAMP)?)))
            .collect::<Result<Vec<_>>>()?;
        let mut cpu = Vec::with_capacity(FRAMES);
        // SAFETY: these are timestamp and timestamp-disjoint queries created
        // by the same device. The pair brackets all submitted waveform work.
        unsafe { self.context.Begin(&disjoint) };
        for (index, (begin, end)) in timestamps.iter().enumerate() {
            let f = (index % 60) as f64 / 30.0;
            let density = view.pixels_per_second * (1.0 + 0.15 * (1.0 - (1.0 - f).abs()));
            let first = (centre * density - view.width as f64 * 0.5).round() as i64;
            let frame = View {
                pixels_per_second: density,
                first,
                ..*view
            };
            unsafe { self.context.End(begin) };
            let start = Instant::now();
            self.pipeline.draw(
                &self.context,
                &target,
                evidence,
                &frame,
                first as f64 * rate / density,
            )?;
            cpu.push(start.elapsed().as_secs_f64() * 1000.0);
            unsafe { self.context.End(end) };
        }
        unsafe {
            self.context.End(&disjoint);
            self.context.Flush();
        }
        let mut clock = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
        let wait = Instant::now();
        loop {
            // SAFETY: the data structures match the queried D3D11 types.
            unsafe {
                self.context.GetData(
                    &disjoint,
                    Some(std::ptr::from_mut(&mut clock).cast()),
                    size_of_val(&clock) as u32,
                    0,
                )?
            };
            if clock.Frequency != 0 {
                break;
            }
            ensure!(
                wait.elapsed() < Duration::from_secs(10),
                "waveform GPU timestamp timed out"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        ensure!(
            !clock.Disjoint.as_bool(),
            "waveform GPU timestamps were disjoint"
        );
        let mut gpu = Vec::with_capacity(FRAMES);
        for (begin, end) in timestamps {
            let read = |query: ID3D11Query| -> Result<u64> {
                let mut value = u64::MAX;
                let start = Instant::now();
                while value == u64::MAX {
                    // S_FALSE leaves the output pending. The windows wrapper
                    // represents both successful HRESULTs as Ok.
                    unsafe {
                        self.context.GetData(
                            &query,
                            Some(std::ptr::from_mut(&mut value).cast()),
                            size_of_val(&value) as u32,
                            0,
                        )?
                    };
                    if value == u64::MAX {
                        ensure!(
                            start.elapsed() < Duration::from_secs(2),
                            "waveform GPU timestamp missing"
                        );
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                Ok(value)
            };
            let start = read(begin)?;
            let end = read(end)?;
            ensure!(end >= start, "waveform GPU clock regressed");
            gpu.push((end - start) as f64 * 1000.0 / clock.Frequency as f64);
        }
        let summary = |mut values: Vec<f64>| {
            values.sort_by(f64::total_cmp);
            [0.5, 0.95, 0.99, 1.0].map(|p| values[((values.len() - 1) as f64 * p).round() as usize])
        };
        Ok(Timing {
            draws: FRAMES,
            cpu_ms: summary(cpu),
            gpu_ms: summary(gpu),
        })
    }

    pub fn geometry(&self, view: &View) -> Result<Geometry> {
        if view.role == Role::Overview {
            return Ok(Geometry::Overview);
        }
        let detail = self
            .pipeline
            .detail
            .as_ref()
            .context("completed Stroke draw")?;
        let (cores, columns) = detail.readback_buffers();
        let partition = Partition::new(view.rate, view.pixels_per_second);
        let first_cell = partition.cell(view.first) - 1;
        let end_cell = partition.cell(view.first + view.width as i64 - 1) + 2;
        let starts: Vec<_> = (first_cell..=end_cell)
            .map(|cell| partition.start(cell))
            .collect();
        let cells = (end_cell - first_cell) as usize;
        let cores = self
            .read_words(cores, cells)?
            .into_iter()
            .enumerate()
            .map(|(index, [lo, hi, a, b])| {
                let clip = |frame: i64| (frame.max(0) as usize).min(view.frame_count);
                if a == u32::MAX {
                    return Core {
                        held: b != 0,
                        bounds: None,
                        witnesses: None,
                    };
                }
                Core {
                    held: true,
                    bounds: Some([f32::from_bits(lo), f32::from_bits(hi)]),
                    witnesses: (clip(starts[index]) != clip(starts[index + 1]))
                        .then_some([detail.origin + a as usize, detail.origin + b as usize]),
                }
            })
            .collect();
        let columns = self
            .read_words(columns, view.width * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[a, b]| [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]].map(f32::from_bits))
            .collect();
        Ok(Geometry::Stroke(Columns {
            first_cell,
            starts,
            cores,
            columns,
        }))
    }

    fn read_words(&self, buffer: &Buffer, words: usize) -> Result<Vec<Word>> {
        let mut staging = None;
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: buffer.bytes,
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..Default::default()
        };
        let mut map = D3D11_MAPPED_SUBRESOURCE::default();
        ensure!(
            words * size_of::<Word>() <= buffer.bytes as usize,
            "geometry readback exceeds its buffer"
        );
        // SAFETY: the staging buffer exactly matches the source ByteWidth and
        // has read access. Map supplies storage for every requested word.
        unsafe {
            self.pipeline
                .device
                .CreateBuffer(&desc, None, Some(&mut staging))?;
            let staging = staging.context("waveform geometry staging buffer")?;
            self.context.CopyResource(&staging, &buffer.buffer);
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
            let result = std::slice::from_raw_parts(map.pData.cast::<Word>(), words).to_vec();
            self.context.Unmap(&staging, 0);
            Ok(result)
        }
    }
}

#[test]
fn shader_projection_matches_the_reference_at_device_pixel_edges() {
    let mut renderer = Renderer::new().unwrap();
    assert!(!renderer.adapter.is_empty());
    let _software_adapter = renderer.software;
    let page = Arc::new(Page {
        frames_per_column: 2,
        frame_count: 16,
        facts: [-0.65, 0.33, 0.24, -0.65, 0.33, -0.13, 0.17].repeat(8),
        extrema: [0, 0].repeat(8),
        fallback_energy: Some(vec![[0.2, 0.13, 0.1]; 8]),
        ..Default::default()
    });
    for role in [Role::Detail, Role::Overview] {
        for light in [false, true] {
            let view = View {
                role,
                style: crate::waveform_render::Style {
                    gain: 1.0,
                    intensity: 0.8,
                    light,
                },
                rate: 40123,
                origin: 0,
                frame_count: 16,
                first: 0,
                pixels_per_second: 40123.0 * 19.0 / 16.0,
                width: 19,
                height: 27,
            };
            let expected = crate::waveform_render::render(&[&page], &view).0;
            let evidence = if role == Role::Overview {
                Evidence {
                    overview: Some(page.clone()),
                    ..Default::default()
                }
            } else {
                Evidence {
                    pages: vec![page.clone()],
                    ..Default::default()
                }
            };
            let actual = renderer.render(&evidence, &view, 0.0).unwrap();
            let _geometry = renderer.geometry(&view).unwrap();
            let error = expected
                .iter()
                .zip(actual.as_raw())
                .map(|(a, b)| a.abs_diff(*b))
                .max();
            assert!(error.unwrap() <= 1, "{role:?} light={light} {error:?}");
        }
    }
}

#[test]
fn sample_zoom_keeps_both_decoded_channels_and_clips_at_the_playable_edges() {
    let mut renderer = Renderer::new().unwrap();
    let evidence = Evidence {
        samples: vec![Arc::new(Tile::Samples {
            first: 0,
            samples: vec![kira::Frame::new(0.5, -0.5); 10],
        })],
        ..Default::default()
    };
    let view = View {
        role: Role::Detail,
        style: Default::default(),
        rate: 48000,
        origin: 2,
        frame_count: 6,
        first: -8,
        pixels_per_second: 192000.0,
        width: 32,
        height: 100,
    };
    let pixels = renderer.render(&evidence, &view, -2.0).unwrap();
    for row in [27, 73] {
        assert!(pixels.get_pixel(16, row).0[0] > 30);
        assert_eq!(pixels.get_pixel(2, row).0[0], 0);
        assert_eq!(pixels.get_pixel(30, row).0[0], 0);
    }
}

#[test]
fn silent_stroke_columns_stay_uncoloured_when_sample_inspection_is_absent() {
    let mut renderer = Renderer::new().unwrap();
    let view = View {
        role: Role::Detail,
        style: Default::default(),
        rate: 48000,
        origin: 0,
        frame_count: 256,
        first: 0,
        pixels_per_second: 48000.0 / 8.0,
        width: 32,
        height: 16,
    };
    let page = Page {
        frames_per_column: 32,
        frame_count: 256,
        facts: vec![0.0; 8 * crate::waveform_render::FACTS],
        extrema: vec![0; 16],
        octave: Some(crate::waveform_render::Octave {
            first_cell_frame: 0,
            analysis: Some("stroke-hann512".into()),
            powers: vec![0.1; 8],
        }),
        ..Default::default()
    };
    let evidence = Evidence {
        pages: vec![Arc::new(page)],
        ..Default::default()
    };
    renderer.render(&evidence, &view, 0.0).unwrap();
    let Geometry::Stroke(result) = renderer.geometry(&view).unwrap() else {
        panic!("expected Stroke columns")
    };
    assert!(result.columns.iter().any(|column| column[7] == 0.0));
    assert!(result.columns.iter().all(|column| column[4..7] == [0.0; 3]));
}
