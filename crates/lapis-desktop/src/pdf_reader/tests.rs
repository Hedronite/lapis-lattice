use super::*;
use crate::services::{Document, FileEntry, PdfPageInfo, SearchPage};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, VisualTestContext};
struct Fixture;
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
        Ok(PdfPage {
            info: PdfPageInfo {
                page,
                pages: 120,
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
    let handle = cx.add_window(|w, cx| PdfReader::new("reference.pdf".into(), Arc::new(Fixture), w, cx));
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
            assert!(this.current.is_some());
            assert!(this.cache.len() <= 3);
            assert!(this.cache.iter().map(|p| p.bytes).sum::<usize>() <= CACHE_BYTES);
            assert_eq!(this.cache.back().unwrap().text, "Text for page 7");
        })
        .unwrap();
    handle.update(cx, |this, w, cx| this.set_visible(false, w, cx)).unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    handle
        .read_with(cx, |this, cx| {
            assert!(this.cache.is_empty());
            assert!(this.current.is_none());
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
    let handle = cx.add_window(|w, cx| PdfReader::new("reference.pdf".into(), Arc::new(Fixture), w, cx));
    handle
        .update(cx, |this, w, cx| {
            this.set_visible(true, w, cx);
            this.load(19, false, w, cx);
            this.set_visible(false, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.page, 19);
            assert!(!this.loading);
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
            assert_eq!(this.cache.back().unwrap().number, 19);
        })
        .unwrap();
}
