use super::*;
use std::ops::Range;

impl CanvasView {
    /// Bound labels have priority over the host's unhandled double-click action.
    pub(super) fn begin_bound_label_at(
        &mut self,
        point: draw::Point<f64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let viewport = self.session.viewport();
        let engine = self.document.engine();
        let target = engine
            .speech_bubble_text_target(viewport, Some(point))
            .ok()
            .flatten()
            .or_else(|| {
                engine
                    .arrow_text_target(viewport, Some(point))
                    .ok()
                    .flatten()
            });
        if let Some(target) = target {
            self.begin_text_with_target(point, None, Some(target), window, cx);
            true
        } else {
            false
        }
    }
    pub(super) fn begin_text(
        &mut self,
        point: draw::Point<f64>,
        existing: Option<draw::ElementId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_text_with_target(point, existing, None, window, cx);
    }
    fn begin_text_with_target(
        &mut self,
        point: draw::Point<f64>,
        existing: Option<draw::ElementId>,
        supplied: Option<(draw::TextElementInfo, draw::TextStyle)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_text(true, window, cx);
        let viewport = self.session.viewport();
        let style = match self
            .document
            .engine()
            .viewport_style_toolbar_state(viewport)
        {
            Ok(state) => state.text_style,
            Err(error) => {
                self.fail(format!("{error:?}"), cx);
                return;
            }
        };
        let info = supplied
            .or_else(|| {
                existing
                    .and_then(|id| self.document.engine().text_element_info(id).ok())
                    .map(|info| {
                        let mut style = style.clone();
                        style.font_size = info.font_size;
                        style.bold = info.bold;
                        style.italic = info.italic;
                        style.font_family = info.font_family.clone();
                        style.horizontal_align = info.horizontal_align;
                        style.vertical_align = info.vertical_align;
                        (info, style)
                    })
            })
            .or_else(|| {
                self.document
                    .engine()
                    .speech_bubble_text_target(viewport, Some(point))
                    .ok()
                    .flatten()
            })
            .or_else(|| {
                self.document
                    .engine()
                    .arrow_text_target(viewport, Some(point))
                    .ok()
                    .flatten()
            });
        let (target, center, rotation, text, width, auto_resize, style) =
            if let Some((info, style)) = info {
                let target = if self.document.engine().text_element_info(info.id).is_ok() {
                    draw::TextCommitTarget::Existing(info.id)
                } else if let Some(bubble) = info.bubble_id {
                    draw::TextCommitTarget::NewSpeechBubble(bubble)
                } else if let Some(arrow) = info.arrow_id {
                    draw::TextCommitTarget::NewArrow(arrow)
                } else {
                    draw::TextCommitTarget::New
                };
                (
                    target,
                    info.center,
                    info.rotation,
                    info.text,
                    info.width.max(1.0),
                    info.auto_resize,
                    style,
                )
            } else {
                (
                    draw::TextCommitTarget::New,
                    point,
                    0.0,
                    String::new(),
                    320.0,
                    true,
                    style,
                )
            };
        if text.len() > crate::text_buffer::MAX_TEXT_BYTES || text.contains('\0') {
            self.fail("Text exceeds the 64 KiB editing limit or contains NUL", cx);
            return;
        }
        let layout = self
            .metrics
            .layout(&text, &style, if auto_resize { 1.0e8 } else { width });
        self.draft = Some(TextDraft {
            buffer: TextBuffer::new(text),
            target,
            center,
            rotation,
            style,
            width,
            auto_resize,
            layout,
            changed: false,
            minimum_grapheme_width: 1.0,
            ink_reserve: 0.0,
            preferred_x: None,
            caret_affinity: None,
        });
        self.focus.focus(window, cx);
        self.update_draft(window, cx);
    }

    /// Opens the selected arrow/bubble label or creates/edits a serial label.
    pub fn edit_selected_label(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_text(true, window, cx);
        let viewport = self.session.viewport();
        let info = self
            .document
            .engine()
            .speech_bubble_text_target(viewport, None)
            .ok()
            .flatten()
            .or_else(|| {
                self.document
                    .engine()
                    .arrow_text_target(viewport, None)
                    .ok()
                    .flatten()
            });
        if let Some((info, style)) = info {
            self.begin_text_with_target(info.center, None, Some((info, style)), window, cx);
            return;
        }
        let selected = self.document.engine().selected_ids();
        let serial = self.session.scene().ordered_nodes().map(|node| node.item.as_ref()).any(|item| matches!(item, draw::SceneDisplayItem::SerialNumber(item)
            if selected.iter().any(|id| id.index == item.id.index && id.generation == item.id.generation)));
        if serial {
            self.create_serial_number_text(window, cx);
            return;
        }
        let existing = selected
            .into_iter()
            .find(|id| self.document.engine().text_element_info(*id).is_ok());
        self.begin_text(
            self.session.scene().frame().camera.center,
            existing,
            window,
            cx,
        );
    }
    pub fn create_serial_number_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = self.session.viewport();
        let Ok(state) = self
            .document
            .engine()
            .viewport_style_toolbar_state(viewport)
        else {
            return;
        };
        let layout = self.metrics.layout("", &state.text_style, 1.0e8);
        let result = self.document.command(|engine| {
            engine.create_serial_number_text_with_viewport_changes(
                viewport,
                draw::TextLayoutSize::with_content(
                    layout.width.max(1.0),
                    layout.height,
                    layout.width.max(1.0),
                    layout.height,
                ),
            )
        });
        match result {
            Ok((_, Some(id))) => {
                self.changed(window, cx);
                self.begin_text(
                    self.session.scene().frame().camera.center,
                    Some(id),
                    window,
                    cx,
                );
            }
            Ok(_) => self.changed(window, cx),
            Err(error) => self.fail(error, cx),
        }
    }

    pub(super) fn update_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        draft.preferred_x = None;
        draft.caret_affinity = None;
        draft.layout = self.metrics.layout(
            &draft.buffer.text,
            &draft.style,
            if draft.auto_resize {
                self.options
                    .text_editing_bounds
                    .filter(|_| draft.rotation.abs() < 1.0e-8)
                    .map_or(1.0e8, |area| area.width.max(1.0))
            } else {
                draft.width
            },
        );
        let width = if draft.auto_resize {
            draft.layout.width.max(1.0)
        } else {
            draft.width
        };
        if draft.auto_resize
            && draft.rotation.abs() < 1.0e-8
            && let Some(area) = self.options.text_editing_bounds
        {
            draft.center.x = draft.center.x.clamp(
                area.x + width / 2.0,
                (area.x + area.width - width / 2.0).max(area.x + width / 2.0),
            );
        }
        let layout = draw::TextLayoutSize::with_content(
            width,
            draft.layout.height,
            draft.layout.width,
            draft.layout.height,
        );
        let style = &draft.style;
        draft.ink_reserve = draft.layout.ink_reserve;
        let text = draw::TextData {
            center: draft.center,
            layout,
            rotation: draft.rotation,
            text: draft.buffer.text.clone(),
            text_ink_reserve: draft.ink_reserve,
            color: style.color,
            font_size: style.font_size,
            bold: style.bold,
            italic: style.italic,
            font_family: style.font_family.clone(),
            fill: style.fill,
            fill_style: style.fill_style,
            stroke: style.stroke,
            stroke_width: style.stroke_width,
            corner_radii: style.corner_radii,
            horizontal_align: style.horizontal_align,
            vertical_align: style.vertical_align,
            auto_resize: draft.auto_resize,
            opacity: style.opacity,
        };
        let target = match draft.target {
            draw::TextCommitTarget::New => draw::ActiveTextDraftTarget::New,
            draw::TextCommitTarget::Existing(id) => draw::ActiveTextDraftTarget::Existing(id),
            draw::TextCommitTarget::NewArrow(id) => draw::ActiveTextDraftTarget::NewArrow(id),
            draw::TextCommitTarget::NewSpeechBubble(id) => {
                draw::ActiveTextDraftTarget::NewSpeechBubble(id)
            }
        };
        let minimum = draft
            .buffer
            .text
            .graphemes(true)
            .map(|g| self.metrics.text_width(g, &draft.style))
            .fold(1.0, f64::max);
        draft.minimum_grapheme_width = minimum;
        let viewport = self.session.viewport();
        if let Err(error) = self.document.command(|engine| {
            engine.set_speech_bubble_aware_text_draft(
                viewport,
                draw::ActiveTextDraftPresentation {
                    target,
                    revision: 0,
                    text,
                },
                minimum,
            )
        }) {
            self.fail(error, cx);
        }
        self.reset_text_caret(window, cx);
        self.changed(window, cx);
    }

    pub(super) fn finish_text(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_text_caret(cx);
        self.text_selection_overlay.clear(window);
        let Some(mut draft) = self.draft.take() else {
            return;
        };
        if !commit {
            self.invalidate_render();
        }
        draft.buffer.unmark();
        let viewport = self.session.viewport();
        let result = self.document.command(|engine| {
            engine.clear_active_text_draft_presentation_with_viewport_changes(viewport)?;
            if commit
                && (draft.changed || !matches!(draft.target, draw::TextCommitTarget::Existing(_)))
            {
                let width = if draft.auto_resize {
                    draft.layout.width.max(1.0)
                } else {
                    draft.width
                };
                let mut commit = draw::TextDraftCommit::new(
                    draft.target,
                    draft.center,
                    draft.buffer.text,
                    draw::TextLayoutSize::with_content(
                        width,
                        draft.layout.height,
                        draft.layout.width,
                        draft.layout.height,
                    ),
                    draft.style,
                    draft.auto_resize,
                    true,
                );
                commit.minimum_grapheme_width = draft.minimum_grapheme_width;
                commit.text_ink_reserve = draft.ink_reserve;
                engine.commit_text_draft_with_viewport_changes(viewport, commit)?;
            }
            Ok(())
        });
        if let Err(error) = result {
            self.fail(error, cx);
        }
        if self.selecting_text {
            window.release_pointer();
        }
        self.selecting_text = false;
        self.changed(window, cx);
    }

    pub(super) fn text_local_position(&mut self, byte: usize) -> Option<(f64, f64)> {
        let draft = self.draft.as_ref()?;
        let width = if draft.auto_resize {
            draft.layout.width.max(1.0)
        } else {
            draft.width
        };
        let mut offset = 0;
        for (index, line) in draft.layout.lines.iter().enumerate() {
            let end = offset + line.text.len();
            let affinity = draft
                .caret_affinity
                .filter(|(position, _, _)| *position == byte);
            let on_line = affinity.map_or(
                byte < end
                    || index + 1 == draft.layout.lines.len()
                    || draft
                        .layout
                        .lines
                        .get(index + 1)
                        .is_some_and(|next| next.start > byte),
                |(_, line, _)| line == index,
            );
            if on_line {
                let local = byte.saturating_sub(offset).min(line.text.len());
                let x = affinity.map_or_else(|| line.caret_x(local), |(_, _, x)| x);
                let left = match draft.style.horizontal_align {
                    draw::TextHorizontalAlign::Left => -width / 2.0,
                    draw::TextHorizontalAlign::Center => -line.width / 2.0,
                    draw::TextHorizontalAlign::Right => width / 2.0 - line.width,
                };
                return Some((
                    left + x,
                    -draft.layout.height / 2.0 + index as f64 * draft.layout.line_height,
                ));
            }
            offset = end;
            if draft.buffer.text[offset..].starts_with('\n') {
                offset += 1;
            }
        }
        None
    }

    pub(super) fn text_world_position(&self, x: f64, y: f64) -> Option<Point<Pixels>> {
        let draft = self.draft.as_ref()?;
        let (sin, cos) = draft.rotation.sin_cos();
        let world = draw::Point::new(
            draft.center.x + x * cos - y * sin,
            draft.center.y + x * sin + y * cos,
        );
        let frame = self.session.scene().frame();
        let view = draw::canvas_to_view(world, &frame.camera, frame.surface);
        Some(self.bounds.origin + point(px(view.x as f32), px(view.y as f32)))
    }

    pub(super) fn text_segments(&mut self, range: Range<usize>) -> Vec<(f64, f64, f64, f64)> {
        if range.is_empty() {
            return self
                .text_local_position(range.start)
                .map(|(x, y)| {
                    vec![(
                        x,
                        y,
                        x + 1.0,
                        y + self.draft.as_ref().unwrap().layout.line_height,
                    )]
                })
                .unwrap_or_default();
        }
        let Some(draft) = &self.draft else {
            return Vec::new();
        };
        let width = if draft.auto_resize {
            draft.layout.width
        } else {
            draft.width
        };
        let mut rectangles = Vec::new();
        for (index, line) in draft.layout.lines.iter().enumerate() {
            let end = line.start + line.text.len();
            if range.start > end || range.end < line.start {
                continue;
            }
            let left = match draft.style.horizontal_align {
                draw::TextHorizontalAlign::Left => -width / 2.0,
                draw::TextHorizontalAlign::Center => -line.width / 2.0,
                draw::TextHorizontalAlign::Right => width / 2.0 - line.width,
            };
            let y = -draft.layout.height / 2.0 + index as f64 * draft.layout.line_height;
            for (a, b) in line.selection_segments(
                range.start.saturating_sub(line.start)
                    ..range.end.saturating_sub(line.start).min(line.text.len()),
            ) {
                rectangles.push((left + a, y, left + b, y + draft.layout.line_height));
            }
            if range.end > end && draft.buffer.text[end..].starts_with('\n') {
                rectangles.push((
                    left + line.width,
                    y,
                    left + line.width + draft.layout.line_height * 0.3,
                    y + draft.layout.line_height,
                ));
            }
        }
        rectangles
    }

    pub(super) fn move_text_vertical(&mut self, down: bool, extend: bool) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        let index = draft
            .caret_affinity
            .filter(|(byte, _, _)| *byte == draft.buffer.caret)
            .map(|(_, line, _)| line)
            .unwrap_or_else(|| {
                draft
                    .layout
                    .lines
                    .iter()
                    .rposition(|line| draft.buffer.caret >= line.start)
                    .unwrap_or(0)
            });
        let line = &draft.layout.lines[index];
        let affinity_x = draft
            .caret_affinity
            .filter(|(byte, row, _)| *byte == draft.buffer.caret && *row == index)
            .map(|(_, _, x)| x);
        let x = *draft.preferred_x.get_or_insert_with(|| {
            affinity_x
                .unwrap_or_else(|| line.caret_x(draft.buffer.caret.saturating_sub(line.start)))
        });
        let next = if down {
            (index + 1).min(draft.layout.lines.len() - 1)
        } else {
            index.saturating_sub(1)
        };
        let line = &draft.layout.lines[next];
        draft.buffer.select(line.start + line.byte_at_x(x), extend);
        draft.caret_affinity = Some((
            draft.buffer.caret,
            next,
            line.caret_x(draft.buffer.caret - line.start),
        ));
    }

    pub(super) fn text_index_at(&mut self, point: Point<Pixels>) -> Option<usize> {
        let draft = self.draft.as_ref()?;
        let frame = self.session.scene().frame();
        let local = point - self.bounds.origin;
        let world = draw::view_to_canvas(
            draw::Point::new(f64::from(f32::from(local.x)), f64::from(f32::from(local.y))),
            &frame.camera,
            frame.surface,
        );
        let (sin, cos) = draft.rotation.sin_cos();
        let dx = world.x - draft.center.x;
        let dy = world.y - draft.center.y;
        let y = -dx * sin + dy * cos;
        let line_index = ((y + draft.layout.height / 2.0) / draft.layout.line_height)
            .floor()
            .max(0.0) as usize;
        let line_index = line_index.min(draft.layout.lines.len().saturating_sub(1));
        let mut offset = 0;
        for line in &draft.layout.lines[..line_index] {
            offset += line.text.len();
            if draft.buffer.text[offset..].starts_with('\n') {
                offset += 1;
            }
        }
        let line = &draft.layout.lines[line_index];
        let width = if draft.auto_resize {
            draft.layout.width
        } else {
            draft.width
        };
        let left = match draft.style.horizontal_align {
            draw::TextHorizontalAlign::Left => -width / 2.0,
            draw::TextHorizontalAlign::Center => -line.width / 2.0,
            draw::TextHorizontalAlign::Right => width / 2.0 - line.width,
        };
        let x = dx * cos + dy * sin - left;
        let byte = offset + line.byte_at_x(x);
        let caret_x = line
            .carets
            .iter()
            .filter(|(index, _)| *index == byte - offset)
            .min_by(|(_, a), (_, b)| (a - x).abs().total_cmp(&(b - x).abs()))
            .map_or(x, |(_, x)| *x);
        self.draft.as_mut().unwrap().caret_affinity = Some((byte, line_index, caret_x));
        Some(byte)
    }

    pub(super) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.options.interaction_enabled {
            return;
        }
        // Windows uses this flag for dead keys and AltGr-generated characters.
        // Their Ctrl/Alt bits must not turn native text into editor commands.
        if self.draft.is_some() && event.prefer_character_input {
            return;
        }
        let resolved_key = self.command_key(&event.keystroke, cx);
        let key = resolved_key.as_ref();
        let modifiers = event.keystroke.modifiers;
        let command = modifiers.secondary(cx);
        if key == "delete" && self.draft.is_some() && !self.options.delete_text_after_cursor {
            let draft = self.draft.as_mut().unwrap();
            draft.buffer.anchor = 0;
            draft.buffer.caret = draft.buffer.text.len();
            draft.buffer.replace(None, "", false, None);
            draft.changed = true;
            self.finish_text(true, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.draft.is_some() {
            if key == "escape" {
                self.finish_text(false, window, cx);
                cx.stop_propagation();
                return;
            }
            if key == "enter" && command {
                self.finish_text(true, window, cx);
                cx.stop_propagation();
                return;
            }
            if matches!(key, "up" | "down") {
                self.move_text_vertical(key == "down", modifiers.shift);
                self.reset_text_caret(window, cx);
                cx.stop_propagation();
                return;
            }
            let mut changed = false;
            if command && matches!(key, "c" | "x") {
                let draft = self.draft.as_ref().unwrap();
                let selected = draft.buffer.selection();
                if !selected.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        draft.buffer.text[selected].to_string(),
                    ));
                }
                if key == "x" {
                    self.draft
                        .as_mut()
                        .unwrap()
                        .buffer
                        .replace(None, "", false, None);
                    changed = true;
                }
            } else if command && key == "v" {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    if !self
                        .draft
                        .as_mut()
                        .unwrap()
                        .buffer
                        .replace(None, &text, false, None)
                    {
                        self.fail("Text exceeds the 64 KiB editing limit or contains NUL", cx);
                        cx.stop_propagation();
                        return;
                    }
                    changed = true;
                }
            } else {
                let draft = self.draft.as_mut().unwrap();
                match key {
                    "a" if command => {
                        draft.buffer.anchor = 0;
                        draft.buffer.caret = draft.buffer.text.len();
                    }
                    "z" if command => {
                        draft.buffer.undo(modifiers.shift);
                        changed = true;
                    }
                    "y" if command => {
                        draft.buffer.undo(true);
                        changed = true;
                    }
                    "left" if command => draft.buffer.move_word(false, modifiers.shift),
                    "right" if command => draft.buffer.move_word(true, modifiers.shift),
                    "left" => draft.buffer.move_horizontal(false, modifiers.shift),
                    "right" => draft.buffer.move_horizontal(true, modifiers.shift),
                    "home" => {
                        let start = draft.buffer.text[..draft.buffer.caret]
                            .rfind('\n')
                            .map_or(0, |index| index + 1);
                        draft
                            .buffer
                            .select(if command { 0 } else { start }, modifiers.shift);
                    }
                    "end" => {
                        let end = draft.buffer.text[draft.buffer.caret..]
                            .find('\n')
                            .map_or(draft.buffer.text.len(), |index| index + draft.buffer.caret);
                        draft.buffer.select(
                            if command {
                                draft.buffer.text.len()
                            } else {
                                end
                            },
                            modifiers.shift,
                        );
                    }
                    "backspace" => {
                        draft.buffer.delete(false);
                        changed = true;
                    }
                    "delete" => {
                        draft.buffer.delete(true);
                        changed = true;
                    }
                    "enter" => {
                        if !draft.buffer.replace(None, "\n", false, None) {
                            self.fail("Text exceeds the 64 KiB editing limit", cx);
                            cx.stop_propagation();
                            return;
                        }
                        changed = true;
                    }
                    "tab" => {
                        self.finish_text(true, window, cx);
                        return;
                    }
                    _ => return,
                }
            }
            if key != "c" || !command {
                self.draft.as_mut().unwrap().preferred_x = None;
                self.draft.as_mut().unwrap().caret_affinity = None;
            }
            if changed {
                self.draft.as_mut().unwrap().changed = true;
                self.update_draft(window, cx);
            } else {
                self.reset_text_caret(window, cx);
            }
            cx.stop_propagation();
            return;
        }
        if key == "space" {
            self.space_down = true;
            self.space_key = Some(event.keystroke.key.clone());
            self.cursor = CursorStyle::OpenHand;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if command {
            if key == "c" && modifiers.shift {
                self.copy_image(window, cx);
                cx.stop_propagation();
                return;
            }
            match key {
                "z" => {
                    self.command(
                        |engine, _| {
                            if modifiers.shift {
                                engine.redo_with_viewport_changes()
                            } else {
                                engine.undo_with_viewport_changes()
                            }
                        },
                        window,
                        cx,
                    );
                }
                "y" => self.command(|engine, _| engine.redo_with_viewport_changes(), window, cx),
                "d" => self.command(
                    |engine, viewport| {
                        engine.duplicate_selected_with_viewport_changes(
                            viewport,
                            draw::Point::new(20.0, 20.0),
                        )
                    },
                    window,
                    cx,
                ),
                "c" => {
                    if let Ok(bytes) = self.document.engine().serialize_selected_draw_template() {
                        use base64::Engine as _;
                        cx.write_to_clipboard(ClipboardItem::new_string(format!(
                            "snow-draw-template:{}",
                            base64::engine::general_purpose::STANDARD.encode(bytes)
                        )));
                    }
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                        && let Some(encoded) = text.strip_prefix("snow-draw-template:")
                    {
                        use base64::Engine as _;
                        match base64::engine::general_purpose::STANDARD.decode(encoded) {
                            Ok(bytes) => {
                                let center = self.session.scene().frame().camera.center;
                                self.command(
                                    |engine, viewport| {
                                        engine.insert_draw_template_with_viewport_changes(
                                            viewport, &bytes, center,
                                        )
                                    },
                                    window,
                                    cx,
                                )
                            }
                            Err(error) => self.fail(error, cx),
                        }
                    }
                }
                "0" => self.fit(window, cx),
                "=" | "+" => self.zoom(1.2, window, cx),
                "-" => self.zoom(1.0 / 1.2, window, cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        match key {
            "escape" => {
                self.cancel_gesture(window, cx);
                self.changed(window, cx);
            }
            "backspace" | "delete" => self.command(
                |engine, viewport| engine.delete_selected_with_viewport_changes(viewport),
                window,
                cx,
            ),
            "enter" => self.edit_selected_label(window, cx),
            "v" => self.set_tool(draw::ActiveTool::Select, window, cx),
            "r" => self.set_tool(draw::ActiveTool::Shape, window, cx),
            "a" => self.set_tool(draw::ActiveTool::Arrow, window, cx),
            "l" => self.set_tool(draw::ActiveTool::Line, window, cx),
            "p" => self.set_tool(draw::ActiveTool::FreeDraw, window, cx),
            "t" => self.set_tool(draw::ActiveTool::Text, window, cx),
            "e" => self.set_tool(draw::ActiveTool::Eraser, window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
    pub(super) fn key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.space_key.as_ref() == Some(&event.keystroke.key) || event.keystroke.key == "space" {
            self.space_down = false;
            self.space_key = None;
            self.cursor = CursorStyle::Arrow;
            cx.notify();
        }
    }
}

impl EntityInputHandler for CanvasView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let draft = self.draft.as_ref()?;
        let range = draft.buffer.byte_range_from_utf16(range);
        *adjusted = Some(draft.buffer.utf16(range.clone()));
        Some(draft.buffer.text[range].to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let draft = self.draft.as_ref()?;
        Some(UTF16Selection {
            range: draft.buffer.utf16(draft.buffer.selection()),
            reversed: draft.buffer.caret < draft.buffer.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let draft = self.draft.as_ref()?;
        draft
            .buffer
            .marked
            .clone()
            .map(|range| draft.buffer.utf16(range))
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.draft {
            draft.buffer.unmark();
            self.reset_text_caret(window, cx);
        }
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = &mut self.draft {
            if !draft.buffer.replace(range, text, false, None) {
                self.fail("Text exceeds the 64 KiB editing limit or contains NUL", cx);
                return;
            }
            draft.changed = true;
            self.update_draft(window, cx);
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = &mut self.draft {
            if !draft.buffer.replace(range, text, true, selected) {
                self.fail("Text exceeds the 64 KiB editing limit or contains NUL", cx);
                return;
            }
            draft.changed = true;
            self.update_draft(window, cx);
        }
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.draft.as_ref()?.buffer.byte_range_from_utf16(range);
        let rectangles = self.text_segments(range);
        let points: Vec<_> = rectangles
            .into_iter()
            .flat_map(|(left, top, right, bottom)| {
                [(left, top), (right, top), (right, bottom), (left, bottom)]
            })
            .filter_map(|(x, y)| self.text_world_position(x, y))
            .collect();
        if points.is_empty() {
            return None;
        }
        let left = points
            .iter()
            .map(|p| f32::from(p.x))
            .fold(f32::INFINITY, f32::min);
        let top = points
            .iter()
            .map(|p| f32::from(p.y))
            .fold(f32::INFINITY, f32::min);
        let right = points
            .iter()
            .map(|p| f32::from(p.x))
            .fold(f32::NEG_INFINITY, f32::max);
        let bottom = points
            .iter()
            .map(|p| f32::from(p.y))
            .fold(f32::NEG_INFINITY, f32::max);
        Some(Bounds::from_corners(
            point(px(left), px(top)),
            point(px(right), px(bottom)),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let index = self.text_index_at(point)?;
        Some(self.draft.as_ref()?.buffer.utf16(0..index).end)
    }
    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = &mut self.draft {
            let range = draft.buffer.byte_range_from_utf16(range);
            draft.buffer.anchor = range.start;
            draft.buffer.caret = range.end;
            draft.preferred_x = None;
            draft.caret_affinity = None;
            self.reset_text_caret(window, cx);
        }
    }
    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.draft.as_ref()?.buffer.text.encode_utf16().count())
    }
}
