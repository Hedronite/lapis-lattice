use super::*;
use crate::services::{Document, FileEntry, PdfPageInfo, SearchPage};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, VisualTestContext};
/// A book of `pages` pages; counts every render it is asked for.
struct Fixture {
    pages: u32,
    renders: std::sync::atomic::AtomicUsize,
}
impl Fixture {
    fn new(pages: u32) -> Arc<Self> {
        Arc::new(Self { pages, renders: 0.into() })
    }
}
impl WorkspaceServices for Fixture {
    fn directory(&self, _: &str) -> Result<Vec<FileEntry>, String> {
        Ok(vec![])
    }
    fn read(&self, _: &str) -> Result<Document, String> {
        Err("unused".into())
    }
    fn save(&self, _: &Document, _: &str) -> Result<Document, String> {
        Err("read only".into())
    }
    fn save_copy(&self, _: &Document, _: &str) -> Result<Document, String> {
        Err("read only".into())
    }
    fn search(&self, _: &str) -> Result<SearchPage, String> {
        Err("unused".into())
    }
    fn reindex(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn build_index(&self) -> Result<u64, String> {
        Ok(0)
    }
    fn pdf_page(&self, _: &str, page: u32, width: u32, cancel: ArcCancel) -> Result<PdfPage, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        self.renders.fetch_add(1, Ordering::SeqCst);
        Ok(PdfPage {
            info: PdfPageInfo {
                page,
                pages: self.pages,
                width,
                height: 1000,
                text: format!("Text for page {}", page + 1),
            },
            bgra: vec![255; width as usize * 1000 * 4],
            revision: "fixture-v1".into(),
        })
    }
}
#[gpui_kit::test]
fn pdf_navigation_keeps_a_bounded_cache_and_releases_hidden_pages(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
    });
    let handle = cx.add_window(|w, cx| PdfReader::new("reference.pdf".into(), Fixture::new(120), w, cx));
    handle
        .update(cx, |this, w, cx| {
            this.set_visible(true, w, cx);
            this.focus(w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    for _ in 0..6 {
        visual.update(|w, cx| w.press("right", cx));
        cx.run_until_parked();
        cx.executor().advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
    }
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.page, 6);
            assert_eq!(this.pages, Some(120));
            assert!(this.cache.len() <= CACHE_PAGES);
            assert!(this.cache.iter().map(|p| p.bytes).sum::<usize>() <= CACHE_BYTES);
            assert!(this.cache.iter().any(|p| p.number == 6 && p.text == "Text for page 7"));
        })
        .unwrap();
    handle.update(cx, |this, w, cx| this.set_visible(false, w, cx)).unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    handle
        .read_with(cx, |this, cx| {
            assert!(this.cache.is_empty());
            assert!(this.inflight.is_none());
            assert!(this.text.read(cx).value().is_empty());
            assert!(this.cancel.load(Ordering::Relaxed));
        })
        .unwrap();
}
#[gpui_kit::test]
fn hidden_pdf_discards_pending_page_and_preserves_reopen_position(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
    });
    let handle = cx.add_window(|w, cx| PdfReader::new("reference.pdf".into(), Fixture::new(120), w, cx));
    handle
        .update(cx, |this, w, cx| {
            this.set_visible(true, w, cx);
            this.jump_to(19, w, cx);
            this.set_visible(false, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.page, 19);
            assert!(this.inflight.is_none());
            assert!(this.cache.is_empty());
        })
        .unwrap();
    handle.update(cx, |this, w, cx| this.set_visible(true, w, cx)).unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.page, 19);
            assert!(this.cache.iter().any(|p| p.number == 19));
        })
        .unwrap();
}

fn settle(cx: &mut TestAppContext, visual: &mut VisualTestContext) {
    for _ in 0..12 {
        visual.update(|w, cx| w.render_frame(cx));
        cx.run_until_parked();
        cx.executor().advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
    }
}

#[gpui_kit::test]
fn a_long_book_scrolls_continuously_and_renders_only_near_the_viewport(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
    });
    let book = Fixture::new(1308);
    let services = book.clone();
    let handle = cx.add_window(move |w, cx| PdfReader::new("book.pdf".into(), services, w, cx));
    handle.update(cx, |this, w, cx| this.set_visible(true, w, cx)).unwrap();
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    settle(cx, &mut visual);
    let opened = book.renders.load(Ordering::SeqCst);
    assert!((1..=4).contains(&opened), "opening rendered {opened} pages");
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.pages, Some(1308));
            assert!(this.view_height > 0., "the viewport was measured");
        })
        .unwrap();

    // Scroll to the middle of the book: the page indicator follows, and only the
    // pages around the new viewport render.
    handle
        .update(cx, |this, _, cx| {
            let top = this.strip().top(654);
            this.scroll.set_offset(point(px(0.), px(-top)));
            cx.notify();
        })
        .unwrap();
    settle(cx, &mut visual);
    handle
        .read_with(cx, |this, cx| {
            assert!(this.page.abs_diff(654) <= 1, "reading page {}", this.page);
            assert_eq!(this.page_input.read(cx).value().to_string(), (this.page + 1).to_string());
            assert!(this.cache.iter().any(|p| p.number.abs_diff(654) <= 1));
            assert!(this.cache.len() <= CACHE_PAGES);
        })
        .unwrap();
    let scrolled = book.renders.load(Ordering::SeqCst) - opened;
    assert!((1..=4).contains(&scrolled), "the jump rendered {scrolled} pages, not one per page passed");

    // Typing a page number jumps straight to its top.
    handle.update(cx, |this, w, cx| this.jump_to(99, w, cx)).unwrap();
    settle(cx, &mut visual);
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.page, 99);
            let top = -f32::from(this.scroll.offset().y);
            assert!((top - (this.strip().top(99) - strip::PAD)).abs() < 1., "scrolled to {top}");
        })
        .unwrap();
}
