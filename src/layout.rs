use keyberon::key_code::KeyCode::*;
// use keyberon::action::{k, Action::*, HoldTapAction, HoldTapConfig};

// type Action = keyberon::action::Action<()>;

#[rustfmt::skip]
pub static LAYERS: keyberon::layout::Layers<3, 3, 1, ()> = keyberon::layout::layout! {
  {
    [F1 F2 F3],
    [F4 F5 F6],
    [F7 F8 F9]
  }
};
