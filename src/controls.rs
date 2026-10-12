//! Key bindings: the bindable actions (`gfx/shell/kb_act.lst`), the default binds, GoldSrc key
//! names, and rebinding on a [`CfgFile`] (`ts/config.cfg`).

use std::path::Path;

use crate::cfgfile::{CfgFile, tokenize};

/// One bindable command from `kb_act.lst`.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub command: String,
    pub description: String,
}

/// A heading from `kb_act.lst` and the actions under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    pub actions: Vec<Action>,
}

/// Default binds of TS 3.0, from the `config.cfg` its installer ships (TS has no `kb_def.lst`, and
/// Half-Life's binds keys TS uses differently, such as `q` and `f`).
const TS_DEFAULTS: &str = r#"
"TAB" "+showscores"
"ENTER" "+attack"
"ESCAPE" "cancelselect"
"SPACE" "+jump"
"'" "+moveup"
"+" "sizeup"
"," "+moveleft"
"-" "sizedown"
"." "+moveright"
"/" "+movedown"
"0" "slot10"
"1" "slot1"
"2" "slot2"
"3" "slot3"
"4" "slot4"
"5" "slot5"
"6" "slot6"
"7" "slot7"
"8" "slot8"
"9" "slot9"
"=" "sizeup"
"[" "invprev"
"]" "invnext"
"`" "toggleconsole"
"a" "+moveleft"
"b" "buy"
"c" "+duck"
"d" "+moveright"
"e" "+use"
"f" "firemode"
"g" "useitems"
"h" "impulse 201"
"k" "+voicerecord"
"q" "+alt1"
"r" "+reload"
"s" "+back"
"t" "drop"
"u" "messagemode2"
"v" "+alt2"
"w" "+forward"
"x" "thirdperson"
"y" "messagemode"
"z" "usepowerup"
"~" "toggleconsole"
"UPARROW" "+forward"
"DOWNARROW" "+back"
"LEFTARROW" "+left"
"RIGHTARROW" "+right"
"CTRL" "+duck"
"SHIFT" "+speed"
"F5" "snapshot"
"F6" "save quick"
"F7" "load quick"
"F10" "quit prompt"
"INS" "+klook"
"PGDN" "+lookdown"
"PGUP" "+lookup"
"END" "centerview"
"MWHEELDOWN" "invnext"
"MWHEELUP" "invprev"
"MOUSE1" "+attack"
"MOUSE2" "+attack2"
"MOUSE3" "+alt1"
"PAUSE" "pause"
"#;

/// Turns a `kb_act.lst` label into display text: TS writes plain text, Half-Life writes
/// localization tokens such as `#Valve_Move_Forward`.
fn label(text: &str) -> String {
    let text = text.trim().trim_end_matches(':').trim();
    match text.strip_prefix('#') {
        Some(token) => {
            let token = token.strip_prefix("Valve_").unwrap_or(token);
            let token = token.strip_suffix("_Title").unwrap_or(token);
            token.replace('_', " ")
        }
        None => text.to_owned(),
    }
}

/// Parses `kb_act.lst`: `"command" "description"` lines, where `"blank"` lines are `====`
/// separators or section headings.
pub fn parse_actions(text: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for line in text.lines() {
        let t = tokenize(line);
        if t.len() < 2 {
            continue;
        }
        if t[0].eq_ignore_ascii_case("blank") {
            if !t[1].trim().is_empty() && !t[1].trim().chars().all(|c| c == '=') {
                sections.push(Section { title: label(&t[1]), actions: Vec::new() });
            }
            continue;
        }
        if sections.is_empty() {
            sections.push(Section { title: "General".into(), actions: Vec::new() });
        }
        let action = Action { command: t[0].clone(), description: label(&t[1]) };
        sections.last_mut().expect("pushed above").actions.push(action);
    }
    sections.retain(|s| !s.actions.is_empty());
    sections
}

/// The actions for the installation at `root`: `ts/gfx/shell/kb_act.lst`, else Half-Life's.
pub fn load_actions(root: &Path) -> Vec<Section> {
    ["ts", "valve"]
        .iter()
        .filter_map(|game| std::fs::read(root.join(game).join("gfx/shell/kb_act.lst")).ok())
        .map(|bytes| parse_actions(&String::from_utf8_lossy(&bytes)))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

/// Parses `kb_def.lst` (`"key" "command"` lines) into lower-case key names and commands.
pub fn parse_defaults(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(tokenize)
        .filter(|t| t.len() >= 2 && !t[0].is_empty())
        .map(|t| (t[0].to_ascii_lowercase(), t[1].clone()))
        .map(|(k, c)| (if k == ";" { "semicolon".into() } else { k }, c))
        .collect()
}

/// Default binds: `ts/gfx/shell/kb_def.lst` if TS has one, else the binds TS 3.0 ships with.
pub fn default_binds(root: &Path) -> Vec<(String, String)> {
    std::fs::read(root.join("ts/gfx/shell/kb_def.lst"))
        .ok()
        .map(|b| parse_defaults(&String::from_utf8_lossy(&b)))
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| parse_defaults(TS_DEFAULTS))
}

/// The engine's name for a key GPUI reports as `key` (`Keystroke::key`), or `None` for keys
/// GoldSrc can't bind. GPUI drops the keypad prefix on Linux, so keypad digits and arrows come in
/// as their main-keyboard twins; only the keypad keys with their own GPUI names map to `kp_*`.
pub fn key_from_gpui(key: &str) -> Option<String> {
    let name = match key {
        "space" | "enter" | "tab" | "escape" | "backspace" | "home" | "end" | "pause" | "shift" | "alt" => key,
        // GPUI reports a lone modifier press as a keystroke when it is released.
        "control" => "ctrl",
        "up" => "uparrow",
        "down" => "downarrow",
        "left" => "leftarrow",
        "right" => "rightarrow",
        "insert" => "ins",
        "delete" => "del",
        "pageup" => "pgup",
        "pagedown" => "pgdn",
        "scroll_lock" => "scrollock",
        "add" => "kp_plus",
        "subtract" => "kp_minus",
        "divide" => "kp_slash",
        "decimal" => "kp_del",
        "begin" => "kp_5",
        "multiply" => "*",
        ";" | ":" => "semicolon",
        _ => {
            if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                return (1..=12).contains(&n).then(|| key.to_owned());
            }
            let mut chars = key.chars();
            let c = chars.next()?;
            if chars.next().is_some() || !c.is_ascii_graphic() {
                return None;
            }
            // The engine names keys by their unshifted symbol (US layout).
            let c = match c {
                '!' => '1',
                '@' => '2',
                '#' => '3',
                '$' => '4',
                '%' => '5',
                '^' => '6',
                '&' => '7',
                '*' => '8',
                '(' => '9',
                ')' => '0',
                '_' => '-',
                '+' => '=',
                '{' => '[',
                '}' => ']',
                '|' => '\\',
                '"' => '\'',
                '<' => ',',
                '>' => '.',
                '?' => '/',
                '~' => '`',
                c => c.to_ascii_lowercase(),
            };
            return Some(c.to_string());
        }
    };
    Some(name.to_owned())
}

/// The engine's name for mouse button `n` (1 = left, 2 = right, 3 = middle, 4/5 = back/forward).
pub fn mouse_key(n: u8) -> String {
    format!("mouse{n}")
}

/// A key name as shown in the UI.
pub fn key_label(key: &str) -> String {
    let key = key.to_ascii_lowercase();
    let named = match key.as_str() {
        "uparrow" => "↑",
        "downarrow" => "↓",
        "leftarrow" => "←",
        "rightarrow" => "→",
        "mwheelup" => "Wheel up",
        "mwheeldown" => "Wheel down",
        "semicolon" => ";",
        "pgup" => "Page Up",
        "pgdn" => "Page Down",
        "ins" => "Insert",
        "del" => "Delete",
        "ctrl" => "Ctrl",
        "alt" => "Alt",
        "shift" => "Shift",
        "space" => "Space",
        "enter" => "Enter",
        "tab" => "Tab",
        "escape" => "Esc",
        "backspace" => "Backspace",
        "capslock" => "Caps Lock",
        "scrollock" => "Scroll Lock",
        "home" => "Home",
        "end" => "End",
        "pause" => "Pause",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_owned();
    }
    if let Some(n) = key.strip_prefix("mouse") {
        return format!("Mouse {n}");
    }
    if let Some(k) = key.strip_prefix("kp_") {
        let k = match k {
            "uparrow" => "↑",
            "downarrow" => "↓",
            "leftarrow" => "←",
            "rightarrow" => "→",
            "pgup" => "Page Up",
            "pgdn" => "Page Down",
            "ins" => "Insert",
            "del" => "Delete",
            "slash" => "/",
            "minus" => "-",
            "plus" => "+",
            "enter" => "Enter",
            "home" => "Home",
            "end" => "End",
            k => k,
        };
        return format!("Keypad {k}");
    }
    key.to_ascii_uppercase()
}

/// Sets the key of `command` in `slot` (0-based) to `key`, keeping the order of its other keys.
/// A key bound to another command moves; that command is returned.
pub fn rebind(cfg: &mut CfgFile, command: &str, slot: usize, key: &str) -> Option<String> {
    let key = key.to_ascii_lowercase();
    let mut keys = cfg.keys_for(command);
    if keys.contains(&key) {
        return None;
    }
    let stolen = cfg.bind_of(&key).filter(|c| !c.eq_ignore_ascii_case(command));
    cfg.unbind(&key);
    match keys.get_mut(slot) {
        // Rewrite the old key's line in place, so the file keeps its order and the diff stays small.
        Some(old) => {
            let mut done = false;
            let mut lines: Vec<String> = cfg.text().lines().map(str::to_owned).collect();
            for line in lines.iter_mut().rev() {
                let t = tokenize(line);
                if !done && t.len() >= 3 && t[0].eq_ignore_ascii_case("bind") && t[1].eq_ignore_ascii_case(old) {
                    *line = format!("bind \"{key}\" \"{}\"", t[2]);
                    done = true;
                }
            }
            *old = key;
            *cfg = CfgFile::parse(&cfg.path, &lines.join("\n"));
            // Earlier duplicate lines of the old key, if any.
            set_keys_if_differs(cfg, command, &keys);
        }
        None => cfg.bind(&key, command),
    }
    stolen
}

fn set_keys_if_differs(cfg: &mut CfgFile, command: &str, keys: &[String]) {
    if cfg.keys_for(command) != keys {
        set_keys(cfg, command, keys);
    }
}

/// Removes the key in `slot` from `command`.
pub fn clear_slot(cfg: &mut CfgFile, command: &str, slot: usize) {
    if let Some(key) = cfg.keys_for(command).get(slot) {
        cfg.unbind(key);
    }
}

/// Makes `keys` (in that order) the only keys bound to `command`, taking them from other commands.
/// Returns the keys taken and the commands they were bound to.
pub fn set_keys(cfg: &mut CfgFile, command: &str, keys: &[String]) -> Vec<(String, String)> {
    let mut stolen = Vec::new();
    for old in cfg.keys_for(command) {
        cfg.unbind(&old);
    }
    for key in keys {
        if let Some(other) = cfg.bind_of(key) {
            stolen.push((key.clone(), other));
            cfg.unbind(key);
        }
        cfg.bind(key, command);
    }
    stolen
}

/// Replaces every bind with `defaults`.
/// The new binds go where the old ones were (before `exec userconfig.cfg`, whose binds must win).
pub fn reset_all(cfg: &mut CfgFile, defaults: &[(String, String)]) {
    let is_bind = |l: &str| tokenize(l).first().is_some_and(|t| t.eq_ignore_ascii_case("bind"));
    let is_unbindall = |l: &str| tokenize(l).first().is_some_and(|t| t.eq_ignore_ascii_case("unbindall"));
    let mut lines: Vec<String> = cfg.text().lines().map(str::to_owned).collect();
    let at =
        lines.iter().position(|l| is_bind(l)).or_else(|| lines.iter().position(|l| is_unbindall(l)).map(|i| i + 1)).unwrap_or(lines.len());
    lines.retain(|l| !is_bind(l));
    let new = defaults.iter().map(|(k, c)| format!("bind \"{}\" \"{}\"", k.to_ascii_lowercase(), c.replace('"', "'")));
    lines.splice(at..at, new);
    *cfg = CfgFile::parse(&cfg.path, &lines.join("\n"));
}

/// A new `config.cfg` holding only `defaults`, for installations that have none yet.
pub fn default_config(path: &Path, defaults: &[(String, String)]) -> CfgFile {
    let mut cfg = CfgFile::parse(path, "unbindall\n");
    reset_all(&mut cfg, defaults);
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS_ACT: &str = "\"blank\"\t\t\"==========\"\r\n\"blank\"\t\t\"SPECIAL:\"\r\n\"blank\"\t\t\"==========\"\r\n\
\"thirdperson\"\t\"Third/First Person View\"\r\n\"blank\"\t\"==========\"\r\n\"blank\"\t\"PLAYER MOVEMENT:\"\r\n\
\"blank\"\t\"==========\"\r\n\"+alt1\"\t\"Stunt button\" \r\n\"+forward\"\t\"move forward\"\r\n\"impulse 201\"\t\"spray logo\"";

    #[test]
    fn parses_kb_act_sections() {
        let s = parse_actions(TS_ACT);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].title, "SPECIAL");
        assert_eq!(s[0].actions, [Action { command: "thirdperson".into(), description: "Third/First Person View".into() }]);
        assert_eq!(s[1].title, "PLAYER MOVEMENT");
        assert_eq!(s[1].actions.iter().map(|a| a.command.as_str()).collect::<Vec<_>>(), ["+alt1", "+forward", "impulse 201"]);
        assert_eq!(s[1].actions[0].description, "Stunt button");
    }

    #[test]
    fn parses_half_life_kb_act() {
        let s = parse_actions(
            "\"blank\" \"===\"\n\"blank\" \"#Valve_Movement_Title\"\n\"blank\" \"===\"\n\"+forward\" \"#Valve_Move_Forward\"\n",
        );
        assert_eq!(s[0].title, "Movement");
        assert_eq!(s[0].actions[0].description, "Move Forward");
    }

    #[test]
    fn reads_the_real_lists() {
        let ts = Path::new(&std::env::var_os("HOME").unwrap_or_default()).join("dev/hl/ts_original/gfx/shell/kb_act.lst");
        if let Ok(bytes) = std::fs::read(ts) {
            let s = parse_actions(&String::from_utf8_lossy(&bytes));
            let titles: Vec<_> = s.iter().map(|s| s.title.as_str()).collect();
            assert_eq!(titles, ["SPECIAL", "PLAYER MOVEMENT", "WEAPON HANDLING", "MENU", "OTHERS"]);
            assert!(s[4].actions.iter().any(|a| a.command == "quit"));
        }
        let defaults = parse_defaults(TS_DEFAULTS);
        assert!(defaults.contains(&("uparrow".into(), "+forward".into())));
        assert!(defaults.contains(&("mouse3".into(), "+alt1".into())));
    }

    #[test]
    fn maps_gpui_keys() {
        let k = |s: &str| key_from_gpui(s);
        assert_eq!(k("a").as_deref(), Some("a"));
        assert_eq!(k("A").as_deref(), Some("a"));
        assert_eq!(k("1").as_deref(), Some("1"));
        assert_eq!(k("!").as_deref(), Some("1"));
        assert_eq!(k("f1").as_deref(), Some("f1"));
        assert_eq!(k("f12").as_deref(), Some("f12"));
        assert_eq!(k("f13"), None);
        assert_eq!(k("up").as_deref(), Some("uparrow"));
        assert_eq!(k("right").as_deref(), Some("rightarrow"));
        assert_eq!(k("space").as_deref(), Some("space"));
        assert_eq!(k("enter").as_deref(), Some("enter"));
        assert_eq!(k("insert").as_deref(), Some("ins"));
        assert_eq!(k("delete").as_deref(), Some("del"));
        assert_eq!(k("pageup").as_deref(), Some("pgup"));
        assert_eq!(k("pagedown").as_deref(), Some("pgdn"));
        assert_eq!(k(";").as_deref(), Some("semicolon"));
        assert_eq!(k("'").as_deref(), Some("'"));
        assert_eq!(k("`").as_deref(), Some("`"));
        assert_eq!(k("add").as_deref(), Some("kp_plus"));
        assert_eq!(k("divide").as_deref(), Some("kp_slash"));
        assert_eq!(k("menu"), None);
        assert_eq!(k("é"), None);
        assert_eq!(mouse_key(4), "mouse4");
        assert_eq!(key_label("mouse1"), "Mouse 1");
        assert_eq!(key_label("w"), "W");
        assert_eq!(key_label("kp_slash"), "Keypad /");
    }

    #[test]
    fn rebinding_steals_a_key() {
        let mut c = CfgFile::parse(
            Path::new("config.cfg"),
            "unbindall\nbind \"w\" \"+forward\"\nbind \"UPARROW\" \"+forward\"\nbind \"q\" \"+alt1\"\nbind \"MOUSE3\" \"+alt1\"\n",
        );
        // q goes from +alt1 to the first slot of +forward, which keeps its second key.
        assert_eq!(rebind(&mut c, "+forward", 0, "Q").as_deref(), Some("+alt1"));
        assert_eq!(c.keys_for("+forward"), ["q", "uparrow"]);
        assert_eq!(c.keys_for("+alt1"), ["mouse3"]);
        assert_eq!(c.bind_of("w"), None);
        // The replaced key keeps its line.
        assert!(c.text().starts_with("unbindall\nbind \"q\" \"+forward\"\nbind \"UPARROW\" \"+forward\"\n"));
        // Already bound to this command: nothing changes.
        assert_eq!(rebind(&mut c, "+forward", 1, "q"), None);
        assert_eq!(c.keys_for("+forward"), ["q", "uparrow"]);
        // An empty second slot gets the key.
        assert_eq!(rebind(&mut c, "+alt1", 1, "e"), None);
        assert_eq!(c.keys_for("+alt1"), ["mouse3", "e"]);
        clear_slot(&mut c, "+alt1", 0);
        assert_eq!(c.keys_for("+alt1"), ["e"]);
        assert!(c.text().starts_with("unbindall\n"));
    }

    #[test]
    fn resets_to_defaults() {
        let defaults = parse_defaults("\"w\" \"+forward\"\n\"MOUSE1\" \"+attack\"\n");
        let mut c = CfgFile::parse(
            Path::new("config.cfg"),
            "unbindall\nbind \"w\" \"+jump\"\nbind \"x\" \"quit\"\nsensitivity \"3\"\nexec userconfig.cfg\n",
        );
        reset_all(&mut c, &defaults);
        assert_eq!(c.binds(), [("w".to_owned(), "+forward".to_owned()), ("mouse1".to_owned(), "+attack".to_owned())]);
        assert_eq!(c.text(), "unbindall\nbind \"w\" \"+forward\"\nbind \"mouse1\" \"+attack\"\nsensitivity \"3\"\nexec userconfig.cfg\n");
        let fresh = default_config(Path::new("config.cfg"), &defaults);
        assert_eq!(fresh.text(), "unbindall\nbind \"w\" \"+forward\"\nbind \"mouse1\" \"+attack\"\n");
    }
}
