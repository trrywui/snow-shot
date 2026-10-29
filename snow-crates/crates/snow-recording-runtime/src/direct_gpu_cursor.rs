//! Cursor/effect draws preserve the desktop's BGR channel order at the video
//! processor boundary. Pixel render targets support BGRA/BGRX directly, unlike
//! the RGBA typed UAV used by the former compute pass.

use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::core::PCSTR;

use super::*;

pub(super) struct CursorRenderer {
    device: SharedDevice,
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    constants: ID3D11Buffer,
    cursor: Option<(u64, ID3D11ShaderResourceView)>,
    // Only compositor-owned desktop/scratch textures enter these bounded caches.
    backgrounds: Vec<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    targets: Vec<(ID3D11Texture2D, ID3D11RenderTargetView)>,
}

impl CursorRenderer {
    pub(super) fn new(device: SharedDevice) -> Result<Self> {
        let vertex_code = compile(c"vertex", c"vs_5_0")?;
        let pixel_code = compile(c"pixel", c"ps_5_0")?;
        let mut vertex = None;
        let mut pixel = None;
        unsafe {
            device
                .device()
                .CreateVertexShader(bytecode(&vertex_code), None, Some(&mut vertex))
                .map_err(gpu_error)?;
            device
                .device()
                .CreatePixelShader(bytecode(&pixel_code), None, Some(&mut pixel))
                .map_err(gpu_error)?;
        }
        let mut constants = None;
        unsafe {
            device.device().CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 48,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )
        }
        .map_err(gpu_error)?;
        Ok(Self {
            device,
            vertex: vertex.ok_or_else(|| gpu_error("missing cursor vertex shader"))?,
            pixel: pixel.ok_or_else(|| gpu_error("missing cursor pixel shader"))?,
            constants: constants.ok_or_else(|| gpu_error("missing cursor constants"))?,
            cursor: None,
            backgrounds: Vec::new(),
            targets: Vec::new(),
        })
    }

    pub(super) fn cursor(
        &mut self,
        background: &Texture,
        output: &Texture,
        cursor: &AttachedCursorSample,
        shape: &CursorShape,
        source_size: (u32, u32),
    ) -> Result<u64> {
        let device = self.device.clone();
        let _lock = device.lock();
        let mut uploaded = 0;
        if self
            .cursor
            .as_ref()
            .is_none_or(|(id, _)| *id != shape.shape_id.get())
        {
            if shape.shape_rgba.len() != shape.width as usize * shape.height as usize * 4 {
                return Err(gpu_error("invalid cursor shape"));
            }
            let texture = device
                .texture(
                    shape.width,
                    shape.height,
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                    D3D11_BIND_SHADER_RESOURCE.0 as u32,
                )
                .map_err(gpu_error)?;
            let mut view = None;
            unsafe {
                device.context().UpdateSubresource(
                    texture.raw(),
                    0,
                    None,
                    shape.shape_rgba.as_ptr().cast(),
                    shape.width * 4,
                    0,
                );
                device
                    .device()
                    .CreateShaderResourceView(texture.raw(), None, Some(&mut view))
                    .map_err(gpu_error)?;
            }
            uploaded = shape.shape_rgba.len() as u64;
            self.cursor = Some((
                shape.shape_id.get(),
                view.ok_or_else(|| gpu_error("missing cursor view"))?,
            ));
        }
        let size = output.dimensions();
        let origin = scale_point(cursor.x, cursor.y, source_size, size);
        let constants = [
            size.0 as i32,
            size.1 as i32,
            origin.0 - scale_coordinate(shape.hotspot_x as i32, source_size.0, size.0),
            origin.1 - scale_coordinate(shape.hotspot_y as i32, source_size.1, size.1),
            ((u64::from(shape.width) * u64::from(size.0) / u64::from(source_size.0)).max(1)) as i32,
            ((u64::from(shape.height) * u64::from(size.1) / u64::from(source_size.1)).max(1))
                as i32,
            shape.width as i32,
            shape.height as i32,
            i32::from(shape.composition_mode == CursorCompositionMode::MaskedColor),
            0,
            0,
            0,
        ];
        self.draw(
            background,
            output,
            &constants,
            Some(self.cursor.as_ref().unwrap().1.clone()),
        )?;
        Ok(uploaded)
    }

    pub(super) fn highlight(
        &mut self,
        background: &Texture,
        output: &Texture,
        center: (i32, i32),
        color: [u8; 4],
        radius: u32,
    ) -> Result<()> {
        let size = output.dimensions();
        self.draw(
            background,
            output,
            &[
                size.0 as i32,
                size.1 as i32,
                center.0,
                center.1,
                i32::from(color[0]),
                i32::from(color[1]),
                i32::from(color[2]),
                i32::from(color[3]),
                2,
                radius as i32,
                0,
                0,
            ],
            None,
        )
    }

    fn draw(
        &mut self,
        background: &Texture,
        output: &Texture,
        constants: &[i32; 12],
        cursor: Option<ID3D11ShaderResourceView>,
    ) -> Result<()> {
        let device = self.device.clone();
        let _lock = device.lock();
        if !device.same_device(background.device())
            || !device.same_device(output.device())
            || background.raw() == output.raw()
            || background.dimensions() != output.dimensions()
            || !matches!(
                output.desc().Format,
                DXGI_FORMAT_B8G8R8X8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM
            )
        {
            return Err(gpu_error(
                "cursor pass requires separate, matching BGR render targets",
            ));
        }
        let background_view = if let Some((_, view)) = self
            .backgrounds
            .iter()
            .find(|(raw, _)| raw == background.raw())
        {
            view.clone()
        } else {
            let mut view = None;
            unsafe {
                device
                    .device()
                    .CreateShaderResourceView(background.raw(), None, Some(&mut view))
            }
            .map_err(gpu_error)?;
            let view = view.ok_or_else(|| gpu_error("missing cursor background view"))?;
            if self.backgrounds.len() == 3 {
                self.backgrounds.remove(0);
            }
            self.backgrounds
                .push((background.raw().clone(), view.clone()));
            view
        };
        let target =
            if let Some((_, view)) = self.targets.iter().find(|(raw, _)| raw == output.raw()) {
                view.clone()
            } else {
                let mut view = None;
                unsafe {
                    device
                        .device()
                        .CreateRenderTargetView(output.raw(), None, Some(&mut view))
                }
                .map_err(gpu_error)?;
                let view = view.ok_or_else(|| gpu_error("missing cursor render target"))?;
                if self.targets.len() == 3 {
                    self.targets.remove(0);
                }
                self.targets.push((output.raw().clone(), view.clone()));
                view
            };
        unsafe {
            device.context().UpdateSubresource(
                &self.constants,
                0,
                None,
                constants.as_ptr().cast(),
                0,
                0,
            );
        }
        // Bind every graphics state this pass consumes and release the views
        // before the video processor or the next overlay reuses either texture.
        let context = device.context();
        let size = output.dimensions();
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.HSSetShader(None, None);
            context.DSSetShader(None, None);
            context.GSSetShader(None, None);
            context.PSSetShader(&self.pixel, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(background_view), cursor]));
            context.RSSetState(None);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: size.0 as f32,
                Height: size.1 as f32,
                MaxDepth: 1.0,
                ..Default::default()
            }]));
            context.OMSetBlendState(None, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            context.OMSetRenderTargets(Some(&[Some(target)]), None);
            context.Draw(3, 0);
            context.PSSetShaderResources(0, Some(&[None, None]));
            context.OMSetRenderTargets(None, None);
            context.PSSetConstantBuffers(0, Some(&[None]));
            context.PSSetShader(None, None);
            context.VSSetShader(None, None);
        }
        device.check().map_err(gpu_error)
    }
}

fn bytecode(blob: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()) }
}

fn compile(entry: &std::ffi::CStr, profile: &std::ffi::CStr) -> Result<ID3DBlob> {
    let source = include_str!("recording_cursor.hlsl");
    let mut code = None;
    let mut errors = None;
    unsafe {
        windows::Win32::Graphics::Direct3D::Fxc::D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            None,
            None,
            None,
            PCSTR(entry.as_ptr().cast()),
            PCSTR(profile.as_ptr().cast()),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    }
    .map_err(|error| {
        let message = errors
            .as_ref()
            .map(|blob| String::from_utf8_lossy(bytecode(blob)));
        gpu_error(format!(
            "compile cursor shader {entry:?}: {error:#}, {message:?}"
        ))
    })?;
    code.ok_or_else(|| gpu_error("missing cursor shader bytecode"))
}
