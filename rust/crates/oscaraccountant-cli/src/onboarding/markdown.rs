//! The Markdown of the client agreement as the lines of the pager: headings, paragraphs, lists,
//! quotes, emphasis and code, wrapped to a width. Pure: a test reads the lines.

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// One block before the wrap: its styled text, and the prefixes of its first and next lines.
#[derive(Debug, Default)]
struct Block {
    segments: Vec<(String, Style)>,
    first: String,
    rest: String,
    /// A gap (an empty line) follows the block.
    gap: bool,
}

impl Block {
    fn is_empty(&self) -> bool {
        self.segments.iter().all(|(text, _)| text.trim().is_empty())
    }
}

/// The renderer: the blocks so far and the state of the inline styles.
struct Builder {
    color: bool,
    blocks: Vec<Block>,
    current: Option<Block>,
    styles: Vec<Style>,
    /// The lists open, each with its next number (`None`: bullets).
    lists: Vec<Option<u64>>,
    quote: usize,
}

impl Builder {
    fn indent(&self) -> String {
        let mut text = "  ".repeat(self.lists.len().saturating_sub(1));
        text.insert_str(0, &"│ ".repeat(self.quote));
        text
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, change: impl FnOnce(Style) -> Style) {
        let next = change(self.style());
        self.styles.push(next);
    }

    fn flush(&mut self) {
        if let Some(block) = self.current.take()
            && !block.is_empty()
        {
            self.blocks.push(block);
        }
    }

    fn open(&mut self, gap: bool) {
        self.flush();
        let indent = if self.lists.is_empty() {
            "│ ".repeat(self.quote)
        } else {
            let mut indent = self.indent();
            indent.push_str("  ");
            indent
        };
        self.current = Some(Block {
            first: indent.clone(),
            rest: indent,
            gap,
            ..Block::default()
        });
    }

    fn text(&mut self, text: &str) {
        let style = self.style();
        if self.current.is_none() {
            self.open(true);
        }
        if let Some(block) = self.current.as_mut() {
            block.segments.push((text.to_owned(), style));
        }
    }

    fn heading_style(&self, level: HeadingLevel) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD);
        let style = if level == HeadingLevel::H1 {
            style.add_modifier(Modifier::UNDERLINED)
        } else {
            style
        };
        if self.color {
            style.fg(Color::Cyan)
        } else {
            style
        }
    }

    fn start(&mut self, tag: &Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.open(true);
                let style = self.heading_style(*level);
                self.styles.push(style);
            }
            Tag::Paragraph => {
                // A paragraph of a list item keeps the marker of the item.
                let keep = self
                    .current
                    .as_ref()
                    .is_some_and(|block| block.segments.is_empty() && !self.lists.is_empty());
                if !keep {
                    self.open(true);
                }
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(*start);
            }
            Tag::Item => {
                self.flush();
                let indent = self.indent();
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => "• ".to_owned(),
                };
                let rest = format!("{indent}{}", " ".repeat(marker.width()));
                self.current = Some(Block {
                    first: format!("{indent}{marker}"),
                    rest,
                    gap: false,
                    ..Block::default()
                });
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.quote += 1;
            }
            Tag::Emphasis => self.push_style(|style| style.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|style| style.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(|style| style.add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { .. } => self.push_style(|style| style.add_modifier(Modifier::UNDERLINED)),
            Tag::CodeBlock(_) => self.open(true),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.flush();
            }
            TagEnd::Paragraph | TagEnd::Item | TagEnd::CodeBlock => self.flush(),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                // The last item of a list leaves a gap after it, unless a list goes on around it.
                if self.lists.is_empty()
                    && let Some(last) = self.blocks.last_mut()
                {
                    last.gap = true;
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.styles.pop();
            }
            _ => {}
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(&tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                let style = self.style().add_modifier(Modifier::BOLD);
                self.styles.push(style);
                self.text(&code);
                self.styles.pop();
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Rule => {
                self.flush();
                self.blocks.push(Block {
                    segments: vec![("───".to_owned(), Style::default())],
                    gap: true,
                    ..Block::default()
                });
            }
            Event::Html(html) | Event::InlineHtml(html) => self.text(&html),
            _ => {}
        }
    }
}

/// The lines of `markdown` at `width` columns (at least 10). `color`: headings in colour.
pub fn render(markdown: &str, width: usize, color: bool) -> Vec<Line<'static>> {
    let mut builder = Builder {
        color,
        blocks: Vec::new(),
        current: None,
        styles: Vec::new(),
        lists: Vec::new(),
        quote: 0,
    };
    for event in Parser::new(markdown) {
        builder.event(event);
    }
    builder.flush();
    let width = width.max(10);
    let mut lines = Vec::new();
    let count = builder.blocks.len();
    for (index, block) in builder.blocks.into_iter().enumerate() {
        lines.extend(wrap(&block, width));
        if block.gap && index + 1 < count {
            lines.push(Line::default());
        }
    }
    lines
}

/// The words of a block with their style; a `\n` is a word of its own.
fn words(block: &Block) -> Vec<(String, Style, bool)> {
    let mut words: Vec<(String, Style, bool)> = Vec::new();
    let mut space = false;
    for (text, style) in &block.segments {
        let mut word = String::new();
        for ch in text.chars() {
            if ch == '\n' {
                if !word.is_empty() {
                    words.push((std::mem::take(&mut word), *style, space));
                }
                words.push(("\n".to_owned(), *style, false));
                space = false;
            } else if ch.is_whitespace() {
                if !word.is_empty() {
                    words.push((std::mem::take(&mut word), *style, space));
                }
                space = true;
            } else {
                word.push(ch);
            }
        }
        if !word.is_empty() {
            // A word cut by a change of style is glued to the next part.
            words.push((word, *style, space));
            space = false;
        }
    }
    // The last line break of a code block ends the block; it is no empty line.
    while words.last().is_some_and(|(word, _, _)| word == "\n") {
        words.pop();
    }
    words
}

/// Wrap one block at `width`.
fn wrap(block: &Block, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut spans: Vec<Span<'static>> = vec![Span::raw(block.first.clone())];
    let mut used = block.first.width();
    let mut fresh = true;
    let rest_width = block.rest.width();
    for (word, style, space) in words(block) {
        if word == "\n" {
            lines.push(Line::from(std::mem::take(&mut spans)));
            spans.push(Span::raw(block.rest.clone()));
            used = rest_width;
            fresh = true;
            continue;
        }
        let gap = usize::from(space && !fresh);
        let size = word.width();
        if !fresh && used + gap + size > width {
            lines.push(Line::from(std::mem::take(&mut spans)));
            spans.push(Span::raw(block.rest.clone()));
            used = rest_width;
        } else if gap == 1 {
            spans.push(Span::raw(" "));
            used += 1;
        }
        // A word longer than a whole line is cut.
        let mut word = word;
        while used + word.width() > width && word.chars().count() > 1 {
            let room = width.saturating_sub(used).max(1);
            let cut: String = take_width(&word, room);
            let tail = word[cut.len()..].to_owned();
            spans.push(Span::styled(cut, style));
            lines.push(Line::from(std::mem::take(&mut spans)));
            spans.push(Span::raw(block.rest.clone()));
            used = rest_width;
            word = tail;
        }
        used += word.width();
        spans.push(Span::styled(word, style));
        fresh = false;
    }
    lines.push(Line::from(spans));
    lines
}

/// The longest start of `text` that fits in `room` columns (at least one character).
fn take_width(text: &str, room: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        let next = format!("{out}{ch}");
        if next.width() > room && !out.is_empty() {
            break;
        }
        out = next;
    }
    out
}

/// The text of a line, without its styles: what the search reads.
pub fn plain(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(plain).collect()
    }

    #[test]
    fn renders_headings_paragraphs_and_lists() {
        let text = "# Client agreement\n\nThe **service** and *the data*.\n\n## 1. Scope\n\n- one\n- two\n  1. first\n  2. second\n\nAfter `code`.\n\n> A quote.\n\n---\n\nEnd.";
        let lines = render(text, 40, false);
        assert_eq!(
            texts(&lines),
            [
                "Client agreement",
                "",
                "The service and the data.",
                "",
                "1. Scope",
                "",
                "• one",
                "• two",
                "  1. first",
                "  2. second",
                "",
                "After code.",
                "",
                "│ A quote.",
                "",
                "───",
                "",
                "End.",
            ]
        );
        let heading = &lines[0].spans[1];
        assert!(heading.style.add_modifier.contains(Modifier::BOLD));
        assert!(heading.style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(heading.style.fg, None);
        let strong = lines[2]
            .spans
            .iter()
            .find(|span| span.content == "service")
            .unwrap();
        assert!(strong.style.add_modifier.contains(Modifier::BOLD));
        let emphasis = lines[2]
            .spans
            .iter()
            .find(|span| span.content == "the")
            .unwrap();
        assert!(emphasis.style.add_modifier.contains(Modifier::ITALIC));
        let coloured = render("## Title", 40, true);
        assert_eq!(coloured[0].spans[1].style.fg, Some(Color::Cyan));
    }

    #[test]
    fn wraps_at_the_width() {
        let text = "1. A long item that does not fit on one line of the pager at all.\n";
        let lines = render(text, 24, false);
        assert_eq!(
            texts(&lines),
            [
                "1. A long item that does",
                "   not fit on one line",
                "   of the pager at all."
            ]
        );
        for line in &lines {
            assert!(line.width() <= 24);
        }
        let long = render(&"x".repeat(25), 10, false);
        assert_eq!(texts(&long), ["xxxxxxxxxx", "xxxxxxxxxx", "xxxxx"]);
        let breaks = render("one  \ntwo\n\n**bo**ld [link](https://x.test)", 40, false);
        assert_eq!(texts(&breaks), ["one", "two", "", "bold link"]);
        assert_eq!(take_width("äbc", 2), "äb");
        assert_eq!(take_width("abc", 0), "a");
    }

    #[test]
    fn loose_lists_and_code_blocks() {
        let text = "- first\n\n  more\n\n- second\n\n```\ncode line\n```\n\n~~old~~ <b>x</b>";
        let lines = render(text, 40, false);
        assert_eq!(
            texts(&lines),
            [
                "• first",
                "  more",
                "",
                "• second",
                "",
                "code line",
                "",
                "~~old~~ <b>x</b>"
            ]
        );
    }
}
