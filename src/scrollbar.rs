//! Scrollbar geometry.

/// Gutter width in pixels.
pub fn width(cell_width: u16) -> u16 {
    (cell_width / 2).max(6)
}

/// Thumb position and height inside the track.
pub fn thumb(rows: usize, saved: usize, offset: usize, track: i32) -> (i32, i32) {
    if saved == 0 || track <= 0 {
        return (0, track);
    }
    let total = saved + rows;
    let height = ((track * rows as i32) / total as i32).max(4).min(track);
    let travel = (track - height).max(0);
    let y = travel - (travel * offset as i32 / saved as i32);
    (y, height)
}

/// Scroll offset for a pointer position in the track.
pub fn offset_from_y(rows: usize, saved: usize, y: i32, track: i32) -> usize {
    if saved == 0 || track <= 0 {
        return 0;
    }
    let (_, height) = thumb(rows, saved, 0, track);
    let travel = (track - height).max(1);
    let position = y.clamp(0, travel);
    saved - (saved * position as usize / travel as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_moves_from_top_to_bottom() {
        let (top, _) = thumb(24, 1000, 1000, 100);
        let (bottom, _) = thumb(24, 1000, 0, 100);
        assert!(top < bottom);
    }

    #[test]
    fn pointer_maps_to_scroll_offset() {
        let (top, _) = thumb(24, 1000, 1000, 100);
        assert_eq!(offset_from_y(24, 1000, top, 100), 1000);
        assert_eq!(offset_from_y(24, 1000, 100, 100), 0);
    }
}
