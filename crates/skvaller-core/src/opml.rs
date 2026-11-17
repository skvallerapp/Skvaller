//! OPML, the file readers exchange subscriptions in. Any account's feeds can be
//! exported; the local account's can be imported, since a sync account's live on its
//! server.

use std::collections::HashMap;

use encoding_rs::{Encoding, UTF_8};
use opml::{Body, Head, OPML, Outline};
use reqwest::Url;
use serde::Serialize;

use crate::db::NewFeed;
use crate::{Db, Error, Result};

/// Deepest element nesting an import accepts. Folders in real files nest a few levels
/// at most.
const MAX_DEPTH: usize = 100;

/// Feeds one import adds at most. Readers export a few hundred.
const MAX_FEEDS: usize = 5000;

/// How deeply elements nest, from a real XML tokenizer, so comments, CDATA and `>` in
/// attribute values count right. Stops counting past `MAX_DEPTH`. A file that doesn't
/// tokenize gets 0: the parser refuses it anyway.
fn depth(xml: &str) -> usize {
    use xmlparser::{ElementEnd, Token, Tokenizer};
    let (mut depth, mut max) = (0usize, 0);
    for token in Tokenizer::from(xml) {
        match token {
            Ok(Token::ElementStart { .. }) => {
                depth += 1;
                max = max.max(depth);
                if max > MAX_DEPTH {
                    break;
                }
            }
            Ok(Token::ElementEnd {
                end: ElementEnd::Empty | ElementEnd::Close(..),
                ..
            }) => {
                depth = depth.saturating_sub(1);
            }
            Ok(_) => {}
            Err(_) => return 0,
        }
    }
    max
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    /// Feeds added.
    pub added: usize,
    /// Feeds subscribed to already, or with an address that isn't http(s).
    pub skipped: usize,
}

/// The account's subscriptions, in their folders, as an OPML 2.0 document.
///
/// # Errors
///
/// Returns `Error::Db` if the cache can't be read.
pub fn export(db: &Db, title: &str) -> Result<String> {
    let feeds: Vec<_> = db
        .feeds()?
        .into_iter()
        .filter(|f| f.subscribed && f.url.is_some())
        .collect();
    let outline = |f: &crate::Feed| Outline {
        text: f.title.clone(),
        title: Some(f.title.clone()),
        r#type: Some("rss".into()),
        xml_url: f.url.clone(),
        html_url: f.site_url.clone(),
        ..Outline::default()
    };
    let mut outlines: Vec<Outline> = Vec::new();
    for folder in db.folders()? {
        let inside: Vec<Outline> = feeds
            .iter()
            .filter(|f| f.folder_id.as_deref() == Some(folder.id.as_str()))
            .map(outline)
            .collect();
        if !inside.is_empty() {
            outlines.push(Outline {
                text: folder.title.clone(),
                title: Some(folder.title),
                outlines: inside,
                ..Outline::default()
            });
        }
    }
    outlines.extend(feeds.iter().filter(|f| f.folder_id.is_none()).map(outline));
    let document = OPML {
        version: "2.0".into(),
        head: Some(Head {
            title: Some(title.to_owned()),
            ..Head::default()
        }),
        body: Body { outlines },
    };
    document
        .to_string()
        .map_err(|e| Error::Local(format!("writing OPML failed: {e}")))
}

/// The text of an XML file: decoded by its byte order mark, else by the encoding its
/// `<?xml?>` declaration names, else as UTF-8.
fn decode(bytes: &[u8]) -> String {
    // `decode` lets a byte order mark win over the encoding given and strips it, and
    // replaces bytes that aren't the encoding's with U+FFFD.
    declared(bytes)
        .unwrap_or(UTF_8)
        .decode(bytes)
        .0
        .into_owned()
}

/// The encoding the `<?xml?>` declaration at the start of `bytes` names, if it names one.
fn declared(bytes: &[u8]) -> Option<&'static Encoding> {
    let rest = bytes.strip_prefix(b"<?xml")?;
    let end = rest.windows(2).position(|w| w == b"?>")?;
    let declaration = std::str::from_utf8(&rest[..end]).ok()?;
    let value = declaration.split_once("encoding")?.1.trim_start();
    let value = value.strip_prefix('=')?.trim_start();
    let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let name = value[1..].split(quote).next()?;
    // A declaration that reads as ASCII isn't in UTF-16: such a file is UTF-8.
    Encoding::for_label(name.as_bytes()).map(Encoding::output_encoding)
}

/// Adds the feeds of an OPML file, in their folders. Folders inside folders count as
/// the outermost one: Skvaller has one level. Feeds are not fetched here; the next
/// refresh does that.
///
/// The file is decoded by its byte order mark, else by the encoding its XML declaration
/// names, else as UTF-8: older readers export in ISO-8859-1 or UTF-16.
///
/// # Errors
///
/// Returns `Error::Local` for a file that isn't OPML or nests too deeply, and
/// `Error::Db` if the cache can't be written.
pub fn import(db: &Db, bytes: &[u8]) -> Result<ImportReport> {
    let xml = &decode(bytes);
    // The parser recurses per level: a crafted file nested thousands deep would
    // overflow the stack and end the app.
    if depth(xml) > MAX_DEPTH {
        return Err(Error::Local("the OPML file nests too deeply".into()));
    }
    #[expect(
        clippy::map_err_ignore,
        reason = "the parser's error is not worded for the user"
    )]
    let document = OPML::from_str(xml).map_err(|_| Error::Local("not an OPML file".into()))?;
    let mut import = Import {
        db,
        folders: HashMap::new(),
        report: ImportReport::default(),
    };
    for outline in &document.body.outlines {
        import.add(outline, None)?;
    }
    Ok(import.report)
}

struct Import<'a> {
    db: &'a Db,
    /// Folder IDs by title, made as the first feed goes in, so no folder is left empty.
    folders: HashMap<String, String>,
    report: ImportReport,
}

impl Import<'_> {
    /// Adds the outline's feed, or its feeds, into the folder titled `folder`.
    fn add(&mut self, outline: &Outline, folder: Option<&str>) -> Result<()> {
        let (db, report) = (self.db, &mut self.report);
        let name = || {
            outline
                .title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(&outline.text)
                .trim()
                .to_owned()
        };
        if let Some(xml_url) = &outline.xml_url {
            if report.added >= MAX_FEEDS {
                report.skipped += 1;
                return Ok(());
            }
            let Some(url) = Url::parse(xml_url.trim())
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https"))
            else {
                report.skipped += 1;
                return Ok(());
            };
            // Subscribed already, maybe by the address after a redirect.
            if db.feed_by_url(&[url.as_str(), xml_url.trim()])?.is_some() {
                report.skipped += 1;
                return Ok(());
            }
            let site = outline
                .html_url
                .as_deref()
                .and_then(|u| Url::parse(u).ok())
                .filter(|u| matches!(u.scheme(), "http" | "https"));
            let icon = site.as_ref().and_then(|s| s.join("/favicon.ico").ok());
            let title = Some(name()).filter(|t| !t.is_empty());
            let folder_id = folder
                .map(|folder| -> Result<String> {
                    if let Some(id) = self.folders.get(folder) {
                        return Ok(id.clone());
                    }
                    let id = db.create_folder(folder)?;
                    self.folders.insert(folder.to_owned(), id.clone());
                    Ok(id)
                })
                .transpose()?;
            let added = db.add_local_feed(&NewFeed {
                id: &format!("feed/{url}"),
                url: url.as_str(),
                title: title.as_deref().or(url.host_str()).unwrap_or(url.as_str()),
                site_url: site.as_ref().map(Url::as_str),
                folder_id: folder_id.as_deref(),
                icon_url: icon.as_ref().map(Url::as_str),
            })?;
            if added {
                self.report.added += 1;
            } else {
                self.report.skipped += 1;
            }
            return Ok(());
        }
        if outline.outlines.is_empty() {
            return Ok(());
        }
        let title = name();
        let inner = match folder {
            Some(folder) => Some(folder.to_owned()),
            None if title.is_empty() => None,
            None => Some(title),
        };
        for child in &outline.outlines {
            self.add(child, inner.as_deref())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
        <opml version="1.0"><head><title>Mine</title></head><body>
          <outline text="Tech" title="Tech">
            <outline type="rss" text="Kernel" xmlUrl="https://kernel.example/feed" htmlUrl="https://kernel.example/"/>
            <outline text="Deeper">
              <outline type="rss" text="Deep" xmlUrl="https://deep.example/rss"/>
            </outline>
          </outline>
          <outline type="rss" text="" xmlUrl="https://loose.example/atom.xml"/>
          <outline type="rss" text="Bad" xmlUrl="file:///etc/passwd"/>
        </body></opml>"#;

    #[test]
    fn importing_adds_feeds_in_one_level_of_folders() {
        let db = Db::open_in_memory().unwrap();

        let report = import(&db, DOC.as_bytes()).unwrap();

        assert_eq!(
            report,
            ImportReport {
                added: 3,
                skipped: 1
            }
        );
        let folders = db.folders().unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].title, "Tech");
        let feeds = db.feeds().unwrap();
        let folder_of = |url: &str| {
            feeds
                .iter()
                .find(|f| f.url.as_deref() == Some(url))
                .map(|f| (f.title.clone(), f.folder_id.clone()))
                .unwrap()
        };
        assert_eq!(
            folder_of("https://kernel.example/feed"),
            ("Kernel".into(), Some(folders[0].id.clone()))
        );
        assert_eq!(
            folder_of("https://deep.example/rss"),
            ("Deep".into(), Some(folders[0].id.clone()))
        );
        assert_eq!(
            folder_of("https://loose.example/atom.xml"),
            ("loose.example".into(), None)
        );
        assert_eq!(db.local_feeds().unwrap().len(), 3);

        // Again: nothing new.
        assert_eq!(
            import(&db, DOC.as_bytes()).unwrap(),
            ImportReport {
                added: 0,
                skipped: 4
            }
        );
        import(&db, b"<html></html>").unwrap_err();
    }

    #[test]
    #[expect(clippy::format_collect, reason = "test input, built once")]
    fn a_file_nested_too_deeply_is_refused_before_parsing() {
        let deep = format!(
            "<opml version=\"2.0\"><body>{}{}</body></opml>",
            "<outline text=\"x\">".repeat(5000),
            "</outline>".repeat(5000)
        );
        let db = Db::open_in_memory().unwrap();
        assert!(matches!(import(&db, deep.as_bytes()), Err(Error::Local(_))));
        // End tags in comments don't hide the depth.
        let hidden = format!(
            "<opml version=\"2.0\"><body>{}{}</body></opml>",
            "<outline text=\"x\"><!--</outline>-->".repeat(5000),
            "</outline>".repeat(5000)
        );
        assert!(matches!(
            import(&db, hidden.as_bytes()),
            Err(Error::Local(_))
        ));
        // A > in a value is no end of the tag.
        let arrows = format!(
            "<opml version=\"2.0\"><body>{}</body></opml>",
            (0..150)
                .map(|i| format!("<outline text=\"A > {i}\" xmlUrl=\"https://a.example/{i}\"/>"))
                .collect::<String>()
        );
        assert_eq!(import(&db, arrows.as_bytes()).unwrap().added, 150);

        // A folder only with a feed in it.
        let other = Db::open_in_memory().unwrap();
        let empty = r#"<opml version="2.0"><body><outline text="Empty"><outline text=""/></outline>
            <outline text="Bad"><outline xmlUrl="file:///x"/></outline></body></opml>"#;
        import(&other, empty.as_bytes()).unwrap();
        assert_eq!(other.folders().unwrap(), []);
        // Many feeds side by side are not deep.
        let flat = format!(
            "<opml version=\"2.0\"><body><outline text=\"F\">{}</outline></body></opml>",
            (0..500)
                .map(|i| format!("<outline text=\"{i}\" xmlUrl=\"https://e.example/{i}\"/>"))
                .collect::<String>()
        );
        assert_eq!(depth(&flat), 4);
        assert_eq!(import(&db, flat.as_bytes()).unwrap().added, 500);
    }

    #[test]
    fn an_export_imports_back_the_same() {
        let db = Db::open_in_memory().unwrap();
        import(&db, DOC.as_bytes()).unwrap();

        let xml = export(&db, "Local").unwrap();
        let other = Db::open_in_memory().unwrap();
        assert_eq!(
            import(&other, xml.as_bytes()).unwrap(),
            ImportReport {
                added: 3,
                skipped: 0
            }
        );

        let summary = |db: &Db| {
            let folders = db.folders().unwrap();
            let mut feeds: Vec<_> = db
                .feeds()
                .unwrap()
                .into_iter()
                .map(|f| {
                    let folder = f
                        .folder_id
                        .and_then(|id| folders.iter().find(|x| x.id == id))
                        .map(|x| x.title.clone());
                    (f.url, f.title, f.site_url, folder)
                })
                .collect();
            feeds.sort();
            feeds
        };
        assert_eq!(summary(&db), summary(&other));
        assert!(xml.contains("<title>Local</title>"));
    }

    #[test]
    fn a_file_is_read_in_the_encoding_it_declares() {
        let doc = |encoding: &str| {
            format!(
                "<?xml version=\"1.0\" encoding=\"{encoding}\"?><opml version=\"1.0\"><body>\
                 <outline text=\"Café Ölstuga\" xmlUrl=\"https://cafe.example/rss\"/></body></opml>"
            )
        };
        let title = |bytes: &[u8]| {
            let db = Db::open_in_memory().unwrap();
            assert_eq!(import(&db, bytes).unwrap().added, 1);
            db.feeds().unwrap().remove(0).title
        };

        let latin1 = encoding_rs::WINDOWS_1252
            .encode(&doc("ISO-8859-1"))
            .0
            .into_owned();
        assert_eq!(title(&latin1), "Café Ölstuga");

        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(doc("UTF-16").encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(title(&utf16), "Café Ölstuga");

        let utf8_bom = [b"\xEF\xBB\xBF".as_slice(), doc("UTF-8").as_bytes()].concat();
        assert_eq!(title(&utf8_bom), "Café Ölstuga");

        // No BOM: the declaration's UTF-16 can't be right, so it's UTF-8.
        assert_eq!(title(doc("UTF-16").as_bytes()), "Café Ölstuga");

        // Bytes that aren't UTF-8, undeclared: imported, with U+FFFD in their place.
        let undeclared = doc("ISO-8859-1").replace(r#" encoding="ISO-8859-1""#, "");
        let bad = encoding_rs::WINDOWS_1252.encode(&undeclared).0.into_owned();
        assert_eq!(title(&bad), "Caf\u{FFFD} \u{FFFD}lstuga");
    }

    #[test]
    fn the_declaration_names_the_encoding() {
        assert_eq!(
            declared(b"<?xml version='1.0' encoding='windows-1252'?>"),
            Some(encoding_rs::WINDOWS_1252)
        );
        assert_eq!(
            declared(b"<?xml version=\"1.0\" encoding = \"latin1\" ?><opml/>"),
            Some(encoding_rs::WINDOWS_1252)
        );
        assert_eq!(
            declared(b"<?xml version=\"1.0\"?><opml encoding=\"latin1\"/>"),
            None
        );
        assert_eq!(declared(b"<opml/>"), None);
    }
}
