//! GPU-resident source images, including completed reconstruction patches. The
//! original texture stays immutable and pinned through every submitted use.
use super::*;
use gpui_windows::gpu_extensions::DirectXGpuTexture;

impl GpuCanvas {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_gpu_source(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        target: &Texture,
        source: &SourceImage,
        frame: FrameView,
        scale: f32,
        region: Region,
        opacity: f64,
        mapping: ([f64; 2], [f64; 2], [f64; 2]),
    ) -> Result<()> {
        let texture = source.gpu_texture().context("GPU source missing")?;
        ensure!(
            texture.service_id() == self.service.service_id()
                && texture.epoch() == self.service.epoch(),
            "GPU source belongs to another device or a retired epoch"
        );
        let native = texture
            .as_any()
            .downcast_ref::<DirectXGpuTexture>()
            .context("GPU source requires a D3D11 texture")?;
        let [width, height] = source.dimensions();
        let zoom = frame.camera.zoom * f64::from(scale);
        let units = source.units_per_pixel();
        let origin = source.origin();
        let (destination, center, factor) = mapping;
        let image_start = canvas_to_view(
            Point::new(
                destination[0] + (origin.x - center[0]) * factor[0],
                destination[1] + (origin.y - center[1]) * factor[1],
            ),
            &frame.camera,
            frame.surface,
        );
        let image_end = canvas_to_view(
            Point::new(
                destination[0] + (origin.x + width as f64 * units - center[0]) * factor[0],
                destination[1] + (origin.y + height as f64 * units - center[1]) * factor[1],
            ),
            &frame.camera,
            frame.surface,
        );
        let bounds = [
            image_start.x * scale as f64,
            image_start.y * scale as f64,
            image_end.x * scale as f64,
            image_end.y * scale as f64,
        ];
        if !intersects(bounds, region.bounds()) {
            return Ok(());
        }
        if !self
            .current_sources
            .iter()
            .any(|old| Arc::ptr_eq(old, texture))
        {
            self.current_sources.push(texture.clone());
        }
        let lod = factor.map(|factor| source_lod(zoom * units * factor));
        let step = lod.map(|lod| 1_u32 << lod);
        let key = (source.pixels_identity(), 0, 0, lod[0], lod[1]);
        let (view, span) = if step == [1, 1] {
            (
                native.shader_resource_view().clone(),
                [width as f64, height as f64],
            )
        } else {
            if !self.sources.contains_key(&key) {
                self.evict_source_pages();
                let factor = step.map(|step| if step > 1 { 2 } else { 1 });
                let size = [width.div_ceil(factor[0]), height.div_ceil(factor[1])];
                let first = self.scratch.take(device, size, 1, &self.memory)?;
                let mut parameters = Parameters::for_target(size);
                parameters.sample_mapping = [
                    factor[0] as f32 / width as f32,
                    factor[1] as f32 / height as f32,
                    0.0,
                    0.0,
                ];
                self.pipeline
                    .as_ref()
                    .context("Pipeline missing")?
                    .effect_views(
                        context,
                        &first,
                        [Some(native.shader_resource_view().clone()), None, None],
                        &parameters,
                        false,
                        None,
                    );
                let reduced = self.reduce_source(
                    device,
                    context,
                    first,
                    [step[0] / factor[0], step[1] / factor[1]],
                )?;
                self.sources.insert(
                    key,
                    SourcePage {
                        texture: reduced,
                        rect: [0, 0, width, height],
                        last_use: self.sequence,
                    },
                );
            }
            let page = self
                .sources
                .get_mut(&key)
                .context("GPU source LOD missing")?;
            page.last_use = self.sequence;
            page.texture.allocation.mark_gpu_use();
            (
                page.texture.view.clone(),
                [
                    f64::from(page.texture.size[0]) * f64::from(step[0]),
                    f64::from(page.texture.size[1]) * f64::from(step[1]),
                ],
            )
        };
        let world = view_to_canvas(
            Point::new(
                region.x as f64 / scale as f64,
                region.y as f64 / scale as f64,
            ),
            &frame.camera,
            frame.surface,
        );
        let mut parameters = Parameters::for_target(region.size());
        parameters.sample_mapping = [
            (1.0 / zoom / factor[0] / units / span[0]) as f32,
            (1.0 / zoom / factor[1] / units / span[1]) as f32,
            (((world.x - destination[0]) / factor[0] + center[0] - origin.x) / units / span[0])
                as f32,
            (((world.y - destination[1]) / factor[1] + center[1] - origin.y) / units / span[1])
                as f32,
        ];
        parameters.effect = [17.0, 0.0, 0.0, opacity as f32];
        parameters.gap = [
            (bounds[0] - region.x as f64) as f32,
            (bounds[1] - region.y as f64) as f32,
            (bounds[2] - region.x as f64) as f32,
            (bounds[3] - region.y as f64) as f32,
        ];
        self.pipeline
            .as_ref()
            .context("Pipeline missing")?
            .effect_views(
                context,
                target,
                [Some(view), None, None],
                &parameters,
                true,
                Some(local_scissor(bounds, region)),
            );
        Ok(())
    }

    pub(super) fn evict_source_pages(&mut self) {
        let cache_budget = (self.memory.limit() / 3).min(128 * 1024 * 1024);
        while self
            .sources
            .values()
            .map(|page| page.texture.bytes())
            .sum::<usize>()
            > cache_budget
        {
            let oldest = self
                .sources
                .iter()
                .filter(|(_, page)| page.last_use != self.sequence)
                .min_by_key(|(_, page)| page.last_use)
                .map(|(key, _)| *key);
            let Some(oldest) = oldest else {
                break;
            };
            if let Some(old) = self.sources.remove(&oldest) {
                self.current_resources.push(old.texture);
            }
        }
    }
}
