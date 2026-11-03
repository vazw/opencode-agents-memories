//! The shell's width arithmetic as a pure model (no DOM).
//!
//! The browser is the authority on layout; these functions are the oracle the
//! browser criteria are compared against, so a drift in either side is visible.

/// Measured `.app-sider` border box above the mobile breakpoint.
pub const SIDER_TEXT: u32 = 260;
/// Measured `.app-main` `padding-left` / `padding-right`.
pub const BODY_PAD_X: u32 = 24;

/// The three widths that matter to a card grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub canvas: u32,
    pub sider: u32,
    pub body: u32,
}

/// Models the shell's width arithmetic. Above 640 the sider is a 260 px column;
/// at or below 640 the stylesheet turns it into a 100 %-wide top band, so it
/// takes no horizontal space. No width claim is made below 641 beyond that.
pub fn layout_at(client_w: u32) -> Frame {
    let sider = if client_w > 640 { SIDER_TEXT } else { 0 };
    Frame {
        canvas: client_w,
        sider,
        body: client_w.saturating_sub(sider + 2 * BODY_PAD_X),
    }
}

/// How many `card_w` tracks fit in `body_w` with `gap` between them:
/// `max(1, floor((body_w + gap) / (card_w + gap)))`. A non-positive `card_w`
/// answers 1 rather than dividing by zero.
pub fn columns_for(body_w: f64, card_w: f64, gap: f64) -> usize {
    if card_w <= 0.0 {
        return 1;
    }
    let fit = ((body_w + gap) / (card_w + gap)).floor();
    if fit.is_finite() && fit >= 1.0 {
        fit as usize
    } else {
        1
    }
}
