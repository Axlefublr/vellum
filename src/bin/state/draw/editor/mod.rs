mod elements;
mod interaction;
mod properties;

use self::interaction::Interaction;
use self::properties::ToolPropertySet;
use super::Modifiers;
use super::history::{Entry as HistoryEntry, History};
use super::picker::{Choice, Picker, ShapeFills, choice, picker_geometry};
use super::scene::{Element, geometry};
use super::scene::{ElementId, ElementKind, Point, Style};
use super::selection;
use super::text_edit::TextEdit;
pub(crate) use super::text_edit::{CursorMove, TextInputBatch};
use super::tool::Tool;
use crate::render::Geometry;

pub(crate) enum Action {
    Undo,
    Redo,
    SelectAll,
    ToggleEraser,
    ToggleFill,
    Delete,
    Clear,
    Cancel,
    CommitText,
    Backspace,
    BackspaceWord,
    MoveCursor(CursorMove, bool),
    InsertText(String),
    ApplyTextInput(TextInputBatch),
}

#[derive(Default)]
pub struct EditorEffect {
    pub changed: bool,
    pub deactivate: bool,
    pub feedback: Option<String>,
}

pub struct Editor {
    tool: Tool,
    style: Style,
    elements: Vec<Element>,
    selected: Vec<ElementId>,
    interaction: Option<Interaction>,
    history: History,
    next_id: ElementId,
    next_text_session: u64,
    picker: Option<Picker>,
    default_tool: Tool,
    last_non_eraser_tool: Tool,
    tool_properties: ToolPropertySet,
    default_tool_properties: ToolPropertySet,
    size_ranges: std::sync::Arc<std::collections::BTreeMap<Tool, crate::config::SizeRange>>,
    remember_last_tool: bool,
    palette: Vec<[f32; 4]>,
}

impl Editor {
    pub fn new(settings: crate::Settings) -> Self {
        let default_tool_properties = ToolPropertySet::new(
            settings.stroke_size,
            settings.default_color[3],
            settings.default_fill_shapes,
            &settings.tool_defaults,
            &settings.size_ranges,
        );
        let tool_properties = default_tool_properties;
        let active = tool_properties.properties(settings.default_tool).copied();
        let fallback = tool_properties
            .properties(Tool::Pen)
            .copied()
            .expect("pen has adjustable properties");
        let active = active.unwrap_or(fallback);
        let mut color = settings.default_color;
        color[3] = active.opacity;
        Self {
            tool: settings.default_tool,
            style: Style {
                size: active.size,
                color,
                roundness: active.roundness,
                filled: active.filled,
            },
            elements: Vec::new(),
            selected: Vec::new(),
            interaction: None,
            history: History::default(),
            next_id: 1,
            next_text_session: 1,
            picker: None,
            default_tool: settings.default_tool,
            last_non_eraser_tool: if settings.default_tool == Tool::Eraser {
                Tool::Pen
            } else {
                settings.default_tool
            },
            tool_properties,
            default_tool_properties,
            size_ranges: settings.size_ranges,
            remember_last_tool: settings.remember_last_tool,
            palette: settings.palette,
        }
    }

    pub fn activate(&mut self) -> bool {
        if self.remember_last_tool || self.tool == self.default_tool {
            return false;
        }
        self.switch_tool(self.default_tool)
    }

    pub fn deactivate(&mut self) -> bool {
        let changed = self.finish_interaction();
        let clear_preview =
            !std::mem::take(&mut self.selected).is_empty() | self.picker.take().is_some();
        changed | clear_preview
    }

    pub fn is_editing_text(&self) -> bool {
        matches!(self.interaction, Some(Interaction::EditingText(_)))
    }

    pub fn is_drawing_pen(&self) -> bool {
        matches!(self.interaction, Some(Interaction::Freehand(_)))
    }

    pub(super) fn text_edit(&self) -> Option<&TextEdit> {
        match &self.interaction {
            Some(Interaction::EditingText(edit)) => Some(edit),
            _ => None,
        }
    }

    fn text_edit_mut(&mut self) -> Option<&mut TextEdit> {
        match &mut self.interaction {
            Some(Interaction::EditingText(edit)) => Some(edit),
            _ => None,
        }
    }

    pub fn current_color(&self) -> [f32; 4] {
        if let Some(edit) = self.text_edit() {
            edit.style.color
        } else if let Some(element) = self.selected.last().and_then(|id| self.element(*id)) {
            element.style.color
        } else {
            self.style.color
        }
    }

    pub fn picker_active(&self) -> bool {
        self.picker.is_some()
    }

    pub fn handle_action(&mut self, action: Action) -> EditorEffect {
        let mut effect = EditorEffect::default();
        if let Action::ApplyTextInput(batch) = action {
            let submit = batch.submit;
            if let Some(edit) = self.text_edit_mut() {
                effect.changed = edit.apply_text_input(batch);
                if submit {
                    effect.changed |= self.commit_text();
                }
            }
            return effect;
        }
        let closed_picker = self.picker.take().is_some();
        if closed_picker && matches!(action, Action::Cancel) {
            effect.changed = true;
            return effect;
        }
        match action {
            Action::Undo if !self.is_editing_text() => effect.changed = self.undo(),
            Action::Redo if !self.is_editing_text() => effect.changed = self.redo(),
            Action::SelectAll => {
                effect.changed = if let Some(edit) = self.text_edit_mut() {
                    edit.select_all()
                } else {
                    self.select_all()
                };
            }
            Action::ToggleEraser => effect.changed = self.toggle_eraser(),
            Action::ToggleFill => {
                let adjustment = self.toggle_fill();
                effect.changed = adjustment.changed;
                effect.feedback = adjustment.feedback;
            }
            Action::Delete => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.delete();
                } else {
                    effect.changed = self.delete_selection();
                }
            }
            Action::Clear => effect.changed = self.clear(),
            Action::Cancel => {
                let cancelled = self.cancel_interaction();
                if cancelled || !std::mem::take(&mut self.selected).is_empty() {
                    effect.changed = true;
                } else {
                    effect.deactivate = true;
                }
            }
            Action::CommitText => effect.changed = self.commit_text(),
            Action::Backspace => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.backspace();
                }
            }
            Action::BackspaceWord => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.backspace_word();
                }
            }
            Action::MoveCursor(movement, extend) => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.move_cursor(movement, extend);
                }
            }
            Action::InsertText(text) => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.insert(&text);
                }
            }
            Action::Undo | Action::Redo | Action::ApplyTextInput(_) => {}
        }
        effect.changed |= closed_picker;
        effect
    }

    pub fn open_picker(&mut self, center: Point) {
        self.picker = Some(Picker {
            center,
            hovered: None,
        });
    }

    pub fn picker_motion(&mut self, point: Point) -> bool {
        let Some(picker) = &mut self.picker else {
            return false;
        };
        let choice = choice(picker.center, point, self.palette.len());
        let changed = picker.hovered != choice;
        picker.hovered = choice;
        changed
    }

    pub fn picker_release(&mut self, point: Point, latch_center: bool) -> bool {
        let Some(picker) = self.picker else {
            return false;
        };
        let choice = choice(picker.center, point, self.palette.len());
        if choice.is_none() && latch_center {
            return false;
        }
        self.picker = None;
        match choice {
            Some(Choice::Color(index)) => {
                self.apply_rgba(self.palette[index]);
            }
            Some(Choice::Tool(tool)) => {
                self.switch_tool(tool);
            }
            None => {}
        }
        true
    }

    pub fn dismiss_picker(&mut self) -> bool {
        self.picker.take().is_some()
    }

    fn toggle_eraser(&mut self) -> bool {
        let tool = if self.tool == Tool::Eraser {
            self.last_non_eraser_tool
        } else {
            Tool::Eraser
        };
        self.switch_tool(tool)
    }

    fn color_tool(&self) -> Tool {
        if self.tool == Tool::Eraser {
            self.last_non_eraser_tool
        } else {
            self.tool
        }
    }

    pub fn append_preview_geometry(&self, output: &mut Vec<Geometry>) {
        match &self.interaction {
            Some(Interaction::Freehand(stroke)) => output.push(stroke.tail_geometry()),
            Some(Interaction::Drawing {
                tool,
                start,
                current,
                modifiers,
            }) => output.push(geometry(
                &drawing_kind(*tool, *start, *current, *modifiers),
                self.style,
            )),
            _ => {}
        }
    }

    pub fn append_selection_geometry(&self, show_handles: bool, output: &mut Vec<Geometry>) {
        if self.tool != Tool::Select {
            return;
        }
        if self.selected.len() > 1 {
            let mut bounds: Option<(Point, Point)> = None;
            for id in &self.selected {
                let Some(element) = self.element(*id) else {
                    continue;
                };
                let offset = self.moving_offset(*id).unwrap_or_default();
                let (min, max) = (element.bounds.min + offset, element.bounds.max + offset);
                bounds = Some(bounds.map_or((min, max), |(current_min, current_max)| {
                    (
                        Point::new(current_min.x.min(min.x), current_min.y.min(min.y)),
                        Point::new(current_max.x.max(max.x), current_max.y.max(max.y)),
                    )
                }));
            }
            if let Some((min, max)) = bounds {
                output.push(selection::outline(min, max));
            }
            return;
        }
        if let Some(id) = self.selected.first() {
            self.append_selection_geometry_for(
                *id,
                show_handles && self.interaction.is_none(),
                output,
            );
        }
    }

    fn append_selection_geometry_for(
        &self,
        id: ElementId,
        show_handles: bool,
        output: &mut Vec<Geometry>,
    ) {
        let Some(element) = self.element(id) else {
            return;
        };
        match &self.interaction {
            Some(Interaction::EditingText(edit)) if edit.id == Some(id) => {
                let bounds = edit.bounds();
                output.push(selection::outline(bounds.min, bounds.max));
                return;
            }
            Some(Interaction::Resizing {
                id: resizing_id,
                current,
                ..
            }) if *resizing_id == id => {
                if !matches!(
                    current.kind,
                    ElementKind::Segment { .. } | ElementKind::Triangle { .. }
                ) {
                    output.push(selection::outline(current.bounds.min, current.bounds.max));
                }
                return;
            }
            _ => {}
        }
        let offset = self.moving_offset(id).unwrap_or_default();
        let kind = &element.kind;
        if !matches!(
            kind,
            ElementKind::Segment { .. } | ElementKind::Triangle { .. }
        ) {
            let bounds = element.bounds;
            output.push(selection::outline(bounds.min + offset, bounds.max + offset));
        }
        if !show_handles {
            return;
        }
        selection::append_handles(kind, element.style, output);
    }

    pub fn picker_geometry(&self) -> Option<crate::render::LocalGeometry> {
        let picker = self.picker?;
        let active = self.color_tool();
        Some(picker_geometry(
            picker.center,
            picker.hovered,
            active,
            self.current_color(),
            ShapeFills {
                triangle: self.tool_fill(Tool::Triangle),
                rectangle: self.tool_fill(Tool::Rectangle),
                ellipse: self.tool_fill(Tool::Ellipse),
            },
            &self.palette,
        ))
    }

    pub(super) fn clear_preedit(&mut self) -> bool {
        self.text_edit_mut().is_some_and(TextEdit::clear_preedit)
    }

    pub(super) fn make_text_edit(
        &mut self,
        id: Option<ElementId>,
        origin: Point,
        content: String,
        style: Style,
        scale: [f32; 2],
    ) -> TextEdit {
        let session = self.next_text_session;
        self.next_text_session = self.next_text_session.wrapping_add(1).max(1);
        TextEdit::new(session, id, origin, content, style, scale)
    }

    pub fn element_geometry_preview(&self, element: &Element) -> Option<Geometry> {
        if let Some(delta) = self.moving_offset(element.id) {
            return Some(element.geometry.translated([delta.x, delta.y]));
        }
        match &self.interaction {
            Some(Interaction::Resizing {
                id: resized,
                current,
                ..
            }) if *resized == element.id => Some(geometry(&current.kind, current.style)),
            _ => None,
        }
    }

    pub fn moving_offset(&self, id: ElementId) -> Option<Point> {
        let Some(Interaction::Moving {
            ids,
            start,
            current,
        }) = &self.interaction
        else {
            return None;
        };
        ids.contains(&id).then_some(*current - *start)
    }

    pub fn text_resize_preview(&self, id: ElementId) -> Option<(&ElementKind, Style)> {
        let Some(Interaction::Resizing {
            id: resized,
            current,
            ..
        }) = &self.interaction
        else {
            return None;
        };
        (*resized == id && matches!(current.kind, ElementKind::Text { .. }))
            .then_some((&current.kind, current.style))
    }

    fn switch_tool(&mut self, tool: Tool) -> bool {
        if self.tool == tool {
            return false;
        }
        self.finish_interaction();
        self.selected.clear();
        self.tool = tool;
        if tool != Tool::Eraser {
            self.last_non_eraser_tool = tool;
        }
        self.sync_active_style();
        true
    }
}

fn drawing_kind(tool: Tool, start: Point, current: Point, modifiers: Modifiers) -> ElementKind {
    match tool {
        Tool::Line | Tool::Arrow => ElementKind::Segment {
            points: [
                start,
                selection::constrained_endpoint(start, current, modifiers.shift),
            ],
            arrow: tool == Tool::Arrow,
        },
        Tool::Triangle => ElementKind::Triangle {
            vertices: selection::triangle_from_drag(start, current, modifiers),
        },
        Tool::Rectangle => {
            let (min, max) =
                selection::constrained_box(start, current, modifiers.shift, modifiers.alt);
            ElementKind::Rectangle { min, max }
        }
        Tool::Ellipse => {
            let (min, max) =
                selection::constrained_box(start, current, modifiers.shift, modifiers.alt);
            ElementKind::Ellipse {
                center: min.midpoint(max),
                radii: Point::new((max.x - min.x) * 0.5, (max.y - min.y) * 0.5),
            }
        }
        Tool::Pen | Tool::Text | Tool::Eraser | Tool::Select => unreachable!(),
    }
}
