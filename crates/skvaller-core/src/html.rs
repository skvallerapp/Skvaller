//! Parsing untrusted HTML with a bound on how deeply it nests.
//!
//! html5ever, and Readability after it, slow down with the square of the nesting depth:
//! a crafted page tens of thousands of levels deep takes minutes, on a thread nothing can
//! stop. A guess at the depth from the text can always be fooled, since only the parser's
//! own rules (misnested formatting tags, tables, SVG, raw text and the rest) say what
//! nests in what. So the parser itself is the measure: the tree it builds is watched as it
//! grows, and parsing stops, a chunk at a time, once any node is deeper than allowed.

use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use dom_query::{Document, NodeId};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tokenizer::TokenizerOpts;
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeBuilderOpts, TreeSink};
use html5ever::{Attribute, ParseOpts, QualName, parse_document};

/// Deeper than any real page, which stays under a hundred or so; shallow enough that a
/// tree this deep parses, and Readability reads it, in good time.
pub(crate) const MAX_DEPTH: usize = 500;

/// The page is fed to the parser this much at a time; between pieces, a page found too
/// deep is given up on. Small enough that the piece after the limit costs milliseconds.
const CHUNK_BYTES: usize = 16 * 1024;

/// `html` as a document, or `None` if anything in it nests deeper than `max_depth`.
/// Parses as `Document::from` does, with scripting off.
pub(crate) fn parse(html: &str, max_depth: usize) -> Option<Document> {
    let too_deep = Rc::new(Cell::new(false));
    let sink = Bounded {
        doc: Document::default(),
        max_depth,
        too_deep: too_deep.clone(),
    };
    let opts = ParseOpts {
        tokenizer: TokenizerOpts::default(),
        tree_builder: TreeBuilderOpts {
            scripting_enabled: false,
            ..Default::default()
        },
    };
    let mut parser = parse_document(sink, opts);
    let mut start = 0;
    while start < html.len() {
        let mut end = (start + CHUNK_BYTES).min(html.len());
        while !html.is_char_boundary(end) {
            end += 1;
        }
        parser.process(StrTendril::from(&html[start..end]));
        if too_deep.get() {
            return None;
        }
        start = end;
    }
    let doc = parser.finish();
    (!too_deep.get()).then_some(doc)
}

/// dom_query's document, as html5ever builds it, noting when a node is attached deeper
/// than `max_depth`.
struct Bounded {
    doc: Document,
    max_depth: usize,
    too_deep: Rc<Cell<bool>>,
}

impl Bounded {
    /// Whether a child of `parent` is too deep. The walk up stops at the limit, so it
    /// costs no more than that however deep the page tries to go.
    fn check(&self, parent: &NodeId) {
        if self.too_deep.get() {
            return;
        }
        let ancestors = self
            .doc
            .tree
            .ancestor_ids_of_it(parent, Some(self.max_depth))
            .count();
        if ancestors + 1 >= self.max_depth {
            self.too_deep.set(true);
        }
    }
}

/// Everything goes to the document; the four ways of attaching a node check the depth
/// first. Moving children (`reparent_children`) can deepen nodes already there; the next
/// node attached below them measures their true depth.
impl TreeSink for Bounded {
    type Handle = NodeId;
    type Output = Document;
    type ElemName<'a> = <Document as TreeSink>::ElemName<'a>;

    fn finish(self) -> Document {
        self.doc
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
        self.doc.create_element(name, attrs, flags)
    }

    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.doc.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        self.doc.create_pi(target, data)
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        self.check(parent);
        self.doc.append(parent, child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        // The child goes under `element`'s parent, or else under `prev_element`.
        self.check(element);
        self.check(prev_element);
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
        self.check(sibling);
        self.doc.append_before_sibling(sibling, new_node);
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        self.doc.add_attrs_if_missing(target, attrs);
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
        self.check(new_parent);
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

    const MAX: usize = MAX_DEPTH;

    #[test]
    fn a_normal_page_parses_as_dom_query_would() {
        let page = r#"<!doctype html><html><head><title>T</title><link rel="alternate" href="/f"></head>
            <body><article><h1>Hi</h1><p>One <b>two <i>three</b> four</i></p>
            <table><tr><td>cell</td></tr></table><svg><path/></svg><template><p>t</p></template>
            </article></body></html>"#;
        let ours = parse(page, MAX).expect("shallow");
        assert_eq!(ours.html(), Document::from(page).html());
        // Across chunk boundaries, with multibyte text, too.
        let long = format!("<p>{}</p>", "Smörgåsbord är gött. ".repeat(3000));
        assert_eq!(
            parse(&long, MAX).unwrap().html(),
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
            assert!(parse(&page, MAX).is_none(), "{name} was parsed");
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
        assert!(parse(&wide, MAX).is_some());
        let deep = |levels: usize| format!("{}{}", "<div>".repeat(levels), "</div>".repeat(levels));
        assert!(parse(&deep(400), MAX).is_some());
        assert!(parse(&deep(600), MAX).is_none());
    }
}
