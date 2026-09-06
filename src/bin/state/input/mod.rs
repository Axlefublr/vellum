mod keyboard;
mod pointer;
mod tablet;
mod text;

const CLICK_DURATION_MS: u32 = 300;

fn short_click(pressed: Option<u32>, released: Option<u32>) -> bool {
    pressed
        .zip(released)
        .is_some_and(|(pressed, released)| released.wrapping_sub(pressed) <= CLICK_DURATION_MS)
}

pub(super) use keyboard::KeyboardState;
pub(super) use pointer::PointerState;
pub(super) use tablet::TabletState;
pub(super) use text::TextInputState;
