//! Command-line words, the Windows way.
//!
//! One implementation of the quoting rules shared by the config parser (the
//! words of a `spawn` bind or `spawn-at-startup` line), the IPC `spawn` request
//! and the process spawner, so a path with spaces or a trailing backslash means
//! the same thing wherever it is written: the rules of `CommandLineToArgvW` and
//! the MSVC runtime, which every Windows program applies to its own command line.

fn is_cmdline_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}')
}

/// Split `line` into words.  Each word comes with a flag telling whether any
/// double quote took part in it (`2` and `"2"` are different things to
/// `focus-workspace`: an index and a workspace name).
///
/// Rules: whitespace separates words, double quotes group (and are removed),
/// `2n` backslashes before a quote are `n` backslashes and the quote toggles
/// grouping, `2n+1` backslashes before a quote are `n` backslashes and a literal
/// quote, every other backslash is literal - so `C:\Program Files\x` and
/// `"C:\My Dir\x.exe" -d "a b"` mean what they look like.  An unbalanced quote
/// runs to the end of the line.
pub fn split_command_line_flagged(line: &str) -> Vec<(String, bool)> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut words = Vec::new();
    let mut i = 0;
    loop {
        while i < n && is_cmdline_space(chars[i]) {
            i += 1;
        }
        if i >= n {
            break;
        }
        let mut cur = String::new();
        let mut quoted = false;
        let mut in_quotes = false;
        while i < n {
            let c = chars[i];
            if c == '\\' {
                let mut backslashes = 0;
                while i < n && chars[i] == '\\' {
                    backslashes += 1;
                    i += 1;
                }
                if i < n && chars[i] == '"' {
                    for _ in 0..backslashes / 2 {
                        cur.push('\\');
                    }
                    if backslashes % 2 == 1 {
                        cur.push('"');
                        quoted = true;
                        i += 1;
                    }
                    // An even count leaves the quote for the next round: it toggles.
                } else {
                    for _ in 0..backslashes {
                        cur.push('\\');
                    }
                }
                continue;
            }
            if c == '"' {
                in_quotes = !in_quotes;
                quoted = true;
                i += 1;
                continue;
            }
            if !in_quotes && is_cmdline_space(c) {
                break;
            }
            cur.push(c);
            i += 1;
        }
        words.push((cur, quoted));
    }
    words
}

/// [`split_command_line_flagged`] without the quoted flags.
pub fn split_command_line(line: &str) -> Vec<String> {
    split_command_line_flagged(line)
        .into_iter()
        .map(|(word, _)| word)
        .collect()
}

/// Quote one argument so that [`split_command_line`] (and every program that
/// parses its command line the MSVC way) reads it back unchanged.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| is_cmdline_space(c) || c == '"') {
        return arg.to_string();
    }
    let chars: Vec<char> = arg.chars().collect();
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut i = 0;
    loop {
        let mut backslashes = 0;
        while i < chars.len() && chars[i] == '\\' {
            backslashes += 1;
            i += 1;
        }
        if i == chars.len() {
            // Backslashes in front of the closing quote must not escape it.
            for _ in 0..backslashes * 2 {
                out.push('\\');
            }
            break;
        } else if chars[i] == '"' {
            for _ in 0..backslashes * 2 + 1 {
                out.push('\\');
            }
            out.push('"');
        } else {
            for _ in 0..backslashes {
                out.push('\\');
            }
            out.push(chars[i]);
        }
        i += 1;
    }
    out.push('"');
    out
}

/// The command line for `argv`: program first, every argument quoted as needed.
pub fn build_command_line(argv: &[String]) -> String {
    argv.iter()
        .map(|a| quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn split_keeps_quoted_paths_with_spaces_together() {
        assert_eq!(
            split_command_line(r#""C:\Program Files\Tool\tool.exe" -d "C:\My Projects" --flag"#),
            v(&[r"C:\Program Files\Tool\tool.exe", "-d", r"C:\My Projects", "--flag"])
        );
        assert_eq!(split_command_line("  wt.exe   -d   .  "), v(&["wt.exe", "-d", "."]));
        assert_eq!(split_command_line(""), Vec::<String>::new());
        assert_eq!(split_command_line("   "), Vec::<String>::new());
    }

    #[test]
    fn split_follows_the_msvc_quote_and_backslash_rules() {
        // Backslashes that do not precede a quote are literal.
        assert_eq!(split_command_line(r"a\b\c"), v(&[r"a\b\c"]));
        assert_eq!(split_command_line(r#""\\server\share\dir""#), v(&[r"\\server\share\dir"]));
        // 2n backslashes + quote: n backslashes, the quote toggles.
        assert_eq!(split_command_line(r#""a\\" b"#), v(&[r"a\", "b"]));
        // 2n+1 backslashes + quote: n backslashes and a literal quote.
        assert_eq!(split_command_line(r#"a\"b"#), v(&[r#"a"b"#]));
        assert_eq!(split_command_line(r#""a \"quoted\" b""#), v(&[r#"a "quoted" b"#]));
        // An empty quoted string is an argument.
        assert_eq!(split_command_line(r#"x "" y"#), v(&["x", "", "y"]));
        // Adjacent quoted and unquoted text is one word.
        assert_eq!(split_command_line(r#"pre"fix suf"fix"#), v(&["prefix suffix"]));
        // An unbalanced quote runs to the end of the line.
        assert_eq!(split_command_line(r#"a "b c"#), v(&["a", "b c"]));
    }

    #[test]
    fn the_quoted_flag_tells_2_from_a_quoted_2() {
        assert_eq!(
            split_command_line_flagged(r#"focus-workspace 2"#),
            vec![("focus-workspace".to_string(), false), ("2".to_string(), false)]
        );
        assert_eq!(
            split_command_line_flagged(r#"focus-workspace "2""#),
            vec![("focus-workspace".to_string(), false), ("2".to_string(), true)]
        );
        assert_eq!(
            split_command_line_flagged(r#"spawn "wt.exe" -d "a b""#),
            vec![
                ("spawn".to_string(), false),
                ("wt.exe".to_string(), true),
                ("-d".to_string(), false),
                ("a b".to_string(), true)
            ]
        );
    }

    #[test]
    fn quoting_round_trips_through_splitting() {
        let samples = [
            "plain",
            "with space",
            r"C:\Program Files\x\y.exe",
            r"C:\dir with space\",
            r#"she said "hi""#,
            r"trailing\\",
            "",
            "tab\tinside",
            r"\",
            r#"a\"b"#,
        ];
        for s in samples {
            let quoted = quote_arg(s);
            assert_eq!(split_command_line(&quoted), v(&[s]), "{:?} -> {:?}", s, quoted);
        }
        let argv = v(&["wt.exe", "-d", r"C:\My Projects", "--title", r#"a "b" c"#, ""]);
        assert_eq!(split_command_line(&build_command_line(&argv)), argv);
    }

    #[test]
    fn plain_words_are_not_quoted() {
        assert_eq!(quote_arg("wt.exe"), "wt.exe");
        assert_eq!(quote_arg(r"C:\x\y.exe"), r"C:\x\y.exe");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(
            build_command_line(&v(&[r"C:\Program Files\a.exe", "x"])),
            "\"C:\\Program Files\\a.exe\" x"
        );
    }
}
