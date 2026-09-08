use super::super::scene::{Element, ElementId, ElementKind, Point, Style};
use super::super::text_edit::TextEdit;
use super::{Editor, HistoryEntry, Interaction};

impl Editor {
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    /// Returns None for a miss, or Some(changed) when the text edit handles the click.
    pub fn text_click_at(&mut self, point: Point, clicks: u8) -> Option<bool> {
        if let Some(edit) = self.text_edit_mut() {
            return edit
                .bounds()
                .contains(point)
                .then(|| edit.click(point, clicks, false));
        }
        if self.tool != super::super::tool::Tool::Select {
            return None;
        }
        let [id] = self.selected.as_slice() else {
            return None;
        };
        let id = *id;
        let element = self.element(id)?;
        if matches!(element.kind, ElementKind::Text { .. }) && element.hit_test(point) {
            let changed = self.begin_text_edit(id);
            if let Some(edit) = self.text_edit_mut() {
                edit.click(point, clicks, false);
            }
            return Some(changed);
        }
        None
    }

    pub fn hit_test(&self, point: Point) -> Option<ElementId> {
        self.elements
            .iter()
            .rev()
            .find(|element| element.hit_test(point))
            .map(|element| element.id)
    }

    pub fn undo(&mut self) -> bool {
        let cancelled = self.cancel_interaction();
        if !self.history.undo(&mut self.elements) {
            return cancelled;
        }
        self.selected.clear();
        true
    }

    pub fn redo(&mut self) -> bool {
        let cancelled = self.cancel_interaction();
        if !self.history.redo(&mut self.elements) {
            return cancelled;
        }
        self.selected.clear();
        true
    }

    pub(super) fn select_all(&mut self) -> bool {
        let cancelled = self.cancel_interaction();
        if self.elements.is_empty() {
            return cancelled;
        }
        let changed = cancelled | self.switch_tool(super::super::tool::Tool::Select);
        let selected = self.elements.iter().map(|element| element.id).collect();
        if self.selected == selected {
            return changed;
        }
        self.selected = selected;
        true
    }

    pub(super) fn clear(&mut self) -> bool {
        let cancelled = self.cancel_interaction();
        if self.elements.is_empty() {
            return cancelled;
        }
        let elements = std::mem::take(&mut self.elements);
        self.history.record(HistoryEntry::Clear(elements));
        self.selected.clear();
        true
    }

    pub(super) fn delete_selection(&mut self) -> bool {
        let selected = std::mem::take(&mut self.selected);
        if selected.is_empty() {
            return false;
        }
        let cancelled = self.cancel_interaction();
        if selected.len() == self.elements.len() {
            let elements = std::mem::take(&mut self.elements);
            self.history.record(HistoryEntry::Clear(elements));
            return true;
        }
        cancelled | self.remove_ids(&selected)
    }

    fn remove_ids(&mut self, ids: &[ElementId]) -> bool {
        let mut removed = Vec::with_capacity(ids.len());
        for index in (0..self.elements.len()).rev() {
            if ids.contains(&self.elements[index].id) {
                removed.push((index, self.elements.remove(index)));
            }
        }
        if removed.is_empty() {
            return false;
        }
        removed.reverse();
        self.history.record(HistoryEntry::Remove(removed));
        self.selected.retain(|selected| !ids.contains(selected));
        true
    }

    pub(super) fn insert_kind(&mut self, kind: ElementKind, style: Style) {
        self.insert_element(Element::new(self.next_id, kind, style));
    }

    pub(super) fn insert_element(&mut self, element: Element) {
        self.next_id += 1;
        let index = self.elements.len();
        let id = element.id;
        self.elements.push(element);
        self.history.record(HistoryEntry::Insert(vec![(index, id)]));
    }

    fn remove_id(&mut self, id: ElementId) -> bool {
        self.remove_ids(&[id])
    }

    pub(super) fn erase_between(&mut self, start: Point, end: Point) -> bool {
        let radius = self
            .properties(super::super::tool::Tool::Eraser)
            .expect("eraser has adjustable properties")
            .size
            * 0.5;
        let hits = self
            .elements
            .iter()
            .filter(|element| element.erase_hit_test(start, end, radius))
            .map(|element| element.id)
            .collect::<Vec<_>>();
        self.remove_ids(&hits)
    }

    pub(super) fn commit_text(&mut self) -> bool {
        if !self.is_editing_text() {
            return false;
        }
        let Some(Interaction::EditingText(edit)) = self.interaction.take() else {
            unreachable!("text editing was checked before taking the interaction");
        };
        let content: String = edit.content().into_iter().collect();
        let TextEdit {
            id,
            origin,
            style,
            scale,
            ..
        } = edit;
        if content.is_empty() {
            return id.is_none_or(|id| self.remove_id(id));
        }
        let kind = ElementKind::Text {
            origin,
            content,
            scale,
        };
        if let Some(id) = id {
            let element = self.element_mut(id).expect("editing text exists");
            if element.kind == kind && element.style == style {
                return true;
            }
            let (kind, style) = element.replace(kind, style);
            self.history
                .record(HistoryEntry::Update(vec![(id, kind, style)]));
        } else {
            self.insert_kind(kind, style);
        }
        true
    }

    fn begin_text_edit(&mut self, id: ElementId) -> bool {
        let Some(element) = self.element(id) else {
            return false;
        };
        let ElementKind::Text {
            origin,
            content,
            scale,
        } = &element.kind
        else {
            return false;
        };
        let edit = self.make_text_edit(Some(id), *origin, content.clone(), element.style, *scale);
        self.interaction = Some(Interaction::EditingText(edit));
        true
    }

    pub(super) fn element(&self, id: ElementId) -> Option<&Element> {
        self.elements.iter().find(|element| element.id == id)
    }

    pub(super) fn element_mut(&mut self, id: ElementId) -> Option<&mut Element> {
        self.elements.iter_mut().find(|element| element.id == id)
    }
}
