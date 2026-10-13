//! Opening the system browser for the loopback login: `$BROWSER` when set, else the opener of the
//! OS (`open` on macOS, the shell's URL handler on Windows, `xdg-open` and its kin on Linux).

use std::path::Path;
use std::process::{Command, Stdio};

use crate::env::Env;

/// Open `url`. `false` when no browser could be started; the caller then prints the URL.
pub fn open(url: &str, env: &Env) -> bool {
    match env.var("BROWSER") {
        Some(browser) => match command(browser, url) {
            Some(mut command) => command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok(),
            None => false,
        },
        None => open::that_detached(url).is_ok(),
    }
}

/// The command of `$BROWSER`: a program path (which may hold spaces), or words split on white
/// space. `%s` stands for the URL; with no `%s`, the URL is the last argument. No shell runs it.
pub fn command(browser: &str, url: &str) -> Option<Command> {
    let words: Vec<String> = if Path::new(browser).is_file() {
        vec![browser.to_owned()]
    } else {
        browser.split_whitespace().map(str::to_owned).collect()
    };
    let (program, args) = words.split_first()?;
    let mut command = Command::new(program);
    let mut placed = false;
    for arg in args {
        if arg.contains("%s") {
            placed = true;
            command.arg(arg.replace("%s", url));
        } else {
            command.arg(arg);
        }
    }
    if !placed {
        command.arg(url);
    }
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(command: &Command) -> Vec<String> {
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|word| word.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn builds_the_command() {
        let command = command("firefox --new-window", "https://x.test/?a=1&b=2").unwrap();
        assert_eq!(
            words(&command),
            ["firefox", "--new-window", "https://x.test/?a=1&b=2"]
        );
        let command = super::command("lynx -dump %s -nolist", "u").unwrap();
        assert_eq!(words(&command), ["lynx", "-dump", "u", "-nolist"]);
        assert!(super::command("   ", "u").is_none());
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_owned();
        assert_eq!(
            words(&super::command(&path, "u").unwrap()),
            [path.as_str(), "u"]
        );
    }

    #[test]
    fn opens_with_browser() {
        let env = Env::with_vars(&[("BROWSER", "oscar-no-such-browser-program")]);
        assert!(!open("https://x.test", &env));
        let env = Env::with_vars(&[("BROWSER", "\t")]);
        assert!(!open("https://x.test", &env));
        #[cfg(unix)]
        {
            let env = Env::with_vars(&[("BROWSER", "true")]);
            assert!(open("https://x.test", &env));
        }
    }
}
