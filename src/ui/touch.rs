//! Long-press detection — the touch stand-in for a right-click. egui has no
//! built-in long-press, so this tracks a held touch and fires once when it has
//! been still for `HOLD` seconds within a small slop radius.

use egui::{Pos2, Response, Ui};
use std::time::Duration;

/// Hold this long (seconds) to trigger a long-press.
const HOLD: f64 = 0.5;
/// Movement (screen px) beyond this turns the press into a drag, not a long-press.
const SLOP: f32 = 12.0;

#[derive(Default)]
pub(crate) struct LongPress {
    /// Press anchor `(pos, start_time)` while a candidate press is being held.
    start: Option<(Pos2, f64)>,
    /// Whether this press already fired (or was disqualified by movement), so it
    /// fires at most once and doesn't re-fire while the finger stays down.
    done: bool,
}

impl LongPress {
    /// Feed once per frame. Returns `Some(anchor)` on the single frame the press
    /// crosses the hold threshold. Touch-only; mouse/pen never long-press (they
    /// have a real right-click).
    pub(crate) fn update(&mut self, ui: &Ui, resp: &Response) -> Option<Pos2> {
        let (down, pos, now, touching) = ui.input(|i| {
            (
                i.pointer.any_down(),
                i.pointer.interact_pos(),
                i.time,
                i.any_touches(),
            )
        });

        // Reset once the finger lifts, leaves the map, or it isn't a touch at all.
        let on_map = pos.is_some_and(|p| resp.rect.contains(p));
        if !touching || !down || !on_map {
            self.start = None;
            self.done = false;
            return None;
        }
        let pos = pos.unwrap();

        match self.start {
            None => {
                self.start = Some((pos, now));
                // Wake up around the threshold even if the finger never moves.
                ui.ctx()
                    .request_repaint_after(Duration::from_millis(((HOLD * 1000.0) as u64) + 20));
                None
            }
            Some((anchor, started)) => {
                if (pos - anchor).length() > SLOP {
                    // Became a drag — disqualify this press from long-pressing.
                    self.done = true;
                    None
                } else if !self.done && now - started >= HOLD {
                    self.done = true;
                    Some(anchor)
                } else {
                    None
                }
            }
        }
    }
}
