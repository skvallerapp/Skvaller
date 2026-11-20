//! The sidebar's rows as a tree: which feeds and folders show, in what order, and
//! how the selection steps through them. Pure, and tested on its own.

use super::*;

/// The article counts per feed that `view` shows: starred for Starred, else unread.
///
/// Feeds without any are left out.
pub(super) fn counts_by_feed(counts: &Counts, view: View) -> &HashMap<String, i64> {
    match view {
        View::Starred => &counts.starred_by_feed,
        View::Unread | View::All => &counts.unread_by_feed,
    }
}

/// What the account, view and selection were, and when the last sync finished.
pub(super) type PinnedKey = (Option<String>, Selection, Option<i64>);

/// The shown account's cache changes, as [`App::changes`] told them, counted by what
/// loads again for them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Cached {
    /// Every change: the library loads again, for its feeds and counts.
    pub(super) library: u64,
    /// Every change but a read or star change, which the list shows through its marks:
    /// the list loads again, keeping what was read or unstarred here meanwhile.
    pub(super) list: u64,
    /// Every mark-all-read: the list loads again, and Unread keeps none of those.
    pub(super) cleared: u64,
}

impl Cached {
    /// Counts `what` changed, or `None` for changes that may have been missed.
    pub(super) fn heard(&mut self, what: Option<Changed>) {
        self.library += 1;
        if what != Some(Changed::Flags) {
            self.list += 1;
        }
        if what == Some(Changed::AllRead) {
            self.cleared += 1;
        }
    }
}

/// The feeds with articles when the key last changed.
#[derive(Debug, Default)]
pub(super) struct Pinned {
    pub(super) key: Option<PinnedKey>,
    pub(super) feeds: Rc<HashSet<String>>,
}

impl Pinned {
    /// The feeds in `counts`, taken again only when `key` changed.
    pub(super) fn update(
        &mut self,
        key: PinnedKey,
        counts: &HashMap<String, i64>,
    ) -> Rc<HashSet<String>> {
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.feeds = Rc::new(counts.keys().cloned().collect());
        }
        Rc::clone(&self.feeds)
    }
}

/// A row at the top level of the sidebar's feeds.
#[derive(Debug)]
pub(super) enum Entry<'a> {
    /// A folder with its shown feeds, collapsed or not.
    Folder(&'a skvaller_core::Folder, Vec<&'a skvaller_core::Feed>),
    /// A feed in no folder.
    Feed(&'a skvaller_core::Feed),
}

impl Entry<'_> {
    pub(super) fn title(&self) -> &str {
        match self {
            Self::Folder(folder, _) => &folder.title,
            Self::Feed(feed) => &feed.title,
        }
    }

    /// The row's key, so a row that moves keeps its state.
    pub(super) fn key(&self) -> String {
        match self {
            Self::Folder(folder, _) => format!("folder-{}", folder.id),
            Self::Feed(feed) => feed.id.clone(),
        }
    }
}

/// Orders titles as SQLite's NOCASE does, as the cache sorts each list.
///
/// ASCII letters are folded, then the UTF-8 bytes compare.
pub(super) fn nocase(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.bytes().map(|c| c.to_ascii_lowercase());
    a.cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

/// What the sidebar shows of `lib` in `view`, in the `sort` order. All shows every
/// subscribed feed. Unread and Starred show the feeds with articles in the view or
/// `pinned`, the folders with any, and always the selected `scope`.
pub(super) fn sidebar_tree<'a>(
    lib: &'a Library,
    view: View,
    pinned: &HashSet<String>,
    scope: &Scope,
    sort: SortSubscriptions,
) -> Vec<Entry<'a>> {
    let counts = counts_by_feed(&lib.counts, view);
    let mut by_folder: HashMap<Option<&str>, Vec<&skvaller_core::Feed>> = HashMap::new();
    for feed in lib.feeds.iter().filter(|f| f.subscribed) {
        let shown = view == View::All
            || counts.contains_key(&feed.id)
            || pinned.contains(&feed.id)
            || matches!(scope, Scope::Feed { id } if *id == feed.id);
        if shown {
            by_folder
                .entry(feed.folder_id.as_deref())
                .or_default()
                .push(feed);
        }
    }
    let folders = lib.folders.iter().filter_map(|folder| {
        let feeds = by_folder
            .remove(&Some(folder.id.as_str()))
            .unwrap_or_default();
        let shown = view == View::All
            || !feeds.is_empty()
            || matches!(scope, Scope::Folder { id } if *id == folder.id);
        shown.then_some(Entry::Folder(folder, feeds))
    });
    let mut entries: Vec<_> = folders.collect();
    let loose = by_folder.remove(&None).unwrap_or_default();
    entries.extend(loose.into_iter().map(Entry::Feed));
    // Each list is A–Z already. Stable, so a folder stays before a feed of its name.
    if sort == SortSubscriptions::Alphabetical {
        entries.sort_by(|a, b| nocase(a.title(), b.title()));
    }
    entries
}

/// The scope `delta` rows from `scope` in the sidebar, if there is a row there.
pub(super) fn step_scope(
    lib: &Library,
    view: View,
    pinned: &HashSet<String>,
    collapsed: &HashSet<String>,
    scope: &Scope,
    delta: isize,
    sort: SortSubscriptions,
) -> Option<Scope> {
    let row = row_scope(lib, scope, collapsed);
    let scopes = sidebar_scopes(&sidebar_tree(lib, view, pinned, scope, sort), collapsed);
    scopes
        .iter()
        .position(|s| *s == row)
        .and_then(|i| i.checked_add_signed(delta))
        .and_then(|i| scopes.get(i))
        .cloned()
}

/// The sidebar row that stands for `scope`: a feed in a collapsed folder has no row of
/// its own, so its folder's stands for it, for the arrow keys and the highlight.
pub(super) fn row_scope(lib: &Library, scope: &Scope, collapsed: &HashSet<String>) -> Scope {
    match scope {
        Scope::Feed { id } => lib
            .feed(id)
            .and_then(|f| f.folder_id.clone())
            .filter(|folder| collapsed.contains(folder))
            .map_or_else(|| scope.clone(), |id| Scope::Folder { id }),
        _ => scope.clone(),
    }
}

/// The next row below `scope` with articles in `view`, else All.
/// `None` when `scope` has no row: All, or a feed in a collapsed folder.
pub(super) fn next_with_articles(
    lib: &Library,
    view: View,
    pinned: &HashSet<String>,
    collapsed: &HashSet<String>,
    scope: &Scope,
    sort: SortSubscriptions,
) -> Option<Scope> {
    if *scope == Scope::All {
        return None;
    }
    let counts = counts_by_feed(&lib.counts, view);
    let entries = sidebar_tree(lib, view, pinned, scope, sort);
    // Every feed with articles is in the entries, collapsed or not.
    let folders: HashSet<&str> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Folder(folder, feeds) if feeds.iter().any(|f| counts.contains_key(&f.id)) => {
                Some(folder.id.as_str())
            }
            _ => None,
        })
        .collect();
    let has_articles = |row: &Scope| match row {
        Scope::Feed { id } => counts.contains_key(id),
        Scope::Folder { id } => folders.contains(id.as_str()),
        Scope::All => false,
    };
    let rows = sidebar_scopes(&entries, collapsed);
    let at = rows.iter().position(|row| row == scope)?;
    Some(
        rows[at + 1..]
            .iter()
            .find(|row| has_articles(row))
            .cloned()
            .unwrap_or(Scope::All),
    )
}

/// The sidebar rows that mark-all-read in `account` and `view` emptied: they close and
/// fade instead of going at once.
#[derive(Debug)]
pub(super) struct Leaving {
    pub(super) account: String,
    pub(super) view: View,
    pub(super) feeds: HashSet<String>,
    /// The folders that go with their last feed.
    pub(super) folders: HashSet<String>,
}

impl Leaving {
    /// The rows the sidebar shows from `shown` with `from` selected that it has none for
    /// once `to` is selected, given `after`'s counts: the cache's after the mark. The
    /// selection then pins only the feeds with articles, so a feed read to zero earlier
    /// goes too. The row selected next stays, even with no articles; a folder goes
    /// with its last feed unless it is selected next, and a selected folder's emptied
    /// feeds go.
    #[expect(
        clippy::too_many_arguments,
        reason = "the state before and after a mark, plain data for the tests"
    )]
    pub(super) fn after_mark(
        shown: &Library,
        after: &Library,
        account: &str,
        view: View,
        pinned: &HashSet<String>,
        from: &Scope,
        to: &Scope,
        sort: SortSubscriptions,
    ) -> Self {
        let kept = counts_by_feed(&after.counts, view);
        let leaves = |f: &skvaller_core::Feed| {
            !kept.contains_key(&f.id) && !matches!(to, Scope::Feed { id } if *id == f.id)
        };
        let (mut feeds, mut folders) = (HashSet::new(), HashSet::new());
        for entry in sidebar_tree(shown, view, pinned, from, sort) {
            match entry {
                Entry::Folder(folder, shown_feeds) => {
                    let before = feeds.len();
                    feeds.extend(
                        shown_feeds
                            .iter()
                            .filter(|f| leaves(f))
                            .map(|f| f.id.clone()),
                    );
                    let all_go = !shown_feeds.is_empty()
                        && feeds.len() - before == shown_feeds.len()
                        && !matches!(to, Scope::Folder { id } if *id == folder.id);
                    if all_go {
                        folders.insert(folder.id.clone());
                    }
                }
                Entry::Feed(f) if leaves(f) => {
                    feeds.insert(f.id.clone());
                }
                Entry::Feed(_) => {}
            }
        }
        Self {
            account: account.to_owned(),
            view,
            feeds,
            folders,
        }
    }
}

/// The sidebar's scopes from top to bottom, as the arrow keys step through them: all
/// feeds, then each entry, with a folder's feeds unless it is `collapsed`.
pub(super) fn sidebar_scopes(entries: &[Entry<'_>], collapsed: &HashSet<String>) -> Vec<Scope> {
    let feed = |f: &skvaller_core::Feed| Scope::Feed { id: f.id.clone() };
    let mut scopes = vec![Scope::All];
    for entry in entries {
        match entry {
            Entry::Folder(folder, feeds) => {
                scopes.push(Scope::Folder {
                    id: folder.id.clone(),
                });
                if !collapsed.contains(&folder.id) {
                    scopes.extend(feeds.iter().map(|f| feed(f)));
                }
            }
            Entry::Feed(f) => scopes.push(feed(f)),
        }
    }
    scopes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(id: &str, folder: Option<&str>) -> skvaller_core::Feed {
        skvaller_core::Feed {
            id: id.into(),
            title: id.into(),
            folder_id: folder.map(Into::into),
            subscribed: true,
            icon: None,
            reader_view: false,
            url: None,
            site_url: None,
            fetch_error: None,
        }
    }

    fn folder(id: &str) -> skvaller_core::Folder {
        skvaller_core::Folder {
            id: id.into(),
            title: id.to_uppercase(),
        }
    }

    fn feed_scope(id: &str) -> Scope {
        Scope::Feed { id: id.into() }
    }

    fn folder_scope(id: &str) -> Scope {
        Scope::Folder { id: id.into() }
    }

    #[test]
    fn the_sidebar_steps_all_then_folders_with_their_open_feeds_then_loose_feeds() {
        let lib = Library {
            folders: vec![folder("d")],
            feeds: vec![feed("loose", None), feed("inner", Some("d"))],
            ..Default::default()
        };
        let entries = sidebar_tree(
            &lib,
            View::All,
            &HashSet::new(),
            &Scope::All,
            SortSubscriptions::FoldersFirst,
        );
        assert_eq!(
            sidebar_scopes(&entries, &HashSet::new()),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("inner"),
                feed_scope("loose")
            ]
        );
        // A collapsed folder's feeds have no rows.
        assert_eq!(
            sidebar_scopes(&entries, &HashSet::from(["d".to_owned()])),
            [Scope::All, folder_scope("d"), feed_scope("loose")]
        );
    }

    #[test]
    fn unread_and_starred_hide_feeds_without_articles() {
        let mut lib = Library {
            folders: vec![folder("d"), folder("e"), folder("f")],
            feeds: vec![
                feed("a", Some("d")),
                feed("b", Some("d")),
                feed("c", Some("e")),
                feed("x", None),
                feed("y", None),
            ],
            ..Default::default()
        };
        // "b" was read to zero but is still pinned; "y" has articles in both views, and
        // "x" only unread ones.
        let pinned = HashSet::from(["b".to_owned()]);
        lib.counts.unread_by_feed.insert("x".into(), 2);
        lib.counts.unread_by_feed.insert("y".into(), 3);
        lib.counts.starred_by_feed.insert("y".into(), 1);
        let scopes = |view, selected: &Scope| {
            sidebar_scopes(
                &sidebar_tree(
                    &lib,
                    view,
                    &pinned,
                    selected,
                    SortSubscriptions::FoldersFirst,
                ),
                &HashSet::new(),
            )
        };
        assert_eq!(
            scopes(View::Unread, &Scope::All),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("b"),
                feed_scope("x"),
                feed_scope("y")
            ]
        );
        assert_eq!(
            scopes(View::Starred, &Scope::All),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("b"),
                feed_scope("y")
            ]
        );
        // The selected feed shows, and so its folder; a selected folder shows empty.
        assert_eq!(
            scopes(View::Unread, &feed_scope("c")),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("b"),
                folder_scope("e"),
                feed_scope("c"),
                feed_scope("x"),
                feed_scope("y"),
            ]
        );
        assert_eq!(
            scopes(View::Unread, &folder_scope("f")),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("b"),
                folder_scope("f"),
                feed_scope("x"),
                feed_scope("y")
            ]
        );
        assert_eq!(
            scopes(View::All, &Scope::All),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("a"),
                feed_scope("b"),
                folder_scope("e"),
                feed_scope("c"),
                folder_scope("f"),
                feed_scope("x"),
                feed_scope("y"),
            ]
        );
    }

    #[test]
    fn a_read_or_star_change_reloads_the_library_but_not_the_list() {
        let mut cache = Cached::default();
        cache.heard(Some(Changed::Flags));
        assert_eq!((cache.library, cache.list, cache.cleared), (1, 0, 0));
        for what in [Some(Changed::Feeds), Some(Changed::Articles), None] {
            cache.heard(what);
        }
        assert_eq!((cache.library, cache.list, cache.cleared), (4, 3, 0));
        cache.heard(Some(Changed::AllRead));
        assert_eq!((cache.library, cache.list, cache.cleared), (5, 4, 1));
    }

    #[test]
    fn pinned_feeds_are_taken_again_only_when_the_key_changes() {
        let key = |view, last_sync| {
            let selection = Selection {
                view,
                scope: Scope::All,
            };
            (Some("1".to_owned()), selection, last_sync)
        };
        let ids = |set: Rc<HashSet<String>>| {
            let mut ids: Vec<_> = set.iter().cloned().collect();
            ids.sort();
            ids
        };
        let mut pinned = Pinned::default();
        let mut counts = HashMap::from([("a".to_owned(), 2), ("b".to_owned(), 1)]);
        assert_eq!(
            ids(pinned.update(key(View::Unread, None), &counts)),
            ["a", "b"]
        );
        // Read to zero, "b" stays while the key is the same.
        counts.remove("b");
        assert_eq!(
            ids(pinned.update(key(View::Unread, None), &counts)),
            ["a", "b"]
        );
        // A finished sync takes them again, as a new view or selection does.
        assert_eq!(
            ids(pinned.update(key(View::Unread, Some(5)), &counts)),
            ["a"]
        );
        let starred = ids(pinned.update(key(View::Starred, Some(5)), &HashMap::new()));
        assert_eq!(starred, Vec::<String>::new());
    }

    #[test]
    fn the_arrow_keys_step_through_the_shown_rows_only() {
        let lib = Library {
            folders: vec![folder("d")],
            feeds: vec![
                feed("a", Some("d")),
                feed("b", Some("d")),
                feed("x", None),
                feed("y", None),
            ],
            ..Default::default()
        };
        let pinned = HashSet::from(["a".to_owned(), "y".to_owned()]);
        let none = HashSet::new();
        let step = |collapsed: &HashSet<String>, from: &Scope, delta| {
            step_scope(
                &lib,
                View::Unread,
                &pinned,
                collapsed,
                from,
                delta,
                SortSubscriptions::FoldersFirst,
            )
        };
        // "x" has no articles: "a" steps over it to "y".
        assert_eq!(step(&none, &feed_scope("a"), 1), Some(feed_scope("y")));
        assert_eq!(step(&none, &feed_scope("y"), -1), Some(feed_scope("a")));
        // A selected feed shows, so it has a row to step from.
        assert_eq!(step(&none, &feed_scope("x"), 1), Some(feed_scope("y")));
        assert_eq!(step(&none, &feed_scope("b"), -1), Some(feed_scope("a")));
        // Past either end, nothing.
        assert_eq!(step(&none, &Scope::All, -1), None);
        assert_eq!(step(&none, &feed_scope("y"), 1), None);
        // From a feed in a collapsed folder, the folder's row.
        let collapsed = HashSet::from(["d".to_owned()]);
        assert_eq!(step(&collapsed, &feed_scope("a"), 1), Some(feed_scope("y")));
        assert_eq!(step(&collapsed, &feed_scope("a"), -1), Some(Scope::All));
    }

    /// A library for the leaving rows: folder "d" with "a" and "b", folder "e" with
    /// "e1", and loose "c", "x" and "y". Before the mark, "a", "b", "e1" and "y" had
    /// unread articles; after it, "a" and "e1" have none. "x" was read to zero earlier and
    /// is still pinned; "c" had no row.
    fn leaving_rows(to: &Scope) -> Leaving {
        let before = Library {
            folders: vec![folder("d"), folder("e")],
            feeds: vec![
                feed("a", Some("d")),
                feed("b", Some("d")),
                feed("e1", Some("e")),
                feed("c", None),
                feed("x", None),
                feed("y", None),
            ],
            ..Default::default()
        };
        let mut shown = before.clone();
        for id in ["a", "b", "e1", "y"] {
            shown.counts.unread_by_feed.insert(id.into(), 1);
        }
        let mut after = before;
        after.counts.unread_by_feed.insert("b".into(), 1);
        after.counts.unread_by_feed.insert("y".into(), 2);
        let pinned = HashSet::from(["a", "b", "e1", "x", "y"].map(String::from));
        Leaving::after_mark(
            &shown,
            &after,
            "1",
            View::Unread,
            &pinned,
            &feed_scope("a"),
            to,
            SortSubscriptions::FoldersFirst,
        )
    }

    #[test]
    fn mark_all_read_lets_go_of_the_rows_it_emptied() {
        let leaving = leaving_rows(&feed_scope("b"));
        // "b" and "y" keep their rows, and so does "d" with "b" in it.
        assert_eq!(
            leaving.feeds,
            HashSet::from(["a", "e1", "x"].map(String::from))
        );
        // "e" goes with its last feed.
        assert_eq!(leaving.folders, HashSet::from(["e".to_owned()]));
    }

    #[test]
    fn mark_all_read_keeps_the_row_it_moves_to() {
        // A feed with no articles left.
        let leaving = leaving_rows(&feed_scope("x"));
        assert_eq!(leaving.feeds, HashSet::from(["a", "e1"].map(String::from)));
        // A folder whose feeds all go: they close, but it stays.
        let leaving = leaving_rows(&folder_scope("e"));
        assert_eq!(
            leaving.feeds,
            HashSet::from(["a", "e1", "x"].map(String::from))
        );
        assert!(leaving.folders.is_empty());
    }

    #[test]
    fn mark_all_read_moves_on_to_the_next_row_below_with_articles() {
        let mut lib = Library {
            folders: vec![folder("d"), folder("e")],
            feeds: vec![
                feed("a", Some("d")),
                feed("b", Some("d")),
                feed("c", Some("e")),
                feed("x", None),
                feed("y", None),
            ],
            ..Default::default()
        };
        // "a" and "b" were just marked read; both are still pinned.
        lib.counts.unread_by_feed.insert("c".into(), 1);
        lib.counts.unread_by_feed.insert("y".into(), 2);
        let pinned = HashSet::from(["a", "b", "c", "x", "y"].map(String::from));
        let none = HashSet::new();
        let next = |collapsed: &HashSet<String>, from: &Scope| {
            next_with_articles(
                &lib,
                View::Unread,
                &pinned,
                collapsed,
                from,
                SortSubscriptions::FoldersFirst,
            )
        };
        // Past "b", which has none: folder "e" comes first, as its feed has some.
        assert_eq!(next(&none, &feed_scope("a")), Some(folder_scope("e")));
        assert_eq!(next(&none, &folder_scope("d")), Some(folder_scope("e")));
        // Only rows below count: "x" has none, "y" has some.
        assert_eq!(next(&none, &feed_scope("c")), Some(feed_scope("y")));
        // None left below: All.
        assert_eq!(next(&none, &feed_scope("y")), Some(Scope::All));
        let collapsed = HashSet::from(["d".to_owned()]);
        // No row to move on from.
        assert_eq!(next(&none, &Scope::All), None);
        assert_eq!(next(&collapsed, &feed_scope("a")), None);
        // A collapsed folder's feeds have no rows: the folder below is next.
        assert_eq!(
            next(&collapsed, &folder_scope("d")),
            Some(folder_scope("e"))
        );
    }

    #[test]
    fn alphabetical_mixes_folders_and_loose_feeds() {
        // As the cache lists them: each list A–Z in NOCASE order. Folder titles are
        // their ids in capitals.
        let lib = Library {
            folders: vec![folder("b"), folder("news")],
            feeds: vec![
                feed("a", None),
                feed("inner", Some("news")),
                feed("News", None),
                feed("Zed", None),
                feed("été", None),
            ],
            ..Default::default()
        };
        let sort = SortSubscriptions::Alphabetical;
        let entries = sidebar_tree(&lib, View::All, &HashSet::new(), &Scope::All, sort);
        // The folder "NEWS" and the feed "News" tie: the folder comes first. Letters
        // outside ASCII aren't folded and sort after Z, as in SQLite.
        assert_eq!(
            sidebar_scopes(&entries, &HashSet::new()),
            [
                Scope::All,
                feed_scope("a"),
                folder_scope("b"),
                folder_scope("news"),
                feed_scope("inner"),
                feed_scope("News"),
                feed_scope("Zed"),
                feed_scope("été"),
            ]
        );
        // The arrow keys follow the shown order.
        let step = |from: &Scope| {
            let none = HashSet::new();
            step_scope(&lib, View::All, &none, &none, from, 1, sort)
        };
        assert_eq!(step(&feed_scope("a")), Some(folder_scope("b")));
        assert_eq!(step(&feed_scope("inner")), Some(feed_scope("News")));
    }
}
