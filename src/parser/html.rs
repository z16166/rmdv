//! The small HTML subset READMEs use, mapped onto the markdown AST.
//!
//! Markdown passes raw HTML through. Without a browser engine the reader
//! keeps what it can show with its own blocks: headings, paragraphs, line
//! breaks, emphasis, links, images (`<picture>` shows its `<img>`), lists,
//! quotes, preformatted code, rules, and simple tables. Styling, scripts, and
//! layout attributes are dropped; unknown tags keep their text.

use super::{inline_to_string, is_blank, slugify};
use crate::ast::{Block, ImageRef, Inline, ListItem};

/// One lexical piece of an HTML fragment.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Token {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
    },
    End(String),
    Text(String),
}

impl Token {
    pub(super) fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        attrs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

/// Split `src` into tags and text. Comments, doctypes, and processing
/// instructions are dropped; a `<` that does not open a tag stays text.
/// Tag and attribute names are lowercased; text and values are decoded.
pub(super) fn tokenize(src: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = src;
    while let Some(at) = rest.find('<') {
        text.push_str(&rest[..at]);
        rest = &rest[at..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
            continue;
        }
        match read_tag(rest) {
            Some((token, len)) => {
                if !text.is_empty() {
                    out.push(Token::Text(decode_entities(&std::mem::take(&mut text))));
                }
                rest = &rest[len..];
                if let Token::Start { name, .. } = &token {
                    if matches!(name.as_str(), "script" | "style") {
                        let close = format!("</{name}");
                        let end = rest.to_ascii_lowercase().find(&close);
                        rest = end.map_or("", |end| {
                            let tail = &rest[end..];
                            tail.find('>').map_or("", |gt| &tail[gt + 1..])
                        });
                        continue;
                    }
                }
                out.push(token);
            }
            None => {
                text.push('<');
                rest = &rest[1..];
            }
        }
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Token::Text(decode_entities(&text)));
    }
    out
}

/// A start or end tag at the start of `s`, and its length in bytes.
fn read_tag(s: &str) -> Option<(Token, usize)> {
    let bytes = s.as_bytes();
    let closing = bytes.get(1) == Some(&b'/');
    let name_start = if closing { 2 } else { 1 };
    if !bytes.get(name_start)?.is_ascii_alphabetic() {
        return None;
    }
    let name_end = name_start
        + s[name_start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(s.len() - name_start);
    let name = s[name_start..name_end].to_ascii_lowercase();
    let mut i = name_end;
    let mut attrs = Vec::new();
    loop {
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        match bytes.get(i)? {
            b'>' => {
                i += 1;
                break;
            }
            b'/' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let key_end = i + s[i..]
            .find(|c: char| c.is_ascii_whitespace() || matches!(c, '=' | '>' | '/'))
            .unwrap_or(s.len() - i);
        let key = s[i..key_end].to_ascii_lowercase();
        i = key_end;
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        let mut value = String::new();
        if bytes.get(i) == Some(&b'=') {
            i += 1;
            while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
                i += 1;
            }
            match bytes.get(i)? {
                quote @ (b'"' | b'\'') => {
                    let end = s[i + 1..].find(*quote as char)? + i + 1;
                    value = decode_entities(&s[i + 1..end]);
                    i = end + 1;
                }
                _ => {
                    let end = i + s[i..]
                        .find(|c: char| c.is_ascii_whitespace() || c == '>')
                        .unwrap_or(s.len() - i);
                    value = decode_entities(&s[i..end]);
                    i = end;
                }
            }
        }
        if !key.is_empty() {
            attrs.push((key, value));
        }
    }
    let token = if closing {
        Token::End(name)
    } else {
        Token::Start { name, attrs }
    };
    Some((token, i))
}

/// Decode character references: numeric ones and the named ones READMEs
/// use. An unknown name stays as written.
pub(super) fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest[1..]
            .find(';')
            .filter(|&end| end <= 32)
            .and_then(|end| {
                let name = &rest[1..end + 1];
                let c = if let Some(num) = name.strip_prefix('#') {
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => num.parse().ok(),
                    };
                    code.and_then(char::from_u32)
                } else {
                    named_entity(name)
                };
                c.map(|c| (c, end + 2))
            });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "middot" => '·',
        "bull" => '•',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "times" => '×',
        "deg" => '°',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "check" => '✓',
        "star" => '☆',
        "hearts" => '♥',
        "emsp" | "ensp" | "thinsp" => ' ',
        _ => return None,
    })
}

/// Inline styling an HTML tag opens.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum InlineTag {
    Strong,
    Emph,
    Strike,
    Link(String),
    Code,
}

impl InlineTag {
    /// The styling `name` opens, `None` for a tag the reader shows as plain
    /// text. A link needs an `href`.
    pub(super) fn open(name: &str, attrs: &[(String, String)]) -> Option<InlineTag> {
        Some(match name {
            "b" | "strong" => InlineTag::Strong,
            "i" | "em" | "cite" | "var" | "dfn" => InlineTag::Emph,
            "s" | "del" | "strike" => InlineTag::Strike,
            "code" | "kbd" | "samp" | "tt" => InlineTag::Code,
            "a" => InlineTag::Link(Token::attr(attrs, "href")?.to_string()),
            _ => return None,
        })
    }

    /// Whether the end tag `name` closes this styling.
    pub(super) fn closed_by(&self, name: &str) -> bool {
        match self {
            InlineTag::Strong => matches!(name, "b" | "strong"),
            InlineTag::Emph => matches!(name, "i" | "em" | "cite" | "var" | "dfn"),
            InlineTag::Strike => matches!(name, "s" | "del" | "strike"),
            InlineTag::Code => matches!(name, "code" | "kbd" | "samp" | "tt"),
            InlineTag::Link(_) => name == "a",
        }
    }
}

/// An image tag's source and description. `None` without a source.
pub(super) fn image(attrs: &[(String, String)]) -> Option<(String, String)> {
    let src = Token::attr(attrs, "src").filter(|s| !s.trim().is_empty())?;
    let alt = Token::attr(attrs, "alt").unwrap_or_default();
    Some((src.trim().to_string(), alt.to_string()))
}

/// Tags that end the paragraph around them and start a new one.
fn breaks_paragraph(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "center"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "nav"
            | "main"
            | "aside"
            | "figure"
            | "figcaption"
            | "details"
            | "summary"
            | "dl"
            | "dt"
            | "dd"
            | "address"
            | "picture"
    )
}

/// Map one HTML block onto reader blocks.
pub(super) fn blocks(html: &str) -> Vec<Block> {
    let mut builder = Builder::default();
    for token in tokenize(html) {
        builder.token(token);
    }
    builder.finish()
}

enum Container {
    Root(Vec<Block>),
    Quote(Vec<Block>),
    List {
        ordered: bool,
        items: Vec<ListItem>,
    },
    Item(Vec<Block>),
    Table {
        rows: Vec<Vec<Vec<Inline>>>,
        row: Option<Vec<Vec<Inline>>>,
        in_cell: bool,
    },
}

struct Builder {
    containers: Vec<Container>,
    /// Open inline styles over the paragraph being built; the first entry is
    /// the paragraph itself.
    inlines: Vec<(Option<InlineTag>, Vec<Inline>)>,
    heading: Option<u8>,
    /// Text of an open `<pre>`, kept as written.
    pre: Option<String>,
    /// A space is owed before the next visible text.
    pending_space: bool,
    /// The last block is an image with nothing visible after it, so the next
    /// image shares its row.
    image_row_open: bool,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            containers: vec![Container::Root(Vec::new())],
            inlines: vec![(None, Vec::new())],
            heading: None,
            pre: None,
            pending_space: false,
            image_row_open: false,
        }
    }
}

impl Builder {
    fn token(&mut self, token: Token) {
        match token {
            Token::Text(text) => self.text(&text),
            Token::Start { name, attrs } => self.start(&name, &attrs),
            Token::End(name) => self.end(&name),
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(pre) = &mut self.pre {
            pre.push_str(text);
            return;
        }
        let mut out = String::new();
        let line_start = self.at_line_start();
        for c in text.chars() {
            // A non-breaking space is visible and does not collapse.
            if c.is_whitespace() && c != '\u{a0}' {
                self.pending_space = true;
                continue;
            }
            if std::mem::take(&mut self.pending_space) && !(line_start && out.is_empty()) {
                out.push(' ');
            }
            out.push(c);
        }
        if !out.is_empty() {
            self.push_inline(Inline::Text(out));
        }
    }

    /// Nothing visible has been written on the current line yet.
    fn at_line_start(&self) -> bool {
        fn last_text(inlines: &[Inline]) -> Option<&str> {
            match inlines.last()? {
                Inline::Text(t) | Inline::Code(t) => Some(t),
                Inline::Emph(c) | Inline::Strong(c) | Inline::Strike(c) => last_text(c),
                Inline::Link { children, .. } => last_text(children),
            }
        }
        self.inlines
            .iter()
            .rev()
            .find_map(|(_, inlines)| last_text(inlines))
            .is_none_or(|t| t.ends_with('\n'))
    }

    fn push_inline(&mut self, inline: Inline) {
        let Some((_, inlines)) = self.inlines.last_mut() else {
            return;
        };
        match (inlines.last_mut(), inline) {
            (Some(Inline::Text(last)), Inline::Text(more)) => last.push_str(&more),
            (_, inline) => inlines.push(inline),
        }
    }

    /// Write the space owed before a style opens, so it stays outside the
    /// style (a link does not underline it).
    fn emit_pending_space(&mut self) {
        if std::mem::take(&mut self.pending_space) && !self.at_line_start() {
            self.push_inline(Inline::Text(" ".into()));
        }
    }

    fn start(&mut self, name: &str, attrs: &[(String, String)]) {
        if self.pre.is_some() {
            return;
        }
        if let Some(tag) = InlineTag::open(name, attrs) {
            self.emit_pending_space();
            self.inlines.push((Some(tag), Vec::new()));
            return;
        }
        match name {
            "br" => {
                self.push_inline(Inline::Text("\n".into()));
                self.pending_space = false;
                self.image_row_open = false;
            }
            "img" => {
                let Some((url, alt)) = image(attrs) else {
                    return;
                };
                if self.in_cell() {
                    if !alt.is_empty() {
                        self.text(&alt);
                    }
                    return;
                }
                let link = self.inlines.iter().rev().find_map(|(tag, _)| match tag {
                    Some(InlineTag::Link(url)) => Some(url.clone()),
                    _ => None,
                });
                let image = ImageRef { url, alt, link };
                if !(self.image_row_open && self.join_image_row(image.clone())) {
                    let ImageRef { url, alt, link } = image;
                    self.push_block(Block::Image { url, alt, link });
                }
                self.image_row_open = true;
            }
            "hr" => self.push_block(Block::Rule),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = name[1..].parse().ok();
            }
            "pre" => {
                self.flush();
                self.pre = Some(String::new());
            }
            "ul" | "ol" => {
                self.flush();
                self.containers.push(Container::List {
                    ordered: name == "ol",
                    items: Vec::new(),
                });
            }
            "li" => {
                self.flush();
                if matches!(self.containers.last(), Some(Container::Item(_))) {
                    self.close_container();
                }
                if !matches!(self.containers.last(), Some(Container::List { .. })) {
                    self.containers.push(Container::List {
                        ordered: false,
                        items: Vec::new(),
                    });
                }
                self.containers.push(Container::Item(Vec::new()));
            }
            "blockquote" => {
                self.flush();
                self.containers.push(Container::Quote(Vec::new()));
            }
            "table" => {
                self.flush();
                self.containers.push(Container::Table {
                    rows: Vec::new(),
                    row: None,
                    in_cell: false,
                });
            }
            "tr" => {
                self.end_row();
                if let Some(Container::Table { row, .. }) = self.containers.last_mut() {
                    *row = Some(Vec::new());
                }
            }
            "td" | "th" => {
                self.end_cell();
                if let Some(Container::Table { row, in_cell, .. }) = self.containers.last_mut() {
                    row.get_or_insert_with(Vec::new);
                    *in_cell = true;
                }
            }
            "summary" => {
                self.flush();
                self.inlines.push((Some(InlineTag::Strong), Vec::new()));
            }
            name if breaks_paragraph(name) => self.flush(),
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        if let Some(pre) = self.pre.take_if(|_| name == "pre") {
            let code = pre
                .strip_prefix('\n')
                .unwrap_or(&pre)
                .trim_end()
                .to_string();
            self.push_block(Block::CodeBlock {
                lang: None,
                code,
                spans: Vec::new(),
            });
            return;
        }
        if self.pre.is_some() {
            return;
        }
        if let Some(at) = self
            .inlines
            .iter()
            .rposition(|(tag, _)| tag.as_ref().is_some_and(|tag| tag.closed_by(name)))
            .filter(|&at| at > 0)
        {
            while self.inlines.len() > at {
                self.close_inline();
            }
            return;
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.flush(),
            "summary" => {
                while self.inlines.len() > 1 {
                    self.close_inline();
                }
                self.flush();
            }
            "ul" | "ol" => {
                self.flush();
                if matches!(self.containers.last(), Some(Container::Item(_))) {
                    self.close_container();
                }
                if matches!(self.containers.last(), Some(Container::List { .. })) {
                    self.close_container();
                }
            }
            "li" => {
                self.flush();
                if matches!(self.containers.last(), Some(Container::Item(_))) {
                    self.close_container();
                }
            }
            "blockquote" => {
                self.flush();
                if matches!(self.containers.last(), Some(Container::Quote(_))) {
                    self.close_container();
                }
            }
            "td" | "th" => self.end_cell(),
            "tr" => self.end_row(),
            "table" => {
                self.end_row();
                if matches!(self.containers.last(), Some(Container::Table { .. })) {
                    self.close_container();
                }
            }
            name if breaks_paragraph(name) => self.flush(),
            _ => {}
        }
    }

    fn in_cell(&self) -> bool {
        matches!(
            self.containers.last(),
            Some(Container::Table { in_cell: true, .. })
        )
    }

    /// Fold the innermost open style into its parent.
    fn close_inline(&mut self) {
        let Some((tag, children)) = self.inlines.pop() else {
            return;
        };
        let redundant = matches!(
            tag,
            Some(InlineTag::Strong | InlineTag::Emph | InlineTag::Strike)
        ) && (is_blank(&children)
            || self.inlines.iter().any(|(open, _)| *open == tag));
        if redundant {
            for child in children {
                self.push_inline(child);
            }
            return;
        }
        let inline = match tag {
            None => {
                self.inlines.push((None, children));
                return;
            }
            Some(InlineTag::Strong) => Inline::Strong(children),
            Some(InlineTag::Emph) => Inline::Emph(children),
            Some(InlineTag::Strike) => Inline::Strike(children),
            Some(InlineTag::Code) => Inline::Code(inline_to_string(&children)),
            // A link around images only: each image carries the link.
            Some(InlineTag::Link(_)) if is_blank(&children) => return,
            Some(InlineTag::Link(url)) => Inline::Link { url, children },
        };
        self.push_inline(inline);
    }

    /// The paragraph's inlines with every open style closed. The styles are
    /// opened again, empty, for the text that follows.
    fn take_inlines(&mut self) -> Vec<Inline> {
        let open: Vec<InlineTag> = self.inlines[1..]
            .iter()
            .filter_map(|(tag, _)| tag.clone())
            .collect();
        while self.inlines.len() > 1 {
            self.close_inline();
        }
        let inlines = std::mem::take(&mut self.inlines[0].1);
        self.inlines
            .extend(open.into_iter().map(|tag| (Some(tag), Vec::new())));
        self.pending_space = false;
        inlines
    }

    /// Add `image` to the row of images that ends the container, when
    /// nothing visible was written since. False when it starts a new block.
    fn join_image_row(&mut self, image: ImageRef) -> bool {
        let blank = self.inlines.iter().all(|(_, inlines)| is_blank(inlines));
        let Some(Container::Root(blocks) | Container::Quote(blocks) | Container::Item(blocks)) =
            self.containers.last_mut()
        else {
            return false;
        };
        if !blank {
            return false;
        }
        match blocks.last_mut() {
            Some(Block::ImageRow(row)) => row.push(image),
            Some(last) => {
                let Block::Image { url, alt, link } = last else {
                    return false;
                };
                let first = ImageRef {
                    url: std::mem::take(url),
                    alt: std::mem::take(alt),
                    link: link.take(),
                };
                *last = Block::ImageRow(vec![first, image]);
            }
            None => return false,
        }
        true
    }

    /// End the paragraph or heading being built.
    fn flush(&mut self) {
        self.image_row_open = false;
        if self.in_cell() {
            return;
        }
        let inlines = trim(self.take_inlines());
        let heading = self.heading.take();
        if is_blank(&inlines) {
            return;
        }
        let block = match heading {
            Some(level) => Block::Heading {
                level,
                id: slugify(&inline_to_string(&inlines)),
                inlines,
            },
            None => Block::Paragraph(inlines),
        };
        self.push_to_container(block);
    }

    fn push_block(&mut self, block: Block) {
        let heading = self.heading;
        self.flush();
        // A heading split by an image goes on after it.
        self.heading = heading;
        self.push_to_container(block);
    }

    fn push_to_container(&mut self, block: Block) {
        match self.containers.last_mut() {
            Some(Container::Root(blocks) | Container::Quote(blocks) | Container::Item(blocks)) => {
                blocks.push(block)
            }
            Some(Container::List { items, .. }) => items.push(ListItem {
                task: None,
                blocks: vec![block],
            }),
            Some(Container::Table { .. }) | None => {}
        }
    }

    fn end_cell(&mut self) {
        if !self.in_cell() {
            return;
        }
        let inlines = trim(self.take_inlines());
        if let Some(Container::Table { row, in_cell, .. }) = self.containers.last_mut() {
            row.get_or_insert_with(Vec::new).push(inlines);
            *in_cell = false;
        }
    }

    fn end_row(&mut self) {
        self.end_cell();
        if let Some(Container::Table { rows, row, .. }) = self.containers.last_mut() {
            if let Some(row) = row.take().filter(|row| !row.is_empty()) {
                rows.push(row);
            }
        }
    }

    /// Close the innermost container into its parent.
    fn close_container(&mut self) {
        if self.containers.len() < 2 {
            return;
        }
        self.end_row();
        self.flush();
        let block = match self.containers.pop() {
            Some(Container::Quote(blocks)) if !blocks.is_empty() => Block::Blockquote(blocks),
            Some(Container::List { ordered, items }) if !items.is_empty() => {
                Block::List { ordered, items }
            }
            Some(Container::Item(blocks)) => {
                if let Some(Container::List { items, .. }) = self.containers.last_mut() {
                    items.push(ListItem { task: None, blocks });
                }
                return;
            }
            Some(Container::Table { mut rows, .. }) if !rows.is_empty() => {
                let headers = rows.remove(0);
                Block::Table { headers, rows }
            }
            _ => return,
        };
        self.push_to_container(block);
    }

    fn finish(mut self) -> Vec<Block> {
        if let Some(pre) = self.pre.take() {
            self.push_block(Block::CodeBlock {
                lang: None,
                code: pre.trim_end().to_string(),
                spans: Vec::new(),
            });
        }
        while self.containers.len() > 1 {
            self.close_container();
        }
        self.flush();
        match self.containers.pop() {
            Some(Container::Root(blocks)) => blocks,
            _ => Vec::new(),
        }
    }
}

/// Drop the spaces and line breaks that open or close a paragraph.
fn trim(mut inlines: Vec<Inline>) -> Vec<Inline> {
    fn trim_edge(inlines: &mut Vec<Inline>, start: bool) {
        loop {
            let at = if start {
                0
            } else {
                inlines.len().saturating_sub(1)
            };
            let Some(inline) = inlines.get_mut(at) else {
                return;
            };
            match inline {
                Inline::Text(t) => {
                    let trimmed = if start { t.trim_start() } else { t.trim_end() };
                    if trimmed.is_empty() {
                        inlines.remove(at);
                        continue;
                    }
                    *t = trimmed.to_string();
                }
                Inline::Emph(c) | Inline::Strong(c) | Inline::Strike(c) => trim_edge(c, start),
                Inline::Link { children, .. } => trim_edge(children, start),
                Inline::Code(_) => {}
            }
            return;
        }
    }
    trim_edge(&mut inlines, true);
    trim_edge(&mut inlines, false);
    inlines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn tags_attributes_comments_and_entities_tokenize() {
        assert_eq!(
            tokenize("<p ALIGN=\"center\">a &amp; b<!-- x --><br/>c &lt; 1 &#169;</P>"),
            vec![
                Token::Start {
                    name: "p".into(),
                    attrs: vec![("align".into(), "center".into())],
                },
                Token::Text("a & b".into()),
                Token::Start {
                    name: "br".into(),
                    attrs: vec![],
                },
                Token::Text("c < 1 ©".into()),
                Token::End("p".into()),
            ]
        );
        assert_eq!(tokenize("1 < 2"), vec![Token::Text("1 < 2".into())]);
        assert_eq!(
            tokenize("<script>x<y</script>z"),
            vec![Token::Text("z".into())]
        );
        assert_eq!(decode_entities("&unknown; &#x41;"), "&unknown; A");
    }

    #[test]
    fn a_centered_readme_header_keeps_its_heading_links_and_pictures() {
        let got = blocks(
            "<p align=\"center\">\n  <picture>\n    <source media=\"(prefers-color-scheme: dark)\" srcset=\"dark.svg\">\n    <img src=\"light.svg\" alt=\"Strata\" width=\"340\">\n  </picture>\n</p>\n",
        );
        assert_eq!(
            got,
            vec![Block::Image {
                url: "light.svg".into(),
                alt: "Strata".into(),
                link: None,
            }]
        );
        let got = blocks("<h3 align=\"center\">Run powerful AI on your own computer.</h3>\n");
        assert_eq!(
            got,
            vec![Block::Heading {
                level: 3,
                id: "run-powerful-ai-on-your-own-computer".into(),
                inlines: vec![text("Run powerful AI on your own computer.")],
            }]
        );
        let got = blocks(
            "<p align=\"center\">\n  <a href=\"#models\">Models</a> · <a href=\"#install\"><b>Install</b></a> ·\n  <a href=\"#help\">Help</a>\n</p>\n",
        );
        assert_eq!(
            got,
            vec![Block::Paragraph(vec![
                Inline::Link {
                    url: "#models".into(),
                    children: vec![text("Models")],
                },
                text(" · "),
                Inline::Link {
                    url: "#install".into(),
                    children: vec![Inline::Strong(vec![text("Install")])],
                },
                text(" · "),
                Inline::Link {
                    url: "#help".into(),
                    children: vec![text("Help")],
                },
            ])]
        );
    }

    #[test]
    fn linked_badges_share_one_row_without_empty_paragraphs() {
        let got = blocks(
            "<p>\n  <a href=\"https://x/rel\"><img src=\"https://img/a.svg\" alt=\"A\"></a>\n  <a href=\"LICENSE\"><img src=\"b.svg\"></a>\n</p>\n",
        );
        assert_eq!(
            got,
            vec![Block::ImageRow(vec![
                ImageRef {
                    url: "https://img/a.svg".into(),
                    alt: "A".into(),
                    link: Some("https://x/rel".into()),
                },
                ImageRef {
                    url: "b.svg".into(),
                    alt: String::new(),
                    link: Some("LICENSE".into()),
                },
            ])]
        );
    }

    #[test]
    fn line_breaks_paragraphs_and_text_end_an_image_row() {
        let shapes: Vec<&str> = blocks(
            "<p><img src=\"a\"><img src=\"b\"><br><img src=\"c\"></p>\
             <p><img src=\"d\"></p><p><img src=\"e\"> text <img src=\"f\"></p>",
        )
        .iter()
        .map(|b| match b {
            Block::ImageRow(row) if row.len() == 2 => "row2",
            Block::Image { .. } => "img",
            Block::Paragraph(_) => "p",
            _ => "other",
        })
        .collect();
        assert_eq!(shapes, ["row2", "img", "img", "img", "p", "img"]);
    }

    #[test]
    fn lists_tables_quotes_rules_and_preformatted_code_map_to_blocks() {
        let got = blocks("<ul><li>one</li><li>two <em>2</em></li></ul>");
        assert_eq!(
            got,
            vec![Block::List {
                ordered: false,
                items: vec![
                    ListItem {
                        task: None,
                        blocks: vec![Block::Paragraph(vec![text("one")])],
                    },
                    ListItem {
                        task: None,
                        blocks: vec![Block::Paragraph(vec![
                            text("two "),
                            Inline::Emph(vec![text("2")]),
                        ])],
                    },
                ],
            }]
        );
        let got = blocks(
            "<table>\n<tr><th>Name</th><th>Size</th></tr>\n<tr><td><code>a</code></td><td>1 <img src=\"x.png\" alt=\"GB\"></td></tr>\n</table>",
        );
        assert_eq!(
            got,
            vec![Block::Table {
                headers: vec![vec![text("Name")], vec![text("Size")]],
                rows: vec![vec![vec![Inline::Code("a".into())], vec![text("1 GB")]]],
            }]
        );
        let got = blocks("<blockquote>quoted</blockquote><hr><pre>\nfn a() {\n    b\n}\n</pre>");
        assert_eq!(
            got,
            vec![
                Block::Blockquote(vec![Block::Paragraph(vec![text("quoted")])]),
                Block::Rule,
                Block::CodeBlock {
                    lang: None,
                    code: "fn a() {\n    b\n}".into(),
                    spans: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn a_summary_icon_leaves_no_empty_or_doubled_bold() {
        let got = blocks(
            "<details>\n<summary><img src=\"q.svg\" alt=\"\"> &nbsp;<b>Qwen</b></summary>\n</details>",
        );
        assert_eq!(
            got,
            vec![
                Block::Image {
                    url: "q.svg".into(),
                    alt: String::new(),
                    link: None,
                },
                Block::Paragraph(vec![Inline::Strong(vec![text("Qwen")])]),
            ]
        );
    }

    #[test]
    fn line_breaks_and_unknown_or_unclosed_tags_keep_their_text() {
        let got = blocks("<div>first<br>\n  second <span>third</span> <b>open");
        assert_eq!(
            got,
            vec![Block::Paragraph(vec![
                text("first\nsecond third "),
                Inline::Strong(vec![text("open")]),
            ])]
        );
        let got = blocks("<details><summary>More</summary>\nHidden text</details>");
        assert_eq!(
            got,
            vec![
                Block::Paragraph(vec![Inline::Strong(vec![text("More")])]),
                Block::Paragraph(vec![text("Hidden text")]),
            ]
        );
    }
}
