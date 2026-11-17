//! The article list as loaded, and the read and star changes made on top of it.
//!
//! [`Relist`] decides which rows a new load lists: the cache's rows, plus those the
//! list keeps so it doesn't jump while you read. The window queries the cache and hands
//! the rows in; nothing here touches the cache or the UI.

use std::collections::{HashMap, HashSet};

use skvaller_core::{ArticleSummary, Selection, View};

/// The article list as loaded, numbered so changes made on top of it know which load
/// they belong to.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Listed {
    pub(super) load: u64,
    pub(super) rows: Vec<ArticleSummary>,
    /// The open article as the cache has it, when search results leave it out.
    pub(super) open: Option<ArticleSummary>,
}

/// Read and starred changes made here since the list loaded, kept on top of it, so an
/// article marked read stays in the Unread list until the list loads again. A new load
/// has them in its rows, from the cache, and starts these over.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Marks {
    /// The [`Listed::load`] these are on top of.
    load: u64,
    read: HashMap<String, bool>,
    starred: HashMap<String, bool>,
}

impl Marks {
    /// `a` with these changes, if they are on top of `load`.
    fn apply(&self, load: u64, a: &ArticleSummary) -> ArticleSummary {
        let mut a = a.clone();
        a.read = self.is_read(load, &a);
        a.starred = self.is_starred(load, &a);
        a
    }

    /// Whether `a`, a row of the list loaded as `load`, is read, with these changes.
    pub(super) fn is_read(&self, load: u64, a: &ArticleSummary) -> bool {
        self.of(&self.read, load, a).unwrap_or(a.read)
    }

    /// Whether `a`, a row of the list loaded as `load`, is starred, with these changes.
    pub(super) fn is_starred(&self, load: u64, a: &ArticleSummary) -> bool {
        self.of(&self.starred, load, a).unwrap_or(a.starred)
    }

    fn of(&self, changes: &HashMap<String, bool>, load: u64, a: &ArticleSummary) -> Option<bool> {
        (self.load == load)
            .then(|| changes.get(&a.id).copied())
            .flatten()
    }

    /// Records changes made here on top of the list loaded as `load`. Those on top of an
    /// older load are dropped: the new load has them from the cache.
    pub(super) fn record(
        &mut self,
        load: u64,
        ids: &[String],
        read: Option<bool>,
        starred: Option<bool>,
    ) {
        if self.load != load {
            *self = Self {
                load,
                ..Self::default()
            };
        }
        for id in ids {
            if let Some(read) = read {
                self.read.insert(id.clone(), read);
            }
            if let Some(starred) = starred {
                self.starred.insert(id.clone(), starred);
            }
        }
    }
}

/// What a load of the list is for: a sync's reload of the same keeps rows.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Load {
    pub(super) account: Option<String>,
    pub(super) selection: Selection,
    /// The search, with whether it is over every feed, while one is active.
    pub(super) searched: Option<(String, bool)>,
}

/// The last load, what it listed and what it was for, which the next one builds on.
#[derive(Debug, Default)]
pub(super) struct Relist {
    before: Listed,
    load: Option<Load>,
    /// The mark-all-read count it was loaded at: a new one keeps nothing.
    cleared: u64,
}

impl Relist {
    /// The list for `load` now that the cache lists `rows` for it.
    ///
    /// `marks` are the changes made on top of the last load, `cleared` is bumped by
    /// each mark-all-read, which leaves Unread nothing to keep, `open` is the open
    /// article, and `fetch` looks an article up in the cache by id.
    pub(super) fn next(
        &mut self,
        load: Load,
        mut rows: Vec<ArticleSummary>,
        cleared: u64,
        marks: &Marks,
        open: Option<&ArticleSummary>,
        fetch: impl FnOnce(&str) -> Option<ArticleSummary>,
    ) -> Listed {
        let before = &self.before;
        let searched = load.searched.is_some();
        let open_id = open.map(|a| a.id.as_str());
        let listed: HashSet<String> = rows.iter().map(|a| a.id.clone()).collect();
        // The changes made here are in the cache, and so in the rows; those of rows
        // the cache no longer lists here are only in the marks.
        let same = self.load.as_ref() == Some(&load);
        let keep = |a: &ArticleSummary| {
            // The open article stays, read or not, until another opens; in search
            // results, only if it was one.
            if open_id == Some(a.id.as_str()) {
                return same || !searched;
            }
            // After a sync, the Unread view keeps what was read here since it opened,
            // and the Starred view what was unstarred, so the list doesn't jump.
            let a = marks.apply(before.load, a);
            same && match load.selection.view {
                View::Unread => a.read && self.cleared == cleared,
                View::Starred => !a.starred,
                View::All => false,
            }
        };
        let kept: Vec<ArticleSummary> = before
            .rows
            .iter()
            .filter(|a| !listed.contains(&a.id) && keep(a))
            .map(|a| marks.apply(before.load, a))
            .collect();
        // The open article from another list, as after a new selection or a search,
        // as the cache has it now: changes made to it since are in no list's marks.
        let unlisted = open
            .filter(|a| !listed.contains(&a.id) && !kept.iter().any(|k| k.id == a.id))
            .map(|a| fetch(&a.id).unwrap_or_else(|| a.clone()));
        // Search results show it only if it matched; else it is kept apart.
        let (from_elsewhere, open_apart) = if searched {
            (None, unlisted)
        } else {
            (unlisted, None)
        };
        if !kept.is_empty() || from_elsewhere.is_some() {
            rows.extend(kept);
            rows.extend(from_elsewhere);
            rows.sort_by_key(|a| std::cmp::Reverse(a.published));
        }
        let listed = Listed {
            load: before.load + 1,
            rows,
            open: open_apart,
        };
        self.before = listed.clone();
        self.load = Some(load);
        self.cleared = cleared;
        listed
    }
}

#[cfg(test)]
mod tests {
    use skvaller_core::Scope;

    use super::*;

    /// An unread, unstarred article published at `id`.
    fn article(id: u8) -> ArticleSummary {
        ArticleSummary {
            id: id.to_string(),
            feed_id: String::new(),
            title: String::new(),
            author: String::new(),
            url: String::new(),
            published: i64::from(id),
            read: false,
            starred: false,
            thumbnail: false,
        }
    }

    fn articles(ids: &[u8]) -> Vec<ArticleSummary> {
        ids.iter().map(|&id| article(id)).collect()
    }

    fn load(view: View) -> Load {
        Load {
            account: Some("a".into()),
            selection: Selection {
                view,
                scope: Scope::All,
            },
            searched: None,
        }
    }

    fn search(view: View, query: &str) -> Load {
        Load {
            searched: Some((query.into(), false)),
            ..load(view)
        }
    }

    fn ids(listed: &Listed) -> Vec<&str> {
        listed.rows.iter().map(|a| a.id.as_str()).collect()
    }

    fn none(_: &str) -> Option<ArticleSummary> {
        None
    }

    /// A list loaded once with `rows`, and changes made on top of it.
    fn loaded(load: &Load, rows: &[u8]) -> (Relist, Listed, Marks) {
        let mut relist = Relist::default();
        let listed = relist.next(
            load.clone(),
            articles(rows),
            0,
            &Marks::default(),
            None,
            none,
        );
        (relist, listed, Marks::default())
    }

    #[test]
    fn loads_are_numbered() {
        let (mut relist, first, marks) = loaded(&load(View::All), &[1]);
        let second = relist.next(load(View::All), articles(&[1]), 0, &marks, None, none);
        assert_eq!(second.load, first.load + 1);
    }

    #[test]
    fn unread_keeps_what_was_read_here_after_a_sync() {
        let unread = load(View::Unread);
        let (mut relist, listed, mut marks) = loaded(&unread, &[3, 2, 1]);
        marks.record(listed.load, &["2".into()], Some(true), None);
        // The sync's reload: the cache no longer lists 2, read here, nor 1, read
        // elsewhere.
        let next = relist.next(unread, articles(&[4, 3]), 0, &marks, None, none);
        assert_eq!(ids(&next), ["4", "3", "2"]);
        assert!(next.rows[2].read, "kept with the change made here");
    }

    #[test]
    fn starred_keeps_what_was_unstarred_here() {
        let starred = load(View::Starred);
        let mut relist = Relist::default();
        let mut rows = articles(&[2, 1]);
        for a in &mut rows {
            a.starred = true;
        }
        let listed = relist.next(starred.clone(), rows, 0, &Marks::default(), None, none);
        let mut marks = Marks::default();
        marks.record(listed.load, &["1".into()], None, Some(false));
        let next = relist.next(starred, articles(&[2]), 0, &marks, None, none);
        assert_eq!(ids(&next), ["2", "1"]);
        assert!(!next.rows[1].starred);
    }

    #[test]
    fn all_keeps_nothing() {
        let all = load(View::All);
        let (mut relist, listed, mut marks) = loaded(&all, &[2, 1]);
        marks.record(listed.load, &["1".into()], Some(true), None);
        let next = relist.next(all, articles(&[2]), 0, &marks, None, none);
        assert_eq!(ids(&next), ["2"]);
    }

    #[test]
    fn a_new_selection_keeps_nothing_read_here() {
        let (mut relist, listed, mut marks) = loaded(&load(View::Unread), &[2, 1]);
        marks.record(listed.load, &["1".into()], Some(true), None);
        let mut other = load(View::Unread);
        other.selection.scope = Scope::Feed { id: "f".into() };
        let next = relist.next(other, articles(&[2]), 0, &marks, None, none);
        assert_eq!(ids(&next), ["2"]);
    }

    #[test]
    fn mark_all_read_keeps_nothing() {
        let unread = load(View::Unread);
        let (mut relist, listed, mut marks) = loaded(&unread, &[2, 1]);
        marks.record(listed.load, &["2".into(), "1".into()], Some(true), None);
        let next = relist.next(unread, Vec::new(), 1, &marks, None, none);
        assert_eq!(next.rows, []);
    }

    #[test]
    fn mark_all_read_in_starred_keeps_what_was_unstarred_here() {
        let starred = load(View::Starred);
        let (mut relist, listed, mut marks) = loaded(&starred, &[2, 1]);
        marks.record(listed.load, &["1".into()], None, Some(false));
        let next = relist.next(starred, articles(&[2]), 1, &marks, None, none);
        assert_eq!(ids(&next), ["2", "1"]);
    }

    #[test]
    fn what_was_read_here_stays_through_several_syncs() {
        let unread = load(View::Unread);
        let (mut relist, listed, mut marks) = loaded(&unread, &[2, 1]);
        marks.record(listed.load, &["1".into()], Some(true), None);
        let between = relist.next(unread.clone(), articles(&[2]), 0, &marks, None, none);
        assert_eq!(ids(&between), ["2", "1"]);
        // The marks are on the first load now, and the kept row carries its change.
        let next = relist.next(unread, articles(&[2]), 0, &marks, None, none);
        assert_eq!(ids(&next), ["2", "1"]);
        assert!(next.rows[1].read);
    }

    #[test]
    fn marks_apply_only_to_their_load() {
        let mut marks = Marks::default();
        marks.record(3, &["1".into()], Some(true), Some(true));
        assert!(marks.is_read(3, &article(1)));
        assert!(marks.is_starred(3, &article(1)));
        assert!(!marks.is_read(4, &article(1)));
        assert!(!marks.is_read(3, &article(2)));
        // A change on top of a new load starts the marks over.
        marks.record(4, &["2".into()], Some(true), None);
        assert!(!marks.is_read(4, &article(1)));
        assert!(marks.is_read(4, &article(2)));
    }

    #[test]
    fn the_open_article_stays_after_a_sync() {
        let unread = load(View::Unread);
        let (mut relist, _, marks) = loaded(&unread, &[2, 1]);
        let open = article(1);
        // Read elsewhere, with no mark here: it stays because it is open.
        let next = relist.next(unread, articles(&[2]), 0, &marks, Some(&open), none);
        assert_eq!(ids(&next), ["2", "1"]);
    }

    #[test]
    fn the_open_article_stays_in_a_new_selection() {
        let (mut relist, _, marks) = loaded(&load(View::Unread), &[2, 1]);
        let open = article(1);
        let mut other = load(View::Unread);
        other.selection.scope = Scope::Feed { id: "f".into() };
        let next = relist.next(other, articles(&[3]), 0, &marks, Some(&open), |_| {
            panic!("the last list had it")
        });
        assert_eq!(ids(&next), ["3", "1"]);
    }

    #[test]
    fn the_open_article_from_another_list_is_taken_from_the_cache() {
        // A search that left the open article out, kept apart.
        let (mut relist, _, marks) = loaded(&search(View::All, "x"), &[2]);
        let open = article(1);
        let mut cached = article(1);
        cached.starred = true;
        let next = relist.next(
            load(View::All),
            articles(&[3]),
            0,
            &marks,
            Some(&open),
            |id| {
                assert_eq!(id, "1");
                Some(cached)
            },
        );
        assert_eq!(ids(&next), ["3", "1"]);
        assert!(next.rows[1].starred, "as the cache has it now");
        assert_eq!(next.open, None);
    }

    #[test]
    fn the_open_article_is_kept_apart_from_search_results_it_does_not_match() {
        let (mut relist, _, marks) = loaded(&load(View::All), &[2, 1]);
        let open = article(1);
        let next = relist.next(
            search(View::All, "x"),
            articles(&[2]),
            0,
            &marks,
            Some(&open),
            none,
        );
        assert_eq!(ids(&next), ["2"]);
        assert_eq!(next.open.as_ref().map(|a| a.id.as_str()), Some("1"));
    }

    #[test]
    fn the_open_article_in_search_results_is_listed() {
        let (mut relist, _, marks) = loaded(&load(View::All), &[2, 1]);
        let open = article(1);
        let next = relist.next(
            search(View::All, "x"),
            articles(&[1]),
            0,
            &marks,
            Some(&open),
            none,
        );
        assert_eq!(ids(&next), ["1"]);
        assert_eq!(next.open, None);
    }
}
