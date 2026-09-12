//! Visits are committed only after navigation succeeds; failed opens keep the cursor.
#[derive(Default)]
pub(super) struct History {
    paths: Vec<String>,
    position: usize,
}
impl History {
    pub fn target(&self, forward: bool) -> Option<(usize, String)> {
        let index = if forward { self.position.checked_add(1)? } else { self.position.checked_sub(1)? };
        self.paths.get(index).cloned().map(|p| (index, p))
    }
    pub fn commit(&mut self, path: String, travel: Option<usize>) {
        if let Some(index) = travel.filter(|&i| self.paths.get(i) == Some(&path)) {
            self.position = index;
            return;
        }
        if self.paths.get(self.position) == Some(&path) {
            return;
        }
        self.paths.truncate(self.position + 1);
        self.paths.push(path);
        if self.paths.len() > 200 {
            self.paths.remove(0);
        }
        self.position = self.paths.len() - 1;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn branching_discards_forward_visits_and_failed_requests_do_not_move() {
        let mut h = History::default();
        for p in ["a", "b", "c"] {
            h.commit(p.into(), None);
        }
        let (i, p) = h.target(false).unwrap();
        assert_eq!(h.target(false), Some((i, p.clone())));
        h.commit(p, Some(i));
        assert_eq!(h.target(true).unwrap().1, "c");
        h.commit("d".into(), None);
        assert!(h.target(true).is_none());
        assert_eq!(h.target(false).unwrap().1, "b");
    }
}
