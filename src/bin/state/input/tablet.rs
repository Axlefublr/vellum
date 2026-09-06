use std::collections::{HashMap, HashSet};

use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::Proxy;
use wayland_client::QueueHandle;
use wayland_client::WEnum;
use wayland_client::backend::ObjectId;

use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};

use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_v2::ZwpTabletPadV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_seat_v2::ZwpTabletSeatV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_tool_v2::ZwpTabletToolV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_v2::ZwpTabletV2;

use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_group_v2::{
    EVT_RING_OPCODE, EVT_STRIP_OPCODE,
};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_v2::EVT_GROUP_OPCODE;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE;

use super::super::draw::{Cursor, Point, ToolOverride};
use super::super::{OutputId, State};
use super::pointer::cursor_shape;
use super::short_click;

const EVDEV_STYLUS: u32 = 331;
const EVDEV_STYLUS2: u32 = 332;
const PEN: u8 = 1;
const BUTTON: u8 = 2;

#[derive(Default)]
struct PadGroup {
    rings: Vec<ZwpTabletPadRingV2>,
    strips: Vec<ZwpTabletPadStripV2>,
}

#[derive(Default)]
pub(in crate::state) struct TabletState {
    event_sequence: EventSequence,

    _tablet_seat: Option<ZwpTabletSeatV2>,
    pad_groups: HashMap<ObjectId, Vec<ZwpTabletPadGroupV2>>,
    group_controls: HashMap<ObjectId, PadGroup>,
    tablet_cursor_shape_devices: HashMap<ObjectId, WpCursorShapeDeviceV1>,
    cursor_serials: HashMap<ObjectId, u32>,
    current_cursors: HashMap<ObjectId, Cursor>,
    eraser_tools: HashSet<ObjectId>,
    output: Option<OutputId>,
    pos: Option<(f64, f64)>,
    pen_held: bool,
    button_held: bool,
    button_press_time: Option<u32>,
}

impl TabletState {
    pub(in crate::state) fn set_tablet_seat(&mut self, tablet_seat: ZwpTabletSeatV2) {
        self._tablet_seat = Some(tablet_seat);
    }

    fn update_state(&mut self, sequence: EventSequence) {
        if let Some(new_pos) = sequence.motion {
            self.pos = Some(new_pos);
        }

        update_held(&mut self.pen_held, sequence, PEN);
        update_held(&mut self.button_held, sequence, BUTTON);
    }

    fn refresh_cursor(&mut self, tablet_tool: &ZwpTabletToolV2, cursor: Cursor) {
        let id = tablet_tool.id();
        let Some(&serial) = self.cursor_serials.get(&id) else {
            return;
        };
        if self
            .current_cursors
            .get(&id)
            .is_some_and(|current| current.same_compositor_cursor(cursor))
        {
            return;
        }
        let Some(device) = self.tablet_cursor_shape_devices.get(&id) else {
            return;
        };
        match cursor {
            Cursor::Hidden => tablet_tool.set_cursor(serial, None, 0, 0),
            Cursor::Shape(hint) => device.set_shape(serial, cursor_shape(hint)),
            Cursor::Tool(_) => tablet_tool.set_cursor(serial, None, 0, 0),
        }
        self.current_cursors.insert(id, cursor);
    }

    pub(in crate::state) fn cursor_active(&self) -> bool {
        !self.cursor_serials.is_empty()
    }

    pub(in crate::state) fn input_grab_active(&self) -> bool {
        self.pen_held || self.button_held
    }

    pub(in crate::state) fn cancel_gesture(&mut self) {
        self.pen_held = false;
        self.button_held = false;
        self.button_press_time = None;
    }

    fn tool_cursor_supported(&self, tablet_tool: &ZwpTabletToolV2) -> bool {
        self.tablet_cursor_shape_devices
            .contains_key(&tablet_tool.id())
    }

    pub(in crate::state) fn restore_cursors(&mut self) {
        for (id, serial) in &self.cursor_serials {
            if let Some(device) = self.tablet_cursor_shape_devices.get(id) {
                device.set_shape(*serial, Shape::Default);
            }
        }
        self.current_cursors.clear();
    }
}

impl Dispatch<ZwpTabletSeatV2, (), State> for TabletState {
    fn event(
        state: &mut State,
        _tablet_seat: &ZwpTabletSeatV2,
        event: <ZwpTabletSeatV2 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<State>,
    ) {
        use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_seat_v2::Event;
        if let Event::ToolAdded { id } = event {
            let object_id = id.id();
            if let Some(manager) = &state.wayland.cursor_shape_manager {
                let device = manager.get_tablet_tool_v2(&id, qhandle, ());
                state
                    .tablet
                    .tablet_cursor_shape_devices
                    .insert(object_id.clone(), device);
            }
        }
    }

    wayland_client::event_created_child!(State, ZwpTabletSeatV2, [
        EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
        EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
        EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, ()),
    ]);
}

impl Dispatch<ZwpTabletV2, (), State> for TabletState {
    fn event(
        _state: &mut State,
        tablet: &ZwpTabletV2,
        event: <ZwpTabletV2 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        if let wayland_protocols::wp::tablet::zv2::client::zwp_tablet_v2::Event::Removed = event {
            tablet.destroy();
        }
    }
}

impl Dispatch<ZwpTabletPadV2, (), State> for TabletState {
    fn event(
        state: &mut State,
        pad: &ZwpTabletPadV2,
        event: <ZwpTabletPadV2 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_v2::Event;
        match event {
            Event::Group { pad_group } => {
                state
                    .tablet
                    .pad_groups
                    .entry(pad.id())
                    .or_default()
                    .push(pad_group);
            }
            Event::Removed => {
                for group in state
                    .tablet
                    .pad_groups
                    .remove(&pad.id())
                    .unwrap_or_default()
                {
                    if let Some(controls) = state.tablet.group_controls.remove(&group.id()) {
                        for ring in controls.rings {
                            ring.destroy();
                        }
                        for strip in controls.strips {
                            strip.destroy();
                        }
                    }
                    group.destroy();
                }
                pad.destroy();
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, ZwpTabletPadV2, [
        EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, ()),
    ]);
}

impl Dispatch<ZwpTabletPadGroupV2, (), State> for TabletState {
    fn event(
        state: &mut State,
        group: &ZwpTabletPadGroupV2,
        event: <ZwpTabletPadGroupV2 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_group_v2::Event;
        match event {
            Event::Ring { ring } => {
                state
                    .tablet
                    .group_controls
                    .entry(group.id())
                    .or_default()
                    .rings
                    .push(ring);
            }
            Event::Strip { strip } => {
                state
                    .tablet
                    .group_controls
                    .entry(group.id())
                    .or_default()
                    .strips
                    .push(strip);
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, ZwpTabletPadGroupV2, [
        EVT_RING_OPCODE => (ZwpTabletPadRingV2, ()),
        EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, ()),
    ]);
}

impl Dispatch<ZwpTabletToolV2, (), State> for TabletState {
    fn event(
        state: &mut State,
        tablet_tool: &ZwpTabletToolV2,
        event: <ZwpTabletToolV2 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_tool_v2::{Event, Type};
        if let Event::ProximityIn { surface, .. } = &event
            && let Some(output) = state.output_for_surface(surface)
        {
            state.focus_output(output);
            state.tablet.output = Some(output);
        }
        match &event {
            Event::Removed => {
                if let Some(device) = state
                    .tablet
                    .tablet_cursor_shape_devices
                    .remove(&tablet_tool.id())
                {
                    device.destroy();
                }
                state.tablet.cursor_serials.remove(&tablet_tool.id());
                state.tablet.current_cursors.remove(&tablet_tool.id());
                state.tablet.eraser_tools.remove(&tablet_tool.id());
                tablet_tool.destroy();
                state.refresh_pointer_cursor();
                return;
            }
            Event::Type {
                tool_type: WEnum::Value(Type::Eraser),
            } => {
                state.tablet.eraser_tools.insert(tablet_tool.id());
            }
            Event::Type { .. } => {
                state.tablet.eraser_tools.remove(&tablet_tool.id());
            }
            _ => {}
        }
        let origin = state
            .tablet
            .output
            .map(|output| state.output_origin(output))
            .unwrap_or_default();
        if let Some(sequence) = state
            .tablet
            .event_sequence
            .dispatch(event, (f64::from(origin.x), f64::from(origin.y)))
        {
            state.tablet.update_state(sequence);
            if let Some(serial) = sequence.enter_serial {
                state.tablet.cursor_serials.insert(tablet_tool.id(), serial);
                state.tablet.current_cursors.remove(&tablet_tool.id());
            }
            if sequence.proximity_out {
                state.tablet.cursor_serials.remove(&tablet_tool.id());
                state.tablet.current_cursors.remove(&tablet_tool.id());
                state.tablet.output = None;
            }
            let pen_pressed = sequence.pressed(PEN);
            let pen_released = sequence.released(PEN);
            let button_pressed = sequence.pressed(BUTTON);
            let button_released = sequence.released(BUTTON);
            let eraser = state.tablet.eraser_tools.contains(&tablet_tool.id());
            if button_pressed {
                state.tablet.button_press_time = Some(sequence.time);
            }
            let short_button_click = button_released
                && short_click(state.tablet.button_press_time.take(), Some(sequence.time));

            let modifiers = state.modifiers();
            if button_pressed && let Some(pos) = state.tablet.pos {
                if state.tablet.pen_held {
                    state.pointer_up(pos, modifiers, false);
                    state.pointer_down(pos, modifiers, ToolOverride::Eraser);
                } else {
                    state.toggle_picker(pos);
                }
            }
            if pen_pressed
                && !button_pressed
                && let Some(pos) = state.tablet.pos
            {
                if eraser {
                    state.dismiss_picker();
                }
                state.pointer_down(
                    pos,
                    modifiers,
                    ToolOverride::from_eraser(eraser || state.tablet.button_held),
                );
            }
            if button_released
                && state.tablet.pen_held
                && !pen_released
                && let Some(pos) = state.tablet.pos
            {
                state.pointer_up(pos, modifiers, false);
                state.pointer_down(pos, modifiers, ToolOverride::from_eraser(eraser));
            } else if button_released
                && state.draw.picker_active()
                && let Some(pos) = state.tablet.pos
            {
                state.pointer_up(pos, modifiers, short_button_click);
            }
            if !button_pressed
                && !button_released
                && sequence.motion.is_some()
                && (state.tablet.pen_held
                    || (state.draw.picker_active() && state.tablet.button_held))
                && let Some(pos) = state.tablet.pos
            {
                state.pointer_motion(pos, modifiers);
            }
            if pen_released && let Some(pos) = state.tablet.pos {
                state.pointer_up(pos, modifiers, false);
            }
            if !sequence.proximity_out {
                let (x, y) = state.tablet.pos.unwrap_or_default();
                let tool_override = ToolOverride::from_eraser(
                    eraser || (state.tablet.button_held && state.tablet.pen_held),
                );
                let cursor = state
                    .draw
                    .cursor(Point::new(x as f32, y as f32), tool_override);
                let point = Point::new(x as f32, y as f32);
                let preview_changed = state.draw.set_tool_cursor(match cursor {
                    Cursor::Tool(preview)
                        if state.active && state.tablet.tool_cursor_supported(tablet_tool) =>
                    {
                        Some((point, preview))
                    }
                    _ => None,
                });
                if state.active {
                    state.tablet.refresh_cursor(tablet_tool, cursor);
                }
                if preview_changed {
                    state.request_render();
                }
            } else {
                state.refresh_pointer_cursor();
            }
            if pen_pressed || pen_released || button_pressed || button_released {
                state.update_output_input();
            }
        }
    }
}

#[derive(Default, Clone, Copy)]
struct EventSequence {
    motion: Option<(f64, f64)>,

    pressed: u8,
    released: u8,

    enter_serial: Option<u32>,
    proximity_out: bool,
    time: u32,
}

impl EventSequence {
    fn pressed(self, input: u8) -> bool {
        self.pressed & input != 0
    }

    fn released(self, input: u8) -> bool {
        self.released & input != 0
    }

    fn dispatch(
        &mut self,
        event: <ZwpTabletToolV2 as Proxy>::Event,
        origin: (f64, f64),
    ) -> Option<Self> {
        use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_tool_v2::Event;
        match event {
            Event::ProximityIn {
                serial,
                tablet: _,
                surface: _,
            } => {
                self.enter_serial = Some(serial);
                None
            }
            Event::ProximityOut => {
                self.proximity_out = true;
                None
            }
            Event::Down { serial: _ } => {
                self.pressed |= PEN;
                None
            }
            Event::Up => {
                self.released |= PEN;
                None
            }
            Event::Motion { x, y } => {
                self.motion = Some((x + origin.0, y + origin.1));
                None
            }
            Event::Button {
                serial: _,
                button,
                state: button_state,
            } => {
                use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_tool_v2::ButtonState;
                if matches!(button, EVDEV_STYLUS | EVDEV_STYLUS2) {
                    match button_state {
                        WEnum::Value(ButtonState::Pressed) => self.pressed |= BUTTON,
                        WEnum::Value(ButtonState::Released) => self.released |= BUTTON,
                        _ => {}
                    }
                }
                None
            }
            Event::Frame { time } => {
                self.time = time;
                let mut tmp = Self::default();
                std::mem::swap(self, &mut tmp);
                Some(tmp)
            }
            _ => None,
        }
    }
}

fn update_held(held: &mut bool, sequence: EventSequence, input: u8) {
    if sequence.pressed(input) {
        *held = true;
    }
    if sequence.released(input) {
        *held = false;
    }
}
