//! The pager of the client agreement.
//!
//! A full-screen view of the Markdown text: arrows, PgUp/PgDn, Home/End, `/` to search, `l` to
//! switch the language, `q` to leave. The accept action is off until the person has reached the
//! end of the text; then `a` asks the person to type `I accept`.
//!
//! [`Pager`] is the state machine and holds no terminal: a key goes in, an [`Effect`] comes out,
//! and [`draw`] paints the state on any ratatui backend. [`run`] is the loop on the real terminal.

use std::io;

use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::markdown;

/// The phrase the person types to accept.
pub const ACCEPT_PHRASE: &str = "I accept";

/// The lines of the footer.
const FOOTER: u16 = 3;

/// The text the pager shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// The version: the date of the text.
    pub version: String,
    /// `en` or `et`.
    pub language: String,
    /// The Markdown text.
    pub text: String,
    /// The SHA-256 of the text, as the server gave it.
    pub sha256: String,
}

/// A key the pager understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Home.
    Home,
    /// End.
    End,
    /// Enter.
    Enter,
    /// Escape.
    Esc,
    /// Backspace.
    Backspace,
    /// Ctrl-C.
    Interrupt,
    /// A character.
    Char(char),
}

/// What the pager is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Reading.
    Read,
    /// Typing a search, with the text so far.
    Search(String),
    /// Typing the phrase of the acceptance, with the text so far.
    Accept(String),
}

/// What a key asks of the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Nothing: draw again.
    None,
    /// Leave without accepting.
    Quit,
    /// Show the other language.
    SwitchLanguage,
    /// The person typed the phrase: accept.
    Accept,
}

/// The state of the pager.
#[derive(Debug, Clone)]
pub struct Pager {
    doc: Document,
    company: String,
    color: bool,
    width: u16,
    height: u16,
    lines: Vec<Line<'static>>,
    plain: Vec<String>,
    top: usize,
    reached_end: bool,
    mode: Mode,
    search: Option<String>,
    message: Option<String>,
}

impl Pager {
    /// A pager of `doc` for `company`, on a screen of `width` by `height`.
    pub fn new(doc: Document, company: &str, color: bool, width: u16, height: u16) -> Self {
        let mut pager = Self {
            doc,
            company: company.to_owned(),
            color,
            width: 0,
            height: 0,
            lines: Vec::new(),
            plain: Vec::new(),
            top: 0,
            reached_end: false,
            mode: Mode::Read,
            search: None,
            message: None,
        };
        pager.resize(width, height);
        pager
    }

    /// The screen changed size: wrap the text again, and keep the top line in range.
    pub fn resize(&mut self, width: u16, height: u16) {
        if (width, height) == (self.width, self.height) {
            return;
        }
        let ratio = if self.lines.is_empty() {
            0.0
        } else {
            self.top as f64 / self.lines.len() as f64
        };
        self.width = width;
        self.height = height;
        self.lines = markdown::render(
            &self.doc.text,
            usize::from(width.saturating_sub(2)),
            self.color,
        );
        self.plain = self.lines.iter().map(markdown::plain).collect();
        self.top = ((ratio * self.lines.len() as f64) as usize).min(self.max_top());
        self.note_end();
    }

    /// The rows of the text.
    pub fn body_height(&self) -> usize {
        usize::from(self.height.saturating_sub(FOOTER).max(1))
    }

    fn max_top(&self) -> usize {
        self.lines.len().saturating_sub(self.body_height())
    }

    /// The last line is on the screen now.
    pub fn at_end(&self) -> bool {
        self.top >= self.max_top()
    }

    fn note_end(&mut self) {
        if self.at_end() {
            self.reached_end = true;
        }
    }

    /// The person may accept: the end of the text was on the screen once.
    pub fn can_accept(&self) -> bool {
        self.reached_end
    }

    /// The first line on the screen.
    #[cfg(test)]
    pub fn top(&self) -> usize {
        self.top
    }

    /// The mode.
    #[cfg(test)]
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// The document.
    #[cfg(test)]
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// The line of the message, if one is shown.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    fn scroll_to(&mut self, top: usize) {
        self.top = top.min(self.max_top());
        self.note_end();
    }

    fn scroll_by(&mut self, delta: isize) {
        let top = self.top.saturating_add_signed(delta);
        self.scroll_to(top);
    }

    /// The next line from `from` (wrapping around) that holds the search.
    fn find(&self, from: usize, forward: bool) -> Option<usize> {
        let needle = self.search.as_deref()?.to_lowercase();
        if needle.is_empty() || self.plain.is_empty() {
            return None;
        }
        let count = self.plain.len();
        (0..count)
            .map(|step| {
                if forward {
                    (from + step) % count
                } else {
                    (from + count - step) % count
                }
            })
            .find(|index| self.plain[*index].to_lowercase().contains(&needle))
    }

    fn jump(&mut self, forward: bool) {
        let from = if forward {
            self.top + 1
        } else {
            self.top + self.plain.len().max(1) - 1
        };
        match self.find(from % self.plain.len().max(1), forward) {
            Some(index) => self.scroll_to(index),
            None => {
                let text = self.search.clone().unwrap_or_default();
                self.message = Some(format!("Not found: {text}"));
            }
        }
    }

    /// One key.
    pub fn key(&mut self, key: Key) -> Effect {
        self.message = None;
        if key == Key::Interrupt {
            return Effect::Quit;
        }
        match std::mem::replace(&mut self.mode, Mode::Read) {
            Mode::Read => self.read_key(key),
            Mode::Search(text) => self.search_key(text, key),
            Mode::Accept(text) => self.accept_key(text, key),
        }
    }

    fn read_key(&mut self, key: Key) -> Effect {
        let page = isize::try_from(self.body_height().saturating_sub(1).max(1)).unwrap_or(1);
        match key {
            Key::Up | Key::Char('k') => self.scroll_by(-1),
            Key::Down | Key::Enter | Key::Char('j') => self.scroll_by(1),
            Key::PageUp | Key::Char('b') => self.scroll_by(-page),
            Key::PageDown | Key::Char(' ' | 'f') => self.scroll_by(page),
            Key::Home | Key::Char('g') => self.scroll_to(0),
            Key::End | Key::Char('G') => self.scroll_to(usize::MAX),
            Key::Char('/') => self.mode = Mode::Search(String::new()),
            Key::Char('n') => self.jump(true),
            Key::Char('N') => self.jump(false),
            Key::Char('l') => return Effect::SwitchLanguage,
            Key::Char('q') | Key::Esc => return Effect::Quit,
            Key::Char('a') => {
                if self.reached_end {
                    self.mode = Mode::Accept(String::new());
                } else {
                    self.message =
                        Some("Read the agreement to the end before you accept it.".into());
                }
            }
            _ => {}
        }
        Effect::None
    }

    fn search_key(&mut self, mut text: String, key: Key) -> Effect {
        match key {
            Key::Enter => {
                if !text.is_empty() {
                    self.search = Some(text);
                    // Search from the line on top, so a match on the screen is found first.
                    let from = self.top;
                    match self.find(from, true) {
                        Some(index) => self.scroll_to(index),
                        None => {
                            let text = self.search.clone().unwrap_or_default();
                            self.message = Some(format!("Not found: {text}"));
                        }
                    }
                }
            }
            Key::Esc => {}
            Key::Backspace => {
                text.pop();
                self.mode = Mode::Search(text);
            }
            Key::Char(ch) => {
                text.push(ch);
                self.mode = Mode::Search(text);
            }
            _ => self.mode = Mode::Search(text),
        }
        Effect::None
    }

    fn accept_key(&mut self, mut text: String, key: Key) -> Effect {
        match key {
            Key::Enter => {
                if text.trim().eq_ignore_ascii_case(ACCEPT_PHRASE) {
                    return Effect::Accept;
                }
                self.message = Some(format!("Type exactly: {ACCEPT_PHRASE}"));
                self.mode = Mode::Accept(String::new());
            }
            Key::Esc => {}
            Key::Backspace => {
                text.pop();
                self.mode = Mode::Accept(text);
            }
            Key::Char(ch) => {
                text.push(ch);
                self.mode = Mode::Accept(text);
            }
            _ => self.mode = Mode::Accept(text),
        }
        Effect::None
    }

    /// The name of the language of the text, and of the other one.
    fn languages(&self) -> (&'static str, &'static str) {
        if self.doc.language == "et" {
            ("Eesti", "English")
        } else {
            ("English", "Eesti")
        }
    }

    /// The three lines of the footer.
    pub fn footer(&self) -> [String; 3] {
        let (language, other) = self.languages();
        let percent = if self.lines.is_empty() || self.at_end() {
            100
        } else {
            (self.top + self.body_height()) * 100 / self.lines.len()
        };
        let status = format!(
            "Client agreement · {} · version {} · {language} · {percent}%",
            self.company, self.doc.version
        );
        let hash = format!("SHA-256 {}", self.doc.sha256);
        let keys = match (&self.message, &self.mode) {
            (Some(message), _) => message.clone(),
            (None, Mode::Search(text)) => format!("/{text}"),
            (None, Mode::Accept(text)) => format!(
                "Type \"{ACCEPT_PHRASE}\" to accept on behalf of {} (Esc to go back): {text}",
                self.company
            ),
            (None, Mode::Read) if self.reached_end => {
                format!("a accept · / search · l {other} · q leave")
            }
            (None, Mode::Read) => format!(
                "Read to the end to accept · ↑↓ PgUp PgDn Home End · / search · l {other} · q leave"
            ),
        };
        [status, hash, keys]
    }

    /// The lines on the screen, with the lines of the search marked.
    pub fn visible(&self) -> Vec<Line<'static>> {
        let needle = self.search.as_deref().map(str::to_lowercase);
        self.lines
            .iter()
            .zip(&self.plain)
            .skip(self.top)
            .take(self.body_height())
            .map(|(line, plain)| {
                let hit = needle.as_deref().is_some_and(|needle| {
                    !needle.is_empty() && plain.to_lowercase().contains(needle)
                });
                let mut spans = vec![Span::raw(if hit { "▌" } else { " " })];
                spans.extend(line.spans.iter().cloned());
                Line::from(spans)
            })
            .collect()
    }
}

/// Paint the pager on a frame.
pub fn draw(frame: &mut ratatui::Frame<'_>, pager: &Pager) {
    let area = frame.area();
    let [body, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(FOOTER)]).areas(area);
    frame.render_widget(Paragraph::new(pager.visible()), body);
    let [status, hash, keys] = pager.footer();
    let bar = Style::default().add_modifier(Modifier::REVERSED);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let lines = vec![
        Line::styled(status, bar),
        Line::styled(hash, dim),
        Line::styled(
            keys,
            if pager.can_accept() || pager.message().is_some() {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            height: footer.height.min(FOOTER),
            ..footer
        },
    );
}

/// How the pager ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The person typed the phrase.
    Accepted,
    /// The person left.
    Quit,
    /// The person asked for the other language.
    SwitchLanguage,
}

/// A source of keys and sizes: the terminal, or a script in a test.
pub trait Input {
    /// The next key, or a new size of the screen.
    fn next(&mut self) -> io::Result<Event>;
}

/// What the input gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A key.
    Key(Key),
    /// The screen has this size now.
    Resize(u16, u16),
    /// Nothing the pager reads.
    Other,
}

/// Run the pager on `terminal` with keys from `input` until it ends.
pub fn drive<B: Backend>(
    terminal: &mut Terminal<B>,
    input: &mut dyn Input,
    pager: &mut Pager,
) -> io::Result<Outcome>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    loop {
        terminal
            .draw(|frame| draw(frame, pager))
            .map_err(io::Error::other)?;
        match input.next()? {
            Event::Key(key) => match pager.key(key) {
                Effect::None => {}
                Effect::Quit => return Ok(Outcome::Quit),
                Effect::SwitchLanguage => return Ok(Outcome::SwitchLanguage),
                Effect::Accept => return Ok(Outcome::Accepted),
            },
            Event::Resize(width, height) => pager.resize(width, height),
            Event::Other => {}
        }
    }
}

/// The keys of the real terminal.
struct Keyboard;

impl Input for Keyboard {
    fn next(&mut self) -> io::Result<Event> {
        use ratatui::crossterm::event::{self, Event as Raw, KeyCode, KeyEventKind, KeyModifiers};
        Ok(match event::read()? {
            Raw::Key(key) if key.kind != KeyEventKind::Release => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                Event::Key(match key.code {
                    KeyCode::Char('c' | 'd') if control => Key::Interrupt,
                    KeyCode::Up => Key::Up,
                    KeyCode::Down => Key::Down,
                    KeyCode::PageUp => Key::PageUp,
                    KeyCode::PageDown => Key::PageDown,
                    KeyCode::Home => Key::Home,
                    KeyCode::End => Key::End,
                    KeyCode::Enter => Key::Enter,
                    KeyCode::Esc => Key::Esc,
                    KeyCode::Backspace => Key::Backspace,
                    KeyCode::Char(ch) => Key::Char(ch),
                    _ => return Ok(Event::Other),
                })
            }
            Raw::Resize(width, height) => Event::Resize(width, height),
            _ => Event::Other,
        })
    }
}

/// Show `pager` on the terminal of the process, full screen, until the person accepts, leaves or
/// switches the language. The terminal is restored in every case.
pub fn run(pager: &mut Pager) -> io::Result<Outcome> {
    use ratatui::crossterm::execute;
    use ratatui::crossterm::terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode, size,
    };
    let (width, height) = size()?;
    pager.resize(width, height);
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // A clear screen, with no query of the cursor: a terminal that never answers one does not
    // hold the pager up.
    if let Err(error) = execute!(stdout, EnterAlternateScreen, Clear(ClearType::All)) {
        let _ = disable_raw_mode();
        return Err(error);
    }
    let result = (|| {
        let mut terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
        let outcome = drive(&mut terminal, &mut Keyboard, pager);
        terminal.show_cursor()?;
        outcome
    })();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = disable_raw_mode();
    result
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;

    use super::*;

    fn doc(paragraphs: usize) -> Document {
        let mut text = String::from("# Client agreement\n\n");
        for index in 1..=paragraphs {
            text.push_str(&format!(
                "{index}. Clause number {index} of the agreement.\n\n"
            ));
        }
        text.push_str("The end of the text.\n");
        Document {
            version: "2026-10-10".into(),
            language: "en".into(),
            text,
            sha256: "ab".repeat(32),
        }
    }

    fn pager(paragraphs: usize, height: u16) -> Pager {
        Pager::new(doc(paragraphs), "Acme OÜ", false, 60, height)
    }

    #[test]
    fn accept_stays_off_before_the_end() {
        let mut pager = pager(40, 13);
        assert!(!pager.can_accept());
        assert_eq!(pager.key(Key::Char('a')), Effect::None);
        assert_eq!(pager.mode(), &Mode::Read);
        assert!(pager.message().unwrap().contains("to the end"));
        // A page down and back up never reaches the end.
        pager.key(Key::PageDown);
        pager.key(Key::Up);
        pager.key(Key::Char('k'));
        pager.key(Key::Home);
        assert!(!pager.can_accept());
        // Typing the phrase in read mode does nothing either.
        for ch in ACCEPT_PHRASE.chars() {
            assert_ne!(pager.key(Key::Char(ch)), Effect::Accept);
        }
        assert_eq!(pager.key(Key::Enter), Effect::None);
        assert!(!pager.can_accept());
    }

    #[test]
    fn reaching_the_end_turns_accept_on_and_keeps_it() {
        let mut pager = pager(40, 13);
        pager.key(Key::End);
        assert!(pager.at_end() && pager.can_accept());
        pager.key(Key::Home);
        assert!(!pager.at_end());
        assert!(pager.can_accept(), "once reached, the end stays reached");
        assert_eq!(pager.key(Key::Char('a')), Effect::None);
        assert_eq!(pager.mode(), &Mode::Accept(String::new()));
        for ch in "i accep".chars() {
            pager.key(Key::Char(ch));
        }
        pager.key(Key::Char('x'));
        pager.key(Key::Backspace);
        assert_eq!(pager.mode(), &Mode::Accept("i accep".into()));
        pager.key(Key::Up);
        assert_eq!(pager.key(Key::Enter), Effect::None, "a wrong phrase");
        assert!(pager.message().unwrap().contains("Type exactly"));
        assert_eq!(pager.mode(), &Mode::Accept(String::new()));
        pager.key(Key::Esc);
        assert_eq!(pager.mode(), &Mode::Read);
        pager.key(Key::Char('a'));
        for ch in ACCEPT_PHRASE.chars() {
            pager.key(Key::Char(ch));
        }
        assert_eq!(pager.key(Key::Enter), Effect::Accept);
    }

    #[test]
    fn a_short_text_is_read_at_once() {
        let pager = pager(1, 40);
        assert!(pager.can_accept());
        assert_eq!(
            pager.footer()[0],
            "Client agreement · Acme OÜ · version 2026-10-10 · English · 100%"
        );
        assert!(pager.footer()[2].starts_with("a accept"));
    }

    #[test]
    fn scrolls() {
        let mut pager = pager(40, 13);
        let page = pager.body_height() - 1;
        pager.key(Key::Down);
        pager.key(Key::Char('j'));
        assert_eq!(pager.top(), 2);
        pager.key(Key::PageDown);
        assert_eq!(pager.top(), 2 + page);
        pager.key(Key::Char(' '));
        pager.key(Key::Char('b'));
        pager.key(Key::PageUp);
        assert_eq!(pager.top(), 2);
        pager.key(Key::Char('G'));
        let end = pager.top();
        pager.key(Key::Down);
        assert_eq!(pager.top(), end, "no scroll past the end");
        pager.key(Key::Char('g'));
        assert_eq!(pager.top(), 0);
        pager.key(Key::Up);
        assert_eq!(pager.top(), 0);
        assert!(pager.footer()[2].starts_with("a accept"));
        let mut fresh = self::pager(40, 13);
        assert!(fresh.footer()[2].starts_with("Read to the end"));
        assert!(fresh.footer()[0].ends_with('%'));
        fresh.key(Key::Char('f'));
        assert!(fresh.top() > 0);
    }

    #[test]
    fn searches() {
        let mut pager = pager(40, 13);
        pager.key(Key::Char('/'));
        for ch in "number 30".chars() {
            pager.key(Key::Char(ch));
        }
        pager.key(Key::Char('x'));
        pager.key(Key::Backspace);
        pager.key(Key::Home);
        assert_eq!(pager.footer()[2], "/number 30");
        pager.key(Key::Enter);
        assert_eq!(pager.mode(), &Mode::Read);
        assert_eq!(
            pager.visible()[0].spans[0].content,
            "▌",
            "{:?}",
            pager.visible()[0]
        );
        assert_eq!(pager.visible()[1].spans[0].content, " ");
        let at = pager.top();
        pager.key(Key::Char('n'));
        assert_eq!(pager.top(), at, "one match: n finds it again");
        pager.key(Key::Char('N'));
        assert_eq!(pager.top(), at);
        pager.key(Key::Char('/'));
        for ch in "nowhere".chars() {
            pager.key(Key::Char(ch));
        }
        pager.key(Key::Enter);
        assert_eq!(pager.message(), Some("Not found: nowhere"));
        pager.key(Key::Char('n'));
        assert_eq!(pager.message(), Some("Not found: nowhere"));
        pager.key(Key::Char('/'));
        pager.key(Key::Esc);
        pager.key(Key::Char('/'));
        assert_eq!(pager.key(Key::Enter), Effect::None, "an empty search");
        assert_eq!(pager.mode(), &Mode::Read);
    }

    #[test]
    fn leaves_and_switches() {
        let mut pager = pager(3, 20);
        assert_eq!(pager.key(Key::Char('l')), Effect::SwitchLanguage);
        assert_eq!(pager.key(Key::Char('q')), Effect::Quit);
        assert_eq!(pager.key(Key::Esc), Effect::Quit);
        pager.key(Key::Char('a'));
        assert_eq!(pager.key(Key::Interrupt), Effect::Quit);
        let mut estonian = doc(3);
        estonian.language = "et".into();
        let pager = Pager::new(estonian, "Acme OÜ", false, 60, 40);
        assert!(pager.footer()[0].contains("Eesti"));
        assert!(pager.footer()[2].contains("l English"));
        assert_eq!(pager.document().language, "et");
    }

    #[test]
    fn resizes() {
        let mut pager = pager(40, 13);
        pager.key(Key::PageDown);
        let before = pager.top();
        pager.resize(30, 13);
        assert!(pager.top() >= before, "a narrower screen has more lines");
        pager.resize(30, 13);
        pager.resize(200, 400);
        assert!(pager.can_accept(), "the whole text fits");
    }

    #[test]
    fn draws_with_the_footer() {
        let mut pager = pager(2, 12);
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal.draw(|frame| draw(frame, &pager)).unwrap();
        let screen = format!("{}", terminal.backend());
        assert!(screen.contains("Client agreement"), "{screen}");
        assert!(screen.contains("SHA-256 abab"), "{screen}");
        assert!(screen.contains("version 2026-10-10"), "{screen}");
        pager.key(Key::End);
        pager.key(Key::Char('a'));
        terminal.draw(|frame| draw(frame, &pager)).unwrap();
        let screen = format!("{}", terminal.backend());
        assert!(screen.contains("Type \"I accept\""), "{screen}");
    }

    struct Script(Vec<Event>);

    impl Input for Script {
        fn next(&mut self) -> io::Result<Event> {
            if self.0.is_empty() {
                return Err(io::Error::other("no more keys"));
            }
            Ok(self.0.remove(0))
        }
    }

    #[test]
    fn drives_a_terminal() {
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        let mut keys = vec![
            Event::Other,
            Event::Resize(50, 12),
            Event::Key(Key::End),
            Event::Key(Key::Char('a')),
        ];
        keys.extend(ACCEPT_PHRASE.chars().map(|ch| Event::Key(Key::Char(ch))));
        keys.push(Event::Key(Key::Enter));
        let mut pager = pager(30, 12);
        let outcome = drive(&mut terminal, &mut Script(keys), &mut pager).unwrap();
        assert_eq!(outcome, Outcome::Accepted);
        let mut pager = self::pager(30, 12);
        let outcome = drive(
            &mut terminal,
            &mut Script(vec![Event::Key(Key::Char('l'))]),
            &mut pager,
        );
        assert_eq!(outcome.unwrap(), Outcome::SwitchLanguage);
        let outcome = drive(
            &mut terminal,
            &mut Script(vec![Event::Key(Key::Char('q'))]),
            &mut pager,
        );
        assert_eq!(outcome.unwrap(), Outcome::Quit);
        assert!(drive(&mut terminal, &mut Script(vec![]), &mut pager).is_err());
    }
}
