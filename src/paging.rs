//! A paged listing that always ends. A lattice that ignores `offset` returns the
//! same page forever, and a loop that stops only on a short page never would.
use crate::error::{LapisError, Result};

/// Most pages one listing reads: a thousand pages of a thousand documents.
pub const MAX_PAGES: u32 = 1_000;

#[derive(Default)]
pub struct Pages {
    seen: u32,
    last_first: Option<String>,
}

impl Pages {
    /// Account for one more page, given its first path. Errors (exit 2, as a
    /// down lattice) when the page repeats the previous one or the listing
    /// passes [`MAX_PAGES`].
    pub fn next(&mut self, first: Option<&str>) -> Result<()> {
        self.seen += 1;
        if self.seen > MAX_PAGES {
            return Err(LapisError::LatticeDown(format!(
                "the document listing did not end after {MAX_PAGES} pages"
            )));
        }
        if first.is_some() && first == self.last_first.as_deref() {
            return Err(LapisError::LatticeDown(
                "the lattice returned the same page twice; it may be ignoring offset".into(),
            ));
        }
        self.last_first = first.map(str::to_string);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_page_or_an_endless_listing_stops() {
        let mut p = Pages::default();
        p.next(Some("a.md")).unwrap();
        p.next(Some("b.md")).unwrap();
        assert!(p.next(Some("b.md")).is_err(), "a server ignoring offset is caught on the second page");
        let mut p = Pages::default();
        for k in 0..MAX_PAGES {
            p.next(Some(&format!("{k}.md"))).unwrap();
        }
        assert!(p.next(Some("one-more.md")).is_err());
        assert!(Pages::default().next(None).is_ok(), "an empty page is not a repeat");
    }
}
