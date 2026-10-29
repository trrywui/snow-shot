use snow_draw_engine_core::ErrorCode;
use snow_draw_engine_document::{ElementId, RectangleData, TextData, validate_text};
use snow_draw_engine_model::DocumentModel;

use crate::{
    ActiveTextDraftPresentation, ActiveTextDraftTarget, Editor,
    state::{InteractionState, ResizeHandle, SelectionEditMode},
    text::{
        MeasuredTextResize, TextSelectionResizeHandle, text_resize_layout_override_matches_rect,
        text_with_selection_rect,
    },
};

impl Editor {
    pub fn active_text_draft_presentation(&self) -> Option<ActiveTextDraftPresentation> {
        self.state.active_text_draft.clone()
    }

    pub fn active_text_draft_display_presentation(&self) -> Option<ActiveTextDraftPresentation> {
        let mut draft = self.state.active_text_draft.clone()?;
        let Some(id) = draft.existing_id() else {
            return Some(draft);
        };
        let InteractionState::EditingSelection(state) = &self.state.interaction else {
            return Some(draft);
        };
        let Some(preview) = state
            .preview_elements
            .iter()
            .find(|preview| preview.id == id)
        else {
            return Some(draft);
        };

        let single_text_resize =
            state.original_arrows.is_empty() && state.original_elements.len() == 1;
        let (resize_handle, measured_resize) = match state.mode {
            SelectionEditMode::Resize {
                handle,
                text_layout_override,
                ..
            } => {
                let measured_resize = text_layout_override
                    .filter(|layout_override| {
                        single_text_resize
                            && text_resize_layout_override_matches_rect(
                                *layout_override,
                                preview.rect,
                            )
                    })
                    .map(MeasuredTextResize::from_layout_override);
                (Some(handle), measured_resize)
            }
            _ => (None, None),
        };
        draft.text = text_with_selection_rect(
            &draft.text,
            preview.rect,
            resize_handle.map(|handle| TextSelectionResizeHandle {
                x_sign: handle.x_sign(),
                y_sign: handle.y_sign(),
            }),
            single_text_resize,
            measured_resize,
        );
        Some(draft)
    }

    pub fn set_active_text_draft_presentation(
        &mut self,
        document: &DocumentModel,
        mut draft: ActiveTextDraftPresentation,
    ) -> Result<bool, ErrorCode> {
        validate_text(&draft.text)?;
        if let ActiveTextDraftTarget::Existing(id) = draft.target {
            document.text(id)?;
        }
        let bubble_id = match draft.target {
            ActiveTextDraftTarget::NewSpeechBubble(id) => Some(id),
            ActiveTextDraftTarget::Existing(id) => document.speech_bubble_id_for_text(id),
            _ => None,
        };
        if let Some(owner) = bubble_id {
            let meta = document.element(owner)?.meta;
            let bubble = document.speech_bubble(owner)?;
            if meta.locked
                || !meta.visible
                || (matches!(draft.target, ActiveTextDraftTarget::NewSpeechBubble(_))
                    && bubble.text_element_id.is_some())
            {
                return Err(ErrorCode::InvalidState);
            }
        }
        let arrow_id = match draft.target {
            ActiveTextDraftTarget::NewArrow(id) => Some(id),
            ActiveTextDraftTarget::Existing(id) => document.arrow_id_for_text(id),
            ActiveTextDraftTarget::New | ActiveTextDraftTarget::NewSpeechBubble(_) => None,
        };
        if let Some(id) = arrow_id {
            let arrow = document.arrow(id)?;
            if arrow.linear_kind != snow_draw_engine_document::LinearElementKind::Arrow
                || document.element(id)?.meta.locked
                || !document.element(id)?.meta.visible
                || (matches!(draft.target, ActiveTextDraftTarget::NewArrow(_))
                    && arrow.text_element_id.is_some())
            {
                return Err(ErrorCode::InvalidState);
            }
            draft.text.center = snow_draw_engine_document::arrow_text_anchor(arrow);
            draft.text.rotation = 0.0;
        }
        let draft_changed = self.state.active_text_draft.as_ref() != Some(&draft);
        let should_clear_selection =
            draft.target == ActiveTextDraftTarget::New && !self.state.selection.is_empty();
        if !draft_changed && !should_clear_selection {
            return Ok(false);
        }

        if should_clear_selection {
            self.clear_selection();
        }
        if !draft_changed {
            return Ok(true);
        }

        self.state.active_distance_value_owner = None;
        self.state.active_text_draft = Some(draft);
        self.bump_scene_state_revision();
        self.bump_overlay_state_revision();
        Ok(true)
    }

    pub fn clear_active_text_draft_presentation(&mut self) -> bool {
        self.state.active_distance_value_owner = None;
        let had_draft = self.state.active_text_draft.take().is_some();
        let had_bubble_geometry = self.speech_bubble_draft_geometry.take().is_some();
        if !had_draft && !had_bubble_geometry {
            return false;
        }

        self.bump_scene_state_revision();
        self.bump_overlay_state_revision();
        true
    }

    pub(crate) fn active_text_draft_existing_id(&self) -> Option<ElementId> {
        self.state
            .active_text_draft
            .as_ref()
            .and_then(ActiveTextDraftPresentation::existing_id)
    }

    pub(crate) fn active_text_draft_rect_for_id(&self, id: ElementId) -> Option<RectangleData> {
        let draft = self.state.active_text_draft.as_ref()?;
        (draft.existing_id() == Some(id)).then_some(draft.rect())
    }

    pub(crate) fn active_text_draft_text_for_id(&self, id: ElementId) -> Option<TextData> {
        let draft = self.state.active_text_draft.as_ref()?;
        (draft.existing_id() == Some(id)).then(|| draft.text.clone())
    }

    pub(crate) fn update_active_text_draft_selection_rect(
        &mut self,
        original_rect: RectangleData,
        rect: RectangleData,
        resize_handle: Option<ResizeHandle>,
        single_text_resize: bool,
        measured_resize: Option<MeasuredTextResize>,
    ) -> Result<bool, ErrorCode> {
        let Some(draft) = self.state.active_text_draft.as_mut() else {
            return Ok(false);
        };

        let original_text = draft.with_rect(original_rect).text;
        let next_text = text_with_selection_rect(
            &original_text,
            rect,
            resize_handle.map(|handle| TextSelectionResizeHandle {
                x_sign: handle.x_sign(),
                y_sign: handle.y_sign(),
            }),
            single_text_resize,
            measured_resize,
        );
        validate_text(&next_text)?;
        if draft.text == next_text {
            return Ok(false);
        }

        draft.text = next_text;
        self.bump_scene_state_revision();
        self.bump_overlay_state_revision();
        Ok(true)
    }
}
