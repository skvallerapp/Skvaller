//! Sync against a fake server, shaped like FreshRSS unless a test says otherwise.

use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::Error;

const API: &str = "/api/greader.php";
const NOW: i64 = 1_800_000_000;

struct Fake {
    server: MockServer,
    client: Client,
    db: Db,
}

impl Fake {
    async fn new() -> Self {
        let server = MockServer::start().await;
        let client = Client::new(&format!("{}{API}", server.uri()), "tok").unwrap();
        Self {
            server,
            client,
            db: Db::open_in_memory().unwrap(),
        }
    }

    async fn mount(&self, mock: Mock) {
        mock.mount(&self.server).await;
    }

    fn get(&self, endpoint: &str) -> wiremock::MockBuilder {
        Mock::given(method("GET"))
            .and(path(format!("{API}/reader/api/0/{endpoint}")))
            .and(header("authorization", "GoogleLogin auth=tok"))
    }

    fn post(&self, endpoint: &str) -> wiremock::MockBuilder {
        Mock::given(method("POST"))
            .and(path(format!("{API}/reader/api/0/{endpoint}")))
            .and(header("authorization", "GoogleLogin auth=tok"))
    }

    /// Mounts the read endpoints of a sync. `recent` is what the reading list has newer
    /// than `ot`; `stream/items/contents` serves those items by ID.
    async fn serve(&self, ot: i64, recent: Vec<Value>, unread: &[i64], starred: &[i64]) {
        let subs = json!({ "subscriptions": [
            { "id": "feed/1", "title": "Kernel Notes", "iconUrl": "/f.php?h=1",
              "categories": [{ "id": "user/-/label/Tech", "label": "Tech" }] },
            { "id": "feed/2", "title": "Slow Mornings", "iconUrl": "/f.php?h=2",
              "categories": [] },
        ]});
        self.mount(self.get("subscription/list").respond_with(ok_json(subs)))
            .await;
        let recent_ids: Vec<Value> = recent.iter().map(|i| json!({ "id": i["id"] })).collect();
        self.mount(
            self.get("stream/items/ids")
                .and(query_param("s", READING_LIST))
                .and(query_param("ot", ot.to_string()))
                .respond_with(ok_json(json!({ "itemRefs": recent_ids }))),
        )
        .await;
        self.mount(
            self.post("stream/items/contents")
                .respond_with(ById(recent)),
        )
        .await;
        self.mount(
            self.get("stream/items/ids")
                .and(query_param("s", READING_LIST))
                .and(query_param("xt", READ))
                .respond_with(ok_json(item_refs(unread))),
        )
        .await;
        self.mount(
            self.get("stream/items/ids")
                .and(query_param("s", STARRED))
                .respond_with(ok_json(item_refs(starred))),
        )
        .await;
    }

    fn flags(&self, id: impl ToString) -> Option<(bool, bool)> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT read, starred FROM articles WHERE id = ?1",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()
        })
    }
}

fn ok_json(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

/// Answers `stream/items/contents` with the items the form asks for.
struct ById(Vec<Value>);

impl wiremock::Respond for ById {
    fn respond(&self, req: &wiremock::Request) -> ResponseTemplate {
        let body = String::from_utf8_lossy(&req.body);
        let asked = |item: &&Value| {
            let id = item["id"].as_str().unwrap();
            body.split('&').any(|field| field == form_field(id))
        };
        ok_json(json!({ "items": self.0.iter().filter(asked).collect::<Vec<_>>() }))
    }
}

/// How `i=<long ID>` looks in a form body.
fn form_field(long_id: &str) -> String {
    let encoded = long_id
        .replace(':', "%3A")
        .replace(',', "%2C")
        .replace('/', "%2F");
    format!("i={encoded}")
}

fn item_refs(ids: &[i64]) -> Value {
    json!({ "itemRefs": ids.iter().map(|id| json!({ "id": id.to_string() })).collect::<Vec<_>>() })
}

/// An item as FreshRSS returns it. `crawl` is Unix seconds.
fn item(id: i64, feed: &str, crawl: i64, read: bool, starred: bool) -> Value {
    let mut categories = vec![READING_LIST.to_owned()];
    if read {
        categories.push(READ.to_owned());
    }
    if starred {
        categories.push(STARRED.to_owned());
    }
    json!({
        "id": format!("tag:google.com,2005:reader/item/{id:016x}"),
        "crawlTimeMsec": (crawl * 1000).to_string(),
        "timestampUsec": (crawl * 1_000_000).to_string(),
        "published": crawl,
        "title": format!("Item {id}"),
        "author": "Ann",
        "canonical": [{ "href": format!("https://example.com/{id}") }],
        "alternate": [{ "href": format!("https://example.com/{id}"), "type": "text/html" }],
        "summary": { "content": "<p>Hello</p>" },
        "origin": { "streamId": feed, "title": "A feed", "htmlUrl": "https://example.com" },
        "categories": categories,
    })
}

/// How `i=<id>` looks in a form body.
fn form_id(id: i64) -> String {
    form_field(&format!("tag:google.com,2005:reader/item/{id:016x}"))
}

#[tokio::test]
async fn login_reads_the_auth_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{API}/accounts/ClientLogin")))
        .and(body_string_contains("Email=carl"))
        .and(body_string_contains("Passwd=s3cret"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("SID=carl/abc\nLSID=null\nAuth=carl/abc\n"),
        )
        .mount(&server)
        .await;
    let client = Client::login(&format!("{}{API}/", server.uri()), "carl", "s3cret")
        .await
        .unwrap();
    assert_eq!(client.token(), "carl/abc");
}

#[tokio::test]
async fn login_with_a_wrong_password_is_unauthorized() {
    for status in [401, 403] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_string("Unauthorized!"))
            .mount(&server)
            .await;
        let err = Client::login(&format!("{}{API}", server.uri()), "carl", "nope").await;
        assert!(matches!(err, Err(Error::Unauthorized)), "HTTP {status}");
    }
}

#[tokio::test]
async fn login_tries_the_address_as_typed_first() {
    // Miniflux and The Old Reader: the API is at the server's root.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/accounts/ClientLogin"))
        .respond_with(ResponseTemplate::new(200).set_body_string("Auth=root\n"))
        .mount(&server)
        .await;
    let client = Client::login(&format!("{}/", server.uri()), "carl", "pw")
        .await
        .unwrap();
    assert_eq!(
        (client.base(), client.token()),
        (server.uri().as_str(), "root")
    );
}

#[tokio::test]
async fn login_falls_back_to_the_freshrss_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{API}/accounts/ClientLogin")))
        .respond_with(ResponseTemplate::new(200).set_body_string("Auth=frss\n"))
        .mount(&server)
        .await;
    // FreshRSS's own page, not the API.
    Mock::given(method("POST"))
        .and(path("/accounts/ClientLogin"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;
    let client = Client::login(&server.uri(), "carl", "pw").await.unwrap();
    assert_eq!(client.base(), format!("{}{API}", server.uri()));
    assert_eq!(client.token(), "frss");
}

#[tokio::test]
async fn login_rejected_at_the_address_as_typed_tries_nothing_else() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/accounts/ClientLogin"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{API}/accounts/ClientLogin")))
        .respond_with(ResponseTemplate::new(200).set_body_string("Auth=x\n"))
        .expect(0)
        .mount(&server)
        .await;
    let err = Client::login(&server.uri(), "carl", "nope").await;
    assert!(matches!(err, Err(Error::Unauthorized)));
}

#[tokio::test]
async fn login_failing_at_the_address_as_typed_says_why() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/accounts/ClientLogin"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{API}/accounts/ClientLogin")))
        .respond_with(ResponseTemplate::new(404))
        .expect(0)
        .mount(&server)
        .await;
    let err = Client::login(&server.uri(), "carl", "pw").await;
    assert!(matches!(err, Err(Error::Protocol(m)) if m.contains("503")));
}

#[tokio::test]
async fn login_without_an_api_at_either_address_says_so() {
    let server = MockServer::start().await;
    let err = Client::login(&server.uri(), "carl", "pw").await;
    assert!(matches!(err, Err(Error::Protocol(m)) if m == "no Google Reader API at this address"));
}

#[tokio::test]
async fn login_names_where_the_server_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("location", "https://rss.example.com/"),
        )
        .mount(&server)
        .await;
    let err = Client::login(&server.uri(), "carl", "pw").await;
    assert!(
        matches!(err, Err(Error::Protocol(m)) if m.ends_with("it redirects to https://rss.example.com/"))
    );
}

#[tokio::test]
async fn revoked_token_is_unauthorized() {
    let fake = Fake::new().await;
    fake.mount(Mock::given(method("GET")).respond_with(ResponseTemplate::new(401)))
        .await;
    assert!(matches!(
        sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS).await,
        Err(Error::Unauthorized)
    ));
}

#[tokio::test]
async fn first_sync_fetches_unread_starred_and_the_last_30_days() {
    let fake = Fake::new().await;
    let recent = NOW - DAY;
    fake.serve(
        NOW - 30 * DAY,
        vec![
            item(11, "feed/2", recent, false, false),
            item(10, "feed/1", recent, true, false),
        ],
        &[11, 5],
        &[3],
    )
    .await;
    // Unread 5 and starred 3 are older than 30 days: only their IDs are listed as such.
    let old = NOW - 400 * DAY;
    fake.mount(
        fake.post("stream/items/contents")
            .and(body_string_contains(form_id(5)))
            .and(body_string_contains(form_id(3)))
            .and(body_string_contains(form_id(11)))
            .respond_with(ById(vec![
                item(11, "feed/2", recent, false, false),
                item(10, "feed/1", recent, true, false),
                item(5, "feed/1", old, false, false),
                item(3, "feed/2", old, true, true),
            ]))
            .with_priority(1)
            .expect(1),
    )
    .await;

    let report = sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    assert_eq!(
        report,
        SyncReport {
            pushed: 0,
            fetched: 4,
            changed: 0,
            pruned: 0
        }
    );
    assert_eq!(fake.flags(3), Some((true, true)));
    assert_eq!(fake.flags(5), Some((false, false)));
    assert_eq!(fake.flags(10), Some((true, false)));
    assert_eq!(fake.flags(11), Some((false, false)));
    let stored = fake.db.with_conn(|c| {
        c.query_row(
            "SELECT a.title, a.author, a.url, a.published, h.html, f.title, d.title
             FROM articles a JOIN article_html h ON h.article_id = a.id
             JOIN feeds f ON f.id = a.feed_id JOIN folders d ON d.id = f.folder_id
             WHERE a.id = 10",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            },
        )
        .unwrap()
    });
    assert_eq!(
        stored,
        (
            "Item 10".into(),
            "Ann".into(),
            "https://example.com/10".into(),
            recent * 1000,
            "<p>Hello</p>".into(),
            "Kernel Notes".into(),
            "Tech".into()
        )
    );
    assert_eq!(fake.db.newest_crawl().unwrap(), Some(recent * 1000));
}

#[tokio::test]
async fn next_sync_fetches_since_the_newest_crawl_and_follows_server_state() {
    let fake = Fake::new().await;
    let crawl = NOW - DAY;
    fake.serve(
        NOW - 30 * DAY,
        vec![
            item(11, "feed/2", crawl, false, false),
            item(10, "feed/1", crawl, true, false),
        ],
        &[11],
        &[],
    )
    .await;
    sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    // Elsewhere, 11 was read and 10 starred. One new article arrived, and the overlap
    // lists 11 again: only the new one is fetched.
    fake.server.reset().await;
    fake.serve(
        crawl - CRAWL_OVERLAP,
        vec![
            item(12, "feed/1", crawl + 60, false, false),
            item(11, "feed/2", crawl, true, false),
        ],
        &[12],
        &[10],
    )
    .await;
    fake.mount(
        fake.post("stream/items/contents")
            .and(body_string_contains(form_id(11)))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(1)
            .expect(0),
    )
    .await;

    let report = sync(&fake.client, &fake.db, NOW + 900, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    assert_eq!(
        report,
        SyncReport {
            pushed: 0,
            fetched: 1,
            changed: 2,
            pruned: 0
        }
    );
    assert_eq!(fake.flags(10), Some((true, true)));
    assert_eq!(fake.flags(11), Some((true, false)));
    assert_eq!(fake.flags(12), Some((false, false)));
}

#[tokio::test]
async fn queued_changes_are_sent_then_cleared() {
    let fake = Fake::new().await;
    let crawl = NOW - DAY;
    fake.serve(
        NOW - 30 * DAY,
        vec![
            item(1, "feed/1", crawl, false, false),
            item(2, "feed/1", crawl, true, true),
        ],
        &[1],
        &[2],
    )
    .await;
    sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();
    fake.db.set_read(&["1"], true).unwrap();
    fake.db.set_starred(&["2"], false).unwrap();

    fake.server.reset().await;
    fake.serve(crawl - CRAWL_OVERLAP, vec![], &[], &[]).await;
    fake.mount(
        fake.get("token")
            .respond_with(ResponseTemplate::new(200).set_body_string("wtok\n")),
    )
    .await;
    fake.mount(
        fake.post("edit-tag")
            .and(body_string_contains(form_id(1)))
            .and(body_string_contains(
                "a=user%2F-%2Fstate%2Fcom.google%2Fread",
            ))
            .and(body_string_contains("T=wtok"))
            .respond_with(ResponseTemplate::new(200).set_body_string("OK"))
            .expect(1),
    )
    .await;
    fake.mount(
        fake.post("edit-tag")
            .and(body_string_contains(form_id(2)))
            .and(body_string_contains(
                "r=user%2F-%2Fstate%2Fcom.google%2Fstarred",
            ))
            .and(body_string_contains("T=wtok"))
            .respond_with(ResponseTemplate::new(200).set_body_string("OK"))
            .expect(1),
    )
    .await;

    let report = sync(&fake.client, &fake.db, NOW + 900, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    assert_eq!(
        report,
        SyncReport {
            pushed: 2,
            fetched: 0,
            changed: 0,
            pruned: 0
        }
    );
    assert!(fake.db.pending().unwrap().is_empty());
    assert_eq!(fake.flags(1), Some((true, false)));
    assert_eq!(fake.flags(2), Some((true, false)));
}

#[tokio::test]
async fn failed_push_keeps_the_queue_and_the_local_state() {
    let fake = Fake::new().await;
    let crawl = NOW - DAY;
    fake.serve(
        NOW - 30 * DAY,
        vec![item(1, "feed/1", crawl, false, false)],
        &[1],
        &[],
    )
    .await;
    sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();
    fake.db.set_read(&["1"], true).unwrap();

    fake.server.reset().await;
    fake.serve(
        crawl - CRAWL_OVERLAP,
        vec![
            item(1, "feed/1", crawl, false, false),
            item(2, "feed/1", crawl, false, false),
        ],
        &[1, 2],
        &[],
    )
    .await;
    fake.mount(
        fake.get("token")
            .respond_with(ResponseTemplate::new(200).set_body_string("wtok")),
    )
    .await;
    fake.mount(
        fake.post("edit-tag")
            .respond_with(ResponseTemplate::new(503)),
    )
    .await;

    assert!(
        sync(&fake.client, &fake.db, NOW + 900, DEFAULT_KEEP_READ_DAYS)
            .await
            .is_err()
    );
    assert_eq!(
        fake.db.pending().unwrap(),
        [("1".to_owned(), Flag::Read, true)]
    );
    assert_eq!(fake.flags(1), Some((true, false)));
    // The pull went ahead anyway.
    assert_eq!(fake.flags(2), Some((false, false)));
}

#[tokio::test]
async fn forbidden_outside_sign_in_is_not_unauthorized() {
    let fake = Fake::new().await;
    fake.mount(Mock::given(method("GET")).respond_with(ResponseTemplate::new(403)))
        .await;
    let result = sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS).await;
    assert!(matches!(result, Err(Error::Protocol(_))));
}

#[tokio::test]
async fn malformed_items_are_skipped_and_missing_dates_use_the_crawl_time() {
    let fake = Fake::new().await;
    let crawl = NOW - DAY;
    let mut undated = item(2, "feed/1", crawl, false, false);
    undated["published"] = Value::Null;
    let mut broken = item(3, "feed/1", crawl, false, false);
    broken["origin"] = Value::Null;
    fake.serve(
        NOW - 30 * DAY,
        vec![item(1, "feed/1", crawl, false, false), undated, broken],
        &[1, 2, 3],
        &[],
    )
    .await;

    let report = sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    assert_eq!(report.fetched, 2);
    let published: i64 = fake.db.with_conn(|c| {
        c.query_row("SELECT published FROM articles WHERE id = '2'", [], |r| {
            r.get(0)
        })
        .unwrap()
    });
    assert_eq!(published, crawl * 1000);
}

// Live tests against a real account:
// `SKVALLER_TEST_URL=https://rss.example.com/api/greader.php SKVALLER_TEST_USER=…
// SKVALLER_TEST_PASSWORD=… cargo test -p skvaller-core live_ -- --ignored --nocapture`

async fn live_client() -> Client {
    let var = |name| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
    Client::login(
        &var("SKVALLER_TEST_URL"),
        &var("SKVALLER_TEST_USER"),
        &var("SKVALLER_TEST_PASSWORD"),
    )
    .await
    .unwrap()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Syncs twice. Read-only: nothing is queued, so nothing is sent.
#[tokio::test]
#[ignore = "needs a live server"]
async fn live_read() {
    let client = live_client().await;
    let db = Db::open_in_memory().unwrap();

    let start = std::time::Instant::now();
    let first = sync(&client, &db, unix_now(), DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();
    println!("first sync: {first:?} in {:?}", start.elapsed());
    let start = std::time::Instant::now();
    let second = sync(&client, &db, unix_now(), DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();
    println!("second sync: {second:?} in {:?}", start.elapsed());

    let counts: (i64, i64, i64, i64, i64) = db.with_conn(|c| {
        c.query_row(
            "SELECT (SELECT count(*) FROM folders), (SELECT count(*) FROM feeds),
                    (SELECT count(*) FROM articles), (SELECT count(*) FROM articles WHERE NOT read),
                    (SELECT count(*) FROM articles WHERE starred)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
    });
    println!("folders, feeds, articles, unread, starred: {counts:?}");
}

/// Marks one unread, unstarred article read and starred, checks the server, then undoes
/// both and checks again. Leaves the account as it was unless it fails halfway.
#[tokio::test]
#[ignore = "needs a live server; changes one article and restores it"]
async fn live_write() {
    let client = live_client().await;
    let db = Db::open_in_memory().unwrap();
    sync(&client, &db, unix_now(), DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();
    let id: String = db.with_conn(|c| {
        c.query_row(
            "SELECT id FROM articles WHERE NOT read AND NOT starred LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("needs an unread, unstarred article")
    });
    let on_server = async || {
        let unread = client
            .item_ids(READING_LIST, Some(READ), None)
            .await
            .unwrap();
        let starred = client.item_ids(STARRED, None, None).await.unwrap();
        (!unread.contains(&id), starred.contains(&id))
    };
    println!("article {id}");

    db.set_read(&[&id], true).unwrap();
    db.set_starred(&[&id], true).unwrap();
    let report = sync(&client, &db, unix_now(), DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();
    assert_eq!(report.pushed, 2);
    assert_eq!(
        on_server().await,
        (true, true),
        "read and starred on the server"
    );

    db.set_read(&[&id], false).unwrap();
    db.set_starred(&[&id], false).unwrap();
    let report = sync(&client, &db, unix_now(), DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();
    assert_eq!(report.pushed, 2);
    assert_eq!(on_server().await, (false, false), "restored on the server");
    assert!(db.pending().unwrap().is_empty());
}

#[tokio::test]
async fn pages_are_followed_and_a_repeated_continuation_stops() {
    let fake = Fake::new().await;
    let crawl = NOW - DAY;
    let items = vec![
        item(1, "feed/1", crawl, true, false),
        item(2, "feed/1", crawl, true, false),
    ];
    // Page 1 points to page 2; page 2 wrongly repeats its own continuation.
    fake.serve(NOW - 30 * DAY, items, &[], &[]).await;
    fake.mount(
        fake.get("stream/items/ids")
            .and(query_param("s", READING_LIST))
            .and(query_param("c", "p2"))
            .respond_with(ok_json(json!({
                "itemRefs": [{ "id": "1" }], "continuation": "p2" })))
            .with_priority(1)
            .expect(1),
    )
    .await;
    fake.mount(
        fake.get("stream/items/ids")
            .and(query_param("s", READING_LIST))
            .and(query_param("ot", (NOW - 30 * DAY).to_string()))
            .and(wiremock::matchers::query_param_is_missing("c"))
            .respond_with(ok_json(json!({
                "itemRefs": [{ "id": "2" }], "continuation": "p2" })))
            .with_priority(1)
            .expect(1),
    )
    .await;

    let report = sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    assert_eq!(report.fetched, 2);
}

#[tokio::test]
async fn feed_only_feeds_are_fetched_on_their_own() {
    let fake = Fake::new().await;
    let crawl = NOW - DAY;
    fake.serve(NOW - 30 * DAY, vec![], &[], &[]).await;
    fake.mount(
        fake.get("subscription/list")
            .respond_with(ok_json(json!({ "subscriptions": [
                { "id": "feed/1", "title": "Kernel Notes", "categories": [] },
                { "id": "feed/3", "title": "Noisy", "categories": [], "frss:priority": "feed" },
            ]})))
            .with_priority(1),
    )
    .await;
    fake.mount(
        fake.get("stream/items/ids")
            .and(query_param("s", "feed/3"))
            .respond_with(ok_json(item_refs(&[30]))),
    )
    .await;
    fake.mount(
        fake.post("stream/items/contents")
            .respond_with(ById(vec![item(30, "feed/3", crawl, false, false)]))
            .with_priority(1),
    )
    .await;

    sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    // Still unread after reconcile, though the reading list does not include it.
    assert_eq!(fake.flags(30), Some((false, false)));
}

#[tokio::test]
async fn first_sync_window_follows_retention() {
    let fake = Fake::new().await;
    // Only matches if the first sync asks for 7 days.
    fake.serve(
        NOW - 7 * DAY,
        vec![item(2, "feed/1", NOW - DAY, true, false)],
        &[],
        &[],
    )
    .await;

    let report = sync(&fake.client, &fake.db, NOW, 7).await.unwrap();

    assert_eq!(report.fetched, 1);
}

#[tokio::test]
async fn old_read_articles_are_pruned() {
    let fake = Fake::new().await;
    let crawl = NOW - DAY;
    fake.serve(
        NOW - 30 * DAY,
        vec![
            item(1, "feed/1", NOW - 10 * DAY, true, false),
            item(2, "feed/1", crawl, true, false),
        ],
        &[],
        &[],
    )
    .await;
    sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    // The user shortens retention to 7 days.
    fake.server.reset().await;
    fake.serve(crawl - CRAWL_OVERLAP, vec![], &[], &[]).await;
    let report = sync(&fake.client, &fake.db, NOW, 7).await.unwrap();

    assert_eq!(report.pruned, 1);
    assert_eq!(fake.flags(1), None);
    assert_eq!(fake.flags(2), Some((true, false)));
}

#[tokio::test]
async fn feed_icons_are_fetched_once() {
    let fake = Fake::new().await;
    fake.serve(NOW - 30 * DAY, vec![], &[], &[]).await;
    let png = b"\x89PNG fake".to_vec();
    fake.mount(
        Mock::given(path("/f.php"))
            .and(query_param("h", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(png, "image/png"))
            .expect(1),
    )
    .await;
    // Not an image: stored as no icon, and not asked for again.
    fake.mount(
        Mock::given(path("/f.php"))
            .and(query_param("h", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html>", "text/html"))
            .expect(1),
    )
    .await;

    sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();
    sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    let icons: Vec<_> = fake
        .db
        .feeds()
        .unwrap()
        .into_iter()
        .map(|f| (f.id, f.icon))
        .collect();
    assert_eq!(
        icons,
        [
            (
                "feed/1".to_owned(),
                Some("data:image/png;base64,iVBORyBmYWtl".to_owned())
            ),
            ("feed/2".to_owned(), None),
        ]
    );
}

#[tokio::test]
async fn failed_icon_fetch_is_retried() {
    let fake = Fake::new().await;
    fake.serve(NOW - 30 * DAY, vec![], &[], &[]).await;
    fake.mount(
        Mock::given(path("/f.php"))
            .respond_with(ResponseTemplate::new(503))
            .expect(4),
    )
    .await;

    for _ in 0..2 {
        sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
            .await
            .unwrap();
    }

    assert!(fake.db.feeds().unwrap().iter().all(|f| f.icon.is_none()));
}

/// The Old Reader: 24-digit hex IDs, short in ID lists and long in articles, and no
/// `crawlTimeMsec`.
#[tokio::test]
async fn ids_longer_than_64_bits_sync_both_ways() {
    let fake = Fake::new().await;
    let (a, b) = ("5d0cfa30d1892a8d3f00002e", "5d0cfa30d1892a8d3f00002f");
    let crawl = NOW - DAY;
    let tor_item = |hex: &str, read: bool| {
        let mut v = item(0, "feed/5d0c", crawl, read, false);
        v["id"] = json!(format!("tag:google.com,2005:reader/item/{hex}"));
        v.as_object_mut().unwrap().remove("crawlTimeMsec");
        v
    };
    fake.serve(
        NOW - 30 * DAY,
        vec![tor_item(a, false), tor_item(b, true)],
        &[],
        &[],
    )
    .await;
    fake.mount(
        fake.get("stream/items/ids")
            .and(query_param("xt", READ))
            .respond_with(ok_json(json!({ "itemRefs": [{ "id": a }] })))
            .with_priority(1),
    )
    .await;

    let report = sync(&fake.client, &fake.db, NOW, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();

    assert_eq!(report.fetched, 2);
    assert_eq!(fake.flags(format!("x{a}")), Some((false, false)));
    assert_eq!(fake.flags(format!("x{b}")), Some((true, false)));
    // From timestampUsec.
    assert_eq!(fake.db.newest_crawl().unwrap(), Some(crawl * 1000));

    fake.db.set_read(&[format!("x{a}")], true).unwrap();
    fake.mount(
        fake.get("stream/items/ids")
            .and(query_param("ot", (crawl - CRAWL_OVERLAP).to_string()))
            .respond_with(ok_json(json!({ "itemRefs": [] }))),
    )
    .await;
    fake.mount(
        fake.get("token")
            .respond_with(ResponseTemplate::new(200).set_body_string("wtok")),
    )
    .await;
    fake.mount(
        fake.post("edit-tag")
            .and(body_string_contains(form_field(&format!(
                "tag:google.com,2005:reader/item/{a}"
            ))))
            .respond_with(ResponseTemplate::new(200).set_body_string("OK"))
            .expect(1),
    )
    .await;
    let report = sync(&fake.client, &fake.db, NOW + 900, DEFAULT_KEEP_READ_DAYS)
        .await
        .unwrap();
    assert_eq!(report.pushed, 1);
}
