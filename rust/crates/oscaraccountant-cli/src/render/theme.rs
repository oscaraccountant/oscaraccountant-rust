//! The one theme of the CLI: six roles on the 16 ANSI colours, the symbols with their ASCII
//! fallback, and OSC 8 links.
//!
//! Two ways to use it:
//!
//! - The free functions [`paint`], [`bold`], [`symbol`] and [`link`] use the theme of standard
//!   error, set once per run by [`init`] (from `Output::process`). Prompts, notes and success
//!   lines use them.
//! - A [`Theme`] value, for text on standard output: the renderer builds one for the stream it
//!   writes to and passes it down.
//!
//! With colour off every function returns the text unchanged, so a caller never checks.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};

use anstyle::{AnsiColor, Effects, Style};

use crate::env::Env;

/// The role of a piece of text. Each role has one style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Done, paid, valid: green.
    Ok,
    /// Waiting, needs attention: yellow.
    Warn,
    /// Failed, overdue, a negative amount: red.
    Bad,
    /// Inactive, cancelled, a draft: bright black.
    Muted,
    /// A name, a command, a link: cyan.
    Accent,
    /// Secondary text, the `Meta` section: dimmed.
    Dim,
}

impl Role {
    /// The role of a status role name of `x-oscarai-cli` (`ok`, `warn`, `bad`, `muted`).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "ok" => Some(Self::Ok),
            "warn" => Some(Self::Warn),
            "bad" => Some(Self::Bad),
            "muted" => Some(Self::Muted),
            "accent" => Some(Self::Accent),
            "dim" => Some(Self::Dim),
            _ => None,
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Ok => AnsiColor::Green.on_default(),
            Self::Warn => AnsiColor::Yellow.on_default(),
            Self::Bad => AnsiColor::Red.on_default(),
            Self::Muted => AnsiColor::BrightBlack.on_default(),
            Self::Accent => AnsiColor::Cyan.on_default(),
            Self::Dim => Style::new().effects(Effects::DIMMED),
        }
    }
}

/// A symbol, in Unicode or in ASCII.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sym {
    /// `✓` or `v`: a success.
    Check,
    /// `✗` or `x`: a failure.
    #[allow(dead_code)] // For the prompts and the onboarding.
    Cross,
    /// `!`: a warning.
    #[allow(dead_code)] // For the prompts and the onboarding.
    Bang,
    /// `●` or `*`: a status.
    Dot,
    /// `›` or `>`: a next step, a prompt.
    Arrow,
    /// `…` or `...`: text cut short.
    Ellipsis,
    /// `·` or `-`: between two parts of a line.
    Middot,
}

impl Sym {
    fn text(self, unicode: bool) -> &'static str {
        match (self, unicode) {
            (Self::Check, true) => "✓",
            (Self::Check, false) => "v",
            (Self::Cross, true) => "✗",
            (Self::Cross, false) => "x",
            (Self::Bang, _) => "!",
            (Self::Dot, true) => "●",
            (Self::Dot, false) => "*",
            (Self::Arrow, true) => "›",
            (Self::Arrow, false) => ">",
            (Self::Ellipsis, true) => "…",
            (Self::Ellipsis, false) => "...",
            (Self::Middot, true) => "·",
            (Self::Middot, false) => "-",
        }
    }
}

/// How text is styled on one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// ANSI colours and bold.
    pub color: bool,
    /// Unicode symbols; ASCII when off.
    pub unicode: bool,
    /// OSC 8 links.
    pub hyperlinks: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self::PLAIN
    }
}

impl Theme {
    /// No colour, Unicode symbols, no links: what a test and a pipe get.
    pub const PLAIN: Self = Self {
        color: false,
        unicode: true,
        hyperlinks: false,
    };

    /// The theme of a stream that is (`tty`) or is not a terminal, in this environment.
    ///
    /// Colour: `NO_COLOR` turns it off, then `CLICOLOR_FORCE` (any value but `0`) turns it on,
    /// then `TERM=dumb` turns it off, else a terminal has colour. Symbols: ASCII with
    /// `TERM=dumb` or a locale that is not UTF-8. Links: a terminal with colour that supports
    /// OSC 8, or `FORCE_HYPERLINK=1`.
    pub fn for_stream(env: &Env, tty: bool, stream: supports_hyperlinks::Stream) -> Self {
        let dumb = env.var("TERM") == Some("dumb");
        let color = if env.var("NO_COLOR").is_some() {
            false
        } else if env.var("CLICOLOR_FORCE").is_some_and(|value| value != "0") {
            true
        } else {
            tty && !dumb
        };
        let hyperlinks = match env.var("FORCE_HYPERLINK") {
            Some(value) => value != "0",
            None => color && tty && !dumb && supports_hyperlinks::on(stream),
        };
        Self {
            color,
            unicode: !dumb && utf8_locale(env),
            hyperlinks,
        }
    }

    /// The text in the style of the role, or the text itself without colour.
    pub fn paint(&self, role: Role, text: &str) -> String {
        self.styled(role.style(), text)
    }

    /// The text in bold, or the text itself without colour.
    pub fn bold(&self, text: &str) -> String {
        self.styled(Style::new().bold(), text)
    }

    /// The text in bold and in the style of the role.
    pub fn paint_bold(&self, role: Role, text: &str) -> String {
        self.styled(role.style().bold(), text)
    }

    fn styled(&self, style: Style, text: &str) -> String {
        if self.color && !text.is_empty() {
            format!("{style}{text}{style:#}")
        } else {
            text.to_owned()
        }
    }

    /// The symbol, in Unicode or in ASCII.
    pub fn symbol(&self, sym: Sym) -> &'static str {
        sym.text(self.unicode)
    }

    /// The text as an OSC 8 link to the URL, or the text itself when links are off.
    pub fn link(&self, url: &str, text: &str) -> String {
        if self.hyperlinks && !url.is_empty() && url.is_ascii() && !url.contains('\u{1b}') {
            format!("\u{1b}]8;;{url}\u{1b}\\{text}\u{1b}]8;;\u{1b}\\")
        } else {
            text.to_owned()
        }
    }
}

/// The locale is UTF-8: the first of `LC_ALL`, `LC_CTYPE`, `LANG` that is set names UTF-8.
/// No locale at all counts as UTF-8 (Windows, and most terminals today).
fn utf8_locale(env: &Env) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|name| env.var(name))
        .is_none_or(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("utf-8") || value.contains("utf8")
        })
}

const SET: u8 = 1;
const COLOR: u8 = 2;
const UNICODE: u8 = 4;
const LINKS: u8 = 8;

/// The theme of standard error for this process.
static STDERR: AtomicU8 = AtomicU8::new(0);

thread_local! {
    static OVERRIDE: Cell<Option<Theme>> = const { Cell::new(None) };
}

/// Set the theme of standard error for this process. `Output::process` calls it once.
pub fn init(theme: Theme) {
    let mut bits = SET;
    for (on, bit) in [
        (theme.color, COLOR),
        (theme.unicode, UNICODE),
        (theme.hyperlinks, LINKS),
    ] {
        if on {
            bits |= bit;
        }
    }
    STDERR.store(bits, Ordering::Relaxed);
}

/// The theme of standard error: the one of this thread (a test), else the one of [`init`], else
/// [`Theme::PLAIN`].
pub fn current() -> Theme {
    if let Some(theme) = OVERRIDE.with(Cell::get) {
        return theme;
    }
    let bits = STDERR.load(Ordering::Relaxed);
    if bits & SET == 0 {
        return Theme::PLAIN;
    }
    Theme {
        color: bits & COLOR != 0,
        unicode: bits & UNICODE != 0,
        hyperlinks: bits & LINKS != 0,
    }
}

/// Use this theme on this thread until the guard is dropped. For tests.
#[cfg(test)]
pub fn scoped(theme: Theme) -> ScopedTheme {
    let previous = OVERRIDE.with(|cell| cell.replace(Some(theme)));
    ScopedTheme { previous }
}

/// Restores the theme of the thread when dropped.
#[cfg(test)]
#[derive(Debug)]
pub struct ScopedTheme {
    previous: Option<Theme>,
}

#[cfg(test)]
impl Drop for ScopedTheme {
    fn drop(&mut self) {
        OVERRIDE.with(|cell| cell.set(self.previous));
    }
}

/// The text in the style of the role, on standard error.
pub fn paint(role: Role, text: &str) -> String {
    current().paint(role, text)
}

/// The symbol, on standard error.
pub fn symbol(sym: Sym) -> &'static str {
    current().symbol(sym)
}

/// An OSC 8 link, on standard error.
pub fn link(url: &str, text: &str) -> String {
    current().link(url, text)
}

#[cfg(test)]
mod tests {
    use supports_hyperlinks::Stream;

    use super::*;

    const COLOR_ON: Theme = Theme {
        color: true,
        unicode: true,
        hyperlinks: true,
    };

    #[test]
    fn paints_only_with_colour() {
        assert_eq!(Theme::PLAIN.paint(Role::Bad, "x"), "x");
        assert_eq!(COLOR_ON.paint(Role::Bad, "x"), "\u{1b}[31mx\u{1b}[0m");
        assert_eq!(COLOR_ON.paint(Role::Ok, ""), "");
        assert_eq!(COLOR_ON.bold("h"), "\u{1b}[1mh\u{1b}[0m");
        assert!(COLOR_ON.paint_bold(Role::Warn, "w").contains("33m"));
        for role in [Role::Warn, Role::Muted, Role::Accent, Role::Dim] {
            assert!(COLOR_ON.paint(role, "t").starts_with('\u{1b}'));
        }
    }

    #[test]
    fn role_names() {
        assert_eq!(Role::from_name("ok"), Some(Role::Ok));
        assert_eq!(Role::from_name("warn"), Some(Role::Warn));
        assert_eq!(Role::from_name("bad"), Some(Role::Bad));
        assert_eq!(Role::from_name("muted"), Some(Role::Muted));
        assert_eq!(Role::from_name("accent"), Some(Role::Accent));
        assert_eq!(Role::from_name("dim"), Some(Role::Dim));
        assert_eq!(Role::from_name("other"), None);
    }

    #[test]
    fn symbols_have_an_ascii_fallback() {
        let ascii = Theme {
            unicode: false,
            ..Theme::PLAIN
        };
        let all = [
            Sym::Check,
            Sym::Cross,
            Sym::Bang,
            Sym::Dot,
            Sym::Arrow,
            Sym::Ellipsis,
            Sym::Middot,
        ];
        for sym in all {
            assert!(ascii.symbol(sym).is_ascii(), "{sym:?}");
        }
        assert_eq!(Theme::PLAIN.symbol(Sym::Check), "✓");
        assert_eq!(Theme::PLAIN.symbol(Sym::Dot), "●");
        assert_eq!(ascii.symbol(Sym::Ellipsis), "...");
    }

    #[test]
    fn links_only_when_supported() {
        assert_eq!(Theme::PLAIN.link("https://a", "t"), "t");
        assert_eq!(
            COLOR_ON.link("https://a", "t"),
            "\u{1b}]8;;https://a\u{1b}\\t\u{1b}]8;;\u{1b}\\"
        );
        assert_eq!(COLOR_ON.link("", "t"), "t");
        assert_eq!(COLOR_ON.link("https://a\u{1b}", "t"), "t");
    }

    #[test]
    fn reads_the_environment() {
        let theme = |vars: &[(&str, &str)], tty| {
            Theme::for_stream(&Env::with_vars(vars), tty, Stream::Stdout)
        };
        assert!(!theme(&[], false).color);
        assert!(theme(&[("FORCE_HYPERLINK", "0")], true).color);
        assert!(!theme(&[("NO_COLOR", "1")], true).color);
        assert!(theme(&[("CLICOLOR_FORCE", "1")], false).color);
        assert!(!theme(&[("CLICOLOR_FORCE", "0")], false).color);
        assert!(!theme(&[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")], false).color);
        let dumb = theme(&[("TERM", "dumb")], true);
        assert!(!dumb.color && !dumb.unicode && !dumb.hyperlinks);
        assert!(theme(&[], false).unicode);
        assert!(theme(&[("LANG", "en_US.UTF-8")], false).unicode);
        assert!(theme(&[("LC_ALL", "et_EE.utf8"), ("LANG", "C")], false).unicode);
        assert!(!theme(&[("LANG", "C")], false).unicode);
        assert!(theme(&[("FORCE_HYPERLINK", "1")], false).hyperlinks);
        assert!(!theme(&[], false).hyperlinks);
    }

    #[test]
    fn the_stderr_theme_and_the_thread_override() {
        {
            let _guard = scoped(COLOR_ON);
            assert_eq!(current(), COLOR_ON);
            assert!(paint(Role::Ok, "a").contains("32m"));
            assert_eq!(symbol(Sym::Check), "✓");
            assert!(link("https://a", "a").contains("]8;;"));
        }
        assert_eq!(OVERRIDE.with(Cell::get), None);
        let _guard = scoped(Theme::PLAIN);
        assert_eq!(paint(Role::Ok, "a"), "a");
    }

    #[test]
    fn init_sets_the_process_theme() {
        // A colour theme here would leak into the other tests of this process.
        init(Theme::PLAIN);
        assert_eq!(STDERR.load(Ordering::Relaxed), SET | UNICODE);
        let _guard = scoped(COLOR_ON);
        assert_eq!(current(), COLOR_ON);
    }
}
