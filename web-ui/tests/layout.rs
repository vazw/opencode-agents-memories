//! The shell's width arithmetic is a pure model; these run with no DOM.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::layout::{Frame, columns_for, layout_at};

#[wasm_bindgen_test]
fn test_layout_at_the_four_widths() {
    // MEASURED clientWidth at the four emulated viewports (scrollbar is not constant).
    for (client, body) in [(1009, 701), (1266, 958), (1585, 1277), (1905, 1597)] {
        assert_eq!(
            layout_at(client),
            Frame {
                canvas: client,
                sider: 260,
                body
            }
        );
    }
    // At or below 640 the sider is a top band and takes no horizontal space.
    assert_eq!(
        layout_at(640),
        Frame {
            canvas: 640,
            sider: 0,
            body: 592
        }
    );
    assert_eq!(layout_at(641).sider, 260);
    // Never underflows.
    assert_eq!(layout_at(10).body, 0);
}

const CARD: f64 = 551.476;
const GAP: f64 = 24.0;

#[wasm_bindgen_test]
fn test_columns_for_boundary() {
    for (body, want) in [
        (653.0, 1),
        (910.0, 1),
        (1125.0, 1),
        (1126.0, 1),
        (1127.0, 2),
        (1229.0, 2),
        (1549.0, 2),
        (1701.0, 2),
        (1702.0, 2),
        (1703.0, 3),
        (2189.0, 3),
    ] {
        assert_eq!(columns_for(body, CARD, GAP), want, "body {body}");
    }
    // The max(1, ..) clamp: a body narrower than one card still holds one track.
    assert_eq!(columns_for(300.0, CARD, GAP), 1);
    // A degenerate card width must not divide by zero.
    assert_eq!(columns_for(1000.0, 0.0, GAP), 1);
    assert_eq!(columns_for(1000.0, -5.0, GAP), 1);
}
