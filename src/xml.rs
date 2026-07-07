//! Minimal XML parser for boundary readers (design_doc §9.9).
//!
//! WITSML drill reports, EDM exports and xlsx internals are all XML at the wire;
//! their boundary readers need just enough of a parser to walk elements, attributes
//! and text.  This is that parser: no validation, no namespace resolution (prefixes
//! are kept verbatim and stripped on lookup), no external entities.  It parses
//! trusted local files but stays safe on garbage: parsing is iterative with an
//! explicit stack, nesting is capped, and every failure is a human-readable error
//! with a byte offset — never a panic.

const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone)]
pub enum Node {
    Elem(Elem),
    Text(String),
}

#[derive(Debug, Clone)]
pub struct Elem {
    /// Name as written, prefix included (e.g. "ns0:drillReport").
    pub name: String,
    /// Attributes as written, in document order.
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Elem {
    /// Name without any namespace prefix ("ns0:drillReport" → "drillReport").
    pub fn local(&self) -> &str {
        local_of(&self.name)
    }

    /// First attribute whose local name (prefix stripped) matches, e.g. attr("uid").
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| local_of(k) == local)
            .map(|(_, v)| v.as_str())
    }

    /// All descendant text, concatenated, trimmed.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut stack: Vec<&Node> = self.children.iter().rev().collect();
        while let Some(node) = stack.pop() {
            match node {
                Node::Text(t) => out.push_str(t),
                Node::Elem(e) => stack.extend(e.children.iter().rev()),
            }
        }
        out.trim().to_string()
    }

    /// Direct child elements.
    pub fn elems(&self) -> impl Iterator<Item = &Elem> {
        self.children.iter().filter_map(|n| match n {
            Node::Elem(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// First direct child element with this local name.
    pub fn find(&self, local: &str) -> Option<&Elem> {
        self.elems().find(|e| e.local() == local)
    }

    /// All direct child elements with this local name.
    pub fn find_all(&self, local: &str) -> Vec<&Elem> {
        self.elems().filter(|e| e.local() == local).collect()
    }
}

fn local_of(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// Parse a whole document, returning the root element.
pub fn parse(text: &str) -> Result<Elem, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser { text, pos: 0 };
    p.skip_misc()?;
    let root = p.parse_root()?;
    p.skip_misc()?;
    if p.pos < p.text.len() {
        return Err(p.err("unexpected content after root element"));
    }
    Ok(root)
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.text[self.pos..]
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.rest().starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        let b = self.text.as_bytes();
        while self.pos < b.len() && b[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    /// Advance past the next occurrence of `end`, or fail if it never comes.
    fn skip_past(&mut self, end: &str, what: &str) -> Result<(), String> {
        match self.rest().find(end) {
            Some(i) => {
                self.pos += i + end.len();
                Ok(())
            }
            None => Err(self.err(&format!("unterminated {what}"))),
        }
    }

    fn err(&self, msg: &str) -> String {
        let mut start = self.pos.min(self.text.len());
        while !self.text.is_char_boundary(start) {
            start -= 1;
        }
        let tail = &self.text[start..];
        if tail.is_empty() {
            return format!("{msg} at byte {} (end of input)", self.pos);
        }
        let mut end = tail.len().min(24);
        while !tail.is_char_boundary(end) {
            end -= 1;
        }
        format!("{msg} at byte {} (near {:?})", self.pos, &tail[..end])
    }

    /// Skip prolog, processing instructions, comments, DOCTYPE and whitespace —
    /// everything allowed around the root element.
    fn skip_misc(&mut self) -> Result<(), String> {
        loop {
            self.skip_ws();
            let rest = self.rest();
            if rest.starts_with("<?") {
                self.skip_past("?>", "processing instruction")?;
            } else if rest.starts_with("<!--") {
                self.skip_past("-->", "comment")?;
            } else if rest.len() >= 9 && rest[..9].eq_ignore_ascii_case("<!doctype") {
                self.skip_doctype()?;
            } else {
                return Ok(());
            }
        }
    }

    /// Skip `<!DOCTYPE ...>` including a bracketed internal subset, resolving
    /// nothing.  Quotes are honored so a `>` inside a system id doesn't end it.
    fn skip_doctype(&mut self) -> Result<(), String> {
        let b = self.text.as_bytes();
        let mut depth = 0usize;
        let mut quote: Option<u8> = None;
        while self.pos < b.len() {
            let c = b[self.pos];
            self.pos += 1;
            match quote {
                Some(q) => {
                    if c == q {
                        quote = None;
                    }
                }
                None => match c {
                    b'"' | b'\'' => quote = Some(c),
                    b'[' => depth += 1,
                    b']' => depth = depth.saturating_sub(1),
                    b'>' if depth == 0 => return Ok(()),
                    _ => {}
                },
            }
        }
        Err(self.err("unterminated <!DOCTYPE"))
    }

    /// A name runs until whitespace or markup punctuation; `:` stays in, so
    /// prefixed names come through verbatim.
    fn read_name(&mut self, what: &str) -> Result<String, String> {
        let start = self.pos;
        let b = self.text.as_bytes();
        while self.pos < b.len() {
            match b[self.pos] {
                b'>' | b'/' | b'=' | b'<' | b'"' | b'\'' | b'?' => break,
                c if c.is_ascii_whitespace() => break,
                _ => self.pos += 1,
            }
        }
        if self.pos == start {
            return Err(self.err(&format!("expected {what}")));
        }
        Ok(self.text[start..self.pos].to_string())
    }

    /// Iterative document walk: open tags push onto an explicit stack, close tags
    /// pop and attach to the parent, so arbitrarily nested garbage can't blow the
    /// call stack.
    fn parse_root(&mut self) -> Result<Elem, String> {
        if !self.rest().starts_with('<') {
            return Err(self.err("expected an element"));
        }
        let mut stack: Vec<Elem> = Vec::new();
        loop {
            if self.pos >= self.text.len() {
                let open = stack.last().map(|e| e.name.as_str()).unwrap_or("?");
                return Err(format!("truncated input: <{open}> is never closed"));
            }
            let rest = self.rest();
            if rest.starts_with("<!--") {
                self.skip_past("-->", "comment")?;
            } else if rest.starts_with("<![CDATA[") {
                self.pos += "<![CDATA[".len();
                let i = match self.rest().find("]]>") {
                    Some(i) => i,
                    None => return Err(self.err("unterminated CDATA section")),
                };
                let content = self.text[self.pos..self.pos + i].to_string();
                self.pos += i + "]]>".len();
                match stack.last_mut() {
                    Some(e) => e.children.push(Node::Text(content)),
                    None => return Err(self.err("CDATA outside the root element")),
                }
            } else if rest.starts_with("</") {
                let at = self.pos;
                self.pos += 2;
                let name = self.read_name("tag name after '</'")?;
                self.skip_ws();
                if !self.eat(">") {
                    return Err(self.err(&format!("expected '>' to end </{name}")));
                }
                let elem = match stack.pop() {
                    Some(e) => e,
                    None => {
                        return Err(format!(
                            "closing tag </{name}> at byte {at} has no matching open tag"
                        ))
                    }
                };
                if elem.name != name {
                    return Err(format!(
                        "mismatched tag: expected </{}>, found </{name}> at byte {at}",
                        elem.name
                    ));
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Elem(elem)),
                    None => return Ok(elem),
                }
            } else if rest.starts_with("<?") {
                self.skip_past("?>", "processing instruction")?;
            } else if rest.starts_with("<!") {
                return Err(self.err("unexpected markup declaration inside document"));
            } else if rest.starts_with('<') {
                let (elem, self_closing) = self.parse_start_tag()?;
                if self_closing {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(Node::Elem(elem)),
                        None => return Ok(elem),
                    }
                } else {
                    if stack.len() >= MAX_DEPTH {
                        return Err(
                            self.err(&format!("elements nested deeper than {MAX_DEPTH} levels"))
                        );
                    }
                    stack.push(elem);
                }
            } else {
                // a text run, up to the next markup; whitespace-only runs are layout
                let i = rest.find('<').unwrap_or(rest.len());
                let raw = &self.text[self.pos..self.pos + i];
                self.pos += i;
                if raw.trim().is_empty() {
                    continue;
                }
                match stack.last_mut() {
                    Some(e) => e.children.push(Node::Text(decode_entities(raw))),
                    None => return Err(self.err("text outside the root element")),
                }
            }
        }
    }

    /// Parse `<name a="1" b='2'>` or `<name/>`; returns the element and whether it
    /// was self-closing.
    fn parse_start_tag(&mut self) -> Result<(Elem, bool), String> {
        self.pos += 1; // consume '<'
        let name = self.read_name("element name after '<'")?;
        let mut attrs = Vec::new();
        loop {
            self.skip_ws();
            if self.eat("/>") {
                return Ok((Elem { name, attrs, children: Vec::new() }, true));
            }
            if self.eat(">") {
                return Ok((Elem { name, attrs, children: Vec::new() }, false));
            }
            if self.pos >= self.text.len() {
                return Err(self.err(&format!("truncated input inside <{name}")));
            }
            let key = self.read_name("attribute name")?;
            self.skip_ws();
            if !self.eat("=") {
                return Err(self.err(&format!("expected '=' after attribute '{key}'")));
            }
            self.skip_ws();
            let q = match self.rest().as_bytes().first() {
                Some(b'"') => "\"",
                Some(b'\'') => "'",
                _ => return Err(self.err(&format!("expected quoted value for attribute '{key}'"))),
            };
            self.pos += 1;
            let i = match self.rest().find(q) {
                Some(i) => i,
                None => {
                    return Err(self.err(&format!("unterminated value for attribute '{key}'")))
                }
            };
            let value = decode_entities(&self.text[self.pos..self.pos + i]);
            self.pos += i + 1;
            attrs.push((key, value));
        }
    }
}

/// Replace the five named entities and `&#NNN;` / `&#xHH;` numeric references.
/// A malformed reference passes through literally — garbage in, garbage kept.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // entity references are short; a distant ';' means this '&' is literal
        match rest.find(';') {
            Some(j) if j <= 10 => match entity_char(&rest[1..j]) {
                Some(c) => {
                    out.push(c);
                    rest = &rest[j + 1..];
                }
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            },
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity_char(ent: &str) -> Option<char> {
    match ent {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "apos" => Some('\''),
        "quot" => Some('"'),
        _ => {
            let num = ent.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse::<u32>().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn witsml_flavored_namespaces_and_uids() {
        let doc = r#"<?xml version="1.0" encoding="UTF-8"?>
<ns0:drillReports xmlns:ns0="http://www.witsml.org/schemas/1series" version="1.4.1.1">
  <ns0:drillReport uidWell="W-123" uid="DR-1">
    <ns0:nameWell>34/10-A-12</ns0:nameWell>
    <ns0:statusInfo md="1234.5"/>
  </ns0:drillReport>
</ns0:drillReports>"#;
        let root = parse(doc).unwrap();
        assert_eq!(root.name, "ns0:drillReports");
        assert_eq!(root.local(), "drillReports");
        assert_eq!(root.attr("version"), Some("1.4.1.1"));
        // the namespace declaration is just another attribute, kept verbatim
        assert_eq!(root.attrs[0].0, "xmlns:ns0");
        let report = root.find("drillReport").unwrap();
        assert_eq!(report.attr("uid"), Some("DR-1"));
        assert_eq!(report.attr("uidWell"), Some("W-123"));
        assert_eq!(report.find("nameWell").unwrap().text(), "34/10-A-12");
        assert_eq!(report.find("statusInfo").unwrap().attr("md"), Some("1234.5"));
        // whitespace between elements is layout, not Text nodes
        assert_eq!(report.children.len(), 2);
    }

    #[test]
    fn entities_named_and_numeric() {
        let root = parse(r#"<a note="x &amp; &#65;">1 &lt; 2 &gt; 0 &#x41;&apos;&quot;</a>"#)
            .unwrap();
        assert_eq!(root.text(), "1 < 2 > 0 A'\"");
        assert_eq!(root.attr("note"), Some("x & A"));
    }

    #[test]
    fn malformed_entity_passes_through() {
        let root = parse("<a>fish &chips; AT&T</a>").unwrap();
        assert_eq!(root.text(), "fish &chips; AT&T");
    }

    #[test]
    fn cdata_becomes_text_verbatim() {
        let root = parse("<a><![CDATA[<not><xml> & stuff]]></a>").unwrap();
        assert_eq!(root.text(), "<not><xml> & stuff");
    }

    #[test]
    fn self_closing_and_find_all() {
        let root = parse("<a><b/><c/><b x='1'/></a>").unwrap();
        let bs = root.find_all("b");
        assert_eq!(bs.len(), 2);
        assert_eq!(bs[1].attr("x"), Some("1"));
        assert_eq!(root.elems().count(), 3);
    }

    #[test]
    fn attr_strips_prefix() {
        let root = parse(r#"<a ns:uid="7"/>"#).unwrap();
        assert_eq!(root.attr("uid"), Some("7"));
        assert_eq!(root.attrs[0].0, "ns:uid");
    }

    #[test]
    fn mismatched_tag_names_the_tag() {
        let err = parse("<a><b></a></b>").unwrap_err();
        assert!(err.contains("</b>"), "error should mention the open tag: {err}");
        assert!(err.contains("</a>"), "error should mention the found tag: {err}");
    }

    #[test]
    fn truncated_input_is_an_error() {
        let err = parse("<a><b>").unwrap_err();
        assert!(err.contains("<b>"), "error should name the unclosed tag: {err}");
        assert!(parse("<a foo=").is_err());
        assert!(parse("<").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn doctype_with_internal_subset_is_skipped() {
        let doc = r#"<!DOCTYPE root [ <!ENTITY x "y"> <!ELEMENT root ANY> ]>
<!-- a comment too -->
<root a="1">hi</root>"#;
        let root = parse(doc).unwrap();
        assert_eq!(root.local(), "root");
        assert_eq!(root.text(), "hi");
    }

    #[test]
    fn deep_nesting_is_capped_not_crashed() {
        let mut doc = String::new();
        for _ in 0..500 {
            doc.push_str("<d>");
        }
        let err = parse(&doc).unwrap_err();
        assert!(err.contains("256"), "should hit the depth cap: {err}");
    }

    #[test]
    fn garbage_never_panics() {
        for junk in ["<a b='1", "</only-close>", "<a><![CDATA[x", "<?pi", "text only",
                     "<a>&#xZZ;</a>", "<!DOCTYPE broken", "<a>after</a>trailing"] {
            let _ = parse(junk); // Ok or Err both fine; panicking is not
        }
        // and a couple that must still parse
        assert!(parse("<a>&#xZZ;</a>").is_ok()); // bad numeric ref kept literally
        assert!(parse("\u{feff}<a/>").is_ok()); // leading BOM is stripped
    }
}
