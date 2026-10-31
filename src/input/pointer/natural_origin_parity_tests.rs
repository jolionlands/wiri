//! TEMP tests-only draft for future src/input/pointer/natural_origin_parity_tests.rs.
//! After checkpoint, pointer.rs may register this cfg(test) child module.
//! No global ENGINE, runtime setter, hook, native input, or new production API.
//! Current baseline expected: two assertion failures and two passing controls.

use super::*;

fn natural_origin_pointer_case(input: &str, mouse_natural: bool) {
    let text = format!(
        "input {{ {input} }}\nbinds {{ extend-defaults false; Ctrl+WheelScrollUp {{ refresh; }}; Ctrl+WheelScrollRight {{ refresh; }}; }}"
    );
    let decoded = crate::config::decode::decode_text(&text, "", None);
    assert!(decoded.issues.is_empty(), "{:?}", decoded.issues);
    let mut pointer = PointerEngine::default();
    pointer.configure(&decoded.config);
    let now = Instant::now();
    let matching_delta = if mouse_natural { -120 } else { 120 };
    for horizontal in [false, true] {
        assert_eq!(
            pointer.wheel(2, horizontal, matching_delta, now),
            (true, vec![Action::Refresh]),
            "Ctrl Up/Right must follow mouse natural-scroll; horizontal={horizontal}"
        );
        assert_eq!(
            pointer.wheel(2, horizontal, -matching_delta, now),
            (true, vec![]),
            "the opposite direction remains consumed on the bound axis; horizontal={horizontal}"
        );
    }
}

#[test]
fn parity_pointer_natural_origin_mouse_true_before_touchpad_false() {
    natural_origin_pointer_case(
        "mouse { natural-scroll true; }; touchpad { natural-scroll false; };",
        true,
    );
}

#[test]
fn parity_pointer_natural_origin_mouse_true_after_touchpad_false_control() {
    natural_origin_pointer_case(
        "touchpad { natural-scroll false; }; mouse { natural-scroll true; };",
        true,
    );
}

#[test]
fn parity_pointer_natural_origin_mouse_false_before_touchpad_true() {
    natural_origin_pointer_case(
        "mouse { natural-scroll false; }; touchpad { natural-scroll true; };",
        false,
    );
}

#[test]
fn parity_pointer_natural_origin_mouse_false_after_touchpad_true_control() {
    natural_origin_pointer_case(
        "touchpad { natural-scroll true; }; mouse { natural-scroll false; };",
        false,
    );
}
