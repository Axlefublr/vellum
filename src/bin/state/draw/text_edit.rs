use std::num::NonZeroUsize;

use parley::{Layout, PlainEditor, PlainEditorDriver, SplitString, StyleProperty};
use unicode_segmentation::UnicodeSegmentation;

use super::scene::{Bounds, ElementId, Point, Style, text_bounds};
use crate::render::{TextSpec, text_styles, with_text_context};

pub(crate) enum CursorMove {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    Home,
    End,
    TextStart,
    TextEnd,
}

#[derive(Debug)]
pub(super) struct TextEdit {
    pub(super) id: Option<ElementId>,
    pub(super) origin: Point,
    editor: Box<PlainEditor<()>>,
    drag_point: Option<Point>,
    pub(super) style: Style,
    pub(super) scale: [f32; 2],
}

impl TextEdit {
    pub(super) fn new(
        id: Option<ElementId>,
        origin: Point,
        content: String,
        style: Style,
        scale: [f32; 2],
    ) -> Self {
        let mut editor = PlainEditor::new(style.size);
        editor.set_quantize(false);
        for property in text_styles() {
            editor.edit_styles().insert(property);
        }
        editor.set_text(&content);
        with_text_context(|fonts, layouts| editor.driver(fonts, layouts).move_to_text_end());
        Self {
            id,
            origin,
            editor: Box::new(editor),
            drag_point: None,
            style,
            scale,
        }
    }

    pub(super) fn content(&self) -> SplitString<'_> {
        self.editor.text()
    }

    pub(super) fn layout(&self) -> &Layout<()> {
        self.editor.try_layout().expect("text edits refresh layout")
    }

    pub(super) fn spec(&self) -> TextSpec<'_> {
        TextSpec {
            key: self.id.unwrap_or(0),
            content: self.editor.raw_text(),
            left: self.origin.x,
            top: self.origin.y,
            font_size: self.style.size,
            color: self.style.color,
            background_roundness: self.style.filled.then_some(self.style.roundness),
            scale: self.scale,
        }
    }

    pub(super) fn bounds(&self) -> Bounds {
        let layout = self.layout();
        text_bounds(
            self.origin,
            [layout.full_width(), layout.height()],
            self.style,
            self.scale,
        )
    }

    pub(super) fn click(&mut self, point: Point, clicks: u8, extend: bool) -> bool {
        self.drag_point = Some(point);
        let x = (point.x - self.origin.x) / self.scale[0];
        let y = (point.y - self.origin.y) / self.scale[1];
        self.external_edit(|driver| match clicks {
            2 => driver.select_word_at_point(x, y),
            3 => driver.select_line_at_point(x, y),
            _ if extend => driver.shift_click_extension(x, y),
            _ => driver.move_to_point(x, y),
        })
    }

    pub(super) fn drag(&mut self, point: Point) -> bool {
        let Some(previous) = &mut self.drag_point else {
            return false;
        };
        if *previous == point {
            return false;
        }
        *previous = point;
        let x = (point.x - self.origin.x) / self.scale[0];
        let y = (point.y - self.origin.y) / self.scale[1];
        self.external_edit(|driver| driver.extend_selection_to_point(x, y))
    }

    pub(super) fn end_drag(&mut self, point: Point) -> bool {
        let changed = self.drag(point);
        self.drag_point = None;
        changed
    }

    pub(super) fn set_size(&mut self, size: f32) {
        self.style.size = size;
        self.editor
            .edit_styles()
            .insert(StyleProperty::FontSize(size));
        with_text_context(|fonts, layouts| self.editor.refresh_layout(fonts, layouts));
    }

    fn external_edit(&mut self, edit: impl FnOnce(&mut PlainEditorDriver<'_, ()>)) -> bool {
        let generation = self.editor.generation();
        with_text_context(|fonts, layouts| {
            let mut driver = self.editor.driver(fonts, layouts);
            edit(&mut driver);
        });
        self.editor.generation() != generation
    }

    pub(super) fn insert(&mut self, text: &str) -> bool {
        self.external_edit(|driver| driver.insert_or_replace_selection(text))
    }

    pub(super) fn backspace(&mut self) -> bool {
        self.delete_grapheme(true)
    }

    pub(super) fn backspace_word(&mut self) -> bool {
        self.external_edit(|driver| driver.backdelete_word())
    }

    pub(super) fn delete(&mut self) -> bool {
        self.delete_grapheme(false)
    }

    fn delete_grapheme(&mut self, backwards: bool) -> bool {
        self.external_edit(|driver| {
            if !driver.editor.raw_selection().is_collapsed() {
                driver.delete_selection();
                return;
            }
            let cursor = driver.editor.raw_selection().focus().index();
            let text = driver.editor.raw_text();
            // Preserve whole-character deletion rather than shaped cluster/scalar deletion.
            let len = if backwards {
                text[..cursor]
                    .graphemes(true)
                    .next_back()
                    .map_or(0, str::len)
            } else {
                text[cursor..].graphemes(true).next().map_or(0, str::len)
            };
            if let Some(len) = NonZeroUsize::new(len) {
                if backwards {
                    driver.delete_bytes_before_selection(len);
                } else {
                    driver.delete_bytes_after_selection(len);
                }
            }
        })
    }

    pub(super) fn move_cursor(&mut self, movement: CursorMove, extend: bool) -> bool {
        self.external_edit(|driver| match (movement, extend) {
            (CursorMove::Left, false) => driver.move_left(),
            (CursorMove::Left, true) => driver.select_left(),
            (CursorMove::Right, false) => driver.move_right(),
            (CursorMove::Right, true) => driver.select_right(),
            (CursorMove::Up, false) => driver.move_up(),
            (CursorMove::Up, true) => driver.select_up(),
            (CursorMove::Down, false) => driver.move_down(),
            (CursorMove::Down, true) => driver.select_down(),
            (CursorMove::WordLeft, false) => driver.move_word_left(),
            (CursorMove::WordLeft, true) => driver.select_word_left(),
            (CursorMove::WordRight, false) => driver.move_word_right(),
            (CursorMove::WordRight, true) => driver.select_word_right(),
            (CursorMove::Home, false) => driver.move_to_line_start(),
            (CursorMove::Home, true) => driver.select_to_line_start(),
            (CursorMove::End, false) => driver.move_to_line_end(),
            (CursorMove::End, true) => driver.select_to_line_end(),
            (CursorMove::TextStart, false) => driver.move_to_text_start(),
            (CursorMove::TextStart, true) => driver.select_to_text_start(),
            (CursorMove::TextEnd, false) => driver.move_to_text_end(),
            (CursorMove::TextEnd, true) => driver.select_to_text_end(),
        })
    }

    pub(super) fn select_all(&mut self) -> bool {
        self.external_edit(|driver| driver.select_all())
    }

    pub(super) fn shows_caret(&self) -> bool {
        self.editor.raw_selection().is_collapsed() && self.editor.cursor_geometry(1.0).is_some()
    }

    pub(super) fn cursor_position(&self) -> [f32; 2] {
        let rect = self
            .editor
            .cursor_geometry(1.0)
            .unwrap_or_else(|| self.editor.ime_cursor_area());
        [rect.x0 as f32, rect.y0 as f32]
    }

    pub(super) fn selection_rectangles(&self, mut draw: impl FnMut([f32; 4])) {
        self.editor
            .raw_selection()
            .geometry_with(self.layout(), |rect, _| {
                draw([
                    rect.x0 as f32,
                    rect.y0 as f32,
                    rect.width() as f32,
                    rect.height() as f32,
                ]);
            });
    }
}
