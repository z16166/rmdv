//! Plain text of a whole document, for ⌘A then ⌘C in the reader.
//!
//! The text comes from the AST, not from the widgets on screen, so blocks
//! outside the virtual-scroll window and folded sections are included.

use crate::ast::{Block, BlockId, DiagramKind, Inline, ListItem};

/// The document as plain text, in document order. Blocks are separated by
/// one blank line.
pub fn document(blocks: &[(BlockId, Block)]) -> String {
    join(blocks.iter().map(|(_, b)| b), "\n\n")
}

fn join<'a>(blocks: impl Iterator<Item = &'a Block>, sep: &str) -> String {
    blocks.filter_map(block).collect::<Vec<_>>().join(sep)
}

/// One block's text, as in [`document`]; `None` when it has no text.
pub(crate) fn block_text(b: &Block) -> Option<String> {
    block(b)
}

/// One block's text, or `None` when it has no text: rules, Mermaid and DOT
/// diagrams, and images without alt text. Display math keeps its TeX source.
fn block(b: &Block) -> Option<String> {
    let text = match b {
        Block::Heading { inlines, .. } | Block::Paragraph(inlines) => inlines_text(inlines),
        Block::CodeBlock { code, .. } => code.trim_end_matches('\n').to_string(),
        Block::Blockquote(inner) => join(inner.iter(), "\n\n"),
        Block::List { ordered, items } => list(*ordered, items),
        Block::Table { headers, rows } => std::iter::once(headers)
            .chain(rows)
            .map(|row| {
                row.iter()
                    .map(|c| inlines_text(c))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Image { alt, .. } => alt.clone(),
        Block::ImageRow(images) => images
            .iter()
            .map(|image| image.alt.as_str())
            .filter(|alt| !alt.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        Block::Diagram {
            kind: DiagramKind::Math,
            source,
            ..
        } => source.trim().to_string(),
        Block::Diagram { .. } | Block::Rule => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// One line per item, `- ` or `1. ` first. Lines after an item's first line,
/// such as a nested list, are indented to the item's text.
fn list(ordered: bool, items: &[ListItem]) -> String {
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let mut marker = if ordered {
            format!("{}. ", i + 1)
        } else {
            "- ".to_string()
        };
        match item.task {
            Some(true) => marker.push_str("[x] "),
            Some(false) => marker.push_str("[ ] "),
            None => {}
        }
        let indent = " ".repeat(marker.chars().count());
        let mut text = marker;
        for (n, line) in join(item.blocks.iter(), "\n").lines().enumerate() {
            if n > 0 {
                text.push('\n');
                if !line.is_empty() {
                    text.push_str(&indent);
                }
            }
            text.push_str(line);
        }
        out.push(text.trim_end().to_string());
    }
    out.join("\n")
}

fn inlines_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for i in inlines {
        push_inline(i, &mut out);
    }
    out
}

fn push_inline(i: &Inline, out: &mut String) {
    match i {
        Inline::Text(s) | Inline::Code(s) => out.push_str(s),
        Inline::Emph(c) | Inline::Strong(c) | Inline::Strike(c) => {
            c.iter().for_each(|x| push_inline(x, out));
        }
        Inline::Link { children, .. } => children.iter().for_each(|x| push_inline(x, out)),
    }
}

#[cfg(test)]
mod tests {
    use super::document;

    fn text_of(md: &str) -> String {
        document(&crate::parser::parse(md).0)
    }

    #[test]
    fn joins_blocks_with_blank_lines_and_marks_lists() {
        let md = "# Title\n\n\
                  Body with **bold**, `code` and [a link](https://x.dev).\n\n\
                  - one\n- two\n  - nested\n\n\
                  1. first\n2. second\n\n\
                  > quoted\n";
        assert_eq!(
            text_of(md),
            "Title\n\n\
             Body with bold, code and a link.\n\n\
             - one\n- two\n  - nested\n\n\
             1. first\n2. second\n\n\
             quoted"
        );
    }

    #[test]
    fn tables_use_tabs_and_code_keeps_its_indent() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\n\
                  ```rust\nfn main() {\n    let x = 1;\n}\n```\n";
        assert_eq!(
            text_of(md),
            "A\tB\n1\t2\n3\t4\n\nfn main() {\n    let x = 1;\n}"
        );
    }

    #[test]
    fn images_give_alt_text_and_diagrams_and_rules_are_skipped() {
        let md = "Before.\n\n![a cat](cat.png)\n\n\
                  ```mermaid\ngraph TD; A-->B\n```\n\n---\n\nAfter.\n";
        assert_eq!(text_of(md), "Before.\n\na cat\n\nAfter.");
    }

    #[test]
    fn display_math_keeps_its_tex_source() {
        assert_eq!(text_of("$$\nx^2 + 1\n$$\n"), "x^2 + 1");
    }

    #[test]
    fn task_items_keep_their_check_state() {
        assert_eq!(
            text_of("- [x] done\n- [ ] open\n"),
            "- [x] done\n- [ ] open"
        );
    }

    #[test]
    fn cjk_punctuation_stays_as_written() {
        // `）**的` needs the parser's synthetic marker to close the bold;
        // the marker must not reach the copied text.
        let md = "「你好」，世界。**（重點）**的文字……完。\n";
        assert_eq!(text_of(md), "「你好」，世界。（重點）的文字……完。");
    }

    #[test]
    fn data_documents_copy_their_source_verbatim() {
        let code = "{\n  \"a\": [1, 2],\n  \"b\": \"x\"\n}";
        let blocks = vec![(
            crate::ast::BlockId(1),
            crate::ast::Block::CodeBlock {
                lang: Some("json".into()),
                code: format!("{code}\n"),
                spans: Vec::new(),
            },
        )];
        assert_eq!(document(&blocks), code);
    }
}
