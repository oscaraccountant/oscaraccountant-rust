//! Borderless tables that never wrap: two spaces between columns, a bold header, one line per
//! row. When a row is wider than the terminal, the columns of the lowest priority go first (the
//! others keep their order), then the text columns are cut short with `…`. Ids, dates, money and
//! statuses are never cut.

use unicode_width::UnicodeWidthStr;

use super::format::{Align, Cell};
use super::theme::Theme;

/// The space between two columns.
const GAP: usize = 2;

/// A text column is not cut below this width while a lower column can still go.
const SOFT_MIN: usize = 16;

/// One column: its header and its cells. A column of a lower `priority` is kept longer; among
/// columns of one priority, the first is kept longest.
#[derive(Debug, Clone)]
pub struct Column {
    /// The header.
    pub header: String,
    /// One cell per row.
    pub cells: Vec<Cell>,
    /// Money and numbers.
    pub align: Align,
    /// 0 is kept longest. Default 0: the order of the columns decides.
    pub priority: usize,
}

impl Column {
    /// A column; its alignment is the one of its cells.
    pub fn new(header: String, cells: Vec<Cell>) -> Self {
        let align = if cells.iter().any(|cell| cell.align == Align::Right) {
            Align::Right
        } else {
            Align::Left
        };
        Self {
            header,
            cells,
            align,
            priority: 0,
        }
    }

    /// The same column with this priority.
    #[must_use]
    pub fn with_priority(mut self, priority: usize) -> Self {
        self.priority = priority;
        self
    }

    fn natural(&self) -> usize {
        self.cells
            .iter()
            .map(Cell::width)
            .chain(std::iter::once(self.header.width()))
            .max()
            .unwrap_or(0)
    }

    fn can_cut(&self) -> bool {
        self.cells.iter().any(|cell| cell.truncate)
    }

    /// The narrowest this column gets: its header, and every cell that is never cut.
    fn hard_min(&self) -> usize {
        self.cells
            .iter()
            .filter(|cell| !cell.truncate)
            .map(Cell::width)
            .chain(std::iter::once(self.header.width().min(SOFT_MIN)))
            .max()
            .unwrap_or(0)
    }
}

/// The columns that are shown and the width of each, for a terminal of `width` columns (`None`:
/// no limit). Gives the indexes of the kept columns, in their order.
pub fn layout(columns: &[Column], width: Option<usize>) -> Vec<(usize, usize)> {
    let mut kept: Vec<usize> = (0..columns.len()).collect();
    loop {
        let widths = fit(columns, &kept, width);
        let fits = width.is_none_or(|limit| total(&widths) <= limit);
        let soft = widths.iter().zip(&kept).all(|(size, index)| {
            let column = &columns[*index];
            !column.can_cut() || *size >= column.natural().min(SOFT_MIN.max(column.hard_min()))
        });
        if (fits && soft) || kept.len() <= 1 {
            return kept.into_iter().zip(widths).collect();
        }
        if let Some(position) = (0..kept.len()).max_by_key(|position| {
            let index = kept[*position];
            (columns[index].priority, index)
        }) {
            kept.remove(position);
        }
    }
}

fn total(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + GAP * widths.len().saturating_sub(1)
}

/// The widths of the kept columns: each at its natural width, then the text columns cut, the
/// widest first, until the row fits.
fn fit(columns: &[Column], kept: &[usize], width: Option<usize>) -> Vec<usize> {
    let mut widths: Vec<usize> = kept.iter().map(|index| columns[*index].natural()).collect();
    let Some(limit) = width else {
        return widths;
    };
    while total(&widths) > limit {
        let widest = widths
            .iter()
            .enumerate()
            .filter(|(position, size)| {
                let column = &columns[kept[*position]];
                column.can_cut() && **size > column.hard_min().max(1)
            })
            .max_by_key(|(_, size)| **size)
            .map(|(position, _)| position);
        match widest {
            Some(position) => widths[position] -= 1,
            None => break,
        }
    }
    widths
}

/// The table as text, one line per row, with no trailing spaces.
pub fn render(columns: &[Column], width: Option<usize>, theme: &Theme) -> String {
    let shown = layout(columns, width);
    let rows = columns.first().map_or(0, |column| column.cells.len());
    let mut lines = Vec::with_capacity(rows + 1);
    let header: Vec<(String, usize, usize, Align)> = shown
        .iter()
        .map(|(index, size)| {
            let column = &columns[*index];
            let text = super::format::cut(&column.header, *size);
            let used = text.width();
            (theme.bold(&text), used, *size, column.align)
        })
        .collect();
    lines.push(join(header));
    for row in 0..rows {
        let cells = shown
            .iter()
            .map(|(index, size)| {
                let column = &columns[*index];
                let cell = column.cells[row].truncated(*size, theme);
                (cell.paint(theme), cell.width(), *size, column.align)
            })
            .collect();
        lines.push(join(cells));
    }
    lines.join("\n")
}

/// One line: each piece padded to its width, the last one without trailing spaces.
fn join(pieces: Vec<(String, usize, usize, Align)>) -> String {
    let count = pieces.len();
    let mut line = String::new();
    for (position, (text, used, size, align)) in pieces.into_iter().enumerate() {
        if position > 0 {
            line.push_str(&" ".repeat(GAP));
        }
        let pad = " ".repeat(size.saturating_sub(used));
        match align {
            Align::Right => {
                line.push_str(&pad);
                line.push_str(&text);
            }
            Align::Left => {
                line.push_str(&text);
                if position + 1 < count {
                    line.push_str(&pad);
                }
            }
        }
    }
    line.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::format::Cell;

    fn columns() -> Vec<Column> {
        vec![
            Column::new(
                "NUMBER".into(),
                vec![
                    Cell::fixed("2026-0042".into(), None, Align::Left),
                    Cell::fixed("2026-0043".into(), None, Align::Left),
                ],
            ),
            Column::new(
                "PARTY".into(),
                vec![
                    Cell::text("Acme Holdings International OÜ".into()),
                    Cell::text("Põhjala Tehas AS".into()),
                ],
            ),
            Column::new(
                "GROSS".into(),
                vec![
                    Cell::fixed("1 220.00 EUR".into(), None, Align::Right),
                    Cell::fixed("12.00 EUR".into(), None, Align::Right),
                ],
            ),
            Column::new(
                "DUE DATE".into(),
                vec![
                    Cell::fixed("2026-10-12".into(), None, Align::Left),
                    Cell::fixed("overdue 5d".into(), None, Align::Left),
                ],
            ),
            Column::new(
                "DESCRIPTION".into(),
                vec![
                    Cell::text("Consulting services for the third quarter of the year".into()),
                    Cell::text("Rent".into()),
                ],
            ),
        ]
    }

    fn widest_line(text: &str) -> usize {
        text.lines().map(UnicodeWidthStr::width).max().unwrap_or(0)
    }

    #[test]
    fn at_200_columns_everything_fits() {
        let text = render(&columns(), Some(200), &Theme::PLAIN);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "NUMBER     PARTY                                  GROSS  DUE DATE    DESCRIPTION"
        );
        assert!(lines[1].contains("Acme Holdings International OÜ  1 220.00 EUR"));
        assert!(lines[2].contains("Põhjala Tehas AS                   12.00 EUR"));
        assert!(lines[1].ends_with("year"));
        assert_eq!(lines[2].trim_end(), lines[2]);
    }

    #[test]
    fn at_120_columns_the_description_is_cut() {
        let text = render(&columns(), Some(100), &Theme::PLAIN);
        assert!(widest_line(&text) <= 100, "{text}");
        assert!(text.contains("DESCRIPTION"));
        assert!(text.contains('…'));
        assert!(text.contains("Acme Holdings International OÜ"));
        let text = render(&columns(), Some(120), &Theme::PLAIN);
        assert!(widest_line(&text) <= 120);
    }

    #[test]
    fn at_80_and_40_columns_lower_columns_go_first() {
        let text = render(&columns(), Some(80), &Theme::PLAIN);
        assert!(widest_line(&text) <= 80, "{text}");
        assert!(text.contains("DUE DATE"));
        let text = render(&columns(), Some(40), &Theme::PLAIN);
        assert!(widest_line(&text) <= 40, "{text}");
        assert!(!text.contains("DESCRIPTION") && !text.contains("DUE DATE"));
        assert!(text.contains("2026-0042"));
        assert!(!text.contains("2026-00…"));
        let text = render(&columns(), Some(12), &Theme::PLAIN);
        assert_eq!(text.lines().next(), Some("NUMBER"));
    }

    #[test]
    fn the_lower_column_goes_then_the_title_is_cut() {
        let columns = vec![
            Column::new(
                "NAME".into(),
                vec![Cell::text("a very long name indeed".into())],
            ),
            Column::new(
                "A".into(),
                vec![Cell::fixed("xxxx".into(), None, Align::Left)],
            ),
        ];
        let text = render(&columns, Some(16), &Theme::PLAIN);
        assert_eq!(text, "NAME\na very long nam…");
        let text = render(&columns, Some(26), &Theme::PLAIN);
        assert_eq!(text, "NAME                  A\na very long name in…  xxxx");
        let text = render(&columns, None, &Theme::PLAIN);
        assert!(text.ends_with("a very long name indeed  xxxx"));
    }

    #[test]
    fn the_lowest_priority_goes_first_and_the_order_stays() {
        let fixed = |text: &str| Cell::fixed(text.into(), None, Align::Left);
        let columns = vec![
            Column::new("A".into(), vec![fixed("aaaa")]),
            Column::new("B".into(), vec![fixed("bbbb")]).with_priority(5),
            Column::new("C".into(), vec![fixed("cccc")]).with_priority(1),
            Column::new("D".into(), vec![fixed("dddd")]).with_priority(5),
        ];
        assert_eq!(
            render(&columns, Some(16), &Theme::PLAIN),
            "A     B     C\naaaa  bbbb  cccc"
        );
        assert_eq!(
            render(&columns, Some(10), &Theme::PLAIN),
            "A     C\naaaa  cccc"
        );
    }

    #[test]
    fn a_bold_header_with_colour() {
        let theme = Theme {
            color: true,
            ..Theme::PLAIN
        };
        let text = render(&columns(), Some(200), &theme);
        assert!(text.starts_with("\u{1b}[1mNUMBER\u{1b}[0m"));
        assert_eq!(render(&[], Some(80), &theme), "");
    }
}
