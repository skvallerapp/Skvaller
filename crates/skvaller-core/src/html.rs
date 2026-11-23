//! Parsing untrusted HTML with a bound on how deeply it nests.
//!
//! html5ever, and Readability after it, slow down with the square of the nesting depth:
//! a crafted page tens of thousands of levels deep takes minutes, on a thread nothing can
//! stop. A guess at the depth from the text can always be fooled, since only the parser's
//! own rules (misnested formatting tags, tables, SVG, raw text and the rest) say what
//! nests in what. So the parser itself is the measure: the tree it builds is watched as it
//! grows, and parsing stops, a chunk at a time, once any node is deeper than allowed.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use dom_query::{Document, NodeId};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tokenizer::TokenizerOpts;
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeBuilderOpts, TreeSink};
use html5ever::{Attribute, ParseOpts, QualName, parse_document};

/// Deeper than any real page, which stays under a hundred or so. Readability's work
/// grows with nodes times depth: a hostile 2 MB page this deep takes it about 4 s,
/// against 10 s at 500.
pub(crate) const MAX_DEPTH: usize = 200;

/// The page is fed to the parser this much at a time; between pieces, a page found too
/// deep is given up on. The piece that crosses the limit can nest a few hundred levels
/// past it, and costs the square of that.
const CHUNK_BYTES: usize = 4 * 1024;

/// Attributes of repeated `<html>` and `<body>` tags that are merged into the first one.
/// Each is compared with all the element has, so a page of such tags would take minutes;
/// real pages repeat them a few times at most.
const MAX_MERGED_ATTRS: usize = 256;

/// `text` safe in HTML text and in quoted attributes.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// `html` as a document, or `None` if anything in it, template contents too, nests deeper
/// than `MAX_DEPTH`. Parses as `Document::from` does, with scripting off.
pub(crate) fn parse(html: &str) -> Option<Document> {
    let opts = ParseOpts {
        tokenizer: TokenizerOpts::default(),
        tree_builder: TreeBuilderOpts {
            scripting_enabled: false,
            ..Default::default()
        },
    };
    let mut parser = parse_document(Bounded::default(), opts);
    let mut start = 0;
    while start < html.len() {
        let mut end = (start + CHUNK_BYTES).min(html.len());
        while !html.is_char_boundary(end) {
            end += 1;
        }
        parser.process(StrTendril::from(&html[start..end]));
        if parser.tokenizer.sink.sink.too_deep.get() {
            return None;
        }
        start = end;
    }
    parser.finish()
}

/// How deeply the nodes of `doc` nest, the document itself at 0. Template contents count
/// as a child of their template.
fn depth(doc: &Document) -> usize {
    let mut max = 0;
    let mut stack = vec![(doc.root(), 0)];
    while let Some((node, d)) = stack.pop() {
        max = max.max(d);
        let contents = node.element_ref().and_then(|e| e.template_contents);
        if let Some(contents) = contents.and_then(|id| doc.tree.get(&id)) {
            stack.push((contents, d + 1));
        }
        let mut child = node.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            stack.push((c, d + 1));
        }
    }
    max
}

/// `dom_query`'s document, as html5ever builds it, noting when a node is attached deeper
/// than `MAX_DEPTH`.
#[derive(Default)]
struct Bounded {
    doc: Document,
    too_deep: Cell<bool>,
    /// Each template's contents, which `dom_query` keeps outside the tree, to its template.
    templates: RefCell<HashMap<NodeId, NodeId>>,
    merged_attrs: Cell<usize>,
}

impl Bounded {
    /// How many ancestors `node` has, up to `MAX_DEPTH`, counting through templates. The
    /// walk stops at the limit, so it costs no more than that however deep the page goes.
    fn ancestors(&self, node: NodeId) -> usize {
        let templates = self.templates.borrow();
        let mut count = 0;
        let mut top = node;
        while count < MAX_DEPTH {
            for id in self
                .doc
                .tree
                .ancestor_ids_of_it(&top, Some(MAX_DEPTH - count))
            {
                count += 1;
                top = id;
            }
            match templates.get(&top) {
                Some(template) if count < MAX_DEPTH => {
                    count += 1;
                    top = *template;
                }
                _ => break,
            }
        }
        count
    }

    /// Notes if a child of `parent` would be deeper than `MAX_DEPTH`.
    fn check(&self, parent: NodeId) {
        if !self.too_deep.get() && self.ancestors(parent) >= MAX_DEPTH {
            self.too_deep.set(true);
        }
    }
}

/// Everything goes to the document; the three ways of attaching a node check the depth
/// first. Moving nodes (`reparent_children`, or attaching one already in the tree) can
/// deepen what is below them unseen: nodes still open are measured again when the next
/// node is attached under them, and the finished tree is measured whole.
impl TreeSink for Bounded {
    type Handle = NodeId;
    type Output = Option<Document>;
    type ElemName<'a> = <Document as TreeSink>::ElemName<'a>;

    fn finish(self) -> Option<Document> {
        (!self.too_deep.get() && depth(&self.doc) <= MAX_DEPTH).then_some(self.doc)
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        self.doc.parse_error(msg);
    }

    fn get_document(&self) -> NodeId {
        self.doc.get_document()
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> Self::ElemName<'a> {
        self.doc.elem_name(target)
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> NodeId {
        let template = flags.template;
        let id = self.doc.create_element(name, attrs, flags);
        if template {
            let contents = self.doc.get_template_contents(&id);
            self.templates.borrow_mut().insert(contents, id);
        }
        id
    }

    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.doc.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        self.doc.create_pi(target, data)
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        self.check(*parent);
        self.doc.append(parent, child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        // The child goes under `element`'s parent, or else under `prev_element`.
        self.check(*element);
        self.check(*prev_element);
        self.doc
            .append_based_on_parent_node(element, prev_element, child);
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        self.doc
            .append_doctype_to_document(name, public_id, system_id);
    }

    fn mark_script_already_started(&self, node: &NodeId) {
        self.doc.mark_script_already_started(node);
    }

    fn pop(&self, node: &NodeId) {
        self.doc.pop(node);
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        self.doc.get_template_contents(target)
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        self.doc.same_node(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.doc.set_quirks_mode(mode);
    }

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        // A sibling's parent is the new node's parent.
        self.check(*sibling);
        self.doc.append_before_sibling(sibling, new_node);
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        let merged = self.merged_attrs.get() + attrs.len();
        if merged <= MAX_MERGED_ATTRS {
            self.merged_attrs.set(merged);
            self.doc.add_attrs_if_missing(target, attrs);
        }
    }

    fn associate_with_form(
        &self,
        target: &NodeId,
        form: &NodeId,
        nodes: (&NodeId, Option<&NodeId>),
    ) {
        self.doc.associate_with_form(target, form, nodes);
    }

    fn remove_from_parent(&self, target: &NodeId) {
        self.doc.remove_from_parent(target);
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        self.doc.reparent_children(node, new_parent);
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &NodeId) -> bool {
        self.doc.is_mathml_annotation_xml_integration_point(handle)
    }

    fn set_current_line(&self, line_number: u64) {
        self.doc.set_current_line(line_number);
    }

    fn allow_declarative_shadow_roots(&self, intended_parent: &NodeId) -> bool {
        self.doc.allow_declarative_shadow_roots(intended_parent)
    }

    fn attach_declarative_shadow(
        &self,
        location: &NodeId,
        template: &NodeId,
        attrs: &[Attribute],
    ) -> bool {
        self.doc
            .attach_declarative_shadow(location, template, attrs)
    }

    fn maybe_clone_an_option_into_selectedcontent(&self, option: &NodeId) {
        self.doc.maybe_clone_an_option_into_selectedcontent(option);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn a_normal_page_parses_as_dom_query_would() {
        let page = r#"<!doctype html><html><head><title>T</title><link rel="alternate" href="/f"></head>
            <body><article><h1>Hi</h1><p>One <b>two <i>three</b> four</i></p>
            <table><tr><td>cell</td></tr></table><svg><path/></svg><template><p>t</p></template>
            </article></body></html>"#;
        let ours = parse(page).expect("shallow");
        assert_eq!(ours.html(), Document::from(page).html());
        let article = include_str!("../testdata/article.html");
        assert_eq!(
            parse(article).expect("shallow").html(),
            Document::from(article).html()
        );
        // Across chunk boundaries, with multibyte text, too.
        let long = format!("<p>{}</p>", "Smörgåsbord är gött. ".repeat(3000));
        assert_eq!(
            parse(&long).unwrap().html(),
            Document::from(long.as_str()).html()
        );
    }

    #[test]
    fn crafted_pages_are_refused_quickly_however_they_nest() {
        let n = 20_000;
        let pages: Vec<(&str, String)> = vec![
            ("divs", "<div>".repeat(n)),
            ("attribute", r#"<div a="</div>">"#.repeat(n)),
            ("bogus end", "<div></ </div>".repeat(n)),
            ("noembed", "<div><noembed></div></noembed>".repeat(n)),
            ("noframes", "<div><noframes></div></noframes>".repeat(n)),
            (
                "scriptx",
                "<div><script></scriptx></div></script>".repeat(n),
            ),
            ("dashes", "<p-x>".repeat(n)),
            ("dashed end", "<span-a></span-b>".repeat(n)),
            ("ruby", "<rt>".repeat(n)),
            ("optgroup", "<optgroup>".repeat(n)),
            ("formatting", "<b><div></b>".repeat(n)),
            ("anchors", "<a><div></a>".repeat(n)),
            ("svg", format!("<svg>{}", "<g>".repeat(n))),
            ("svg self-closing", format!("<svg>{}", "<div/>".repeat(n))),
            (
                "foreign object",
                format!("<svg><foreignObject>{}", "<div/>".repeat(n)),
            ),
            ("tables", "<table><td>".repeat(n / 4)),
            ("comment", "<div><!--</div>-->".repeat(n)),
            ("bang comment", format!("<!-- --!>{}", "<div>".repeat(n))),
            ("cdata", "<div><![CDATA[</div>]]>".repeat(n)),
        ];
        // Generous for debug builds; a release build takes milliseconds each.
        let budget = Duration::from_secs(if cfg!(debug_assertions) { 3 } else { 1 });
        for (name, page) in pages {
            let started = Instant::now();
            assert!(parse(&page).is_none(), "{name} was parsed");
            assert!(
                started.elapsed() < budget,
                "{name} took {:?}",
                started.elapsed()
            );
        }
    }

    #[test]
    fn the_limit_is_on_depth_not_on_size() {
        // Wide is fine: a million siblings are no deeper than one.
        let wide = format!("<ul>{}</ul>", "<li>x</li>".repeat(50_000));
        assert!(parse(&wide).is_some());
        // The document is at 0, `<html>` at 1 and `<body>` at 2, so the deepest div is at
        // the limit.
        let deep = |levels: usize| "<div>".repeat(levels);
        let at_limit = parse(&deep(MAX_DEPTH - 2)).expect("at the limit");
        assert_eq!(depth(&at_limit), MAX_DEPTH);
        assert!(parse(&deep(MAX_DEPTH - 1)).is_none());
    }

    #[test]
    fn template_contents_count_toward_the_depth() {
        let deep = format!("<template>{}", "<div>".repeat(MAX_DEPTH));
        assert!(parse(&deep).is_none());
        assert!(parse(&"<template>".repeat(MAX_DEPTH)).is_none());
        let shallow = parse("<template><p>t</p></template>").expect("shallow");
        // Document, html, head, template, contents, p, text.
        assert_eq!(depth(&shallow), 6);
    }

    #[test]
    #[expect(clippy::format_collect, reason = "test input, built once")]
    fn repeated_html_and_body_tags_are_merged_only_so_far() {
        let page = format!(
            "<html a=1><body b=1><html a=2 c=1>{}",
            (0..100_000)
                .map(|i| format!("<body x{i}>"))
                .collect::<String>()
        );
        let started = Instant::now();
        let doc = parse(&page).expect("shallow");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        // The first merges go in as they would; the rest are dropped.
        let html = doc.select("html");
        assert_eq!(html.attr("a").as_deref(), Some("1"));
        assert_eq!(html.attr("c").as_deref(), Some("1"));
        let body = doc.select("body");
        assert!(body.has_attr("x0"));
        assert!(!body.has_attr("x99999"));
    }
}
