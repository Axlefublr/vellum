mod editor;
mod freehand;
mod history;
mod picker;
mod scene;
mod selection;
mod text_edit;
mod tool;
mod triangle;

use crate::render::{Geometry, SceneItem, TextSpec, Viewport, WgpuState, text_line_height};
use peniko::Fill;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::OutputId;

pub(crate) use self::editor::{Action, CursorMove, TextInputBatch};
use self::editor::{Editor, EditorEffect};
use self::scene::ElementKind;
pub(super) use self::scene::Point;
pub(crate) use self::selection::CursorHint;
use self::text_edit::TextEdit;
pub(crate) use self::text_edit::{Preedit, PreeditHint, PreeditSpan, TextInputSnapshot};
pub(crate) use self::tool::Tool;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ToolOverride {
    #[default]
    None,
    Eraser,
    InvertEraser,
}

impl ToolOverride {
    pub(crate) fn from_eraser(enabled: bool) -> Self {
        if enabled { Self::Eraser } else { Self::None }
    }

    fn effective_tool(self, active: Tool) -> Tool {
        match self {
            Self::None => active,
            Self::Eraser => Tool::Eraser,
            Self::InvertEraser if active == Tool::Eraser => Tool::Pen,
            Self::InvertEraser => Tool::Eraser,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ToolCursor {
    pub tool: Tool,
    pub size: f32,
    pub roundness: f32,
    pub color: [f32; 4],
}

const CIRCLE_KAPPA: f64 = 0.552_284_749_830_793_6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Cursor {
    Hidden,
    Shape(CursorHint),
    Tool(ToolCursor),
}

impl Cursor {
    pub(crate) fn same_compositor_cursor(self, other: Self) -> bool {
        self == other
            || matches!(self, Self::Hidden | Self::Tool(_))
                && matches!(other, Self::Hidden | Self::Tool(_))
    }
}

const CARET_BLINK_INTERVAL: Duration = Duration::from_millis(530);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

pub(super) struct PenMotion {
    pub end: Point,
    pub bend: Option<Point>,
}

#[derive(Default)]
struct Adjustment {
    changed: bool,
    feedback: Option<String>,
    hit_stop: bool,
}

struct Feedback {
    text: String,
    anchor: Point,
    until: Instant,
}

pub struct DrawState {
    editor: Editor,
    changed: BTreeMap<OutputId, bool>,
    feedback: Option<Feedback>,
    feedback_duration: Duration,
    caret_visible: bool,
    caret_until: Option<Instant>,
    tool_cursor: Option<(Point, ToolCursor)>,
    previews: Vec<Geometry>,
}

impl DrawState {
    pub(super) fn new(settings: crate::Settings) -> Self {
        let feedback_duration = settings.feedback_duration;
        let editor = Editor::new(settings);
        Self {
            editor,
            changed: BTreeMap::new(),
            feedback: None,
            feedback_duration,
            caret_visible: true,
            caret_until: None,
            tool_cursor: None,
            previews: Vec::new(),
        }
    }

    pub fn activate(&mut self) -> bool {
        let changed = self.editor.activate();
        self.record(changed)
    }

    pub fn deactivate(&mut self) -> bool {
        let mut changed = self.editor.deactivate();
        if self.feedback.take().is_some() | self.tool_cursor.take().is_some() {
            changed = true;
        }
        self.caret_until = None;
        self.record(changed)
    }

    pub fn is_editing_text(&self) -> bool {
        self.editor.is_editing_text()
    }

    pub fn is_drawing_pen(&self) -> bool {
        self.editor.is_drawing_pen()
    }

    pub fn handle_action(&mut self, action: Action, at: Option<Point>) -> EditorEffect {
        let user_input = !matches!(action, Action::ApplyTextInput(_));
        let mut effect = self.editor.handle_action(action);
        if let (Some(label), Some(at)) = (effect.feedback.take(), at) {
            self.feedback = Some(Feedback {
                text: label,
                anchor: at,
                until: Instant::now() + self.feedback_duration,
            });
            effect.changed = true;
        }
        if (user_input || effect.changed) && self.show_caret() {
            effect.changed = true;
        }
        self.record(effect.changed);
        effect
    }

    pub fn set_current_color(&mut self, rgba: [f32; 4]) -> bool {
        let mut changed = self.editor.apply_rgba(rgba);
        if let Some((_, cursor)) = &mut self.tool_cursor {
            changed |= cursor.color != rgba;
            cursor.color = rgba;
        }
        self.record(changed)
    }

    pub fn pointer_down(
        &mut self,
        point: Point,
        modifiers: Modifiers,
        tool_override: ToolOverride,
    ) -> bool {
        let mut changed = self.editor.pointer_down(point, modifiers, tool_override);
        if self.show_caret() {
            changed = true;
        }
        self.record(changed)
    }

    pub fn pointer_motion(&mut self, point: Point, modifiers: Modifiers) -> bool {
        let changed = self.editor.pointer_motion(point, modifiers);
        if changed {
            self.show_caret();
        }
        self.record(changed)
    }

    pub fn pen_motion(&mut self, motion: PenMotion, modifiers: Modifiers) -> bool {
        let changed = self.editor.pen_motion(motion, modifiers);
        self.record(changed)
    }

    pub fn modifiers_changed(&mut self, modifiers: Modifiers) -> bool {
        let changed = self.editor.modifiers_changed(modifiers);
        self.record(changed)
    }

    pub fn pointer_up(&mut self, point: Point, modifiers: Modifiers) -> bool {
        let changed = self.editor.pointer_up(point, modifiers);
        if changed {
            self.show_caret();
        }
        self.record(changed)
    }

    pub fn picker_active(&self) -> bool {
        self.editor.picker_active()
    }

    pub fn cursor(&self, point: Point, tool_override: ToolOverride) -> Cursor {
        self.editor.cursor(point, tool_override)
    }

    pub fn set_tool_cursor(&mut self, cursor: Option<(Point, ToolCursor)>) -> bool {
        if self.tool_cursor == cursor {
            return false;
        }
        self.tool_cursor = cursor;
        self.record(true);
        true
    }

    pub fn open_picker(&mut self, center: Point) {
        self.editor.open_picker(center);
        self.record(true);
    }

    pub fn picker_motion(&mut self, point: Point) -> bool {
        let changed = self.editor.picker_motion(point);
        self.record(changed)
    }

    pub fn picker_release(&mut self, point: Point, latch_center: bool) -> bool {
        let changed = self.editor.picker_release(point, latch_center);
        self.record(changed)
    }

    pub fn dismiss_picker(&mut self) -> bool {
        let changed = self.editor.dismiss_picker();
        self.record(changed)
    }

    pub fn text_click_at(&mut self, point: Point, clicks: u8) -> Option<bool> {
        let mut changed = self.editor.text_click_at(point, clicks)?;
        changed |= self.show_caret();
        Some(self.record(changed))
    }

    pub fn adjust(&mut self, steps: f32, at: Point, modifiers: Modifiers) -> bool {
        let adjustment = if modifiers.shift {
            self.editor.adjust_roundness(steps)
        } else if modifiers.ctrl {
            self.editor.adjust_opacity(steps)
        } else {
            self.editor.adjust_size(steps)
        };
        self.record(adjustment.changed || adjustment.feedback.is_some());
        if let Some(text) = adjustment.feedback {
            let anchor = self
                .feedback
                .as_ref()
                .map_or(at, |feedback| feedback.anchor);
            self.feedback = Some(Feedback {
                text,
                anchor,
                until: Instant::now() + self.feedback_duration,
            });
        }
        adjustment.hit_stop
    }

    pub fn add_output(&mut self, output: OutputId) {
        self.changed.insert(output, true);
    }

    pub fn remove_output(&mut self, output: OutputId) {
        self.changed.remove(&output);
    }

    pub(crate) fn text_input_snapshot(&self) -> Option<TextInputSnapshot<'_>> {
        self.editor.text_edit().map(|edit| edit.snapshot(None))
    }

    pub(crate) fn clear_preedit(&mut self) -> bool {
        if self.editor.clear_preedit() {
            self.show_caret();
            self.record(true)
        } else {
            false
        }
    }

    pub fn needs_render(&self, output: OutputId) -> bool {
        self.changed.get(&output).is_some_and(|changed| *changed)
    }

    pub fn damaged_outputs(&self) -> impl Iterator<Item = OutputId> + '_ {
        self.changed
            .iter()
            .filter(|(_, changed)| **changed)
            .map(|(&output, _)| output)
    }

    pub fn damage(&mut self, output: OutputId) {
        self.changed.insert(output, true);
    }

    fn record(&mut self, changed: bool) -> bool {
        if changed {
            for current in self.changed.values_mut() {
                *current = true;
            }
        }
        changed
    }

    pub fn next_wakeup(&self) -> Option<Instant> {
        [
            self.feedback.as_ref().map(|feedback| feedback.until),
            self.caret_until,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn handle_timeouts(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self
            .feedback
            .as_ref()
            .is_some_and(|feedback| now >= feedback.until)
        {
            self.feedback = None;
            changed = true;
        }
        if self.caret_until.is_some_and(|until| now >= until) {
            if self.editor.is_editing_text() {
                self.caret_visible = !self.caret_visible;
                self.caret_until = Some(now + CARET_BLINK_INTERVAL);
                changed = true;
            } else {
                self.caret_until = None;
            }
        }
        if changed {
            self.record(true);
        }
        changed
    }

    pub fn render(
        &mut self,
        output: OutputId,
        origin: Point,
        scale: [f64; 2],
        wgpu: &mut WgpuState,
        before_present: impl FnOnce(Option<TextInputSnapshot<'_>>),
    ) -> Result<(), String> {
        if !self.needs_render(output) {
            return Ok(());
        }
        let active_text = self.editor.text_edit();
        let items = {
            let mut items = Vec::with_capacity(self.editor.elements().len());
            for element in self.editor.elements() {
                if let Some(edit) = active_text.filter(|edit| edit.id == Some(element.id)) {
                    items.push(SceneItem::Text(edit.spec()));
                    continue;
                }
                let (kind, style) = self
                    .editor
                    .text_resize_preview(element.id)
                    .unwrap_or((&element.kind, element.style));
                let ElementKind::Text {
                    origin,
                    content,
                    scale,
                } = kind
                else {
                    items.push(SceneItem::Geometry(
                        self.editor
                            .element_geometry_preview(element)
                            .map(std::borrow::Cow::Owned)
                            .unwrap_or(std::borrow::Cow::Borrowed(&element.geometry)),
                    ));
                    continue;
                };
                let offset = self.editor.moving_offset(element.id).unwrap_or_default();
                items.push(SceneItem::Text(TextSpec {
                    key: element.id,
                    content,
                    left: origin.x + offset.x,
                    top: origin.y + offset.y,
                    font_size: style.size,
                    color: style.color,
                    background_roundness: style.filled.then_some(style.roundness),
                    scale: *scale,
                }));
            }
            if let Some(edit) = active_text.filter(|edit| edit.id.is_none()) {
                items.push(SceneItem::Text(edit.spec()));
            }
            if let Some(Feedback {
                text: content,
                anchor: at,
                ..
            }) = &self.feedback
            {
                for [x, y] in [[15.0, 16.0], [17.0, 16.0], [16.0, 15.0], [16.0, 17.0]].into_iter() {
                    items.push(SceneItem::Text(TextSpec {
                        key: u64::MAX - 30,
                        content,
                        left: at.x + x,
                        top: at.y + y,
                        font_size: 18.0,
                        color: [0.0, 0.0, 0.0, 0.9],
                        background_roundness: None,
                        scale: [1.0; 2],
                    }));
                }
                items.push(SceneItem::Text(TextSpec {
                    key: u64::MAX - 30,
                    content,
                    left: at.x + 16.0,
                    top: at.y + 16.0,
                    font_size: 18.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    background_roundness: None,
                    scale: [1.0; 2],
                }));
            }
            items
        };

        self.previews.clear();
        let mut cursor_rectangle = None;
        if let Some(edit) = self.editor.text_edit() {
            let [scale_x, scale_y] = edit.scale;
            let [x, y] = edit.cursor_position();
            cursor_rectangle = Some(text_cursor_rectangle(
                edit.origin,
                edit.ime_area(),
                edit.scale,
                origin,
            ));
            if self.caret_visible && edit.shows_caret() {
                self.previews.push(text_caret(
                    edit.origin.x + x * scale_x,
                    edit.origin.y + y * scale_y,
                    edit.style.size * scale_y,
                ));
            }
            edit.decoration_rectangles(|[x, y, width, height], style| {
                self.previews.push(text_preedit_span(
                    edit.origin.x + x * scale_x,
                    edit.origin.x + (x + width) * scale_x,
                    edit.origin.y + y * scale_y,
                    height * scale_y,
                    style,
                ));
            });
        }
        self.editor.append_preview_geometry(&mut self.previews);
        self.editor
            .append_selection_geometry(self.feedback.is_none(), &mut self.previews);
        if let Some((point, cursor)) = self.tool_cursor {
            self.previews.push(tool_cursor_geometry(point, cursor));
        }
        let picker = self.editor.picker_geometry();
        if wgpu.render(
            &items,
            &self.previews,
            picker.as_ref(),
            Viewport {
                origin: [origin.x, origin.y],
                scale,
            },
            self.editor
                .text_edit()
                .map(|edit| (edit.id.unwrap_or(0), edit.layout())),
            || {
                before_present(
                    self.editor
                        .text_edit()
                        .map(|edit| edit.snapshot(cursor_rectangle)),
                );
            },
        )? {
            self.changed.insert(output, false);
        }
        Ok(())
    }

    fn show_caret(&mut self) -> bool {
        if !self.editor.text_edit().is_some_and(TextEdit::shows_caret) {
            self.caret_until = None;
            return false;
        }
        let changed = !self.caret_visible;
        self.caret_visible = true;
        self.caret_until = Some(Instant::now() + CARET_BLINK_INTERVAL);
        changed
    }
}

fn tool_cursor_geometry(point: Point, cursor: ToolCursor) -> Geometry {
    use kurbo::Shape;

    let radius = f64::from(match cursor.tool {
        Tool::Pen | Tool::Eraser => cursor.size * 0.5,
        _ => unreachable!("only pen and eraser have tool cursors"),
    });
    let point = if cursor.tool == Tool::Pen {
        scene::pixel_aligned_point(point, cursor.size)
    } else {
        point
    };
    let center = kurbo::Point::new(f64::from(point.x), f64::from(point.y));
    if cursor.tool == Tool::Eraser {
        const OUTLINE_WIDTH: f64 = 0.75;
        let mut geometry = Geometry::fill(
            kurbo::Circle::new(center, radius + OUTLINE_WIDTH).to_path(0.1),
            Fill::NonZero,
            [0.0, 0.0, 0.0, 1.0],
        );
        geometry.push_fill(
            kurbo::Circle::new(center, radius).to_path(0.1),
            Fill::NonZero,
            [1.0, 1.0, 1.0, 1.0],
        );
        return geometry;
    }

    let mut color = cursor.color;
    color[3] = color[3].sqrt();
    let corner_radius = radius * f64::from(cursor.roundness.clamp(0.0, 1.0));
    Geometry::fill(
        kurbo::RoundedRect::new(
            center.x - radius,
            center.y - radius,
            center.x + radius,
            center.y + radius,
            corner_radius,
        )
        .to_path(0.1),
        Fill::NonZero,
        color,
    )
}

fn text_caret(left: f32, top: f32, scaled_font_size: f32) -> Geometry {
    use kurbo::Shape;

    let line_height = text_line_height(scaled_font_size);
    let caret_height = (scaled_font_size.abs() - 2.0)
        .max(1.0)
        .copysign(scaled_font_size);
    let inset = (line_height - caret_height) * 0.5;
    let top = top + inset;
    let bottom = top + caret_height;
    let (top, bottom) = (top.min(bottom), top.max(bottom));
    let black = [0.0, 0.0, 0.0, 1.0];
    let white = [1.0, 1.0, 1.0, 1.0];
    let mut geometry = Geometry::fill(
        kurbo::Rect::new(
            f64::from(left - 1.0),
            f64::from(top),
            f64::from(left + 1.0),
            f64::from(bottom),
        )
        .to_path(0.1),
        Fill::NonZero,
        black,
    );
    geometry.push_fill(
        kurbo::Rect::new(
            f64::from(left - 0.5),
            f64::from(top),
            f64::from(left + 0.5),
            f64::from(bottom),
        )
        .to_path(0.1),
        Fill::NonZero,
        white,
    );
    geometry
}

fn text_cursor_rectangle(
    text_origin: Point,
    area: parley::BoundingBox,
    [scale_x, scale_y]: [f32; 2],
    output_origin: Point,
) -> [i32; 4] {
    // Text-input rectangles use logical surface coordinates, before buffer scaling.
    let x0 = text_origin.x + area.x0 as f32 * scale_x - output_origin.x;
    let y0 = text_origin.y + area.y0 as f32 * scale_y - output_origin.y;
    let x1 = text_origin.x + area.x1 as f32 * scale_x - output_origin.x;
    let y1 = text_origin.y + area.y1 as f32 * scale_y - output_origin.y;
    let left = x0.min(x1).floor();
    let top = y0.min(y1).floor();
    [
        left as i32,
        top as i32,
        (x0.max(x1).ceil() - left).max(1.0) as i32,
        (y0.max(y1).ceil() - top).max(1.0) as i32,
    ]
}

fn text_preedit_span(
    start: f32,
    end: f32,
    top: f32,
    line_height: f32,
    style: PreeditHint,
) -> Geometry {
    use kurbo::Shape;

    let left = start.min(end);
    let right = start.max(end).max(left + 1.0);
    let bottom = top + line_height;
    if style == PreeditHint::Selection {
        return Geometry::fill(
            kurbo::Rect::new(
                f64::from(left),
                f64::from(top.min(bottom)),
                f64::from(right),
                f64::from(top.max(bottom)),
            )
            .to_path(0.1),
            Fill::NonZero,
            [0.2, 0.45, 1.0, 0.25],
        );
    }

    let color = match style {
        PreeditHint::SpellingError => [1.0, 0.15, 0.1, 1.0],
        PreeditHint::ComposeError => [1.0, 0.45, 0.05, 1.0],
        PreeditHint::Prediction => [0.55, 0.55, 0.55, 0.8],
        _ => [0.2, 0.45, 1.0, 1.0],
    };
    let baseline = bottom - 1.5;
    Geometry::fill(
        kurbo::Rect::new(
            f64::from(left),
            f64::from(baseline),
            f64::from(right),
            f64::from(baseline + 1.5),
        )
        .to_path(0.1),
        Fill::NonZero,
        color,
    )
}
