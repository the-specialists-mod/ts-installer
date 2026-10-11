//! Reading and editing GoldSrc `.cfg` files (`ts/config.cfg`): cvar settings and key bindings.
//!
//! Edits keep every other line (comments, `exec`, aliases) as it was. The engine rewrites
//! `config.cfg` when it quits, so only edit it while the game isn't running.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// A `.cfg` file held as lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CfgFile {
    pub path: PathBuf,
    lines: Vec<String>,
    /// The file used `\r\n` line endings (as shipped by TS); saving keeps them.
    crlf: bool,
}

/// Splits one cfg line into tokens: quoted strings or whitespace-separated words. Stops at `//`
/// outside quotes. (Several commands on one line separated by `;` are returned as one command;
/// config.cfg never writes those.)
pub fn tokenize(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        match chars.peek() {
            None => break,
            Some('"') => {
                chars.next();
                let mut tok = String::new();
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    tok.push(c);
                }
                out.push(tok);
            }
            Some('/') => {
                let mut ahead = chars.clone();
                ahead.next();
                if ahead.peek() == Some(&'/') {
                    break;
                }
                out.push(word(&mut chars));
            }
            Some(_) => out.push(word(&mut chars)),
        }
    }
    out
}

fn word(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut tok = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c == '"' {
            break;
        }
        tok.push(c);
        chars.next();
    }
    tok
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "'"))
}

impl CfgFile {
    /// Loads `path`; a missing file is an empty config.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read(path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        Ok(Self::parse(path, &text))
    }

    pub fn parse(path: &Path, text: &str) -> Self {
        Self { path: path.to_path_buf(), lines: text.lines().map(str::to_owned).collect(), crlf: text.contains("\r\n") }
    }

    pub fn text(&self) -> String {
        let eol = if self.crlf { "\r\n" } else { "\n" };
        let mut s = self.lines.join(eol);
        s.push_str(eol);
        s
    }

    /// Writes the file back, keeping a `.bak` copy of the previous version.
    pub fn save(&self) -> Result<()> {
        if self.path.exists() {
            let _ = std::fs::copy(&self.path, self.path.with_extension("cfg.bak"));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, self.text()).with_context(|| format!("write {}", self.path.display()))
    }

    /// Index of the last line that sets `name` (a cvar or command name, case-insensitive).
    fn find_cvar(&self, name: &str) -> Option<usize> {
        self.lines.iter().rposition(|l| tokenize(l).first().is_some_and(|t| t.eq_ignore_ascii_case(name)))
    }

    /// The value of cvar `name`, as the last line setting it has it.
    pub fn get(&self, name: &str) -> Option<String> {
        let line = &self.lines[self.find_cvar(name)?];
        tokenize(line).into_iter().nth(1)
    }

    /// Sets cvar `name`, replacing the last line that sets it, or appending one.
    pub fn set(&mut self, name: &str, value: &str) {
        let line = format!("{name} {}", quote(value));
        match self.find_cvar(name) {
            Some(i) => self.lines[i] = line,
            None => self.lines.push(line),
        }
    }

    /// Every cvar-style line (`name value`) other than binds, in file order, last setting wins.
    pub fn cvars(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for line in &self.lines {
            let toks = tokenize(line);
            if toks.len() != 2 || toks[0].eq_ignore_ascii_case("bind") || toks[0].eq_ignore_ascii_case("exec") {
                continue;
            }
            match out.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&toks[0])) {
                Some(entry) => entry.1 = toks[1].clone(),
                None => out.push((toks[0].clone(), toks[1].clone())),
            }
        }
        out
    }

    fn find_bind(&self, key: &str) -> Option<usize> {
        self.lines.iter().rposition(|l| {
            let t = tokenize(l);
            t.len() >= 2 && t[0].eq_ignore_ascii_case("bind") && t[1].eq_ignore_ascii_case(key)
        })
    }

    /// Every `bind key command`, in file order, with lower-case key names.
    pub fn binds(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for line in &self.lines {
            let t = tokenize(line);
            if t.len() >= 3 && t[0].eq_ignore_ascii_case("bind") {
                let key = t[1].to_ascii_lowercase();
                match out.iter_mut().find(|(k, _)| *k == key) {
                    Some(entry) => entry.1 = t[2].clone(),
                    None => out.push((key, t[2].clone())),
                }
            }
        }
        out
    }

    /// The command bound to `key`.
    pub fn bind_of(&self, key: &str) -> Option<String> {
        tokenize(&self.lines[self.find_bind(key)?]).into_iter().nth(2)
    }

    /// Keys bound to `command` (case-insensitive).
    pub fn keys_for(&self, command: &str) -> Vec<String> {
        self.binds().into_iter().filter(|(_, c)| c.eq_ignore_ascii_case(command)).map(|(k, _)| k).collect()
    }

    /// Binds `key` to `command`, replacing an existing bind of that key.
    pub fn bind(&mut self, key: &str, command: &str) {
        let line = format!("bind {} {}", quote(&key.to_ascii_lowercase()), quote(command));
        match self.find_bind(key) {
            Some(i) => self.lines[i] = line,
            None => {
                // Keep binds together, after the last one.
                let at = self.lines.iter().rposition(|l| tokenize(l).first().is_some_and(|t| t.eq_ignore_ascii_case("bind")));
                match at {
                    Some(i) => self.lines.insert(i + 1, line),
                    None => self.lines.push(line),
                }
            }
        }
    }

    /// Removes every bind of `key` (the engine's `unbind`).
    pub fn unbind(&mut self, key: &str) {
        self.lines.retain(|l| {
            let t = tokenize(l);
            !(t.len() >= 2 && t[0].eq_ignore_ascii_case("bind") && t[1].eq_ignore_ascii_case(key))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "// This file is overwritten whenever you change your user settings in the game.\n\
unbindall\n\
bind \"w\" \"+forward\"\n\
bind \"MOUSE1\" \"+attack\"\n\
sensitivity \"3\"\n\
name \"Player\"\n\
alias foo \"say hi; wait\"\n";

    #[test]
    fn tokenizes() {
        assert_eq!(tokenize(r#"bind "w" "+forward" // move"#), ["bind", "w", "+forward"]);
        assert_eq!(tokenize("fps_max 100"), ["fps_max", "100"]);
        assert_eq!(tokenize(r#"name """#), ["name", ""]);
        assert_eq!(tokenize("  // only a comment"), Vec::<String>::new());
    }

    #[test]
    fn reads_and_edits_cvars() {
        let mut c = CfgFile::parse(Path::new("config.cfg"), SAMPLE);
        assert_eq!(c.get("sensitivity").as_deref(), Some("3"));
        assert_eq!(c.get("NAME").as_deref(), Some("Player"));
        c.set("sensitivity", "2.5");
        c.set("fps_max", "200");
        assert_eq!(c.get("sensitivity").as_deref(), Some("2.5"));
        assert!(c.text().contains("sensitivity \"2.5\"\n"));
        assert!(c.text().ends_with("fps_max \"200\"\n"));
        assert!(c.text().contains("alias foo \"say hi; wait\""));
        assert!(c.cvars().iter().any(|(n, v)| n == "name" && v == "Player"));
    }

    #[test]
    fn keeps_crlf() {
        let mut c = CfgFile::parse(Path::new("config.cfg"), "bind \"w\" \"+forward\"\r\nname \"a\"\r\n");
        c.set("name", "b");
        assert_eq!(c.text(), "bind \"w\" \"+forward\"\r\nname \"b\"\r\n");
    }

    #[test]
    fn reads_and_edits_binds() {
        let mut c = CfgFile::parse(Path::new("config.cfg"), SAMPLE);
        assert_eq!(c.bind_of("mouse1").as_deref(), Some("+attack"));
        assert_eq!(c.keys_for("+forward"), ["w"]);
        c.bind("UPARROW", "+forward");
        assert_eq!(c.keys_for("+forward"), ["w", "uparrow"]);
        // New binds go after the last existing one.
        assert_eq!(tokenize(&c.lines[4]), ["bind", "uparrow", "+forward"]);
        c.bind("w", "+jump");
        assert_eq!(c.bind_of("w").as_deref(), Some("+jump"));
        c.unbind("w");
        assert_eq!(c.bind_of("w"), None);
        assert_eq!(c.binds().len(), 2);
    }
}
