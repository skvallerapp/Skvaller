//! OPML, the file readers exchange subscriptions in. Any account's feeds can be
//! exported; the local account's can be imported, since a sync account's live on its
//! server.

use opml::{Body, Head, OPML, Outline};
use reqwest::Url;
use serde::Serialize;

use crate::db::NewFeed;
use crate::{Db, Error, Result};

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    /// Feeds added.
    pub added: usize,
    /// Feeds subscribed to already, or with an address that isn't http(s).
    pub skipped: usize,
}

/// The account's subscriptions, in their folders, as an OPML 2.0 document.
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
        .map_err(|e| Error::Protocol(format!("writing OPML failed: {e}")))
}

/// Adds the feeds of an OPML document, in their folders. Folders inside folders count
/// as the outermost one: Skvaller has one level. Feeds are not fetched here; the next
/// refresh does that.
pub fn import(db: &Db, xml: &str) -> Result<ImportReport> {
    let document = OPML::from_str(xml).map_err(|_| Error::Protocol("not an OPML file".into()))?;
    let mut report = ImportReport::default();
    for outline in &document.body.outlines {
        add(db, outline, None, &mut report)?;
    }
    Ok(report)
}

fn add(db: &Db, outline: &Outline, folder: Option<&str>, report: &mut ImportReport) -> Result<()> {
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
        let Some(url) = Url::parse(xml_url.trim())
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
        else {
            report.skipped += 1;
            return Ok(());
        };
        let site = outline.html_url.as_deref().and_then(|u| Url::parse(u).ok());
        let icon = site.as_ref().and_then(|s| s.join("/favicon.ico").ok());
        let title = Some(name()).filter(|t| !t.is_empty());
        let added = db.add_local_feed(&NewFeed {
            id: &format!("feed/{url}"),
            url: url.as_str(),
            title: title.as_deref().or(url.host_str()).unwrap_or(url.as_str()),
            site_url: site.as_ref().map(Url::as_str),
            folder_id: folder,
            icon_url: icon.as_ref().map(Url::as_str),
        })?;
        if added {
            report.added += 1;
        } else {
            report.skipped += 1;
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
        None => Some(db.create_folder(&title)?),
    };
    for child in &outline.outlines {
        add(db, child, inner.as_deref(), report)?;
    }
    Ok(())
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

        let report = import(&db, DOC).unwrap();

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
            import(&db, DOC).unwrap(),
            ImportReport {
                added: 0,
                skipped: 4
            }
        );
        assert!(import(&db, "<html></html>").is_err());
    }

    #[test]
    fn an_export_imports_back_the_same() {
        let db = Db::open_in_memory().unwrap();
        import(&db, DOC).unwrap();

        let xml = export(&db, "On This Device").unwrap();
        let other = Db::open_in_memory().unwrap();
        assert_eq!(
            import(&other, &xml).unwrap(),
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
        assert!(xml.contains("<title>On This Device</title>"));
    }
}
