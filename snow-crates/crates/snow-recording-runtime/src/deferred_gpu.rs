//! Clean GPU source pixels, with the same fit, area filtering and edge padding
//! as the CPU canvas. Cursor and effect observations remain separate assets.
use super::*;
use crate::direct::gpu::resize::{AreaScaler, CropCopy};
use crate::gpu_startup::{Negotiation, ProbePurpose, prepare_capture, probe_encoder};
use snow_capture::gpu::{GpuCaptureStream, GpuCapturedFrame};
use snow_d3d11::{Rect, SharedDevice, Texture, VideoLayer, VideoProcessor};
use snow_recording_export::gpu::{GpuEncoderFrame, GpuInputConfig};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

fn error(cause: impl std::fmt::Display) -> ScreenRecorderError {
    ScreenRecorderError::Encode(format!("clean GPU source: {cause}"))
}

pub(super) fn prepare(
    config: &DirectRecordingConfig,
    settings: &StreamingEncoderConfig,
    logical: (u32, u32),
    coded: (u32, u32),
    diagnostics: &mut Negotiation,
) -> Result<Option<(GpuCaptureStream, CleanGpuCanvas, StreamingEncoder)>> {
    #[cfg(feature = "bench-pipeline-timing")]
    if std::env::var_os("SNOW_BENCH_SOURCE_TRACE").is_some() {
        return Ok(None);
    }
    // This capture API cannot apply explicit exclusion lists. Keep those
    // requests on the capture path that already implements their semantics.
    if !config.excluded_windows.is_empty() || !config.excluded_processes.is_empty() {
        return Ok(None);
    }
    let Some((capture, observation)) = prepare_capture(config, true, diagnostics)? else {
        return Ok(None);
    };
    let device = capture.device().clone();
    diagnostics.stage = Some("compositor_startup".into());
    let mut canvas = CleanGpuCanvas::new(device.clone(), logical, coded, settings.fps)?;
    probe_encoder(
        settings.clone(),
        ProbePurpose::RecordingSource,
        device.clone(),
        diagnostics,
        |_, output| canvas.compose(&observation, output),
    )?;
    diagnostics.stage = Some("encoder_startup".into());
    let encoder = StreamingEncoder::builder(settings.clone())
        .recording_source()
        .recoverable()
        .gpu_input(GpuInputConfig { device })
        .create()?;
    diagnostics.stage = None;
    Ok(Some((capture, canvas, encoder)))
}

pub(super) fn recover(
    reason: &str,
    config: &DirectRecordingConfig,
    encoder: &mut StreamingEncoder,
    capture: &mut crate::direct::capture::DirectCapture,
    canvas: &mut Option<CleanGpuCanvas>,
    paused: bool,
) -> Result<()> {
    capture.stop();
    encoder.recover_to_software(reason)?;
    canvas.take();
    *capture = crate::direct::capture::DirectCapture::cpu(config, true)?;
    if paused {
        capture.pause();
    }
    Ok(())
}

pub(super) struct CleanGpuCanvas {
    device: SharedDevice,
    logical: (u32, u32),
    coded: (u32, u32),
    processor: VideoProcessor,
    copy: Option<CropCopy>,
    scaler: Option<AreaScaler>,
    source_canvas: Option<Texture>,
    coded_canvas: Option<(Texture, ID3D11RenderTargetView, PixelRect)>,
}

impl CleanGpuCanvas {
    fn new(device: SharedDevice, logical: (u32, u32), coded: (u32, u32), fps: u32) -> Result<Self> {
        let processor = VideoProcessor::new(device.clone(), coded, coded, fps).map_err(error)?;
        processor
            .check_conversion(DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_NV12)
            .map_err(error)?;
        Ok(Self {
            device,
            logical,
            coded,
            processor,
            copy: None,
            scaler: None,
            source_canvas: None,
            coded_canvas: None,
        })
    }

    pub(super) fn compose(
        &mut self,
        frame: &GpuCapturedFrame,
        output: &GpuEncoderFrame,
    ) -> Result<()> {
        if !self.device.same_device(output.device()) {
            return Err(error("encoder output belongs to another device"));
        }
        let device = self.device.clone();
        let _lock = device.lock();
        let rgb = self.compose_rgb(frame)?;
        self.processor
            .blit(
                &[VideoLayer {
                    source: Rect::full(self.coded),
                    destination: Rect::full(self.coded),
                    texture: rgb,
                    rotation: 0,
                    alpha: false,
                }],
                &output.texture()?,
                output.slice(),
            )
            .map_err(error)
    }

    fn compose_rgb(&mut self, frame: &GpuCapturedFrame) -> Result<Texture> {
        let source_size = frame.dimensions();
        let fitted = fitted_rect(source_size, self.logical)?;
        let fitted_size = (fitted.width, fitted.height);
        let direct = frame.layers.first().filter(|layer| {
            let oriented = if layer.rotation.is_multiple_of(2) {
                (layer.source.width, layer.source.height)
            } else {
                (layer.source.height, layer.source.width)
            };
            frame.layers.len() == 1
                && !layer.alpha
                && layer.destination == Rect::full(source_size)
                && oriented == source_size
        });
        let layer = if let Some(layer) = direct {
            layer.clone()
        } else {
            self.copy_source(&frame.layers, source_size)?
        };
        let mut rgb = if source_size != fitted_size {
            if self.scaler.is_none() {
                self.scaler = Some(AreaScaler::new(self.device.clone())?);
            }
            self.scaler.as_mut().unwrap().resize(&layer, fitted_size)?
        } else if layer.rotation == 0 && layer.source == Rect::full(layer.texture.dimensions()) {
            layer.texture.clone()
        } else {
            self.copy_source(std::slice::from_ref(&layer), source_size)?
                .texture
        };
        if fitted_size == self.coded {
            return Ok(rgb);
        }
        // Region copies require matching formats. Resolve BGRA to RGBA once
        // only when letterboxing or odd 4:2:0 padding needs a coded canvas.
        if rgb.desc().Format != DXGI_FORMAT_R8G8B8A8_UNORM {
            rgb = self
                .copy_source(
                    &[VideoLayer {
                        source: Rect::full(fitted_size),
                        destination: Rect::full(fitted_size),
                        texture: rgb,
                        rotation: 0,
                        alpha: false,
                    }],
                    fitted_size,
                )?
                .texture;
        }
        if self.coded_canvas.is_none() {
            let texture = self.rgba(self.coded)?;
            let mut view = None;
            unsafe {
                self.device
                    .device()
                    .CreateRenderTargetView(texture.raw(), None, Some(&mut view))
            }
            .map_err(error)?;
            self.coded_canvas = Some((
                texture,
                view.ok_or_else(|| error("missing canvas render target"))?,
                PixelRect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
            ));
        }
        let (canvas, view, previous) = self.coded_canvas.as_mut().unwrap();
        if *previous != fitted {
            unsafe {
                self.device
                    .context()
                    .ClearRenderTargetView(&*view, &[0.0, 0.0, 0.0, 1.0])
            };
            *previous = fitted;
        }
        let copy = |source: Rect, x: u32, y: u32| unsafe {
            self.device.context().CopySubresourceRegion(
                canvas.raw(),
                0,
                x,
                y,
                0,
                rgb.raw(),
                0,
                Some(&D3D11_BOX {
                    left: source.x as u32,
                    top: source.y as u32,
                    front: 0,
                    right: source.x as u32 + source.width,
                    bottom: source.y as u32 + source.height,
                    back: 1,
                }),
            );
        };
        copy(Rect::full(fitted_size), fitted.x, fitted.y);
        let right = self.coded.0 > self.logical.0 && fitted.x + fitted.width == self.logical.0;
        let bottom = self.coded.1 > self.logical.1 && fitted.y + fitted.height == self.logical.1;
        if right {
            copy(
                Rect {
                    x: fitted.width as i32 - 1,
                    y: 0,
                    width: 1,
                    height: fitted.height,
                },
                self.logical.0,
                fitted.y,
            );
        }
        if bottom {
            copy(
                Rect {
                    x: 0,
                    y: fitted.height as i32 - 1,
                    width: fitted.width,
                    height: 1,
                },
                fitted.x,
                self.logical.1,
            );
        }
        if right && bottom {
            copy(
                Rect {
                    x: fitted.width as i32 - 1,
                    y: fitted.height as i32 - 1,
                    width: 1,
                    height: 1,
                },
                self.logical.0,
                self.logical.1,
            );
        }
        Ok(canvas.clone())
    }

    fn rgba(&self, size: (u32, u32)) -> Result<Texture> {
        self.device
            .texture(
                size.0,
                size.1,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                (D3D11_BIND_SHADER_RESOURCE
                    | D3D11_BIND_RENDER_TARGET
                    | D3D11_BIND_UNORDERED_ACCESS)
                    .0 as u32,
            )
            .map_err(error)
    }

    fn copy_source(&mut self, layers: &[VideoLayer], size: (u32, u32)) -> Result<VideoLayer> {
        if self
            .source_canvas
            .as_ref()
            .is_none_or(|texture| texture.dimensions() != size)
        {
            self.source_canvas = Some(self.rgba(size)?);
        }
        if self.copy.is_none() {
            self.copy = Some(CropCopy::new(self.device.clone())?);
        }
        let texture = self.source_canvas.as_ref().unwrap();
        self.copy.as_mut().unwrap().copy_into(layers, texture)?;
        Ok(VideoLayer {
            source: Rect::full(size),
            destination: Rect::full(size),
            texture: texture.clone(),
            rotation: 0,
            alpha: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    use windows::core::Interface;

    fn rgba_pixels(device: &SharedDevice, texture: &Texture) -> Result<Vec<u8>> {
        let mut desc = texture.desc();
        let bgra = desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        unsafe {
            device
                .device()
                .CreateTexture2D(&desc, None, Some(&mut staging))
        }
        .map_err(error)?;
        let staging = staging.unwrap();
        let mut pixels = Vec::new();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            device.context().CopyResource(&staging, texture.raw());
            device
                .context()
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(error)?;
            for y in 0..desc.Height as usize {
                pixels.extend_from_slice(std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    desc.Width as usize * 4,
                ));
            }
            device.context().Unmap(&staging, 0);
        }
        if bgra {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        Ok(pixels)
    }

    #[test]
    #[ignore = "requires an offscreen D3D11 VideoProcessor"]
    fn hardware_clean_source_matches_cpu_fit_filter_padding_rotation_and_reuse() -> Result<()> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(error)?;
        let adapter = unsafe { factory.EnumAdapters1(0) }
            .map_err(error)?
            .cast()
            .map_err(error)?;
        let device = SharedDevice::create(&adapter).map_err(error)?;
        let _lock = device.lock();
        for logical in [(64u32, 48u32), (63, 47), (64, 63)] {
            let coded = (logical.0.div_ceil(2) * 2, logical.1.div_ceil(2) * 2);
            let mut gpu = CleanGpuCanvas::new(device.clone(), logical, coded, 60)?;
            let mut cpu = CleanCanvas::new(logical, coded);
            for native in [(64u32, 48u32), (126, 94), (77, 53), (19, 9), (9, 29)] {
                let rgba: Vec<_> = (0..native.1)
                    .flat_map(|y| {
                        (0..native.0).flat_map(move |x| {
                            [
                                (x * 17 + y * 31) as u8,
                                (x * 3 + y * 7) as u8,
                                if x.is_multiple_of(2) { 0 } else { 255 },
                                255,
                            ]
                        })
                    })
                    .collect();
                let mut bgra = rgba.clone();
                for pixel in bgra.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                let texture = device
                    .texture(
                        native.0,
                        native.1,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
                    )
                    .map_err(error)?;
                unsafe {
                    device.context().UpdateSubresource(
                        texture.raw(),
                        0,
                        None,
                        bgra.as_ptr().cast(),
                        native.0 * 4,
                        0,
                    );
                }
                for rotation in 0..4 {
                    let size = if rotation % 2 == 0 {
                        native
                    } else {
                        (native.1, native.0)
                    };
                    let mut oriented = Vec::new();
                    for y in 0..size.1 {
                        for x in 0..size.0 {
                            let (sx, sy) = match rotation {
                                0 => (x, y),
                                1 => (y, native.1 - 1 - x),
                                2 => (native.0 - 1 - x, native.1 - 1 - y),
                                _ => (native.0 - 1 - y, x),
                            };
                            let offset = (sy * native.0 + sx) as usize * 4;
                            oriented.extend_from_slice(&rgba[offset..offset + 4]);
                        }
                    }
                    let frame =
                        snow_capture::frame::Frame::from_rgba8(size.0, size.1, oriented)?.into();
                    cpu.pixels = cpu.compose(&frame)?;
                    let frame = GpuCapturedFrame::from_layers(
                        vec![VideoLayer {
                            texture: texture.clone(),
                            source: Rect::full(native),
                            destination: Rect::full(size),
                            rotation,
                            alpha: false,
                        }],
                        size,
                        Default::default(),
                    )?;
                    let output = gpu.compose_rgb(&frame)?;
                    if native == logical && logical == coded && rotation == 0 {
                        assert_eq!(
                            output.raw(),
                            texture.raw(),
                            "native even sources need no RGB copy"
                        );
                    }
                    let actual = rgba_pixels(&device, &output)?;
                    for (index, (actual, expected)) in
                        actual.iter().zip(cpu.pixels.iter()).enumerate()
                    {
                        assert!(
                            actual.abs_diff(*expected) <= 1,
                            "{native:?}, {logical:?}, rotation {rotation}, byte {index}: {actual} != {expected}"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires an offscreen D3D11 VideoProcessor"]
    fn hardware_clean_source_crops_layers_and_clears_desktop_gaps() -> Result<()> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(error)?;
        let adapter = unsafe { factory.EnumAdapters1(0) }
            .map_err(error)?
            .cast()
            .map_err(error)?;
        let device = SharedDevice::create(&adapter).map_err(error)?;
        let _lock = device.lock();
        let native = (80, 60);
        let pixels: Vec<_> = (0..native.1)
            .flat_map(|y| {
                (0..native.0)
                    .flat_map(move |x| [(x * 17) as u8, (y * 13) as u8, (x + y) as u8, 255])
            })
            .collect();
        let texture = device
            .texture(
                native.0,
                native.1,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                D3D11_BIND_SHADER_RESOURCE.0 as u32,
            )
            .map_err(error)?;
        unsafe {
            device.context().UpdateSubresource(
                texture.raw(),
                0,
                None,
                pixels.as_ptr().cast(),
                native.0 * 4,
                0,
            );
        }
        let source_size = (61u32, 43u32);
        for logical in [source_size, (37, 29), (80, 60)] {
            let coded = (logical.0.div_ceil(2) * 2, logical.1.div_ceil(2) * 2);
            let mut gpu = CleanGpuCanvas::new(device.clone(), logical, coded, 60)?;
            let mut cpu = CleanCanvas::new(logical, coded);
            // Full crop, then two separated monitors, then a different gap.
            // Reuse the same compositor to expose stale desktop pixels.
            for rectangles in [
                vec![(7, 9, 0, 0, 61, 43)],
                vec![(3, 5, 0, 0, 23, 43), (40, 11, 31, 7, 30, 31)],
                vec![(11, 13, 19, 3, 29, 37)],
            ] {
                let mut expected = vec![0; source_size.0 as usize * source_size.1 as usize * 4];
                for pixel in expected.chunks_exact_mut(4) {
                    pixel[3] = 255;
                }
                let mut layers = Vec::new();
                for (sx, sy, dx, dy, width, height) in rectangles {
                    for y in 0..height {
                        let from = ((sy + y) * native.0 + sx) as usize * 4;
                        let to = ((dy + y) * source_size.0 + dx) as usize * 4;
                        let bytes = width as usize * 4;
                        expected[to..to + bytes].copy_from_slice(&pixels[from..from + bytes]);
                    }
                    layers.push(VideoLayer {
                        texture: texture.clone(),
                        source: Rect {
                            x: sx as i32,
                            y: sy as i32,
                            width,
                            height,
                        },
                        destination: Rect {
                            x: dx as i32,
                            y: dy as i32,
                            width,
                            height,
                        },
                        rotation: 0,
                        alpha: false,
                    });
                }
                let cpu_frame =
                    snow_capture::frame::Frame::from_rgba8(source_size.0, source_size.1, expected)?
                        .into();
                cpu.pixels = cpu.compose(&cpu_frame)?;
                let frame = GpuCapturedFrame::from_layers(layers, source_size, Default::default())?;
                let output = gpu.compose_rgb(&frame)?;
                let actual = rgba_pixels(&device, &output)?;
                assert_eq!(actual.len(), cpu.pixels.len());
                for (index, (actual, expected)) in actual.iter().zip(cpu.pixels.iter()).enumerate()
                {
                    assert!(
                        actual.abs_diff(*expected) <= 1,
                        "{logical:?}, byte {index}: {actual} != {expected}"
                    );
                }
            }
        }
        Ok(())
    }
}
