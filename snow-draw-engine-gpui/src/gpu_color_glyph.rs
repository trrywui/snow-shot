//! Windows color-font rasterization on the exact D3D11 device owned by GPUI.
//! DirectWrite consumes our already shaped glyph IDs; Direct2D handles native
//! COLR, SVG and bitmap formats without a CPU scene raster or another device.
use std::{collections::HashMap, mem::ManuallyDrop, sync::Arc};

use anyhow::{Result, ensure};
use windows::{
    Win32::Graphics::{
        Direct2D::{Common::*, *},
        Direct3D11::{ID3D11Device, ID3D11Texture2D},
        DirectWrite::*,
        Dxgi::{Common::DXGI_FORMAT_UNKNOWN, IDXGIDevice, IDXGISurface},
    },
    core::{HSTRING, Interface, implement},
};
use windows_numerics::{Matrix3x2, Vector2};

use crate::{engine::ColorRgba8, vector::ColorGlyph};

pub(crate) const NATIVE_GLYPH_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// DirectWrite retains this COM owner instead of copying the shared font file.
#[implement(IDWriteFontFileStream)]
struct FontBytesOwner {
    bytes: Arc<dyn AsRef<[u8]> + Send + Sync>,
}
#[allow(non_snake_case)]
impl IDWriteFontFileStream_Impl for FontBytesOwner_Impl {
    fn ReadFileFragment(
        &self,
        output: *mut *mut core::ffi::c_void,
        offset: u64,
        size: u64,
        context: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        let bytes: &[u8] = self.bytes.as_ref().as_ref();
        if output.is_null()
            || context.is_null()
            || offset
                .checked_add(size)
                .is_none_or(|end| end > bytes.len() as u64)
        {
            return Err(windows::Win32::Foundation::E_INVALIDARG.into());
        }
        unsafe {
            *output = bytes.as_ptr().add(offset as usize).cast_mut().cast();
            *context = std::ptr::null_mut();
        }
        Ok(())
    }
    fn ReleaseFileFragment(&self, _context: *mut core::ffi::c_void) {}
    fn GetFileSize(&self) -> windows::core::Result<u64> {
        Ok(self.bytes.as_ref().as_ref().len() as u64)
    }
    fn GetLastWriteTime(&self) -> windows::core::Result<u64> {
        Ok(0)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GlyphRasterMetrics {
    pub size: [u32; 2],
    /// Rectangle in font units, y upwards. Texture top maps to bounds[3].
    pub bounds: [f64; 4],
    pub baseline: [f32; 2],
    pub em_size: f32,
}

pub(crate) struct ColorGlyphRenderer {
    context: ID2D1DeviceContext4,
    context7: Option<ID2D1DeviceContext7>,
    factory: IDWriteFactory5,
    loader: IDWriteInMemoryFontFileLoader,
    fonts: HashMap<usvg::fontdb::ID, IDWriteFontFace>,
}

impl ColorGlyphRenderer {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        unsafe {
            let dxgi: IDXGIDevice = device.cast()?;
            let d2d = D2D1CreateDevice(&dxgi, None)?;
            d2d.SetMaximumTextureMemory(NATIVE_GLYPH_CACHE_BYTES as u64);
            let context: ID2D1DeviceContext4 = d2d
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?
                .cast()?;
            context.SetDpi(96.0, 96.0);
            context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let context7 = context.cast().ok();
            let factory: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let loader = factory.CreateInMemoryFontFileLoader()?;
            factory.RegisterFontFileLoader(&loader)?;
            Ok(Self {
                context,
                context7,
                factory,
                loader,
                fonts: HashMap::new(),
            })
        }
    }

    pub fn raster_metrics(
        glyph: &ColorGlyph,
        pixels_per_font_unit: f64,
    ) -> Result<GlyphRasterMetrics> {
        ensure!(
            pixels_per_font_unit.is_finite() && pixels_per_font_unit > 0.0,
            "Invalid color glyph scale"
        );
        let [left, bottom, right, top] = glyph.bounds;
        let first = [
            (left * pixels_per_font_unit).floor() - 2.0,
            (-top * pixels_per_font_unit).floor() - 2.0,
        ];
        let last = [
            (right * pixels_per_font_unit).ceil() + 2.0,
            (-bottom * pixels_per_font_unit).ceil() + 2.0,
        ];
        let size = [last[0] - first[0], last[1] - first[1]];
        ensure!(
            size.into_iter()
                .all(|size| size.is_finite() && size > 0.0 && size <= f64::from(u32::MAX)),
            "Invalid color glyph bounds"
        );
        let em_size = (glyph.units_per_em * pixels_per_font_unit) as f32;
        ensure!(
            em_size.is_finite() && em_size > 0.0,
            "Invalid native glyph size"
        );
        Ok(GlyphRasterMetrics {
            size: size.map(|size| size as u32),
            bounds: [
                first[0] / pixels_per_font_unit,
                -last[1] / pixels_per_font_unit,
                last[0] / pixels_per_font_unit,
                -first[1] / pixels_per_font_unit,
            ],
            baseline: [(-first[0]) as f32, (-first[1]) as f32],
            em_size,
        })
    }

    fn font(&mut self, glyph: &ColorGlyph) -> Result<IDWriteFontFace> {
        if let Some(font) = self.fonts.get(&glyph.font_id) {
            return Ok(font.clone());
        }
        unsafe {
            use usvg::fontdb::Source;
            let file = match &glyph.source {
                Source::File(path) | Source::SharedFile(path, _) => self
                    .factory
                    .CreateFontFileReference(&HSTRING::from(path.as_os_str()), None)?,
                Source::Binary(bytes) => {
                    let owner: IDWriteFontFileStream = FontBytesOwner {
                        bytes: bytes.clone(),
                    }
                    .into();
                    let bytes: &[u8] = bytes.as_ref().as_ref();
                    ensure!(
                        bytes.len() <= u32::MAX as usize,
                        "Native font exceeds file size limit"
                    );
                    self.loader.CreateInMemoryFontFileReference(
                        &self.factory,
                        bytes.as_ptr().cast(),
                        bytes.len() as u32,
                        &owner,
                    )?
                }
            };
            let mut supported = windows::core::BOOL::default();
            let mut file_type = DWRITE_FONT_FILE_TYPE_UNKNOWN;
            let mut face_type = DWRITE_FONT_FACE_TYPE_UNKNOWN;
            let mut faces = 0;
            file.Analyze(
                &mut supported,
                &mut file_type,
                Some(&mut face_type),
                &mut faces,
            )?;
            ensure!(
                supported.as_bool() && glyph.face_index < faces,
                "Unsupported native color font face"
            );
            let face = self.factory.CreateFontFace(
                face_type,
                &[Some(file)],
                glyph.face_index,
                DWRITE_FONT_SIMULATIONS_NONE,
            )?;
            if self.fonts.len() >= 32 {
                self.fonts.clear();
            }
            self.fonts.insert(glyph.font_id, face.clone());
            Ok(face)
        }
    }

    /// Call inside the GPUI native context scope after unbinding Snow's targets.
    /// The caller owns allocation accounting and can provide a cropped page by
    /// adjusting metrics.size/baseline/bounds before calling this function.
    pub fn paint(
        &mut self,
        target: &ID3D11Texture2D,
        glyph: &ColorGlyph,
        metrics: &GlyphRasterMetrics,
        foreground: ColorRgba8,
    ) -> Result<()> {
        let face = self.font(glyph)?;
        unsafe {
            let surface: IDXGISurface = target.cast()?;
            let bitmap = self.context.CreateBitmapFromDxgiSurface(
                &surface,
                Some(&D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_UNKNOWN,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    ..Default::default()
                }),
            )?;
            let brush = self.context.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: f32::from(foreground.r) / 255.0,
                    g: f32::from(foreground.g) / 255.0,
                    b: f32::from(foreground.b) / 255.0,
                    a: 1.0,
                },
                None,
            )?;
            let run = DWRITE_GLYPH_RUN {
                fontFace: ManuallyDrop::new(Some(face)),
                fontEmSize: metrics.em_size,
                glyphCount: 1,
                glyphIndices: &glyph.glyph_id,
                ..Default::default()
            };
            let origin = Vector2::new(metrics.baseline[0], metrics.baseline[1]);
            self.context.SetTarget(&bitmap);
            self.context.SetTransform(&Matrix3x2 {
                M11: 1.0,
                M22: 1.0,
                ..Default::default()
            });
            self.context.BeginDraw();
            self.context.Clear(Some(&D2D1_COLOR_F::default()));
            let drawing = if let Some(context) = &self.context7 {
                context.DrawGlyphRunWithColorSupport(
                    origin,
                    &run,
                    None,
                    &brush,
                    None,
                    0,
                    DWRITE_MEASURING_MODE_NATURAL,
                    D2D1_COLOR_BITMAP_GLYPH_SNAP_OPTION_DISABLE,
                );
                Ok(())
            } else {
                self.paint_translated_color_runs(origin, &run, &brush)
            };
            // Flush is valid only inside BeginDraw/EndDraw. EndDraw also
            // submits the native batch before Snow binds this texture as input.
            let flushed = self.context.Flush(None, None);
            let completed = self.context.EndDraw(None, None);
            self.context.SetTarget(None);
            let mut run = run;
            ManuallyDrop::drop(&mut run.fontFace);
            drawing?;
            flushed?;
            completed?;
        }
        Ok(())
    }

    unsafe fn paint_translated_color_runs(
        &self,
        origin: Vector2,
        run: &DWRITE_GLYPH_RUN,
        foreground: &ID2D1SolidColorBrush,
    ) -> Result<()> {
        unsafe {
            let enumerator = self.factory.TranslateColorGlyphRun(
                origin,
                run,
                None,
                DWRITE_GLYPH_IMAGE_FORMATS_TRUETYPE
                    | DWRITE_GLYPH_IMAGE_FORMATS_CFF
                    | DWRITE_GLYPH_IMAGE_FORMATS_COLR
                    | DWRITE_GLYPH_IMAGE_FORMATS_SVG
                    | DWRITE_GLYPH_IMAGE_FORMATS_PNG
                    | DWRITE_GLYPH_IMAGE_FORMATS_JPEG
                    | DWRITE_GLYPH_IMAGE_FORMATS_TIFF
                    | DWRITE_GLYPH_IMAGE_FORMATS_PREMULTIPLIED_B8G8R8A8,
                DWRITE_MEASURING_MODE_NATURAL,
                None,
                0,
            )?;
            while enumerator.MoveNext()?.as_bool() {
                let layer = &*enumerator.GetCurrentRun()?;
                let origin = Vector2::new(layer.Base.baselineOriginX, layer.Base.baselineOriginY);
                match layer.glyphImageFormat {
                    DWRITE_GLYPH_IMAGE_FORMATS_SVG => self.context.DrawSvgGlyphRun(
                        origin,
                        &layer.Base.glyphRun,
                        foreground,
                        None,
                        0,
                        DWRITE_MEASURING_MODE_NATURAL,
                    ),
                    DWRITE_GLYPH_IMAGE_FORMATS_PNG
                    | DWRITE_GLYPH_IMAGE_FORMATS_JPEG
                    | DWRITE_GLYPH_IMAGE_FORMATS_TIFF
                    | DWRITE_GLYPH_IMAGE_FORMATS_PREMULTIPLIED_B8G8R8A8 => {
                        self.context.DrawColorBitmapGlyphRun(
                            layer.glyphImageFormat,
                            origin,
                            &layer.Base.glyphRun,
                            DWRITE_MEASURING_MODE_NATURAL,
                            D2D1_COLOR_BITMAP_GLYPH_SNAP_OPTION_DISABLE,
                        );
                    }
                    _ => {
                        if layer.Base.paletteIndex == u16::MAX {
                            self.context.DrawGlyphRun(
                                origin,
                                &layer.Base.glyphRun,
                                None,
                                foreground,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        } else {
                            let color = layer.Base.runColor;
                            let brush = self.context.CreateSolidColorBrush(
                                &D2D1_COLOR_F {
                                    r: color.r,
                                    g: color.g,
                                    b: color.b,
                                    a: color.a,
                                },
                                None,
                            )?;
                            self.context.DrawGlyphRun(
                                origin,
                                &layer.Base.glyphRun,
                                None,
                                &brush,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Drop for ColorGlyphRenderer {
    fn drop(&mut self) {
        self.fonts.clear();
        unsafe {
            let _ = self.factory.UnregisterFontFileLoader(&self.loader);
        }
    }
}
