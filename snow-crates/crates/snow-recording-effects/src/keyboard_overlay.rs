//! Recording-time key state and bounded, output-pixel overlay composition.
//! Native input and font APIs are adapters; animation uses only explicit timestamps.
use crate::surface::{RgbaSurface, Surface};
pub use snow_recording_model::{
    KeyEventRecord as KeyEvent, KeyboardOverlayConfig, KeyboardOverlayFont,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

pub use snow_recording_model::input::ScrollDirection;
pub const SCROLL_GAP_MS: u64 = 250;

pub const MOVE_MS: u64 = 180;
pub const HOLD_MS: u64 = 1_200;
pub const FADE_MS: u64 = 400;
/// Native keycap height in output pixels for video and live preview.
pub const KEYCAP_SIZE: u32 = snow_core::keycap_layout::HEIGHT;
const KEYCAP_GAP: f32 = 10.0;
const KEYCAP_MARGIN: f32 = 48.0;
const ROW_PITCH: f32 = KEYCAP_SIZE as f32 + 12.0;
const MAX_ROWS: usize = 4;
const MAX_CACHE: usize = 256;
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;

pub fn modifier(key: u16) -> bool {
    matches!(key, 0x10..=0x12 | 0x5b..=0x5c | 0xa0..=0xa5)
}

fn modifier_order(key: u16) -> u16 {
    match key {
        0x11 | 0xa2 | 0xa3 => 0,
        0x12 | 0xa4 | 0xa5 => 1,
        0x10 | 0xa0 | 0xa1 => 2,
        _ => 3,
    }
}

#[derive(Debug)]
struct Row {
    keys: Vec<(u16, String)>,
    is_scroll: bool,
    scroll_last: Option<u64>,
    born: u64,
    released: Option<u64>,
    from: f32,
    target: f32,
    moved: u64,
}

impl Row {
    fn y(&self, now: u64) -> f32 {
        let t = (now.saturating_sub(self.moved) as f32 / MOVE_MS as f32).min(1.0);
        self.from + (self.target - self.from) * (1.0 - (1.0 - t).powi(3))
    }

    fn opacity(&self, now: u64) -> f32 {
        self.released.map_or(1.0, |released| {
            1.0 - (now.saturating_sub(released + HOLD_MS) as f32 / FADE_MS as f32).min(1.0)
        })
    }
}

#[derive(Default)]
pub struct KeyboardModel {
    rows: VecDeque<Row>,
    held: BTreeSet<u16>,
    dirty: bool,
}

impl KeyboardModel {
    pub fn reset(&mut self, now: u64) {
        self.held.clear();
        for row in &mut self.rows {
            row.scroll_last = None;
            if row.released.is_none() || (row.is_scroll && row.released.is_some_and(|at| at > now))
            {
                row.released = Some(now);
            }
        }
        self.dirty = true;
    }

    fn retarget(&mut self, now: u64) {
        for (index, row) in self.rows.iter_mut().rev().enumerate() {
            let target = index as f32;
            if row.target != target {
                row.from = row.y(now);
                row.target = target;
                row.moved = now;
            }
        }
    }

    fn make_room(&mut self, now: u64) {
        if self.rows.len() < MAX_ROWS {
            return;
        }
        // Keep the visible scroll indicator and a held chord when inputs add history.
        let oldest_history = self
            .rows
            .iter()
            .position(|row| {
                if row.is_scroll {
                    row.scroll_last.is_none() || row.opacity(now) == 0.0
                } else {
                    row.released.is_some()
                }
            })
            .unwrap_or(0);
        self.rows.remove(oldest_history);
    }

    pub fn scroll_event(&mut self, event: KeyEvent) -> bool {
        let now = event.at_ms;
        if let Some(row) = self.rows.iter_mut().rev().find(|row| row.is_scroll)
            && row.scroll_last.is_some_and(|last| now >= last)
            && row.opacity(now) > 0.0
        {
            // A new burst can begin while the previous one is holding or fading.
            // Reuse that row so a brief pause never creates stacked scroll history.
            let content_changed = row.keys[0].0 != event.key || row.keys[0].1 != event.label;
            let changed = content_changed || row.opacity(now) < 1.0;
            if content_changed {
                row.keys[0] = (event.key, event.label);
            }
            row.scroll_last = Some(now);
            row.released = Some(now.saturating_add(SCROLL_GAP_MS));
            self.dirty |= changed;
            return changed;
        }
        // Reset or expired rows may still be present before the next rendered frame.
        self.rows.retain(|row| !row.is_scroll);
        self.make_room(now);
        self.rows.push_back(Row {
            keys: vec![(event.key, event.label)],
            is_scroll: true,
            scroll_last: Some(now),
            born: now,
            released: Some(now.saturating_add(SCROLL_GAP_MS)),
            from: -0.3,
            target: 0.0,
            moved: now,
        });
        self.retarget(now);
        self.dirty = true;
        true
    }

    pub fn event(&mut self, event: KeyEvent) {
        if ScrollDirection::from_key(event.key).is_some() {
            self.scroll_event(event);
            return;
        }
        let now = event.at_ms;
        if !event.down {
            self.held.remove(&event.key);
            if let Some(row) = self
                .rows
                .iter_mut()
                .rev()
                .find(|row| !row.is_scroll && row.released.is_none())
            {
                let non_modifiers: Vec<_> =
                    row.keys.iter().filter(|(key, _)| !modifier(*key)).collect();
                let active = if non_modifiers.is_empty() {
                    !event.modifiers.is_empty()
                } else {
                    non_modifiers.iter().any(|(key, _)| self.held.contains(key))
                };
                if !active {
                    row.released = Some(now);
                    self.dirty = true;
                }
            }
            return;
        }
        if !self.held.insert(event.key) {
            return;
        }
        let mut modifiers = event.modifiers;
        modifiers.sort_by_key(|(key, _)| (modifier_order(*key), *key));
        modifiers.dedup_by_key(|(key, _)| *key);
        let extend = self
            .rows
            .iter()
            .any(|row| !row.is_scroll && row.released.is_none());
        if !extend {
            self.make_room(now);
            self.rows.push_back(Row {
                keys: vec![],
                is_scroll: false,
                scroll_last: None,
                born: now,
                released: None,
                from: -0.3,
                target: 0.0,
                moved: now,
            });
        }
        let row = self
            .rows
            .iter_mut()
            .rev()
            .find(|row| !row.is_scroll && row.released.is_none())
            .expect("row was created");
        // Modifier snapshots also collapse Windows' synthetic Ctrl + right Alt into AltGr.
        row.keys.retain(|(key, _)| !modifier(*key));
        modifiers.append(&mut row.keys);
        row.keys = modifiers;
        if !modifier(event.key) && !row.keys.iter().any(|(key, _)| *key == event.key) {
            row.keys.push((event.key, event.label));
        }
        self.dirty = true;
        self.retarget(now);
    }

    pub fn needs_frame(&self, now: u64) -> bool {
        self.dirty
            || self.rows.iter().any(|row| {
                now.saturating_sub(row.moved) < MOVE_MS
                    || row.released.is_some_and(|at| now >= at + HOLD_MS)
            })
    }

    /// Absolute deadline; stable held keys require no timer.
    pub fn next_frame_at(&self, now: u64) -> Option<u64> {
        if self.needs_frame(now) {
            return Some(now.saturating_add(16));
        }
        self.rows
            .iter()
            .filter_map(|row| row.released.map(|at| at.saturating_add(HOLD_MS)))
            .min()
    }

    fn advance(&mut self, now: u64) {
        let previous = self.rows.len();
        self.rows.retain(|row| row.opacity(now) > 0.0);
        if previous != self.rows.len() {
            self.retarget(now);
        }
        self.dirty = false;
    }
}

pub struct Keycap {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA, including rounded background, border and glyphs.
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum KeycapIcon {
    Mouse,
    Scroll(ScrollDirection),
}

impl KeycapIcon {
    fn from_key(key: u16) -> Option<Self> {
        if (0x200..=0x204).contains(&key) {
            Some(Self::Mouse)
        } else {
            ScrollDirection::from_key(key).map(Self::Scroll)
        }
    }

    fn covers(self, x: f32, y: f32) -> bool {
        match self {
            Self::Mouse => {
                // Capsule body, button divider and wheel, in keycap-height units.
                let dy = (y.abs() - 0.10).max(0.0);
                let distance = x.hypot(dy);
                let outline = (0.145..=0.18).contains(&distance);
                let divider = x.abs() <= 0.16 && (y + 0.065).abs() <= 0.0175;
                let wheel = x.abs() <= 0.018 && (-0.20..=-0.11).contains(&y);
                outline || divider || wheel
            }
            Self::Scroll(direction) => {
                let y = if direction == ScrollDirection::Up {
                    y
                } else {
                    -y
                };
                let shaft = x.abs() <= 0.025 && (-0.19..=0.22).contains(&y);
                let head = x.abs() <= 0.18 && (y - (x.abs() - 0.22)).abs() <= 0.035;
                shaft || head
            }
        }
    }
}

fn paint_keycap_icon(cap: &mut Keycap, icon: KeycapIcon, color: [u8; 4]) {
    let h = cap.height as f32;
    let cx = h * 0.65;
    let cy = h * 0.5;
    // Supersample geometry independently of font glyph coverage on every platform.
    for y in 0..cap.height {
        for x in 0..cap.width.min(cap.height) {
            let mut covered = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = (x as f32 + (sx as f32 + 0.5) / 4.0 - cx) / h;
                    let py = (y as f32 + (sy as f32 + 0.5) / 4.0 - cy) / h;
                    covered += u32::from(icon.covers(px, py));
                }
            }
            if covered == 0 {
                continue;
            }
            let alpha = u32::from(color[3]) * covered / 16;
            let i = ((y * cap.width + x) * 4) as usize;
            for (channel, value) in color.iter().enumerate().take(3) {
                cap.pixels[i + channel] = ((u32::from(*value) * alpha
                    + u32::from(cap.pixels[i + channel]) * (255 - alpha)
                    + 127)
                    / 255) as u8;
            }
            cap.pixels[i + 3] = (alpha + u32::from(cap.pixels[i + 3]) * (255 - alpha) / 255) as u8;
        }
    }
}

pub trait KeycapRasterizer {
    /// Scale is relative to the default 64-pixel keycap height.
    fn rasterize(&mut self, label: &str, scale: f32) -> Result<Keycap, String>;
    fn rasterize_scroll(
        &mut self,
        label: &str,
        scale: f32,
        direction: ScrollDirection,
        color: [u8; 4],
    ) -> Result<Keycap, String> {
        let mut cap = self.rasterize(&format!("\u{2003}\u{2003}{label}"), scale)?;
        paint_keycap_icon(&mut cap, KeycapIcon::Scroll(direction), color);
        Ok(cap)
    }
    fn rasterize_mouse(
        &mut self,
        label: &str,
        scale: f32,
        color: [u8; 4],
    ) -> Result<Keycap, String> {
        let mut cap = self.rasterize(&format!("\u{2003}\u{2003}{label}"), scale)?;
        paint_keycap_icon(&mut cap, KeycapIcon::Mouse, color);
        Ok(cap)
    }
    /// A single playback-time glyph. Native adapters keep the requested font size.
    /// Bitmaps must share a vertical origin so numerals and punctuation retain one baseline.
    fn rasterize_glyph(&mut self, label: &str, font_pixels: f32) -> Result<Keycap, String> {
        self.rasterize(label, font_pixels / 32.0)
    }
}

pub struct KeyboardOverlay {
    pub model: KeyboardModel,
    rasterizer: Box<dyn KeycapRasterizer>,
    cache: BTreeMap<(Option<KeycapIcon>, String), (Arc<Keycap>, u64)>,
    icon_color: [u8; 4],
    cache_bytes: usize,
    cache_clock: u64,
    #[cfg(feature = "bench-timing")]
    cache_hits: u64,
    #[cfg(feature = "bench-timing")]
    cache_misses: u64,
    output: (u32, u32),
    scale: f32,
    top_reservation: u32,
}

struct TopClippedSurface<'a, S> {
    surface: &'a mut S,
    top: u32,
}

impl<S: Surface> Surface for TopClippedSurface<'_, S> {
    const OPAQUE: bool = S::OPAQUE;

    fn size(&self) -> (u32, u32) {
        self.surface.size()
    }

    fn span(&mut self, x: u32, y: u32, width: u32, visit: impl FnMut(&mut [u8], usize)) {
        if y >= self.top {
            self.surface.span(x, y, width, visit);
        }
    }
}

impl KeyboardOverlay {
    pub fn new(output: (u32, u32), rasterizer: Box<dyn KeycapRasterizer>) -> Self {
        Self {
            model: KeyboardModel::default(),
            rasterizer,
            cache: BTreeMap::new(),
            icon_color: [255; 4],
            cache_bytes: 0,
            cache_clock: 0,
            #[cfg(feature = "bench-timing")]
            cache_hits: 0,
            #[cfg(feature = "bench-timing")]
            cache_misses: 0,
            output,
            scale: 1.0,
            top_reservation: 0,
        }
    }

    pub fn with_keycap_size(mut self, size: u32) -> Self {
        self.scale = size.clamp(32, 128) as f32 / KEYCAP_SIZE as f32;
        self
    }

    pub fn with_icon_color(mut self, color: [u8; 4]) -> Self {
        self.icon_color = color;
        self
    }

    /// Keep history below the playback badge without moving the bottom baseline.
    pub fn set_top_reservation(&mut self, pixels: u32) {
        self.top_reservation = pixels.min(self.output.1);
    }

    #[cfg(feature = "bench-timing")]
    pub fn cache_stats(&self) -> (u64, u64) {
        (self.cache_hits, self.cache_misses)
    }

    fn keycap(&mut self, key: u16, label: &str) -> Result<Arc<Keycap>, String> {
        let icon = KeycapIcon::from_key(key);
        let cache_key = (icon, label.to_owned());
        self.cache_clock = self.cache_clock.wrapping_add(1);
        if let Some((cap, used)) = self.cache.get_mut(&cache_key) {
            #[cfg(feature = "bench-timing")]
            {
                self.cache_hits += 1;
            }
            *used = self.cache_clock;
            return Ok(Arc::clone(cap));
        }
        #[cfg(feature = "bench-timing")]
        {
            self.cache_misses += 1;
        }
        let cap = Arc::new(match icon {
            Some(KeycapIcon::Scroll(direction)) => {
                self.rasterizer
                    .rasterize_scroll(label, self.scale, direction, self.icon_color)?
            }
            Some(KeycapIcon::Mouse) => {
                self.rasterizer
                    .rasterize_mouse(label, self.scale, self.icon_color)?
            }
            None => self.rasterizer.rasterize(label, self.scale)?,
        });
        let bytes = cap.pixels.len();
        if bytes <= MAX_CACHE_BYTES {
            while !self.cache.is_empty()
                && (self.cache.len() >= MAX_CACHE || self.cache_bytes + bytes > MAX_CACHE_BYTES)
            {
                let key = self
                    .cache
                    .iter()
                    .min_by_key(|(_, (_, used))| *used)
                    .unwrap()
                    .0
                    .clone();
                self.cache_bytes -= self.cache.remove(&key).unwrap().0.pixels.len();
            }
            self.cache_bytes += bytes;
            self.cache
                .insert(cache_key, (Arc::clone(&cap), self.cache_clock));
        }
        Ok(cap)
    }

    pub fn draw(&mut self, rgba: &mut [u8], now: u64) -> Result<(), String> {
        self.draw_to(
            &mut RgbaSurface {
                pixels: rgba,
                dimensions: self.output,
            },
            now,
        )
    }

    pub fn draw_to(&mut self, surface: &mut impl Surface, now: u64) -> Result<(), String> {
        self.model.advance(now);
        let available_height = self.output.1.saturating_sub(self.top_reservation);
        if available_height == 0 {
            return Ok(());
        }
        if self.top_reservation == 0 {
            return self.draw_rows_to(surface, now, available_height as f32);
        }
        self.draw_rows_to(
            &mut TopClippedSurface {
                surface,
                top: self.top_reservation,
            },
            now,
            available_height as f32,
        )
    }

    fn draw_rows_to(
        &mut self,
        surface: &mut impl Surface,
        now: u64,
        available_height: f32,
    ) -> Result<(), String> {
        let row_pitch = ROW_PITCH * self.scale;
        let keycap_size = KEYCAP_SIZE as f32 * self.scale;
        let gap = KEYCAP_GAP * self.scale;
        let margin = KEYCAP_MARGIN;
        let row_limit = ((available_height - 2.0 * margin + row_pitch - keycap_size) / row_pitch)
            .floor()
            .max(1.0) as usize;
        for index in (0..self.model.rows.len()).rev().take(row_limit) {
            let row = &self.model.rows[index];
            if row.born > now {
                continue;
            }
            let row_y = row.y(now);
            let opacity = row.opacity(now);
            let labels: Vec<_> = row.keys.clone();
            let caps: Vec<_> = labels
                .iter()
                .map(|(key, label)| self.keycap(*key, label))
                .collect::<Result<_, _>>()?;
            let width = caps.iter().map(|cap| cap.width as f32).sum::<f32>()
                + gap * caps.len().saturating_sub(1) as f32;
            // Keep the most recent key at the right edge. Narrow outputs clip the chord
            // through the surface instead of shrinking every key below its fixed size.
            let mut x = self.output.0 as f32 - margin - width;
            let bottom = self.output.1 as f32 - margin - row_y * row_pitch;
            for cap in caps {
                blend_keycap_to(surface, &cap, x, bottom - cap.height as f32, 1.0, opacity);
                x += cap.width as f32 + gap;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
fn blend_keycap(
    rgba: &mut [u8],
    size: (u32, u32),
    cap: &Keycap,
    x: f32,
    y: f32,
    scale: f32,
    opacity: f32,
) {
    blend_keycap_to(
        &mut RgbaSurface {
            pixels: rgba,
            dimensions: size,
        },
        cap,
        x,
        y,
        scale,
        opacity,
    );
}

pub(crate) fn blend_keycap_to<S: Surface>(
    surface: &mut S,
    cap: &Keycap,
    x: f32,
    y: f32,
    scale: f32,
    opacity: f32,
) {
    let size = surface.size();
    let width = (cap.width as f32 * scale).round().max(1.0) as i32;
    let height = (cap.height as f32 * scale).round().max(1.0) as i32;
    let origin = (x.round() as i32, y.round() as i32);
    let opacity = (opacity * 255.0).round().clamp(0.0, 255.0) as u32;
    let left = (-origin.0).max(0).min(width);
    let right = (size.0 as i32 - origin.0).clamp(left, width);
    let top = (-origin.1).max(0).min(height);
    let bottom = (size.1 as i32 - origin.1).clamp(top, height);
    if left == right || top == bottom {
        return;
    }
    for dy in top..bottom {
        let target_y = origin.1 + dy;
        surface.span(
            (origin.0 + left) as u32,
            target_y as u32,
            (right - left) as u32,
            |destination, offset| {
                if width == cap.width as i32 && height == cap.height as i32 {
                    let start = (dy as usize * cap.width as usize + left as usize + offset) * 4;
                    let source = &cap.pixels[start..start + destination.len()];
                    if S::OPAQUE {
                        blend_span::<true>(destination, source, opacity);
                    } else {
                        blend_span::<false>(destination, source, opacity);
                    }
                } else {
                    for (index, dst) in destination.chunks_exact_mut(4).enumerate() {
                        let dx = left + (offset + index) as i32;
                        // Sample pixel centers so a badge fitted to a tiny canvas
                        // retains its content instead of its transparent corner.
                        let sx = ((2 * dx as u64 + 1) * u64::from(cap.width) / (2 * width as u64))
                            as usize;
                        let sy = ((2 * dy as u64 + 1) * u64::from(cap.height) / (2 * height as u64))
                            as usize;
                        let source = (sy * cap.width as usize + sx) * 4;
                        if S::OPAQUE {
                            blend_faded_pixel::<true>(
                                dst,
                                &cap.pixels[source..source + 4],
                                opacity,
                            );
                        } else {
                            blend_faded_pixel::<false>(
                                dst,
                                &cap.pixels[source..source + 4],
                                opacity,
                            );
                        }
                    }
                }
            },
        );
    }
}

fn blend_span<const OPAQUE: bool>(destination: &mut [u8], source: &[u8], opacity: u32) {
    assert_eq!(destination.len(), source.len());
    #[cfg(target_arch = "aarch64")]
    // SAFETY: AArch64 provides baseline NEON. The helper processes only full
    // vectors in the equal-sized, non-overlapping pixel slices.
    let offset = unsafe { blend_span_neon::<OPAQUE>(destination, source, opacity) };
    #[cfg(target_arch = "x86_64")]
    let mut offset = 0;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let offset = 0;
    // SSE2 is part of the x86-64 baseline. Process four premultiplied pixels together;
    // use unaligned loads because rounded keycap widths need not be multiples of four.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::*;
        let zero = _mm_setzero_si128();
        let full = _mm_set1_epi16(255);
        let round = _mm_set1_epi16(127);
        let one = _mm_set1_epi16(1);
        let fade = _mm_set1_epi16(opacity as i16);
        let alpha_mask = _mm_set1_epi32(0xff000000u32 as i32);
        while offset + 16 <= destination.len() {
            let src = _mm_loadu_si128(source.as_ptr().add(offset).cast());
            let dst = _mm_loadu_si128(destination.as_ptr().add(offset).cast());
            let blend = |src16, dst16| {
                let alpha = _mm_shufflehi_epi16::<255>(_mm_shufflelo_epi16::<255>(src16));
                // Exact unsigned division by 255 for the rounded 16-bit product.
                let divide = |product| {
                    _mm_srli_epi16::<8>(_mm_add_epi16(
                        _mm_add_epi16(product, one),
                        _mm_srli_epi16::<8>(product),
                    ))
                };
                if opacity == 255 {
                    let product =
                        _mm_add_epi16(_mm_mullo_epi16(dst16, _mm_sub_epi16(full, alpha)), round);
                    _mm_add_epi16(src16, divide(product))
                } else {
                    let alpha = divide(_mm_add_epi16(_mm_mullo_epi16(alpha, fade), round));
                    let product = _mm_add_epi16(
                        _mm_mullo_epi16(src16, fade),
                        _mm_mullo_epi16(dst16, _mm_sub_epi16(full, alpha)),
                    );
                    divide(_mm_add_epi16(product, round))
                }
            };
            let low = blend(_mm_unpacklo_epi8(src, zero), _mm_unpacklo_epi8(dst, zero));
            let high = blend(_mm_unpackhi_epi8(src, zero), _mm_unpackhi_epi8(dst, zero));
            let packed = _mm_packus_epi16(low, high);
            let result = if OPAQUE {
                let rgb = _mm_andnot_si128(alpha_mask, packed);
                _mm_or_si128(rgb, _mm_and_si128(alpha_mask, dst))
            } else {
                packed
            };
            _mm_storeu_si128(destination.as_mut_ptr().add(offset).cast(), result);
            offset += 16;
        }
    }
    for (dst, src) in destination[offset..]
        .chunks_exact_mut(4)
        .zip(source[offset..].chunks_exact(4))
    {
        blend_faded_pixel::<OPAQUE>(dst, src, opacity);
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn blend_span_neon<const OPAQUE: bool>(
    destination: &mut [u8],
    source: &[u8],
    opacity: u32,
) -> usize {
    use std::arch::aarch64::*;
    let mut offset = 0;
    let fade = vdup_n_u8(opacity as u8);
    let full = vdupq_n_u8(255);
    let round = vdupq_n_u16(127);
    let one = vdupq_n_u16(1);
    let divide =
        |value| vshrq_n_u16::<8>(vaddq_u16(vaddq_u16(value, one), vshrq_n_u16::<8>(value)));
    let indices = [3, 3, 3, 3, 7, 7, 7, 7, 11, 11, 11, 11, 15, 15, 15, 15];
    let alpha_mask = [0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255];
    // SAFETY: both tables own sixteen initialized bytes.
    let indices = unsafe { vld1q_u8(indices.as_ptr()) };
    let alpha_mask = unsafe { vld1q_u8(alpha_mask.as_ptr()) };
    while offset + 16 <= destination.len() {
        // SAFETY: both slices have the same length and each iteration reads
        // and writes exactly four complete, potentially unaligned pixels.
        unsafe {
            let src = vld1q_u8(source.as_ptr().add(offset));
            let dst = vld1q_u8(destination.as_ptr().add(offset));
            let alpha = vqtbl1q_u8(src, indices);
            let low_alpha = divide(vaddq_u16(vmull_u8(vget_low_u8(alpha), fade), round));
            let high_alpha = divide(vaddq_u16(vmull_u8(vget_high_u8(alpha), fade), round));
            let inverse = vsubq_u8(
                full,
                vcombine_u8(vmovn_u16(low_alpha), vmovn_u16(high_alpha)),
            );
            let blend = |src, dst, inverse| {
                let numerator = vmlal_u8(vmull_u8(src, fade), dst, inverse);
                vqmovn_u16(divide(vaddq_u16(numerator, round)))
            };
            let low = blend(vget_low_u8(src), vget_low_u8(dst), vget_low_u8(inverse));
            let high = blend(vget_high_u8(src), vget_high_u8(dst), vget_high_u8(inverse));
            let pixels = vcombine_u8(low, high);
            let pixels = if OPAQUE {
                vbslq_u8(alpha_mask, dst, pixels)
            } else {
                pixels
            };
            vst1q_u8(destination.as_mut_ptr().add(offset), pixels);
        }
        offset += 16;
    }
    offset
}

fn blend_faded_pixel<const OPAQUE: bool>(dst: &mut [u8], src: &[u8], opacity: u32) {
    let inverse = 255 - (u32::from(src[3]) * opacity + 127) / 255;
    for channel in 0..if OPAQUE { 3 } else { 4 } {
        dst[channel] =
            ((u32::from(src[channel]) * opacity + u32::from(dst[channel]) * inverse + 127) / 255)
                .min(255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scroll(at_ms: u64, direction: ScrollDirection) -> KeyEvent {
        KeyEvent {
            at_ms,
            key: direction.key(),
            down: true,
            label: direction.label().into(),
            modifiers: vec![],
        }
    }
    #[test]
    fn scrolling_visible_indicator_is_reused_across_wheel_gaps() {
        let mut model = KeyboardModel::default();
        assert!(model.scroll_event(scroll(0, ScrollDirection::Down)));
        model.advance(MOVE_MS);
        for at in [400, 800, 1800, 2800] {
            assert!(!model.scroll_event(scroll(at, ScrollDirection::Down)));
            assert_eq!(
                model.rows.len(),
                1,
                "visible scroll indicators must not stack"
            );
            assert_eq!((model.rows[0].born, model.rows[0].moved), (0, 0));
            assert!(!model.needs_frame(at));
            assert_eq!(model.next_frame_at(at), Some(at + SCROLL_GAP_MS + HOLD_MS));
        }
        assert!(model.scroll_event(scroll(3200, ScrollDirection::Up)));
        assert_eq!(model.rows.len(), 1);
        assert_eq!((model.rows[0].born, model.rows[0].moved), (0, 0));
        assert_eq!(model.rows[0].keys[0].0, ScrollDirection::Up.key());
    }

    #[test]
    fn scrolling_during_fade_restores_one_indicator_and_expired_rows_restart() {
        let mut model = KeyboardModel::default();
        model.scroll_event(scroll(0, ScrollDirection::Down));
        let fading_at = SCROLL_GAP_MS + HOLD_MS + FADE_MS / 2;
        model.advance(fading_at);
        assert_eq!(model.rows[0].opacity(fading_at), 0.5);
        assert!(model.scroll_event(scroll(fading_at, ScrollDirection::Down)));
        assert_eq!(model.rows.len(), 1);
        assert_eq!(model.rows[0].opacity(fading_at), 1.0);
        assert_eq!((model.rows[0].born, model.rows[0].moved), (0, 0));
        assert!(model.dirty);
        model.advance(fading_at);
        assert!(!model.scroll_event(scroll(fading_at + 1, ScrollDirection::Down)));
        assert!(!model.dirty);

        let expired_at = fading_at + 1 + SCROLL_GAP_MS + HOLD_MS + FADE_MS;
        // Expiration must work even if no frame has pruned the old row yet.
        assert!(model.scroll_event(scroll(expired_at, ScrollDirection::Down)));
        assert_eq!(model.rows.len(), 1);
        assert_eq!(
            (model.rows[0].born, model.rows[0].moved),
            (expired_at, expired_at)
        );
        let next_at = expired_at + SCROLL_GAP_MS + HOLD_MS + FADE_MS;
        model.advance(next_at);
        assert!(model.rows.is_empty());
        assert!(model.scroll_event(scroll(next_at, ScrollDirection::Down)));
        assert_eq!(model.rows.len(), 1);
        assert_eq!(model.rows[0].born, next_at);
    }

    #[test]
    fn scrolling_coalesces_without_restarting_or_dirtying_and_reverses_in_place() {
        let mut model = KeyboardModel::default();
        assert!(model.scroll_event(scroll(0, ScrollDirection::Up)));
        model.advance(180);
        assert!(!model.needs_frame(180));
        for at in (200..=2000).step_by(200) {
            assert!(!model.scroll_event(scroll(at, ScrollDirection::Up)));
            assert!(!model.needs_frame(at));
            assert_eq!(model.rows.len(), 1);
            assert_eq!((model.rows[0].born, model.rows[0].moved), (0, 0));
        }
        assert!(model.scroll_event(scroll(2250, ScrollDirection::Down)));
        assert_eq!(model.rows.len(), 1);
        assert_eq!((model.rows[0].born, model.rows[0].moved), (0, 0));
        assert_eq!(model.rows[0].keys[0].0, ScrollDirection::Down.key());
        model.advance(2250);
        assert_eq!(model.rows[0].opacity(3700), 1.0);
        assert!((model.rows[0].opacity(3900) - 0.5).abs() < 0.001);
        model.advance(4100);
        assert!(model.rows.is_empty());
    }
    #[test]
    fn scrolling_gap_reset_and_held_keys_are_independent() {
        let mut model = KeyboardModel::default();
        model.event(KeyEvent {
            at_ms: 0,
            key: 65,
            down: true,
            label: "A".into(),
            modifiers: vec![],
        });
        model.scroll_event(scroll(10, ScrollDirection::Up));
        model.event(KeyEvent {
            at_ms: 20,
            key: 65,
            down: false,
            label: "A".into(),
            modifiers: vec![],
        });
        assert!(model.held.is_empty());
        assert_eq!(model.rows[0].released, Some(20));
        assert!(!model.scroll_event(scroll(260, ScrollDirection::Up)));
        assert!(!model.scroll_event(scroll(511, ScrollDirection::Up)));
        assert_eq!(model.rows.len(), 2);
        model.reset(520);
        assert!(model.scroll_event(scroll(530, ScrollDirection::Up)));
        assert_eq!(model.rows.len(), 2);
        assert_eq!(model.rows.back().unwrap().born, 530);
    }
    #[test]
    fn scrolling_survives_input_history_pressure_without_renewing_old_rows_on_reset() {
        let mut model = KeyboardModel::default();
        model.scroll_event(scroll(0, ScrollDirection::Up));
        for at in (100..1000).step_by(100) {
            model.event(KeyEvent {
                at_ms: at,
                key: 65,
                down: true,
                label: "A".into(),
                modifiers: vec![],
            });
            model.event(KeyEvent {
                at_ms: at + 1,
                key: 65,
                down: false,
                label: "A".into(),
                modifiers: vec![],
            });
        }
        assert!(!model.scroll_event(scroll(1000, ScrollDirection::Up)));
        assert_eq!(model.rows.iter().find(|r| r.is_scroll).unwrap().born, 0);
        model.reset(2000);
        assert_eq!(
            model.rows.iter().find(|r| r.is_scroll).unwrap().released,
            Some(1250)
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_scroll_keycaps_render_all_localized_labels() {
        let style = KeyboardOverlayConfig {
            font: None,
            keycap_size: 64,
            background_rgba: [0, 0, 0, 204],
            text_rgba: [255; 4],
            border_rgba: [64, 64, 64, 204],
            labels: Default::default(),
        };
        let mut rasterizer = crate::keyboard_rasterizer::create(&style).unwrap();
        for (index, label) in [
            "Scroll up",
            "Scroll down",
            "向上滚动",
            "向下滚动",
            "向上捲動",
            "向下捲動",
        ]
        .iter()
        .enumerate()
        {
            let direction = if index % 2 == 0 {
                ScrollDirection::Up
            } else {
                ScrollDirection::Down
            };
            let cap = rasterizer
                .rasterize_scroll(label, 1.0, direction, style.text_rgba)
                .unwrap();
            assert_eq!(cap.height, 64);
            assert!(cap.width > 100);
            if let Some(path) = std::env::var_os("SNOW_SCROLL_KEYCAP_SNAPSHOT_DIR") {
                let path = std::path::PathBuf::from(path);
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(
                    path.join(format!("scroll-{index}-{}-{}.rgba", cap.width, cap.height)),
                    &cap.pixels,
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn mouse_icons_are_scaled_tinted_and_distinct_from_keyboard_and_scroll() {
        struct Blank;
        impl KeycapRasterizer for Blank {
            fn rasterize(&mut self, _: &str, scale: f32) -> Result<Keycap, String> {
                let height = (64.0 * scale) as u32;
                Ok(Keycap {
                    width: height * 4,
                    height,
                    pixels: vec![0; (height * height * 16) as usize],
                })
            }
        }
        for size in [32, 64, 128] {
            let mut overlay = KeyboardOverlay::new((640, 480), Box::new(Blank))
                .with_keycap_size(size)
                .with_icon_color([240, 80, 40, 128]);
            let keyboard = overlay.keycap(0x25, "Left").unwrap();
            let mouse = overlay.keycap(0x200, "Left").unwrap();
            assert_eq!(mouse.height, size);
            assert_ne!(mouse.pixels, keyboard.pixels);
            assert!(
                mouse
                    .pixels
                    .chunks_exact(4)
                    .any(|p| p == [120, 40, 20, 128])
            );
            assert!(
                mouse
                    .pixels
                    .chunks_exact(4)
                    .all(|p| p[0] <= p[3] && p[3] <= 128)
            );
            assert!(
                mouse
                    .pixels
                    .chunks_exact((mouse.width * 4) as usize)
                    .all(|row| row[(size * 4) as usize..].iter().all(|v| *v == 0))
            );
            for key in 0x200..=0x204 {
                assert!(Arc::ptr_eq(&mouse, &overlay.keycap(key, "Left").unwrap()));
            }
            let scroll = overlay.keycap(0x205, "Left").unwrap();
            assert_ne!(mouse.pixels, scroll.pixels);
            assert_eq!(overlay.cache.len(), 3);
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_mouse_keycaps_render_localized_labels_at_all_sizes() {
        let style = KeyboardOverlayConfig {
            font: None,
            keycap_size: 64,
            background_rgba: [0, 0, 0, 204],
            text_rgba: [255; 4],
            border_rgba: [64, 64, 64, 204],
            labels: Default::default(),
        };
        let mut rasterizer = crate::keyboard_rasterizer::create(&style).unwrap();
        for size in [32, 64, 128] {
            for (index, label) in [
                "Left", "Right", "Middle", "Button 4", "Button 5", "左键", "右键", "中键",
                "按键 4", "按键 5", "左鍵", "右鍵", "中鍵", "按鍵 4", "按鍵 5",
            ]
            .iter()
            .enumerate()
            {
                let scale = size as f32 / 64.0;
                let plain = rasterizer
                    .rasterize(&format!("\u{2003}\u{2003}{label}"), scale)
                    .unwrap();
                let cap = rasterizer
                    .rasterize_mouse(label, scale, style.text_rgba)
                    .unwrap();
                assert_eq!(cap.height, size);
                assert_eq!(cap.width, plain.width);
                assert_ne!(cap.pixels, plain.pixels);
                for (actual, expected) in cap
                    .pixels
                    .chunks_exact((cap.width * 4) as usize)
                    .zip(plain.pixels.chunks_exact((plain.width * 4) as usize))
                {
                    assert_eq!(
                        &actual[(size * 4) as usize..],
                        &expected[(size * 4) as usize..]
                    );
                }
                if let Ok(path) = std::env::var("SNOW_MOUSE_KEYCAP_DUMP") {
                    let path = std::path::PathBuf::from(path);
                    std::fs::create_dir_all(&path).unwrap();
                    std::fs::write(
                        path.join(format!("mouse-{index}-{}-{}.rgba", cap.width, cap.height)),
                        &cap.pixels,
                    )
                    .unwrap();
                }
            }
        }
    }

    #[test]
    fn scrolling_icons_are_geometric_and_cached_by_direction() {
        struct Blank;
        impl KeycapRasterizer for Blank {
            fn rasterize(&mut self, _: &str, _: f32) -> Result<Keycap, String> {
                Ok(Keycap {
                    width: 300,
                    height: 64,
                    pixels: vec![0; 300 * 64 * 4],
                })
            }
        }
        let mut overlay = KeyboardOverlay::new((640, 480), Box::new(Blank));
        let up = overlay.keycap(0x205, "Same localized label").unwrap();
        let same = overlay.keycap(0x205, "Same localized label").unwrap();
        let down = overlay.keycap(0x206, "Same localized label").unwrap();
        assert!(Arc::ptr_eq(&up, &same));
        assert_ne!(up.pixels, down.pixels);
        assert!(up.pixels.iter().any(|v| *v != 0));
        assert!(
            up.pixels
                .chunks_exact(300 * 4)
                .all(|row| row[64 * 4..].iter().all(|v| *v == 0))
        );
        assert_eq!(overlay.cache.len(), 2);
    }
    fn event(at: u64, key: u16, down: bool, mods: &[u16]) -> KeyEvent {
        KeyEvent {
            at_ms: at,
            key,
            down,
            label: format!("{key}"),
            modifiers: mods.iter().map(|key| (*key, format!("{key}"))).collect(),
        }
    }
    #[test]
    fn mouse_and_keyboard_holds_share_a_chord_and_release_independently() {
        let mut model = KeyboardModel::default();
        let key = |key, down, at_ms| KeyEvent {
            at_ms,
            key,
            down,
            label: format!("{key}"),
            modifiers: vec![],
        };
        model.event(key(65, true, 0));
        model.event(key(0x200, true, 10));
        assert_eq!(model.rows.len(), 1);
        assert_eq!(model.rows[0].keys.len(), 2);
        model.event(key(65, false, 20));
        assert!(model.rows[0].released.is_none());
        model.event(key(0x200, false, 30));
        assert_eq!(model.rows[0].released, Some(30));
    }

    #[test]
    fn physical_modifiers_are_not_deduplicated_by_their_labels() {
        let mut model = KeyboardModel::default();
        let mut input = event(0, 65, true, &[0xa2, 0xa3]);
        for (_, label) in &mut input.modifiers {
            *label = "Ctrl".into();
        }
        model.event(input);
        assert_eq!(
            model.rows[0]
                .keys
                .iter()
                .map(|key| key.0)
                .collect::<Vec<_>>(),
            [0xa2, 0xa3, 65]
        );
    }

    #[test]
    fn mouse_chords_preserve_both_modifier_sides() {
        use crate::keyboard_hook::MODIFIER_KEYS;
        use crate::mouse_hook::{MouseClickObservation, ObservedMouseButton};
        let style = KeyboardOverlayConfig {
            font: None,
            keycap_size: 64,
            background_rgba: [0; 4],
            text_rgba: [255; 4],
            border_rgba: [0; 4],
            labels: Default::default(),
        };
        let click = MouseClickObservation {
            at: std::time::Instant::now(),
            x: 0,
            y: 0,
            button: ObservedMouseButton::Left,
            down: true,
            modifiers: [true; 8],
        };
        let input = click.event(0, &style, true);
        assert_eq!(
            input
                .modifiers
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            MODIFIER_KEYS
        );
        for pair in input.modifiers.chunks_exact(2) {
            assert!(pair[0].1.starts_with("Left "));
            assert!(pair[1].1.starts_with("Right "));
        }
        let mut model = KeyboardModel::default();
        model.event(input);
        assert_eq!(model.rows[0].keys.len(), 9);
        assert!(click.event(1, &style, false).modifiers.is_empty());
    }

    #[test]
    fn mouse_buttons_share_hold_release_and_fade_without_colliding_with_keys() {
        use crate::mouse_hook::{MouseClickObservation, ObservedMouseButton};
        let style = KeyboardOverlayConfig {
            font: None,
            keycap_size: 64,
            background_rgba: [0; 4],
            text_rgba: [255; 4],
            border_rgba: [0; 4],
            labels: [(0xa2, "Left Ctrl".into()), (0x203, "Side four".into())].into(),
        };
        for button in [
            ObservedMouseButton::Left,
            ObservedMouseButton::Right,
            ObservedMouseButton::Middle,
            ObservedMouseButton::Button4,
            ObservedMouseButton::Button5,
        ] {
            let mut model = KeyboardModel::default();
            let mut observation = MouseClickObservation {
                at: std::time::Instant::now(),
                x: 0,
                y: 0,
                button,
                down: true,
                modifiers: [true, false, false, false, false, false, false, false],
            };
            let mut default_style = style.clone();
            default_style.labels.clear();
            assert!(
                observation
                    .event(0, &default_style, true)
                    .modifiers
                    .iter()
                    .all(|(_, label)| !label.is_empty())
            );
            let event = observation.event(10, &style, false);
            assert!(event.key >= 0x200 && event.modifiers.is_empty());
            model.event(event.clone());
            model.event(event);
            assert_eq!(model.rows.len(), 1);
            assert_eq!(model.rows[0].keys.len(), 1);
            assert!(model.rows[0].released.is_none());
            assert_eq!(
                observation.event(20, &style, true).modifiers,
                [(0xa2, "Left Ctrl".into())]
            );
            observation.down = false;
            model.event(observation.event(100, &style, false));
            assert_eq!(model.rows[0].released, Some(100));
            assert_eq!(model.rows[0].opacity(100 + HOLD_MS), 1.0);
            assert_eq!(model.rows[0].opacity(100 + HOLD_MS + FADE_MS), 0.0);
            observation.down = true;
            model.event(observation.event(200, &style, false));
            assert_eq!(model.rows.len(), 2, "double clicks remain distinct");
            model.reset(300);
            assert!(model.held.is_empty());
            assert!(model.rows.iter().all(|row| row.released.is_some()));
        }
    }

    #[test]
    fn shortcuts_are_combined_but_successive_actions_are_separate() {
        let mut m = KeyboardModel::default();
        m.event(event(0, 0xa2, true, &[0xa2]));
        m.event(event(10, 0xa0, true, &[0xa2, 0xa0]));
        m.event(event(20, 83, true, &[0xa2, 0xa0]));
        assert_eq!(m.rows.len(), 1);
        assert_eq!(
            m.rows[0].keys.iter().map(|k| k.0).collect::<Vec<_>>(),
            [0xa2, 0xa0, 83]
        );
        m.event(event(30, 83, false, &[0xa2, 0xa0]));
        m.event(event(40, 86, true, &[0xa2, 0xa0]));
        assert_eq!(m.rows.len(), 2);
        assert_eq!(m.rows[0].released, Some(30));
        assert_eq!(m.rows[1].keys.last().unwrap().0, 86);
    }
    #[test]
    fn held_keys_ignore_repeats_and_overlapping_keys_form_a_chord() {
        let mut m = KeyboardModel::default();
        m.event(event(0, 65, true, &[]));
        m.event(event(50, 65, true, &[]));
        m.event(event(100, 66, true, &[]));
        m.event(event(150, 65, false, &[]));
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.rows[0].keys.len(), 2);
        assert_eq!(m.rows[0].opacity(20_000), 1.0);
        m.event(event(200, 66, false, &[]));
        assert_eq!(m.rows[0].opacity(1400), 1.0);
        assert_eq!(m.rows[0].opacity(1600), 0.5);
        assert!(m.needs_frame(1800));
        m.advance(1800);
        assert!(m.rows.is_empty());
        assert!(!m.needs_frame(1800));
    }
    #[test]
    fn movement_retargets_continuously_and_history_is_bounded() {
        let mut m = KeyboardModel::default();
        for index in 0..20 {
            m.event(event(index * 30, 65, true, &[]));
            m.event(event(index * 30 + 1, 65, false, &[]));
        }
        assert_eq!(m.rows.len(), MAX_ROWS);
        let at = 580;
        let position = m.rows.back().unwrap().y(at);
        m.event(event(at, 66, true, &[]));
        assert!((m.rows[m.rows.len() - 2].y(at) - position).abs() < 0.001);
        m.reset(590);
        assert!(m.held.is_empty());
        assert!(m.rows.iter().all(|row| row.released.is_some()));
    }
    struct Solid;
    impl KeycapRasterizer for Solid {
        fn rasterize(&mut self, _: &str, _: f32) -> Result<Keycap, String> {
            Ok(Keycap {
                width: 40,
                height: 20,
                pixels: [100, 0, 0, 128].repeat(800),
            })
        }
    }

    struct FixedSquare;
    impl KeycapRasterizer for FixedSquare {
        fn rasterize(&mut self, label: &str, scale: f32) -> Result<Keycap, String> {
            assert_eq!(scale, 1.0, "output resolution must not scale keycaps");
            Ok(Keycap {
                width: 64,
                height: 64,
                pixels: [label.parse().unwrap(), 0, 0, 255].repeat(64 * 64),
            })
        }
    }

    #[test]
    fn animated_keycaps_and_history_clip_below_the_top_badge() {
        struct ScaledSquare;
        impl KeycapRasterizer for ScaledSquare {
            fn rasterize(&mut self, _: &str, scale: f32) -> Result<Keycap, String> {
                let side = (KEYCAP_SIZE as f32 * scale).round() as u32;
                Ok(Keycap {
                    width: side,
                    height: side,
                    pixels: [255, 0, 0, 255].repeat(side as usize * side as usize),
                })
            }
        }
        struct BadgeGlyph;
        impl KeycapRasterizer for BadgeGlyph {
            fn rasterize(&mut self, _: &str, _: f32) -> Result<Keycap, String> {
                Ok(Keycap {
                    width: 4,
                    height: 12,
                    pixels: [255; 4].repeat(4 * 12),
                })
            }
        }
        for size in [
            (120, 100),
            (640, 480),
            (1920, 1080),
            (8, 100),
            (16, 22),
            (1, 1),
        ] {
            let playback = crate::playback::PlaybackRenderer::new(
                snow_recording_model::PlaybackOverlay::PlaybackTime { rgba: [255; 4] },
                size,
                Some(Box::new(BadgeGlyph)),
            )
            .unwrap();
            let reservation = playback.top_reservation();
            for keycap_size in [32, 64, 128] {
                let mut overlay = KeyboardOverlay::new(size, Box::new(ScaledSquare))
                    .with_keycap_size(keycap_size);
                overlay.set_top_reservation(reservation);
                for index in 0..MAX_ROWS as u64 {
                    overlay.model.event(event(index * 30, 65, true, &[]));
                    overlay.model.event(event(index * 30 + 1, 65, false, &[]));
                }
                for now in [MOVE_MS, MOVE_MS * 2, MOVE_MS * 3] {
                    let mut pixels = [20, 40, 60, 255].repeat(size.0 as usize * size.1 as usize);
                    overlay.draw(&mut pixels, now).unwrap();
                    let reserved = reservation as usize * size.0 as usize * 4;
                    assert!(
                        pixels[..reserved]
                            .chunks_exact(4)
                            .all(|pixel| pixel == [20, 40, 60, 255]),
                        "key history enters timer at {size:?}, size {keycap_size}, time {now}"
                    );
                    if size.1.saturating_sub(KEYCAP_MARGIN as u32) > reservation
                        && size.0 > KEYCAP_MARGIN as u32
                    {
                        assert!(
                            pixels[reserved..]
                                .chunks_exact(4)
                                .any(|pixel| pixel[0] == 255)
                        );
                    }
                    let mut tiles = crate::surface::TileSurface::new(size);
                    overlay.draw_to(&mut tiles, now).unwrap();
                    for tile in tiles.snapshot() {
                        for (index, pixel) in tile.pixels.chunks_exact(4).enumerate() {
                            let y = tile.y + index as u32 / crate::surface::TILE_SIZE;
                            if y < reservation {
                                assert_eq!(pixel, [0; 4]);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn keycaps_remain_64_square_with_fixed_chord_spacing_at_every_resolution() {
        for size in [
            (320, 240),
            (1280, 720),
            (1920, 1080),
            (3840, 2160),
            (1080, 1920),
        ] {
            let mut overlay = KeyboardOverlay::new(size, Box::new(FixedSquare));
            for key in 65..=67 {
                overlay.model.event(event(0, key, true, &[]));
            }
            let mut pixels = [0, 0, 0, 255].repeat(size.0 as usize * size.1 as usize);
            overlay.draw(&mut pixels, MOVE_MS).unwrap();
            let rows: Vec<_> = pixels
                .chunks_exact(size.0 as usize * 4)
                .filter(|row| row.chunks_exact(4).any(|p| p[0] != 0))
                .collect();
            assert_eq!(rows.len(), 64, "key height at {size:?}");
            for row in rows {
                let mut previous_end = None;
                for key in 65..=67 {
                    let columns: Vec<_> = row
                        .chunks_exact(4)
                        .enumerate()
                        .filter_map(|(x, p)| (p[0] == key).then_some(x))
                        .collect();
                    assert_eq!(columns.len(), 64, "key width at {size:?}");
                    assert_eq!(columns[63] - columns[0], 63);
                    if let Some(end) = previous_end {
                        assert_eq!(columns[0] - end - 1, 10, "fixed gap at {size:?}");
                    }
                    previous_end = Some(columns[63]);
                }
            }
        }
    }

    #[test]
    fn fixed_pixel_keyboard_inset_does_not_depend_on_output_dimensions() {
        for size in [
            (640, 480),
            (1280, 720),
            (1920, 1080),
            (3840, 2160),
            (1080, 1920),
        ] {
            let mut overlay = KeyboardOverlay::new(size, Box::new(FixedSquare));
            overlay.model.event(event(0, 65, true, &[]));
            let mut tiles = crate::surface::TileSurface::new(size);
            overlay.draw_to(&mut tiles, MOVE_MS).unwrap();
            let (mut right, mut bottom) = (0, 0);
            for tile in tiles.snapshot() {
                for (index, pixel) in tile.pixels.chunks_exact(4).enumerate() {
                    if pixel[3] != 0 {
                        right = right.max(tile.x + index as u32 % crate::surface::TILE_SIZE);
                        bottom = bottom.max(tile.y + index as u32 / crate::surface::TILE_SIZE);
                    }
                }
            }
            assert_eq!(size.0 - right - 1, 48, "right inset at {size:?}");
            assert_eq!(size.1 - bottom - 1, 48, "bottom inset at {size:?}");
        }
    }

    #[test]
    fn mixed_width_keycaps_keep_native_width_spacing_and_right_alignment() {
        struct VariableWidth;
        impl KeycapRasterizer for VariableWidth {
            fn rasterize(&mut self, label: &str, scale: f32) -> Result<Keycap, String> {
                assert_eq!(scale, 1.0);
                let key: u8 = label.parse().unwrap();
                let width = match key {
                    65 => 64,
                    66 => 113,
                    _ => 187,
                };
                Ok(Keycap {
                    width,
                    height: 64,
                    pixels: [key, 0, 0, 255].repeat(width as usize * 64),
                })
            }
        }
        for size in [(100, 180), (640, 480), (1920, 1080), (3840, 2160)] {
            let mut overlay = KeyboardOverlay::new(size, Box::new(VariableWidth));
            for key in 65..=67 {
                overlay.model.event(event(0, key, true, &[]));
            }
            let mut pixels = [0, 0, 0, 255].repeat(size.0 as usize * size.1 as usize);
            overlay.draw(&mut pixels, MOVE_MS).unwrap();
            let right = size.0 as usize - 48;
            let rows: Vec<_> = pixels
                .chunks_exact(size.0 as usize * 4)
                .filter(|row| row.chunks_exact(4).any(|p| p[0] != 0))
                .collect();
            assert_eq!(rows.len(), 64);
            for row in rows {
                let mut end = right;
                for (key, width) in [(67, 187), (66, 113), (65, 64)] {
                    let start = end.saturating_sub(width);
                    for (x, pixel) in row.chunks_exact(4).enumerate() {
                        assert_eq!(pixel[0] == key, (start..end).contains(&x));
                    }
                    end = start.saturating_sub(10);
                }
            }
        }
    }

    #[test]
    fn narrow_outputs_clip_chords_instead_of_shrinking_keys() {
        for size in [(100, 100), (40, 100), (2, 2)] {
            let mut overlay = KeyboardOverlay::new(size, Box::new(FixedSquare));
            overlay.model.event(event(0, 65, true, &[]));
            overlay.model.event(event(0, 66, true, &[]));
            let mut pixels = [0, 0, 0, 255].repeat(size.0 as usize * size.1 as usize);
            overlay.draw(&mut pixels, MOVE_MS).unwrap();
            let rows: Vec<_> = pixels
                .chunks_exact(size.0 as usize * 4)
                .filter(|row| row.chunks_exact(4).any(|p| p[0] == 66))
                .collect();
            let visible_width = 64.min(size.0.saturating_sub(48) as usize);
            let visible_height = if visible_width == 0 {
                0
            } else {
                64.min(size.1.saturating_sub(48) as usize)
            };
            assert_eq!(rows.len(), visible_height);
            // Fixed insets and key dimensions are clipped by the canvas bounds.
            for row in rows {
                assert_eq!(
                    row.chunks_exact(4).filter(|p| p[0] == 66).count(),
                    visible_width
                );
            }
        }
    }

    #[test]
    fn fixed_height_history_rows_do_not_overlap_at_low_resolution() {
        let size = (320, 400);
        let mut overlay = KeyboardOverlay::new(size, Box::new(FixedSquare));
        for key in 65..=68 {
            let at = u64::from(key - 65) * 20;
            overlay.model.event(event(at, key, true, &[]));
            overlay.model.event(event(at + 1, key, false, &[]));
        }
        let mut pixels = [0, 0, 0, 255].repeat(size.0 as usize * size.1 as usize);
        overlay.draw(&mut pixels, 300).unwrap();
        let mut previous_bottom = None;
        for key in 65..=68 {
            let rows: Vec<_> = pixels
                .chunks_exact(size.0 as usize * 4)
                .enumerate()
                .filter_map(|(y, row)| row.chunks_exact(4).any(|p| p[0] == key).then_some(y))
                .collect();
            assert_eq!(rows.len(), 64);
            if let Some(bottom) = previous_bottom {
                assert_eq!(rows[0] - bottom - 1, 12);
            }
            previous_bottom = Some(rows[63]);
        }
    }

    #[test]
    fn keycap_cache_is_bounded_and_keeps_recent_labels() {
        let mut overlay = KeyboardOverlay::new((3840, 2160), Box::new(Solid));
        for index in 0..1000 {
            overlay.keycap(0, &format!("Label {index}")).unwrap();
        }
        assert_eq!(overlay.cache.len(), MAX_CACHE);
        assert!(overlay.cache_bytes <= MAX_CACHE_BYTES);
        assert!(overlay.cache.contains_key(&(None, "Label 999".into())));
        assert!(!overlay.cache.contains_key(&(None, "Label 0".into())));
    }

    #[test]
    fn cache_evicts_by_bytes_and_does_not_retain_oversized_keycaps() {
        struct Large;
        impl KeycapRasterizer for Large {
            fn rasterize(&mut self, label: &str, _: f32) -> Result<Keycap, String> {
                let height = if label == "oversized" { 8193 } else { 4096 };
                Ok(Keycap {
                    width: 1024,
                    height,
                    pixels: vec![0; (1024 * height * 4) as usize],
                })
            }
        }
        let mut overlay = KeyboardOverlay::new((3840, 2160), Box::new(Large));
        overlay.keycap(0, "a").unwrap();
        overlay.keycap(0, "b").unwrap();
        overlay.keycap(0, "a").unwrap();
        overlay.keycap(0, "c").unwrap();
        assert_eq!(overlay.cache_bytes, MAX_CACHE_BYTES);
        assert_eq!(overlay.cache.len(), 2);
        assert!(overlay.cache.contains_key(&(None, "a".into())));
        assert!(!overlay.cache.contains_key(&(None, "b".into())));
        let cap = overlay.keycap(0, "oversized").unwrap();
        assert!(cap.pixels.len() > MAX_CACHE_BYTES);
        assert!(!overlay.cache.contains_key(&(None, "oversized".into())));
        assert_eq!(overlay.cache_bytes, MAX_CACHE_BYTES);
    }

    #[test]
    fn transparent_simd_blending_updates_alpha_and_matches_scalar() {
        let src = [80, 20, 10, 128].repeat(37);
        for opacity in [0, 1, 127, 128, 254, 255] {
            let mut pixels = [15, 30, 45, 64].repeat(37);
            let mut expected = pixels.clone();
            for (dst, src) in expected.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                blend_faded_pixel::<false>(dst, src, opacity);
            }
            blend_span::<false>(&mut pixels, &src, opacity);
            assert_eq!(pixels, expected);
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn neon_blending_executes_unaligned_vectors_and_preserves_bounds() {
        fn check<const OPAQUE: bool>() {
            for offset in 0..16 {
                for count in [0, 1, 3, 4, 5, 7, 8, 9, 31, 32, 33] {
                    for opacity in [0, 1, 63, 127, 128, 254, 255] {
                        let mut source = vec![0; offset];
                        for i in 0..count {
                            let alpha = (i * 71 % 256) as u8;
                            source.extend_from_slice(&[alpha, alpha / 2, alpha / 3, alpha]);
                        }
                        let mut actual = vec![0xa5; offset + count * 4 + 17];
                        for (i, pixel) in actual[offset..offset + count * 4]
                            .chunks_exact_mut(4)
                            .enumerate()
                        {
                            pixel.copy_from_slice(&[17, 230, 91, (i * 37 % 256) as u8]);
                        }
                        let mut expected = actual.clone();
                        let end = unsafe {
                            blend_span_neon::<OPAQUE>(
                                &mut actual[offset..offset + count * 4],
                                &source[offset..],
                                opacity,
                            )
                        };
                        assert_eq!(end, count / 4 * 16);
                        for (dst, src) in expected[offset..offset + end]
                            .chunks_exact_mut(4)
                            .zip(source[offset..offset + end].chunks_exact(4))
                        {
                            blend_faded_pixel::<OPAQUE>(dst, src, opacity);
                        }
                        assert_eq!(
                            actual, expected,
                            "offset {offset}, count {count}, opacity {opacity}"
                        );
                    }
                }
            }
        }
        check::<false>();
        check::<true>();
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    #[ignore = "run explicitly in the performance Release build"]
    fn keycap_simd_performance_comparison() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};
        if cfg!(debug_assertions) {
            panic!("use the performance Release build");
        }
        println!("pair,kernel,width,opacity,nanoseconds_per_span");
        for width in [65, 325] {
            let source: Vec<_> = (0..width)
                .flat_map(|i| {
                    let alpha = (i * 71 % 256) as u8;
                    [alpha, alpha / 2, alpha / 3, alpha]
                })
                .collect();
            for opacity in [127, 255] {
                let original = [17, 230, 91, 255].repeat(width);
                let mut expected = original.clone();
                let scalar = |destination: &mut [u8]| {
                    for (dst, src) in destination.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
                        blend_faded_pixel::<true>(dst, src, opacity);
                    }
                };
                scalar(&mut expected);
                let mut selected = original.clone();
                blend_span::<true>(&mut selected, &source, opacity);
                assert_eq!(selected, expected);
                for pair in 0..5 {
                    for position in 0..2 {
                        let neon = (position + pair) % 2 != 0;
                        let mut pixels = original.clone();
                        let started = Instant::now();
                        let mut count = 0;
                        while started.elapsed() < Duration::from_millis(100) {
                            if neon {
                                blend_span::<true>(
                                    black_box(&mut pixels),
                                    black_box(&source),
                                    black_box(opacity),
                                );
                            } else {
                                scalar(black_box(&mut pixels));
                            }
                            black_box(&pixels);
                            count += 1;
                        }
                        let kernel = if neon { "neon" } else { "portable" };
                        println!(
                            "{},{kernel},{width},{opacity},{}",
                            pair + 1,
                            started.elapsed().as_nanos() / count
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn offscreen_composition_is_bounded_blended_and_cleared() {
        for size in [(1920, 1080), (100, 100), (40, 100), (2, 2), (1080, 1920)] {
            let mut overlay = KeyboardOverlay::new(size, Box::new(Solid));
            overlay.model.event(event(0, 65, true, &[]));
            overlay.model.event(event(1, 65, false, &[]));
            let original = [20, 40, 60, 255].repeat(size.0 as usize * size.1 as usize);
            let mut pixels = original.clone();
            overlay.draw(&mut pixels, 200).unwrap();
            assert_eq!(pixels != original, size.0 > 48 && size.1 > 48);
            let mut cleared = original.clone();
            overlay.draw(&mut cleared, 1601).unwrap();
            assert_eq!(cleared, original);
        }
    }

    #[test]
    fn modifier_release_does_not_dismantle_chord_and_sides_release_independently() {
        let mut m = KeyboardModel::default();
        m.event(event(0, 0xa2, true, &[0xa2]));
        m.event(event(10, 0xa3, true, &[0xa2, 0xa3]));
        assert_eq!(
            m.rows[0].keys.iter().map(|key| key.0).collect::<Vec<_>>(),
            [0xa2, 0xa3]
        );
        m.event(event(20, 0xa2, false, &[0xa3]));
        assert!(m.rows[0].released.is_none());
        m.event(event(30, 83, true, &[0xa3]));
        m.event(event(40, 0xa3, false, &[]));
        assert_eq!(m.rows[0].keys.len(), 2);
        assert!(m.rows[0].released.is_none());
        m.event(event(50, 83, false, &[]));
        assert_eq!(m.rows[0].released, Some(50));
    }

    #[test]
    fn premultiplied_blend_handles_clipping_and_fades_without_dark_fringe() {
        let cap = Solid.rasterize("A", 1.0).unwrap();
        let mut pixels = [20, 40, 60, 255].repeat(100);
        let original = pixels.clone();
        blend_keycap(&mut pixels, (10, 10), &cap, -100.0, 0.0, 1.0, 1.0);
        blend_keycap(&mut pixels, (10, 10), &cap, 100.0, 0.0, 1.0, 1.0);
        assert_eq!(pixels, original);
        blend_keycap(&mut pixels, (10, 10), &cap, 0.0, 0.0, 1.0, 1.0);
        assert_eq!(&pixels[..4], &[110, 20, 30, 255]);
        pixels.copy_from_slice(&original);
        blend_keycap(&mut pixels, (10, 10), &cap, 0.0, 0.0, 1.0, 0.5);
        assert_eq!(&pixels[..4], &[65, 30, 45, 255]);
    }

    #[test]
    fn wide_keycap_blending_matches_scalar_alpha_contract_including_unaligned_tails() {
        let mut source = Vec::new();
        let mut destination = Vec::new();
        for alpha in 0..=255u8 {
            source.extend_from_slice(&[alpha, alpha / 2, alpha / 3, alpha]);
            destination.extend_from_slice(&[17, 230, 91, alpha]);
        }
        for (start, opacity) in
            (0..4).flat_map(|start| [0, 1, 127, 128, 254, 255].map(|opacity| (start, opacity)))
        {
            let mut actual = destination[start * 4..destination.len() - 4].to_vec();
            let source = &source[start * 4..source.len() - 4];
            let mut expected = actual.clone();
            for (dst, src) in expected.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
                let inverse = 255 - (u32::from(src[3]) * opacity + 127) / 255;
                for channel in 0..3 {
                    dst[channel] = ((u32::from(src[channel]) * opacity
                        + u32::from(dst[channel]) * inverse
                        + 127)
                        / 255) as u8;
                }
            }
            blend_span::<true>(&mut actual, source, opacity);
            assert_eq!(actual, expected);
        }
    }
}
