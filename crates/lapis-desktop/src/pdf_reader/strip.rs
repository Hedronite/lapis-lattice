//! Where every page sits in the continuous column, from page aspect ratios alone.
use std::ops::Range;

/// Space above the first page and below the last.
pub(super) const PAD: f32 = 24.;
/// Space between pages.
pub(super) const GAP: f32 = 16.;

pub(super) struct Strip {
    tops: Vec<f32>,
    heights: Vec<f32>,
    pub total: f32,
}

impl Strip {
    /// `aspect(page)` is height over width. Pages are `width` wide.
    pub fn new(pages: u32, width: f32, aspect: impl Fn(u32) -> f32) -> Self {
        let mut tops = Vec::with_capacity(pages as usize);
        let mut heights = Vec::with_capacity(pages as usize);
        let mut y = PAD;
        for page in 0..pages {
            let h = (width * aspect(page)).max(1.);
            tops.push(y);
            heights.push(h);
            y += h + GAP;
        }
        let total = if pages == 0 { 0. } else { y - GAP + PAD };
        Self { tops, heights, total }
    }
    pub fn top(&self, page: u32) -> f32 {
        self.tops.get(page as usize).copied().unwrap_or(0.)
    }
    pub fn height(&self, page: u32) -> f32 {
        self.heights.get(page as usize).copied().unwrap_or(0.)
    }
    /// The page whose span (with the gap below it) contains `y`, clamped to the strip.
    pub fn at(&self, y: f32) -> u32 {
        let after = self.tops.partition_point(|top| *top <= y);
        after.saturating_sub(1).min(self.tops.len().saturating_sub(1)) as u32
    }
    /// Pages that intersect `[y0, y1)`.
    pub fn visible(&self, y0: f32, y1: f32) -> Range<u32> {
        if self.tops.is_empty() {
            return 0..0;
        }
        let first = self.at(y0);
        let last = self.at(y1.max(y0));
        first..last + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_stack_with_gaps_and_lookups_agree() {
        let strip = Strip::new(4, 100., |p| if p == 1 { 2. } else { 1. });
        assert_eq!(strip.top(0), PAD);
        assert_eq!(strip.top(1), PAD + 100. + GAP);
        assert_eq!(strip.top(2), PAD + 100. + GAP + 200. + GAP);
        assert_eq!(strip.total, strip.top(3) + 100. + PAD);
        assert_eq!(strip.at(0.), 0);
        assert_eq!(strip.at(strip.top(1) + 5.), 1);
        assert_eq!(strip.at(1e9), 3);
        assert_eq!(strip.visible(strip.top(1) + 10., strip.top(2) + 10.), 1..3);
        assert_eq!(Strip::new(0, 100., |_| 1.).visible(0., 100.), 0..0);
    }

    #[test]
    fn a_long_book_is_cheap_to_lay_out() {
        let strip = Strip::new(1308, 800., |_| 1.3);
        assert_eq!(strip.at(1e12), 1307);
        let middle = strip.top(654) + 1.;
        assert_eq!(strip.at(middle), 654);
        assert_eq!(strip.visible(middle, middle + 900.).len(), 1);
    }
}
