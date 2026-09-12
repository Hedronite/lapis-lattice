//! Note-visit history for back/forward, mirroring the desktop workspace: a visit is
//! committed only after an open succeeds, and travelling to an earlier visit keeps
//! the forward entries instead of branching.

#[derive(Default)]
pub(crate) struct Visits {
    paths: Vec<String>,
    position: usize,
}

impl Visits {
    pub(crate) fn target(&self, forward: bool) -> Option<(usize, &str)> {
        let index = if forward { self.position.checked_add(1)? } else { self.position.checked_sub(1)? };
        self.paths.get(index).map(|p| (index, p.as_str()))
    }

    pub(crate) fn commit(&mut self, rel: &str, travel: Option<usize>) {
        if let Some(index) = travel.filter(|&i| self.paths.get(i).map(String::as_str) == Some(rel)) {
            self.position = index;
            return;
        }
        if self.paths.get(self.position).map(String::as_str) == Some(rel) {
            return;
        }
        self.paths.truncate(self.position + 1);
        self.paths.push(rel.to_string());
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
    fn travel_keeps_forward_entries_and_a_new_open_branches() {
        let mut v = Visits::default();
        for p in ["a", "b", "c"] {
            v.commit(p, None);
        }
        assert!(v.target(true).is_none());
        let (i, back) = v.target(false).map(|(i, p)| (i, p.to_string())).unwrap();
        assert_eq!(back, "b");
        v.commit(&back, Some(i));
        assert_eq!(v.target(true).unwrap().1, "c");
        assert_eq!(v.target(false).unwrap().1, "a");
        v.commit("d", None);
        assert!(v.target(true).is_none());
        assert_eq!(v.target(false).unwrap().1, "b");
        // Re-opening the current note is not a new visit.
        v.commit("d", None);
        assert_eq!(v.target(false).unwrap().1, "b");
    }
}
