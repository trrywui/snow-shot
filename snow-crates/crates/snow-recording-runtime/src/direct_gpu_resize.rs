//! Deterministic GPU pixel-area reduction before chroma subsampling.
//! Crops and rotations are resolved at load time, never by a driver sampler.

use snow_d3d11::{SharedDevice, Texture, VideoLayer};
use windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_BUFFER;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, PCSTR};

use super::{Result, gpu_error};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct AxisSpan {
    first: u32,
    count: u32,
    first_weight: f32,
    last_weight: f32,
}

fn axis_spans(source: u32, output: u32) -> Vec<AxisSpan> {
    assert!(source != 0 && output != 0);
    (0..output)
        .map(|index| {
            if output >= source {
                return AxisSpan {
                    first: (u64::from(index) * u64::from(source) / u64::from(output)) as u32,
                    count: 1,
                    first_weight: 1.0,
                    last_weight: 1.0,
                };
            }
            // Integer coverage avoids shifting an endpoint across a pixel due to
            // floating-point division, especially at exact integer reductions.
            let denominator = u64::from(output);
            let left = u64::from(index) * u64::from(source);
            let right = u64::from(index + 1) * u64::from(source);
            let first = left / denominator;
            let last = right.div_ceil(denominator) - 1;
            AxisSpan {
                first: first as u32,
                count: (last - first + 1) as u32,
                first_weight: (((first + 1) * denominator).min(right) - left) as f32
                    / source as f32,
                last_weight: (right - (last * denominator).max(left)) as f32 / source as f32,
            }
        })
        .collect()
}

#[repr(C)]
struct Constants {
    source: [u32; 2],
    output: [u32; 2],
    crop: [i32; 2],
    rotation: u32,
    interior_weight: f32,
    destination: [i32; 2],
    dispatch: [u32; 2],
}

struct Plan {
    source: (u32, u32),
    size: (u32, u32),
    horizontal_spans: ID3D11ShaderResourceView,
    vertical_spans: ID3D11ShaderResourceView,
    intermediate_input: ID3D11ShaderResourceView,
    intermediate_output: ID3D11UnorderedAccessView,
    output: Texture,
    output_view: ID3D11UnorderedAccessView,
}

pub(crate) struct AreaScaler {
    device: SharedDevice,
    horizontal: ID3D11ComputeShader,
    vertical: ID3D11ComputeShader,
    constants: ID3D11Buffer,
    plan: Option<Plan>,
    // Retain raw COM resources, not the capture pool's immutable Texture leases.
    // A bounded cache accommodates the pool without preventing lease reuse.
    source_views: Vec<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    source_geometry: Option<((u32, u32), snow_d3d11::Rect, u32)>,
}

impl AreaScaler {
    pub fn new(device: SharedDevice) -> Result<Self> {
        let _lock = device.lock();
        let horizontal = compile(&device, c"horizontal")?;
        let vertical = compile(&device, c"vertical")?;
        let mut constants = None;
        unsafe {
            device.device().CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<Constants>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )
        }
        .map_err(gpu_error)?;
        let constants = constants.ok_or_else(|| gpu_error("missing resize constants"))?;
        drop(_lock);
        Ok(Self {
            device,
            horizontal,
            vertical,
            constants,
            plan: None,
            source_views: Vec::new(),
            source_geometry: None,
        })
    }

    pub fn resize(&mut self, layer: &VideoLayer, size: (u32, u32)) -> Result<Texture> {
        let device = self.device.clone();
        let _lock = device.lock();
        let crop = layer.source;
        let dimensions = layer.texture.dimensions();
        if !device.same_device(layer.texture.device())
            || layer.rotation >= 4
            || crop.x < 0
            || crop.y < 0
            || crop.width == 0
            || crop.height == 0
            || u64::from(crop.x as u32) + u64::from(crop.width) > u64::from(dimensions.0)
            || u64::from(crop.y as u32) + u64::from(crop.height) > u64::from(dimensions.1)
            || size.0 == 0
            || size.1 == 0
        {
            return Err(gpu_error(
                "invalid GPU resize source crop or output geometry",
            ));
        }
        let source = if layer.rotation.is_multiple_of(2) {
            (crop.width, crop.height)
        } else {
            (crop.height, crop.width)
        };
        let geometry = (dimensions, crop, layer.rotation);
        if self.source_geometry != Some(geometry) {
            self.source_views.clear();
            self.source_geometry = Some(geometry);
        }
        if self
            .plan
            .as_ref()
            .is_none_or(|plan| plan.source != source || plan.size != size)
        {
            self.source_views.clear();
            self.plan = Some(Plan::new(&device, source, size)?);
        }
        let input = self.source_view(&layer.texture)?;
        let plan = self.plan.as_ref().unwrap();
        self.dispatch(
            &self.horizontal,
            input,
            plan.horizontal_spans.clone(),
            plan.intermediate_output.clone(),
            Constants {
                source: [source.0, source.1],
                output: [size.0, source.1],
                crop: [crop.x, crop.y],
                rotation: layer.rotation,
                interior_weight: size.0 as f32 / source.0 as f32,
                destination: [0, 0],
                dispatch: [size.0, source.1],
            },
        );
        self.dispatch(
            &self.vertical,
            plan.intermediate_input.clone(),
            plan.vertical_spans.clone(),
            plan.output_view.clone(),
            Constants {
                source: [size.0, source.1],
                output: [size.0, size.1],
                crop: [0, 0],
                rotation: 0,
                interior_weight: size.1 as f32 / source.1 as f32,
                destination: [0, 0],
                dispatch: [size.0, size.1],
            },
        );
        device.check().map_err(gpu_error)?;
        Ok(plan.output.clone())
    }

    pub fn clear_source_views(&mut self) {
        self.source_views.clear();
        self.source_geometry = None;
    }

    #[cfg(test)]
    pub fn cached_view_count(&self) -> usize {
        self.source_views.len()
    }

    fn source_view(&mut self, texture: &Texture) -> Result<ID3D11ShaderResourceView> {
        if let Some((_, view)) = self
            .source_views
            .iter()
            .find(|(source, _)| source.as_raw() == texture.raw().as_raw())
        {
            return Ok(view.clone());
        }
        let mut view = None;
        unsafe {
            self.device
                .device()
                .CreateShaderResourceView(texture.raw(), None, Some(&mut view))
        }
        .map_err(gpu_error)?;
        let view = view.ok_or_else(|| gpu_error("missing resize source view"))?;
        if self.source_views.len() == 6 {
            self.source_views.remove(0);
        }
        self.source_views
            .push((texture.raw().clone(), view.clone()));
        Ok(view)
    }

    fn dispatch(
        &self,
        shader: &ID3D11ComputeShader,
        input: ID3D11ShaderResourceView,
        spans: ID3D11ShaderResourceView,
        output: ID3D11UnorderedAccessView,
        constants: Constants,
    ) {
        let context = self.device.context();
        unsafe {
            context.UpdateSubresource(
                &self.constants,
                0,
                None,
                (&constants as *const Constants).cast(),
                0,
                0,
            );
            context.CSSetShader(shader, None);
            context.CSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            context.CSSetShaderResources(0, Some(&[Some(input), Some(spans)]));
            context.CSSetUnorderedAccessViews(0, 1, Some(&Some(output)), None);
            context.Dispatch(
                constants.output[0].div_ceil(16),
                constants.output[1].div_ceil(16),
                1,
            );
            // Explicitly unbind to permit the next pass to read the previous UAV
            // and to leave the shared immediate context ready for VideoProcessor.
            context.CSSetShaderResources(0, Some(&[None, None]));
            context.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
            context.CSSetConstantBuffers(0, Some(&[None]));
            context.CSSetShader(None, None);
        }
    }
}

impl Plan {
    fn new(device: &SharedDevice, source: (u32, u32), size: (u32, u32)) -> Result<Self> {
        let binds = (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS).0 as u32;
        let intermediate = device
            .texture(size.0, source.1, DXGI_FORMAT_R16G16B16A16_FLOAT, binds)
            .map_err(gpu_error)?;
        let output = device
            .texture(
                size.0,
                size.1,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                binds | D3D11_BIND_RENDER_TARGET.0 as u32,
            )
            .map_err(gpu_error)?;
        let mut intermediate_input = None;
        let mut intermediate_output = None;
        let mut output_view = None;
        unsafe {
            device
                .device()
                .CreateShaderResourceView(intermediate.raw(), None, Some(&mut intermediate_input))
                .map_err(gpu_error)?;
            device
                .device()
                .CreateUnorderedAccessView(intermediate.raw(), None, Some(&mut intermediate_output))
                .map_err(gpu_error)?;
            device
                .device()
                .CreateUnorderedAccessView(output.raw(), None, Some(&mut output_view))
                .map_err(gpu_error)?;
        }
        Ok(Self {
            source,
            size,
            horizontal_spans: span_view(device, &axis_spans(source.0, size.0))?,
            vertical_spans: span_view(device, &axis_spans(source.1, size.1))?,
            intermediate_input: intermediate_input
                .ok_or_else(|| gpu_error("missing resize intermediate SRV"))?,
            intermediate_output: intermediate_output
                .ok_or_else(|| gpu_error("missing resize intermediate UAV"))?,
            output,
            output_view: output_view.ok_or_else(|| gpu_error("missing resize output UAV"))?,
        })
    }
}

/// Exact opaque native crop placement. It reuses the compositor's existing
/// RGBA surface and does not allocate area-filter intermediates.
pub(crate) struct CropCopy {
    device: SharedDevice,
    shader: ID3D11ComputeShader,
    constants: ID3D11Buffer,
    source_views: Vec<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    target: Option<(ID3D11Texture2D, ID3D11UnorderedAccessView)>,
    geometry: Vec<((u32, u32), snow_d3d11::Rect, snow_d3d11::Rect, u32)>,
}

impl CropCopy {
    pub fn new(device: SharedDevice) -> Result<Self> {
        let _lock = device.lock();
        let shader = compile(&device, c"copyCrop")?;
        let mut constants = None;
        unsafe {
            device.device().CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<Constants>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )
        }
        .map_err(gpu_error)?;
        let constants = constants.ok_or_else(|| gpu_error("missing crop copy constants"))?;
        drop(_lock);
        Ok(Self {
            device,
            shader,
            constants,
            source_views: Vec::new(),
            target: None,
            geometry: Vec::new(),
        })
    }

    pub fn clear_views(&mut self) {
        self.source_views.clear();
        self.target = None;
        self.geometry.clear();
    }

    #[cfg(test)]
    pub fn cached_views(&self) -> (usize, bool) {
        (self.source_views.len(), self.target.is_some())
    }

    pub fn copy_into(&mut self, layers: &[VideoLayer], output: &Texture) -> Result<()> {
        let device = self.device.clone();
        let _lock = device.lock();
        if self.geometry.len() != layers.len()
            || self.geometry.iter().zip(layers).any(|(old, layer)| {
                *old != (
                    layer.texture.dimensions(),
                    layer.source,
                    layer.destination,
                    layer.rotation,
                )
            })
        {
            self.clear_views();
            self.geometry.extend(layers.iter().map(|layer| {
                (
                    layer.texture.dimensions(),
                    layer.source,
                    layer.destination,
                    layer.rotation,
                )
            }));
        }
        if !device.same_device(output.device())
            || output.desc().Format != DXGI_FORMAT_R8G8B8A8_UNORM
            || output.desc().BindFlags & D3D11_BIND_UNORDERED_ACCESS.0 as u32 == 0
        {
            return Err(gpu_error("invalid crop copy output"));
        }
        if self
            .target
            .as_ref()
            .is_none_or(|(texture, _)| texture.as_raw() != output.raw().as_raw())
        {
            let mut view = None;
            unsafe {
                device
                    .device()
                    .CreateUnorderedAccessView(output.raw(), None, Some(&mut view))
            }
            .map_err(gpu_error)?;
            self.target = Some((
                output.raw().clone(),
                view.ok_or_else(|| gpu_error("missing crop target UAV"))?,
            ));
        }
        let target = self.target.as_ref().unwrap().1.clone();
        let size = output.dimensions();
        let context = device.context();
        if layers.len() != 1 || layers[0].destination != snow_d3d11::Rect::full(size) {
            unsafe { context.ClearUnorderedAccessViewFloat(&target, &[0.0, 0.0, 0.0, 1.0]) };
        }
        for layer in layers {
            let crop = layer.source;
            let native = layer.texture.dimensions();
            let oriented = if layer.rotation.is_multiple_of(2) {
                (crop.width, crop.height)
            } else {
                (crop.height, crop.width)
            };
            if !device.same_device(layer.texture.device())
                || layer.rotation >= 4
                || layer.alpha
                || crop.x < 0
                || crop.y < 0
                || crop.width == 0
                || crop.height == 0
                || u64::from(crop.x as u32) + u64::from(crop.width) > u64::from(native.0)
                || u64::from(crop.y as u32) + u64::from(crop.height) > u64::from(native.1)
                || oriented != (layer.destination.width, layer.destination.height)
            {
                return Err(gpu_error("invalid opaque native GPU crop geometry"));
            }
            let input = if let Some((_, view)) = self
                .source_views
                .iter()
                .find(|(texture, _)| texture.as_raw() == layer.texture.raw().as_raw())
            {
                view.clone()
            } else {
                let mut view = None;
                unsafe {
                    device.device().CreateShaderResourceView(
                        layer.texture.raw(),
                        None,
                        Some(&mut view),
                    )
                }
                .map_err(gpu_error)?;
                let view = view.ok_or_else(|| gpu_error("missing crop input SRV"))?;
                if self.source_views.len() == 6 {
                    self.source_views.remove(0);
                }
                self.source_views
                    .push((layer.texture.raw().clone(), view.clone()));
                view
            };
            let constants = Constants {
                source: [oriented.0, oriented.1],
                output: [size.0, size.1],
                crop: [crop.x, crop.y],
                rotation: layer.rotation,
                interior_weight: 1.0,
                destination: [layer.destination.x, layer.destination.y],
                dispatch: [oriented.0, oriented.1],
            };
            unsafe {
                context.UpdateSubresource(
                    &self.constants,
                    0,
                    None,
                    (&constants as *const Constants).cast(),
                    0,
                    0,
                );
                context.CSSetShader(&self.shader, None);
                context.CSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
                context.CSSetShaderResources(0, Some(&[Some(input)]));
                context.CSSetUnorderedAccessViews(0, 1, Some(&Some(target.clone())), None);
                context.Dispatch(oriented.0.div_ceil(16), oriented.1.div_ceil(16), 1);
                context.CSSetShaderResources(0, Some(&[None]));
                context.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
                context.CSSetConstantBuffers(0, Some(&[None]));
                context.CSSetShader(None, None);
            }
        }
        device.check().map_err(gpu_error)
    }
}

fn span_view(device: &SharedDevice, spans: &[AxisSpan]) -> Result<ID3D11ShaderResourceView> {
    let mut buffer = None;
    let mut view = None;
    unsafe {
        device
            .device()
            .CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of_val(spans) as u32,
                    Usage: D3D11_USAGE_IMMUTABLE,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
                    StructureByteStride: std::mem::size_of::<AxisSpan>() as u32,
                    ..Default::default()
                },
                Some(&D3D11_SUBRESOURCE_DATA {
                    pSysMem: spans.as_ptr().cast(),
                    ..Default::default()
                }),
                Some(&mut buffer),
            )
            .map_err(gpu_error)?;
        device
            .device()
            .CreateShaderResourceView(
                &buffer.ok_or_else(|| gpu_error("missing resize span buffer"))?,
                Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_UNKNOWN,
                    ViewDimension: D3D11_SRV_DIMENSION_BUFFER,
                    Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                        Buffer: D3D11_BUFFER_SRV {
                            Anonymous1: D3D11_BUFFER_SRV_0 { FirstElement: 0 },
                            Anonymous2: D3D11_BUFFER_SRV_1 {
                                NumElements: spans.len() as u32,
                            },
                        },
                    },
                }),
                Some(&mut view),
            )
            .map_err(gpu_error)?;
    }
    view.ok_or_else(|| gpu_error("missing resize span view"))
}

fn compile(device: &SharedDevice, entry: &std::ffi::CStr) -> Result<ID3D11ComputeShader> {
    let source = include_str!("recording_resize.hlsl");
    let mut bytecode = None;
    let mut errors = None;
    unsafe {
        windows::Win32::Graphics::Direct3D::Fxc::D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            None,
            None,
            None,
            PCSTR(entry.as_ptr().cast()),
            PCSTR(c"cs_5_0".as_ptr().cast()),
            windows::Win32::Graphics::Direct3D::Fxc::D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut bytecode,
            Some(&mut errors),
        )
        .map_err(|error| {
            let diagnostic = errors.as_ref().map_or_else(String::new, |blob| {
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    blob.GetBufferPointer().cast::<u8>(),
                    blob.GetBufferSize(),
                ))
                .into_owned()
            });
            gpu_error(format!("resize shader {entry:?}: {error}; {diagnostic}"))
        })?;
    }
    let bytecode = bytecode.ok_or_else(|| gpu_error("missing resize shader bytecode"))?;
    let mut shader = None;
    unsafe {
        device.device().CreateComputeShader(
            std::slice::from_raw_parts(
                bytecode.GetBufferPointer().cast(),
                bytecode.GetBufferSize(),
            ),
            None,
            Some(&mut shader),
        )
    }
    .map_err(gpu_error)?;
    shader.ok_or_else(|| gpu_error("missing resize shader"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_spans_cover_integer_fractional_identity_and_single_pixel_axes() {
        for (source, output) in [
            (256, 128),
            (192, 128),
            (191, 127),
            (1, 1),
            (127, 127),
            (129, 1),
            (1, 4),
            (19, 34),
        ] {
            let spans = axis_spans(source, output);
            for span in spans {
                assert!(span.count > 0 && span.first + span.count <= source);
                let total = if span.count == 1 {
                    span.first_weight
                } else {
                    span.first_weight
                        + span.last_weight
                        + (span.count - 2) as f32 * output as f32 / source as f32
                };
                assert!((total - 1.0).abs() < 1e-6);
            }
        }
    }
}
