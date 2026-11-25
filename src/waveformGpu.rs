//! Waveform producer on the D3D11 device that owns GPUI's window.
use super::{Page, Role, View};
use crate::{overview::Evidence, waveform_detail::Tile};
use anyhow::{Context as _, Result, ensure};
use gpui_kit::{App, Bounds, DevicePixels, Global, Pixels, Window, size};
use gpui_windows_gpu::{GpuTexture, with_d3d11};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Weak},
};
use windows::Win32::Graphics::{Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Direct3D11::*};

#[cfg(any(test, feature = "qualification"))]
#[path = "waveformGpu/qualification.rs"]
pub(crate) mod qualification;
#[path = "waveformGpu/shape.rs"]
mod shape;
#[path = "waveformGpu/stroke.rs"]
mod stroke;

type Word = [u32; 4];

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Parameters {
    extent: Word,
    background: [f32; 4],
    stroke: [f32; 4],
    detail: Word,
    source: Word,
    preview: [f32; 4],
    samples: [f32; 4],
    sample_tiles: Word,
}

impl Parameters {
    fn new(view: &View) -> Self {
        let [r, g, b] = view.background();
        let closeup = (8.0 * view.pixels_per_second / view.width as f64)
            .log2()
            .clamp(0.0, 1.0) as f32;
        Self {
            extent: [view.width as u32, view.height as u32, 0, 0],
            background: [r, g, b, 0.0],
            stroke: [
                0.35 * (1.0 - closeup),
                if view.style.light { 0.0 } else { 54.0 },
                view.style.gain as f32,
                view.style.intensity as f32,
            ],
            detail: [0; 4],
            source: [0; 4],
            preview: [0.0; 4],
            samples: [0.0; 4],
            sample_tiles: [0; 4],
        }
    }
}

struct Buffer {
    buffer: ID3D11Buffer,
    srv: Option<ID3D11ShaderResourceView>,
    uav: Option<ID3D11UnorderedAccessView>,
    bytes: u32,
    bind: D3D11_BIND_FLAG,
    stride: u32,
}

impl Buffer {
    fn new(device: &ID3D11Device, bytes: u32, bind: D3D11_BIND_FLAG, stride: u32) -> Result<Self> {
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: bytes,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind.0 as u32,
            MiscFlags: if stride > 0 {
                D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32
            } else {
                0
            },
            StructureByteStride: stride,
            ..Default::default()
        };
        let mut buffer = None;
        let mut srv = None;
        let mut uav = None;
        // SAFETY: the descriptors and output slots live through these calls.
        unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer))? };
        let buffer = buffer.context("create waveform buffer")?;
        if bind.0 & D3D11_BIND_SHADER_RESOURCE.0 != 0 {
            unsafe { device.CreateShaderResourceView(&buffer, None, Some(&mut srv))? };
        }
        if bind.0 & D3D11_BIND_UNORDERED_ACCESS.0 != 0 {
            unsafe { device.CreateUnorderedAccessView(&buffer, None, Some(&mut uav))? };
        }
        Ok(Self {
            buffer,
            srv,
            uav,
            bytes,
            bind,
            stride,
        })
    }

    fn ensure(&mut self, device: &ID3D11Device, bytes: u32) -> Result<()> {
        if bytes > self.bytes {
            *self = Self::new(device, bytes.next_power_of_two(), self.bind, self.stride)?;
        }
        Ok(())
    }

    fn write(&self, context: &ID3D11DeviceContext, offset: u32, bytes: &[u8], constant: bool) {
        let start = crate::diagnostics::begin_stage();
        let right = offset + bytes.len() as u32;
        assert!(right <= self.bytes && (!constant || right == self.bytes));
        if bytes.is_empty() {
            return;
        }
        crate::diagnostics::stage(start, "shared_device_update", bytes.len() as u64, 0);
        let region = D3D11_BOX {
            left: offset,
            right,
            bottom: 1,
            back: 1,
            ..Default::default()
        };
        // SAFETY: the context and buffer share a device. The region describes
        // exactly the bytes in the slice. D3D11 copies them before returning.
        unsafe {
            context.UpdateSubresource(
                &self.buffer,
                0,
                (!constant).then_some(&region),
                bytes.as_ptr().cast(),
                0,
                0,
            );
        }
    }
}

struct Pipeline {
    device: ID3D11Device,
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    parameters: Buffer,
    columns: Buffer,
    detail: Option<stroke::Detail>,
}

impl Pipeline {
    fn new(device: &ID3D11Device) -> Result<Self> {
        let mut vertex = None;
        let mut pixel = None;
        // SAFETY: shader bytecode has been built for this stage by D3DCompile.
        unsafe {
            device.CreateVertexShader(
                include_bytes!(concat!(env!("OUT_DIR"), "/waveform.vs")),
                None,
                Some(&mut vertex),
            )?;
            device.CreatePixelShader(
                include_bytes!(concat!(env!("OUT_DIR"), "/waveform.ps")),
                None,
                Some(&mut pixel),
            )?;
        }
        Ok(Self {
            device: device.clone(),
            vertex: vertex.context("create waveform vertex shader")?,
            pixel: pixel.context("create waveform pixel shader")?,
            parameters: Buffer::new(
                device,
                size_of::<Parameters>() as u32,
                D3D11_BIND_CONSTANT_BUFFER,
                0,
            )?,
            columns: Buffer::new(
                device,
                64 * 1024,
                D3D11_BIND_SHADER_RESOURCE,
                size_of::<Word>() as u32,
            )?,
            detail: None,
        })
    }

    fn draw(
        &mut self,
        context: &ID3D11DeviceContext,
        target: &ID3D11RenderTargetView,
        evidence: &Evidence,
        view: &View,
        exact_first: f64,
    ) -> Result<()> {
        let mut params = Parameters::new(view);
        let start = crate::diagnostics::begin_stage();
        let bindings = if view.role == Role::Detail {
            if self.detail.is_none() {
                self.detail = Some(stroke::Detail::new(&self.device)?);
            }
            let detail = self.detail.as_mut().unwrap();
            detail.prepare(
                &self.device,
                context,
                evidence,
                view,
                exact_first,
                &mut params,
            )?;
            crate::diagnostics::stage(
                start,
                "shader_projection",
                view.width as u64,
                view.height as u64,
            );
            self.parameters
                .write(context, 0, bytemuck::bytes_of(&params), true);
            detail.dispatch(context, &params, &self.parameters);
            detail.bindings()
        } else {
            let data = shape::columns(evidence.overview.as_deref(), view, &mut params);
            let words = bytemuck::cast_slice(&data);
            self.columns.ensure(&self.device, words.len() as u32)?;
            self.columns.write(context, 0, words, false);
            self.parameters
                .write(context, 0, bytemuck::bytes_of(&params), true);
            [self.columns.srv.clone(), None, None, None, None]
        };
        let submit = crate::diagnostics::begin_stage();
        let (width, height) = (view.width, view.height);
        let viewport = D3D11_VIEWPORT {
            Width: width as f32,
            Height: height as f32,
            MaxDepth: 1.0,
            ..Default::default()
        };
        // SAFETY: all resources share the context's device. The full-screen
        // triangle has no vertex data. Unbind the target and SRV before GPUI
        // samples the surface or the next waveform updates the column buffer.
        unsafe {
            let rasterizer = context.RSGetState();
            context.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            context.OMSetBlendState(None, None, u32::MAX);
            context.RSSetState(None);
            context.RSSetViewports(Some(&[viewport]));
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.PSSetShader(&self.pixel, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.parameters.buffer.clone())]));
            context.PSSetShaderResources(0, Some(&bindings));
            context.Draw(3, 0);
            context.PSSetShaderResources(0, Some(&[None, None, None, None, None]));
            context.OMSetRenderTargets(None, None);
            context.RSSetState(rasterizer.ok().as_ref());
        }
        crate::diagnostics::stage(submit, "shader_draw", view.width as u64, view.height as u64);
        Ok(())
    }
}

#[derive(Default)]
struct DeviceResources(Option<Pipeline>);

impl Global for DeviceResources {}

#[derive(Default, Clone)]
pub(crate) struct SurfaceTexture(Rc<RefCell<SurfaceState>>);

#[derive(Default)]
struct SurfaceState {
    painted: Option<Painted>,
    failure: Option<Failure>,
}

struct Failure {
    device: ID3D11Device,
    size: (usize, usize),
    at: std::time::Instant,
}

struct Painted {
    device: ID3D11Device,
    target: GpuTexture,
    view: View,
    pages: Vec<Weak<Page>>,
    overview: Option<Weak<Page>>,
    overview_start: usize,
    samples: Vec<Weak<Tile>>,
    sample_phase: f64,
    valid: bool,
}

impl SurfaceTexture {
    pub fn clear(&self) {
        *self.0.borrow_mut() = SurfaceState::default();
    }

    pub fn unavailable(&self) -> bool {
        self.0.borrow().failure.is_some()
    }

    pub fn retry_due(&self) -> bool {
        self.0
            .borrow()
            .failure
            .as_ref()
            .is_some_and(|failure| failure.at.elapsed() >= std::time::Duration::from_millis(500))
    }

    pub fn paint(
        &self,
        window: &mut Window,
        cx: &mut App,
        bounds: Bounds<Pixels>,
        view: View,
        evidence: &Evidence,
        exact_first: f64,
    ) -> bool {
        self.draw(window, cx, Some(bounds), view, evidence, exact_first)
    }

    /// Updates the existing scene binding without a GPUI layout or scene rebuild.
    pub fn repaint(
        &self,
        window: &mut Window,
        cx: &mut App,
        view: View,
        evidence: &Evidence,
        exact_first: f64,
    ) -> bool {
        self.draw(window, cx, None, view, evidence, exact_first)
    }

    fn draw(
        &self,
        window: &mut Window,
        cx: &mut App,
        bounds: Option<Bounds<Pixels>>,
        view: View,
        evidence: &Evidence,
        exact_first: f64,
    ) -> bool {
        let mut surface = self.0.borrow_mut();
        let result = with_d3d11(window, |window, device, context| {
            let size = (view.width, view.height);
            if surface.failure.as_ref().is_some_and(|failure| {
                failure.device == *device
                    && failure.size == size
                    && failure.at.elapsed() < std::time::Duration::from_millis(500)
            }) {
                return Ok(false);
            }
            let result = surface.draw(
                window,
                cx,
                device,
                context,
                bounds,
                view,
                evidence,
                exact_first,
            );
            match result {
                Ok(ready) => {
                    if ready {
                        surface.failure = None;
                    }
                    Ok(ready)
                }
                Err(error) => {
                    surface.painted = None;
                    if surface
                        .failure
                        .as_ref()
                        .is_none_or(|failure| failure.device != *device || failure.size != size)
                    {
                        eprintln!("Waveform display: {error:#}");
                    }
                    if let Some(trace) = crate::diagnostics::get() {
                        trace
                            .wave_gpu_errors
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    surface.failure = Some(Failure {
                        device: device.clone(),
                        size,
                        at: std::time::Instant::now(),
                    });
                    Ok(false)
                }
            }
        });
        if result.is_err() {
            surface.painted = None;
        }
        result.unwrap_or(false)
    }
}

impl SurfaceState {
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        window: &mut Window,
        cx: &mut App,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        bounds: Option<Bounds<Pixels>>,
        view: View,
        evidence: &Evidence,
        exact_first: f64,
    ) -> Result<bool> {
        let resources = &mut cx.default_global::<DeviceResources>().0;
        if resources
            .as_ref()
            .is_some_and(|pipeline| pipeline.device != *device)
        {
            resources.take();
        }
        let reusable = self.painted.as_ref().is_some_and(|painted| {
            painted.device == *device
                && painted.view.width == view.width
                && painted.view.height == view.height
        });
        if !reusable {
            self.painted = None;
            if bounds.is_none() {
                return Ok(false);
            }
            ensure!(
                (1..=16_384).contains(&view.width) && (1..=16_384).contains(&view.height),
                "waveform target exceeds D3D11 dimensions"
            );
            #[cfg(feature = "qualification")]
            ensure!(
                view.role != Role::Detail || !qualification::fail_target_once(),
                "injected waveform target allocation failure"
            );
            let target = GpuTexture::new(
                device,
                size(
                    DevicePixels(view.width as i32),
                    DevicePixels(view.height as i32),
                ),
            )?;
            self.painted = Some(Painted {
                device: device.clone(),
                target,
                view,
                pages: Vec::new(),
                overview: None,
                overview_start: 0,
                samples: Vec::new(),
                sample_phase: 0.0,
                valid: false,
            });
        }
        let painted = self.painted.as_mut().unwrap();
        let sample_phase = if evidence.samples.is_empty() {
            0.0
        } else {
            exact_first
        };
        if !painted.valid
            || painted.view != view
            || painted.sample_phase != sample_phase
            || !same_data(&painted.pages, &evidence.pages)
            || !same_data(&painted.samples, &evidence.samples)
            || painted.overview.as_ref().map(Weak::as_ptr)
                != evidence.overview.as_ref().map(Arc::as_ptr)
            || painted.overview_start != evidence.overview_start
        {
            if resources.is_none() {
                *resources = Some(Pipeline::new(device)?);
            }
            resources.as_mut().unwrap().draw(
                context,
                painted.target.target(),
                evidence,
                &view,
                exact_first,
            )?;
            if bounds.is_some() {
                painted.target.written(window)?;
            }
            painted.view = view;
            painted.pages = evidence.pages.iter().map(Arc::downgrade).collect();
            painted.overview = evidence.overview.as_ref().map(Arc::downgrade);
            painted.overview_start = evidence.overview_start;
            painted.samples = evidence.samples.iter().map(Arc::downgrade).collect();
            painted.sample_phase = sample_phase;
            painted.valid = true;
            if let Some(trace) = crate::diagnostics::get() {
                trace
                    .wave_gpu_draws
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if let Some(bounds) = bounds {
            painted.target.paint(window, bounds)?;
        }
        Ok(true)
    }
}

fn same_data<T>(held: &[Weak<T>], next: &[Arc<T>]) -> bool {
    held.len() == next.len()
        && held
            .iter()
            .zip(next)
            .all(|(a, b)| a.as_ptr() == Arc::as_ptr(b))
}
