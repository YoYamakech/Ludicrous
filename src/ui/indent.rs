// indent.rs: where the vertical indent guides go. Pure arithmetic, no drawing.
//
// A guide is a 1 px vertical line at every tab stop that lies strictly inside
// a line's leading whitespace. Stops are counted in *columns* (a tab jumps to
// the next stop); each guide also carries the character index where that stop
// is reached, because that is what `Renderer::text_pen_x` takes. It already
// knows how wide a tab is, so the guides move with the text when the tab width
// setting changes instead of drifting away from it.

/// Most guides any one line will draw. A line indented by 100 levels would
/// otherwise queue that many slivers per visible row, and past this depth the
/// guides are noise anyway.
pub const MAX_PER_LINE: usize = 24;

/// One guide: where it sits and which indent level it closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guide {
    /// Character index in the line at which the stop is reached. Feeding this
    /// to `text_pen_x` gives the x of the stop, so the guide is drawn at the
    /// boundary between the indent and the code.
    pub col: usize,
    /// 1 for the first indent level, 2 for the second, and so on.
    pub level: usize,
}

/// Guides for one line, given the tab stop width in columns.
pub fn guides(line: &str, tab_width: usize) -> Vec<Guide> {
    let tab_width = tab_width.max(1);
    let mut out = Vec::new();
    let mut column = 0usize;
    for (i, ch) in line.chars().enumerate() {
        if !ch.is_whitespace() {
            break;
        }
        // Column reached after this character.
        let next = if ch == '\t' {
            (column / tab_width + 1) * tab_width
        } else {
            column + 1
        };
        // Every stop this character reaches gets a guide: a tab that jumps
        // over one lands on it, a tab that jumps over several produces one
        // guide per level it crosses, so the reader can still count to the
        // code. `col` is the index just after the character, which is where
        // the pen sits once the indent has been laid out.
        let mut stop = (column / tab_width + 1) * tab_width;
        while stop <= next && out.len() < MAX_PER_LINE {
            out.push(Guide {
                col: i + 1,
                level: out.len() + 1,
            });
            stop += tab_width;
        }
        column = next;
        if out.len() >= MAX_PER_LINE {
            break;
        }
    }
    out
}

/// The level that contains character `col` of the line: the deepest guide at
/// or left of it. `0` means the caret sits before the first stop.
///
/// Used for the brighter "you are here" guide. It follows the caret's column
/// rather than the line's indent, so a caret parked in the middle of a run of
/// spaces still lights up the level it is in.
pub fn level_at(line: &str, col: usize, tab_width: usize) -> usize {
    let tab_width = tab_width.max(1);
    let mut column = 0usize;
    for (i, ch) in line.chars().enumerate() {
        if i >= col || !ch.is_whitespace() {
            break;
        }
        column = if ch == '\t' {
            (column / tab_width + 1) * tab_width
        } else {
            column + 1
        };
    }
    column / tab_width
}

/// Which of `line`'s guides should be drawn brighter for a caret at `col`.
///
/// Always answers with a level that exists on that line (so exactly one guide
/// lights up on the caret's line), or `None` when the line has no guides at
/// all. A caret at the very start of an indented line is inside the first
/// level, which is why level 0 is lifted to 1 rather than leaving nothing lit.
pub fn active_level(guides: &[Guide], line: &str, col: usize, tab_width: usize) -> Option<usize> {
    if guides.is_empty() {
        return None;
    }
    let level = level_at(line, col, tab_width).clamp(1, guides.len());
    Some(level)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(col: usize, level: usize) -> Guide {
        Guide { col, level }
    }

    #[test]
    fn straight_spaces_give_one_guide_per_level() {
        assert_eq!(guides("x", 4), vec![]);
        assert_eq!(guides("    x", 4), vec![g(4, 1)]);
        assert_eq!(guides("        x", 4), vec![g(4, 1), g(8, 2)]);
    }

    #[test]
    fn a_partial_indent_gets_no_guide() {
        // Three spaces is inside the first level: no boundary to draw at.
        assert!(guides("   x", 4).is_empty());
    }

    #[test]
    fn tabs_count_to_the_next_stop() {
        assert_eq!(guides("\tx", 4), vec![g(1, 1)]);
        assert_eq!(guides("\t\tx", 4), vec![g(1, 1), g(2, 2)]);
        // A wide tab crosses several stops in one character, and each one is a
        // real level the reader can count.
        assert_eq!(guides("\tx", 2), vec![g(1, 1)]);
        assert_eq!(guides("\tx", 1), vec![g(1, 1)]);
    }

    #[test]
    fn a_tab_can_cross_two_stops_at_once() {
        // Two spaces then a tab with a width of 4: the tab lands on 4.
        assert_eq!(guides("  \tx", 4), vec![g(3, 1)]);
        // A width of 6 after one space reaches 6, still one stop.
        assert_eq!(guides(" \tx", 6), vec![g(2, 1)]);
    }

    #[test]
    fn mixed_indent_adds_up_in_columns() {
        // Four spaces, a tab (width 4 -> 8), four spaces -> 12.
        assert_eq!(guides("    \t    x", 4), vec![g(4, 1), g(5, 2), g(9, 3)]);
    }

    #[test]
    fn an_empty_or_whitespace_only_line_draws_what_its_spaces_cover() {
        assert!(guides("", 4).is_empty());
        assert_eq!(guides("    ", 4), vec![g(4, 1)]);
    }

    #[test]
    fn deep_indentation_is_capped() {
        let line = " ".repeat(4 * 100) + "x";
        assert_eq!(guides(&line, 4).len(), MAX_PER_LINE);
    }

    #[test]
    fn the_caret_level_follows_its_column() {
        let line = "        let x";
        assert_eq!(level_at(line, 0, 4), 0);
        assert_eq!(level_at(line, 3, 4), 0, "still inside the first level");
        assert_eq!(level_at(line, 4, 4), 1);
        assert_eq!(level_at(line, 8, 4), 2);
        // Past the whitespace the level stops growing.
        assert_eq!(level_at(line, 12, 4), 2);
    }

    #[test]
    fn exactly_one_guide_lights_up_on_the_caret_line() {
        let line = "            let c = 3;";
        let g = guides(line, 4);
        assert_eq!(g.len(), 3);
        // Column 0 is inside the first level, so the first guide is lit even
        // though `level_at` reports 0 there.
        assert_eq!(active_level(&g, line, 0, 4), Some(1));
        assert_eq!(active_level(&g, line, 11, 4), Some(2));
        assert_eq!(active_level(&g, line, 12, 4), Some(3));
        // Far past the indentation: still the deepest guide on the line.
        assert_eq!(active_level(&g, line, 40, 4), Some(3));
        // Nothing to light up on a flush-left line.
        assert_eq!(active_level(&[], "let x = 1;", 0, 4), None);
    }

    #[test]
    fn the_caret_level_counts_tabs_as_their_width() {
        assert_eq!(level_at("\t\tx", 2, 4), 2);
        assert_eq!(level_at("\tx", 1, 4), 1);
    }

    #[test]
    fn a_zero_width_tab_setting_does_not_divide_by_zero() {
        // Clamped to 1, so three spaces are three levels.
        assert_eq!(guides("   x", 0).len(), 3);
        assert_eq!(level_at("   x", 3, 0), 3);
    }
}
