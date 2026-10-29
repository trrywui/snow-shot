use std::{
    collections::HashMap,
    mem::size_of,
    sync::{Arc, Weak},
};

use anyhow::{Context, Result, ensure};
use windows::{
    Win32::{
        Foundation::RECT,
        Graphics::{Direct3D::*, Direct3D11::*, Dxgi::Common::*},
    },
    core::s,
};

#[derive(Clone)]
pub(crate) struct Texture {
    pub image: ID3D11Texture2D,
    pub view: ID3D11ShaderResourceView,
    pub target: ID3D11RenderTargetView,
    pub size: [u32; 2],
    pub samples: u32,
    pub allocation: Arc<crate::gpu_memory::Allocation>,
}

impl Texture {
    pub fn new(
        device: &ID3D11Device,
        size: [u32; 2],
        memory: &Arc<crate::gpu_memory::GpuMemory>,
    ) -> Result<Self> {
        Self::with_samples(device, size, 1, memory)
    }

    pub fn with_samples(
        device: &ID3D11Device,
        size: [u32; 2],
        samples: u32,
        memory: &Arc<crate::gpu_memory::GpuMemory>,
    ) -> Result<Self> {
        Self::with_format(device, size, samples, DXGI_FORMAT_R8G8B8A8_UNORM, memory)
    }

    pub fn bgra(
        device: &ID3D11Device,
        size: [u32; 2],
        memory: &Arc<crate::gpu_memory::GpuMemory>,
    ) -> Result<Self> {
        Self::with_format(device, size, 1, DXGI_FORMAT_B8G8R8A8_UNORM, memory)
    }

    fn with_format(
        device: &ID3D11Device,
        size: [u32; 2],
        samples: u32,
        format: DXGI_FORMAT,
        memory: &Arc<crate::gpu_memory::GpuMemory>,
    ) -> Result<Self> {
        ensure!(
            size[0] > 0 && size[1] > 0 && size[0] <= 16384 && size[1] <= 16384,
            "Invalid GPU surface extent"
        );
        let allocation =
            memory.reserve(size[0] as usize * size[1] as usize * 4 * samples as usize)?;
        let description = D3D11_TEXTURE2D_DESC {
            Width: size[0],
            Height: size[1],
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: samples,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
            ..Default::default()
        };
        let mut image = None;
        unsafe {
            device.CreateTexture2D(&description, None, Some(&mut image))?;
        }
        let image = image.context("Creating canvas texture")?;
        let mut view = None;
        let mut target = None;
        unsafe {
            device.CreateShaderResourceView(&image, None, Some(&mut view))?;
            device.CreateRenderTargetView(&image, None, Some(&mut target))?;
        }
        Ok(Self {
            image,
            view: view.context("Creating canvas texture view")?,
            target: target.context("Creating canvas render target")?,
            size,
            samples,
            allocation,
        })
    }

    pub fn bytes(&self) -> usize {
        self.size[0] as usize * self.size[1] as usize * 4 * self.samples as usize
    }
}

#[derive(Clone)]
pub(crate) struct UploadedMesh {
    vertices: ID3D11Buffer,
    indices: ID3D11Buffer,
    count: u32,
    bytes: usize,
    _allocation: Arc<crate::gpu_memory::Allocation>,
}

#[derive(Clone, Copy, Default)]
#[repr(C, align(16))]
pub(crate) struct Parameters {
    pub target: [f32; 4],
    pub matrix: [f32; 4],
    pub translation: [f32; 4],
    pub color: [f32; 4],
    pub sample_mapping: [f32; 4],
    pub effect: [f32; 4],
    pub world: [f32; 4],
    pub gap: [f32; 4],
}

impl Parameters {
    pub fn for_target(size: [u32; 2]) -> Self {
        Self {
            target: [size[0] as f32, size[1] as f32, 0.0, 0.0],
            matrix: [1.0, 0.0, 0.0, 1.0],
            translation: [0.0, 0.0, 1.0, 0.0],
            sample_mapping: [1.0 / size[0] as f32, 1.0 / size[1] as f32, 0.0, 0.0],
            effect: [0.0, 0.0, 0.0, 1.0],
            ..Default::default()
        }
    }
}

pub(crate) struct Pipeline {
    mesh_vertex: ID3D11VertexShader,
    quad_vertex: ID3D11VertexShader,
    mesh_pixel: ID3D11PixelShader,
    effect_pixel: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    parameters: ID3D11Buffer,
    source_over: ID3D11BlendState,
    replace: ID3D11BlendState,
    coverage_union: ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
    linear: ID3D11SamplerState,
    point: ID3D11SamplerState,
    meshes: HashMap<u64, (Weak<crate::gpu_geometry::Mesh>, UploadedMesh)>,
    pub uploaded_bytes: u64,
    pub uniform_bytes: std::sync::atomic::AtomicU64,
    pub mesh_bytes: usize,
    memory: Arc<crate::gpu_memory::GpuMemory>,
}

fn buffer(device: &ID3D11Device, bytes: &[u8], binding: D3D11_BIND_FLAG) -> Result<ID3D11Buffer> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= u32::MAX as usize,
        "Invalid mesh buffer size"
    );
    let description = D3D11_BUFFER_DESC {
        ByteWidth: bytes.len() as u32,
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: binding.0 as u32,
        ..Default::default()
    };
    let initial = D3D11_SUBRESOURCE_DATA {
        pSysMem: bytes.as_ptr().cast(),
        ..Default::default()
    };
    let mut output = None;
    unsafe {
        device.CreateBuffer(&description, Some(&initial), Some(&mut output))?;
    }
    output.context("Creating immutable geometry buffer")
}

fn bytes<T: Copy>(values: &[T]) -> &[u8] {
    // Only initialized numeric arrays and repr(C) structs without padding are passed here.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

impl Pipeline {
    pub fn new(device: &ID3D11Device, memory: Arc<crate::gpu_memory::GpuMemory>) -> Result<Self> {
        let mesh_bytes = include_bytes!(concat!(env!("OUT_DIR"), "/mesh_vertex.cso"));
        let quad_bytes = include_bytes!(concat!(env!("OUT_DIR"), "/quad_vertex.cso"));
        let mut mesh_vertex = None;
        let mut quad_vertex = None;
        let mut mesh_pixel = None;
        let mut effect_pixel = None;
        let mut layout = None;
        unsafe {
            device.CreateVertexShader(mesh_bytes, None, Some(&mut mesh_vertex))?;
            device.CreateVertexShader(quad_bytes, None, Some(&mut quad_vertex))?;
            device.CreatePixelShader(
                include_bytes!(concat!(env!("OUT_DIR"), "/mesh_pixel.cso")),
                None,
                Some(&mut mesh_pixel),
            )?;
            device.CreatePixelShader(
                include_bytes!(concat!(env!("OUT_DIR"), "/effect_pixel.cso")),
                None,
                Some(&mut effect_pixel),
            )?;
            device.CreateInputLayout(
                &[D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("POSITION"),
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    ..Default::default()
                }],
                mesh_bytes,
                Some(&mut layout),
            )?;
        }
        let mut parameters = None;
        unsafe {
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: size_of::<Parameters>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut parameters),
            )?;
        }
        let blend = |enabled: bool| -> Result<ID3D11BlendState> {
            let mut output = None;
            let mut description = D3D11_BLEND_DESC::default();
            description.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: enabled.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            unsafe {
                device.CreateBlendState(&description, Some(&mut output))?;
            }
            output.context("Creating canvas blend state")
        };
        let mut rasterizer = None;
        let mut coverage_union = None;
        let mut union_description = D3D11_BLEND_DESC::default();
        union_description.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D11_BLEND_ONE,
            DestBlend: D3D11_BLEND_ONE,
            BlendOp: D3D11_BLEND_OP_MAX,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_ONE,
            BlendOpAlpha: D3D11_BLEND_OP_MAX,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        unsafe {
            device.CreateBlendState(&union_description, Some(&mut coverage_union))?;
        }
        unsafe {
            device.CreateRasterizerState(
                &D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    CullMode: D3D11_CULL_NONE,
                    DepthClipEnable: true.into(),
                    ScissorEnable: true.into(),
                    MultisampleEnable: true.into(),
                    ..Default::default()
                },
                Some(&mut rasterizer),
            )?;
        }
        let sampler = |filter| -> Result<ID3D11SamplerState> {
            let mut output = None;
            unsafe {
                device.CreateSamplerState(
                    &D3D11_SAMPLER_DESC {
                        Filter: filter,
                        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                        MaxLOD: f32::MAX,
                        ComparisonFunc: D3D11_COMPARISON_NEVER,
                        ..Default::default()
                    },
                    Some(&mut output),
                )?;
            }
            output.context("Creating canvas sampler")
        };
        Ok(Self {
            mesh_vertex: mesh_vertex.context("Creating mesh vertex shader")?,
            quad_vertex: quad_vertex.context("Creating quad vertex shader")?,
            mesh_pixel: mesh_pixel.context("Creating mesh pixel shader")?,
            effect_pixel: effect_pixel.context("Creating effect pixel shader")?,
            layout: layout.context("Creating mesh input layout")?,
            parameters: parameters.context("Creating canvas constants")?,
            source_over: blend(true)?,
            replace: blend(false)?,
            rasterizer: rasterizer.context("Creating canvas rasterizer")?,
            coverage_union: coverage_union.context("Creating coverage union blend state")?,
            linear: sampler(D3D11_FILTER_MIN_MAG_MIP_LINEAR)?,
            point: sampler(D3D11_FILTER_MIN_MAG_MIP_POINT)?,
            meshes: HashMap::new(),
            uploaded_bytes: 0,
            uniform_bytes: std::sync::atomic::AtomicU64::new(0),
            mesh_bytes: 0,
            memory,
        })
    }

    pub fn upload_mesh(
        &mut self,
        device: &ID3D11Device,
        mesh: &Arc<crate::gpu_geometry::Mesh>,
    ) -> Result<Option<UploadedMesh>> {
        if mesh.indices.is_empty() {
            return Ok(None);
        }
        let key = mesh.identity;
        if let Some((_, uploaded)) = self.meshes.get(&key) {
            return Ok(Some(uploaded.clone()));
        }
        let vertex_data = bytes(&mesh.vertices);
        let index_data = bytes(&mesh.indices);
        let allocation = self.memory.reserve(vertex_data.len() + index_data.len())?;
        let uploaded = UploadedMesh {
            vertices: buffer(device, vertex_data, D3D11_BIND_VERTEX_BUFFER)?,
            indices: buffer(device, index_data, D3D11_BIND_INDEX_BUFFER)?,
            count: u32::try_from(mesh.indices.len())?,
            bytes: vertex_data.len() + index_data.len(),
            _allocation: allocation,
        };
        self.uploaded_bytes += uploaded.bytes as u64;
        self.mesh_bytes += uploaded.bytes;
        self.meshes
            .insert(key, (Arc::downgrade(mesh), uploaded.clone()));
        Ok(Some(uploaded))
    }

    pub fn collect_meshes(&mut self) {
        self.meshes.retain(|_, (mesh, uploaded)| {
            let keep = mesh.strong_count() != 0;
            if !keep {
                self.mesh_bytes -= uploaded.bytes;
            }
            keep
        });
    }

    pub fn clear(context: &ID3D11DeviceContext, target: &Texture, color: [f32; 4]) {
        target.allocation.mark_gpu_use();
        Self::unbind(context);
        unsafe {
            context.ClearRenderTargetView(&target.target, &color);
        }
    }

    pub fn copy(context: &ID3D11DeviceContext, source: &Texture, target: &Texture) {
        source.allocation.mark_gpu_use();
        target.allocation.mark_gpu_use();
        Self::unbind(context);
        unsafe {
            context.CopyResource(&target.image, &source.image);
        }
    }

    pub fn resolve(context: &ID3D11DeviceContext, source: &Texture, target: &Texture) {
        source.allocation.mark_gpu_use();
        target.allocation.mark_gpu_use();
        Self::unbind(context);
        unsafe {
            context.ResolveSubresource(
                &target.image,
                0,
                &source.image,
                0,
                DXGI_FORMAT_R8G8B8A8_UNORM,
            );
        }
    }

    pub fn unbind(context: &ID3D11DeviceContext) {
        unsafe {
            context.PSSetShaderResources(0, Some(&[None, None, None]));
            context.OMSetRenderTargets(None, None);
        }
    }

    fn bind(
        &self,
        context: &ID3D11DeviceContext,
        size: [u32; 2],
        target: &ID3D11RenderTargetView,
        parameters: &Parameters,
        blend: bool,
        scissor: Option<[i32; 4]>,
    ) {
        self.uniform_bytes.fetch_add(
            size_of::<Parameters>() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        Self::unbind(context);
        let rectangle = scissor.unwrap_or([0, 0, size[0] as i32, size[1] as i32]);
        unsafe {
            context.UpdateSubresource(
                &self.parameters,
                0,
                None,
                (parameters as *const Parameters).cast(),
                0,
                0,
            );
            context.VSSetConstantBuffers(0, Some(&[Some(self.parameters.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(self.parameters.clone())]));
            context.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            context.OMSetBlendState(
                if blend {
                    &self.source_over
                } else {
                    &self.replace
                },
                None,
                u32::MAX,
            );
            context.OMSetDepthStencilState(None, 0);
            context.RSSetState(&self.rasterizer);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: size[0] as f32,
                Height: size[1] as f32,
                MaxDepth: 1.0,
                ..Default::default()
            }]));
            context.RSSetScissorRects(Some(&[RECT {
                left: rectangle[0],
                top: rectangle[1],
                right: rectangle[2],
                bottom: rectangle[3],
            }]));
            context.GSSetShader(None, None);
            context.HSSetShader(None, None);
            context.DSSetShader(None, None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.PSSetSamplers(
                0,
                Some(&[Some(self.linear.clone()), Some(self.point.clone())]),
            );
        }
    }

    pub fn mesh(
        &self,
        context: &ID3D11DeviceContext,
        target: &Texture,
        mesh: &UploadedMesh,
        parameters: &Parameters,
        scissor: Option<[i32; 4]>,
    ) {
        self.draw_mesh(context, target, mesh, parameters, scissor, None);
    }

    pub fn textured_mesh(
        &self,
        context: &ID3D11DeviceContext,
        target: &Texture,
        mesh: &UploadedMesh,
        texture: &Texture,
        parameters: &Parameters,
    ) {
        texture.allocation.mark_gpu_use();
        self.draw_mesh(context, target, mesh, parameters, None, Some(texture));
    }

    fn draw_mesh(
        &self,
        context: &ID3D11DeviceContext,
        target: &Texture,
        mesh: &UploadedMesh,
        parameters: &Parameters,
        scissor: Option<[i32; 4]>,
        texture: Option<&Texture>,
    ) {
        target.allocation.mark_gpu_use();
        mesh._allocation.mark_gpu_use();
        self.bind(
            context,
            target.size,
            &target.target,
            parameters,
            true,
            scissor,
        );
        unsafe {
            if parameters.target[3] == 1.0 {
                context.OMSetBlendState(&self.coverage_union, None, u32::MAX);
            }
            context.IASetInputLayout(&self.layout);
            context.IASetVertexBuffers(
                0,
                1,
                Some([Some(mesh.vertices.clone())].as_ptr()),
                Some(&8),
                Some(&0),
            );
            context.IASetIndexBuffer(&mesh.indices, DXGI_FORMAT_R32_UINT, 0);
            context.VSSetShader(&self.mesh_vertex, None);
            context.PSSetShader(&self.mesh_pixel, None);
            if let Some(texture) = texture {
                context.PSSetShaderResources(0, Some(&[Some(texture.view.clone())]));
            }
            context.DrawIndexed(mesh.count, 0, 0);
        }
        if texture.is_some() {
            Self::unbind(context);
        }
    }

    pub fn effect(
        &self,
        context: &ID3D11DeviceContext,
        target: &Texture,
        inputs: [Option<&Texture>; 3],
        parameters: &Parameters,
        blend: bool,
        scissor: Option<[i32; 4]>,
    ) {
        for texture in inputs.iter().flatten() {
            texture.allocation.mark_gpu_use();
        }
        self.effect_views(
            context,
            target,
            inputs.map(|texture| texture.map(|texture| texture.view.clone())),
            parameters,
            blend,
            scissor,
        );
    }

    pub fn effect_views(
        &self,
        context: &ID3D11DeviceContext,
        target: &Texture,
        inputs: [Option<ID3D11ShaderResourceView>; 3],
        parameters: &Parameters,
        blend: bool,
        scissor: Option<[i32; 4]>,
    ) {
        target.allocation.mark_gpu_use();
        self.bind(
            context,
            target.size,
            &target.target,
            parameters,
            blend,
            scissor,
        );
        unsafe {
            context.IASetInputLayout(None);
            context.VSSetShader(&self.quad_vertex, None);
            context.PSSetShader(&self.effect_pixel, None);
            context.PSSetShaderResources(0, Some(&inputs));
            context.Draw(3, 0);
        }
        Self::unbind(context);
    }
}
