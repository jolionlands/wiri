//! A reader for the KDL documents wiri's configuration is written in.
//!
//! Why not the `kdl` crate: niri's own config uses KDL v1 (`true`, `r#"raw"#`), the `kdl`
//! crate 6.x reads KDL v2 (`#true`, `#"raw"#`) and carries `miette` and `winnow` along, and
//! wiri's own older configs use spellings neither version accepts (bare words as values:
//! `hide-method hide`, `Ctrl+Alt+Left focus-column-left`; `key=value` as a whole line:
//! `repeat_delay=600`).  This reader is a superset of both KDL versions where they agree
//! and takes those two extras, has no dependency (Phase 4 counts weight), and reports every
//! position as `line:column`.
//!
//! Accepted:
//! * nodes with arguments, properties (`key=value`) and children (`{ ... }`), ended by a
//!   newline, `;` or the `}` of the parent; `\` continues a line;
//! * `//` comments, nested `/* */` comments, and `/-` (slashdash) before a node, an
//!   argument, a property or a children block;
//! * quoted strings with KDL's escapes (`\n \r \t \\ \" \/ \b \f \s \u{..}`), raw strings
//!   in both spellings (`r#"..."#`, `#"..."#`), integers (decimal, `0x`, `0o`, `0b`, `_`
//!   separators), floats, `true`/`false`/`null` and `#true`/`#false`/`#null`, type
//!   annotations `(type)value`;
//! * a byte-order mark anywhere is whitespace.
//!
//! wiri's two extras: a bare word that is not a keyword or a number is a string
//! (`Scalar::Str { quoted: false }`, so `focus-workspace 2` and `focus-workspace "2"` stay
//! different), and `name=value` / `name = value` right after a node name is a node with one
//! argument.
//!
//! One compatibility rule for backslashes: wiri's old format read `"C:\Program Files\x"`
//! literally, and Windows paths are the one place where KDL's escapes (`\P` is an error,
//! `\t` is a tab) surprise people.  A quoted string that *starts* like a Windows path
//! (`C:\...` or `\\server`) therefore keeps its backslashes as written, except that `\"`
//! is a quote and a backslash run before the closing quote counts like the Windows
//! command-line rules say.  Everywhere else a bad escape is an error that says how to fix
//! it.  Raw strings (`r"C:\Program Files"`) always work.

use std::fmt;

/// A position in the source: 1-based line and column (columns count characters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// A KDL scalar.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    /// A string.  `quoted` is false for a bare word (`hide-method hide`).
    Str { text: String, quoted: bool },
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

/// An argument or a property value.
#[derive(Debug, Clone, PartialEq)]
pub struct Value {
    pub scalar: Scalar,
    /// The `(type)` annotation, if any.
    pub ty: Option<String>,
    pub pos: Pos,
}

impl Value {
    /// The text of a string value (quoted or bare).
    pub fn as_str(&self) -> Option<&str> {
        match &self.scalar {
            Scalar::Str { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    /// True for a string that was written in quotes (or as a raw string).
    pub fn is_quoted_str(&self) -> bool {
        matches!(&self.scalar, Scalar::Str { quoted: true, .. })
    }

    pub fn as_bool(&self) -> Option<bool> {
        match &self.scalar {
            Scalar::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match &self.scalar {
            Scalar::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// An integer or a float.
    pub fn as_f64(&self) -> Option<f64> {
        match &self.scalar {
            Scalar::Int(i) => Some(*i as f64),
            Scalar::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// What kind of value this is, for error messages.
    pub fn kind(&self) -> &'static str {
        match &self.scalar {
            Scalar::Str { quoted: true, .. } => "string",
            Scalar::Str { quoted: false, .. } => "word",
            Scalar::Int(_) => "integer",
            Scalar::Float(_) => "number",
            Scalar::Bool(_) => "boolean",
            Scalar::Null => "null",
        }
    }

    /// The value as the text a user would have typed (`2`, `0.5`, `true`, `wt.exe`).
    pub fn to_text(&self) -> String {
        match &self.scalar {
            Scalar::Str { text, .. } => text.clone(),
            Scalar::Int(i) => i.to_string(),
            Scalar::Float(f) => f.to_string(),
            Scalar::Bool(b) => b.to_string(),
            Scalar::Null => "null".to_string(),
        }
    }
}

/// `key=value`.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    pub key: String,
    pub key_pos: Pos,
    pub value: Value,
}

/// One node.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    /// The `(type)` annotation of the node name, if any.
    pub ty: Option<String>,
    /// Position of the node name.
    pub pos: Pos,
    pub args: Vec<Value>,
    pub props: Vec<Prop>,
    /// `None` when the node has no `{ }` block; `Some(vec![])` for an empty one.
    pub children: Option<Vec<Node>>,
    /// Position of the `{` of the children block.
    pub children_pos: Option<Pos>,
}

impl Node {
    /// The value of the property `key`.
    pub fn prop(&self, key: &str) -> Option<&Value> {
        self.props.iter().find(|p| p.key == key).map(|p| &p.value)
    }

    /// The children, or an empty slice.
    pub fn kids(&self) -> &[Node] {
        match &self.children {
            Some(v) => v.as_slice(),
            None => &[],
        }
    }
}

/// A parsed document.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub nodes: Vec<Node>,
}

/// A syntax error: the first thing in the text that is not KDL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub pos: Pos,
    pub message: String,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.pos, self.message)
    }
}

impl std::error::Error for SyntaxError {}

/// Parse a KDL document.
pub fn parse(text: &str) -> Result<Document, SyntaxError> {
    let mut parser = Parser::new(text);
    let nodes = parser.parse_nodes(None)?;
    Ok(Document { nodes })
}

type PResult<T> = Result<T, SyntaxError>;

fn is_newline(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{0085}' | '\u{000C}' | '\u{2028}' | '\u{2029}'
    )
}

fn is_ws(c: char) -> bool {
    matches!(
        c,
        '\t' | ' '
            | '\u{000B}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

/// A character that can be part of a bare word.  Wider than KDL v1 (which also excludes
/// `,` `<` `>`) so that `Ctrl+Alt+,` is a chord; `/` is handled by the caller (it may start
/// a comment).
fn is_word_char(c: char) -> bool {
    !is_ws(c)
        && !is_newline(c)
        && !matches!(c, '\\' | '(' | ')' | '{' | '}' | ';' | '[' | ']' | '"' | '=')
}

enum ArgOrProp {
    Arg(Value),
    Prop(Prop),
}

enum Word {
    Str { text: String, quoted: bool },
    Number(Scalar),
    Keyword(Scalar, String),
}

struct Parser {
    chars: Vec<char>,
    i: usize,
    line: u32,
    col: u32,
}

impl Parser {
    fn new(text: &str) -> Parser {
        Parser {
            chars: text.chars().collect(),
            i: 0,
            line: 1,
            col: 1,
        }
    }

    fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            col: self.col,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.i + n).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.i).copied()?;
        self.i += 1;
        if c == '\r' {
            if self.chars.get(self.i).copied() == Some('\n') {
                // The '\n' that follows ends the line.
                self.col += 1;
            } else {
                self.line += 1;
                self.col = 1;
            }
        } else if is_newline(c) {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn err<T>(&self, message: impl Into<String>) -> PResult<T> {
        Err(SyntaxError {
            pos: self.pos(),
            message: message.into(),
        })
    }

    fn err_at<T>(pos: Pos, message: impl Into<String>) -> PResult<T> {
        Err(SyntaxError {
            pos,
            message: message.into(),
        })
    }

    fn consume_newline(&mut self) {
        if self.peek() == Some('\r') && self.peek_at(1) == Some('\n') {
            self.bump();
        }
        self.bump();
    }

    // ---- whitespace and comments ----------------------------------------------------

    /// Spaces, tabs and block comments on the current line.  True when anything was skipped.
    fn skip_hspace(&mut self) -> PResult<bool> {
        let mut any = false;
        loop {
            match self.peek() {
                Some(c) if is_ws(c) => {
                    self.bump();
                    any = true;
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.skip_block_comment()?;
                    any = true;
                }
                _ => return Ok(any),
            }
        }
    }

    fn skip_block_comment(&mut self) -> PResult<()> {
        let start = self.pos();
        self.bump();
        self.bump();
        let mut depth = 1u32;
        while depth > 0 {
            match self.bump() {
                None => return Self::err_at(start, "this /* comment is never closed (missing */)"),
                Some('*') if self.peek() == Some('/') => {
                    self.bump();
                    depth -= 1;
                }
                Some('/') if self.peek() == Some('*') => {
                    self.bump();
                    depth += 1;
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// Skips to the end of the line; the newline itself is left.
    fn skip_line_comment(&mut self) {
        while let Some(c) = self.peek() {
            if is_newline(c) {
                break;
            }
            self.bump();
        }
    }

    /// Whitespace, newlines and comments between nodes.
    fn skip_linespace(&mut self) -> PResult<()> {
        loop {
            match self.peek() {
                Some(c) if is_ws(c) || is_newline(c) => {
                    self.bump();
                }
                Some('/') if self.peek_at(1) == Some('/') => self.skip_line_comment(),
                Some('/') if self.peek_at(1) == Some('*') => self.skip_block_comment()?,
                _ => return Ok(()),
            }
        }
    }

    /// Space between the parts of one node: spaces, block comments and `\` line
    /// continuations.  True when anything was skipped.
    fn skip_node_space(&mut self) -> PResult<bool> {
        let mut any = false;
        loop {
            if self.skip_hspace()? {
                any = true;
            }
            if self.peek() == Some('\\') {
                let at = self.pos();
                self.bump();
                self.skip_hspace()?;
                if self.peek() == Some('/') && self.peek_at(1) == Some('/') {
                    self.skip_line_comment();
                }
                match self.peek() {
                    Some(c) if is_newline(c) => {
                        self.consume_newline();
                        any = true;
                    }
                    None => {
                        any = true;
                    }
                    _ => {
                        return Self::err_at(
                            at,
                            "a backslash outside a quoted string must end the line; to write a \
                             backslash use a quoted string with \\\\ or a raw string r\"...\"",
                        )
                    }
                }
                continue;
            }
            return Ok(any);
        }
    }

    // ---- nodes -------------------------------------------------------------------------

    /// Nodes up to the `}` that closes the block opened at `open` (not consumed), or to the
    /// end of the text at the top level.
    fn parse_nodes(&mut self, open: Option<Pos>) -> PResult<Vec<Node>> {
        let mut nodes = Vec::new();
        loop {
            self.skip_linespace()?;
            match self.peek() {
                None => {
                    return match open {
                        None => Ok(nodes),
                        Some(at) => Self::err_at(at, "this '{' is never closed (missing '}')"),
                    };
                }
                Some('}') => {
                    return match open {
                        Some(_) => Ok(nodes),
                        None => self.err("unexpected '}' (no block is open here)"),
                    };
                }
                Some(';') => {
                    self.bump();
                }
                Some('/') if self.peek_at(1) == Some('-') => {
                    self.bump();
                    self.bump();
                    self.skip_linespace()?;
                    if matches!(self.peek(), None | Some('}')) {
                        return self.err("expected a node after '/-'");
                    }
                    // A commented-out node still has to be well formed.
                    let _dropped = self.parse_node()?;
                }
                Some(_) => {
                    let node = self.parse_node()?;
                    nodes.push(node);
                }
            }
        }
    }

    fn expect_close_brace(&mut self) -> PResult<()> {
        if self.peek() == Some('}') {
            self.bump();
            Ok(())
        } else {
            self.err("expected '}'")
        }
    }

    /// Distance to a `=` that follows only spaces and tabs, if there is one.
    fn eq_ahead(&self) -> Option<usize> {
        let mut n = 0usize;
        loop {
            match self.peek_at(n) {
                Some(c) if is_ws(c) => n += 1,
                Some('=') => return Some(n),
                _ => return None,
            }
        }
    }

    fn skip_spaces(&mut self) {
        while let Some(c) = self.peek() {
            if is_ws(c) {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn parse_node(&mut self) -> PResult<Node> {
        let ty = self.parse_type_annotation()?;
        let pos = self.pos();
        let name = match self.peek() {
            Some('"') => self.parse_quoted()?,
            Some('r') if self.raw_string_ahead() => self.parse_raw()?,
            Some('#') if self.v2_raw_string_ahead() => self.parse_raw()?,
            Some(c) if is_word_char(c) && c != '#' => {
                let w = self.scan_word();
                if w.is_empty() {
                    return self.err(format!("unexpected '{}'", c));
                }
                w
            }
            Some(c) => return self.err(unexpected_message(c, "a node name")),
            None => return self.err("expected a node name"),
        };
        let mut node = Node {
            name,
            ty,
            pos,
            args: Vec::new(),
            props: Vec::new(),
            children: None,
            children_pos: None,
        };

        // wiri's older `name=value` line: a node with one argument.
        if let Some(n) = self.eq_ahead() {
            for _ in 0..=n {
                self.bump();
            }
            self.skip_spaces();
            let value = self.parse_value()?;
            node.args.push(value);
        }

        let mut closed_children = false;
        loop {
            let spaced = self.skip_node_space()?;
            match self.peek() {
                None => break,
                Some(c) if is_newline(c) => {
                    self.consume_newline();
                    break;
                }
                Some(';') => {
                    self.bump();
                    break;
                }
                Some('}') => break,
                Some('/') if self.peek_at(1) == Some('/') => {
                    // The comment ends the node; the newline after it is taken next round.
                    self.skip_line_comment();
                }
                Some('/') if self.peek_at(1) == Some('-') => {
                    self.bump();
                    self.bump();
                    self.skip_node_space()?;
                    if self.peek() == Some('{') {
                        let at = self.pos();
                        self.bump();
                        let _dropped = self.parse_nodes(Some(at))?;
                        self.expect_close_brace()?;
                    } else if matches!(self.peek(), None | Some(';') | Some('}'))
                        || self.peek().map_or(false, is_newline)
                    {
                        return self.err("expected an argument, property or block after '/-'");
                    } else {
                        let _dropped = self.parse_arg_or_prop()?;
                    }
                }
                Some('{') => {
                    if closed_children {
                        return self.err("a node can have only one children block");
                    }
                    let at = self.pos();
                    self.bump();
                    let kids = self.parse_nodes(Some(at))?;
                    self.expect_close_brace()?;
                    node.children = Some(kids);
                    node.children_pos = Some(at);
                    closed_children = true;
                }
                Some('=') => {
                    return self.err("a property needs a name before the '='");
                }
                Some(_) => {
                    if closed_children {
                        return self.err(
                            "nothing can follow the '}' of a node's children block on the same line",
                        );
                    }
                    if !spaced {
                        return self.err("expected a space or a line break here");
                    }
                    match self.parse_arg_or_prop()? {
                        ArgOrProp::Arg(v) => node.args.push(v),
                        ArgOrProp::Prop(p) => node.props.push(p),
                    }
                }
            }
        }
        Ok(node)
    }

    // ---- values ------------------------------------------------------------------------

    /// `(type)` or nothing.
    fn parse_type_annotation(&mut self) -> PResult<Option<String>> {
        if self.peek() != Some('(') {
            return Ok(None);
        }
        let at = self.pos();
        self.bump();
        self.skip_spaces();
        let name = match self.peek() {
            Some('"') => self.parse_quoted()?,
            Some(c) if is_word_char(c) => self.scan_word(),
            _ => return Self::err_at(at, "expected a type name after '('"),
        };
        self.skip_spaces();
        if self.peek() != Some(')') {
            return Self::err_at(at, "this '(' is never closed (missing ')')");
        }
        self.bump();
        Ok(Some(name))
    }

    fn parse_arg_or_prop(&mut self) -> PResult<ArgOrProp> {
        let ty = self.parse_type_annotation()?;
        let pos = self.pos();
        let word = self.parse_word()?;
        // A string or a bare word directly followed by '=' is a property key.
        if let Some(n) = self.eq_ahead() {
            let key = match &word {
                Word::Str { text, .. } => Some(text.clone()),
                Word::Keyword(_, spelling) => Some(spelling.clone()),
                Word::Number(_) => None,
            };
            if let Some(key) = key {
                if ty.is_some() {
                    return Self::err_at(pos, "a property name cannot have a type annotation");
                }
                for _ in 0..=n {
                    self.bump();
                }
                self.skip_spaces();
                let value = self.parse_value()?;
                return Ok(ArgOrProp::Prop(Prop {
                    key,
                    key_pos: pos,
                    value,
                }));
            }
        }
        Ok(ArgOrProp::Arg(word_to_value(word, ty, pos)))
    }

    /// A value: used after `=` and for the argument of the `name=value` form.
    fn parse_value(&mut self) -> PResult<Value> {
        let ty = self.parse_type_annotation()?;
        let pos = self.pos();
        let word = self.parse_word()?;
        Ok(word_to_value(word, ty, pos))
    }

    fn parse_word(&mut self) -> PResult<Word> {
        match self.peek() {
            None => self.err("expected a value"),
            Some('"') => {
                let text = self.parse_quoted()?;
                Ok(Word::Str { text, quoted: true })
            }
            Some('r') if self.raw_string_ahead() => {
                let text = self.parse_raw()?;
                Ok(Word::Str { text, quoted: true })
            }
            Some('#') => {
                if self.v2_raw_string_ahead() {
                    let text = self.parse_raw()?;
                    return Ok(Word::Str { text, quoted: true });
                }
                let at = self.pos();
                self.bump();
                let name = self.scan_word();
                let spelling = format!("#{}", name);
                match name.as_str() {
                    "true" => Ok(Word::Keyword(Scalar::Bool(true), spelling)),
                    "false" => Ok(Word::Keyword(Scalar::Bool(false), spelling)),
                    "null" => Ok(Word::Keyword(Scalar::Null, spelling)),
                    "inf" => Ok(Word::Number(Scalar::Float(f64::INFINITY))),
                    "-inf" => Ok(Word::Number(Scalar::Float(f64::NEG_INFINITY))),
                    "nan" => Ok(Word::Number(Scalar::Float(f64::NAN))),
                    _ => Self::err_at(
                        at,
                        format!(
                            "'#{}' is not a value (#true, #false, #null or a raw string #\"...\"#)",
                            name
                        ),
                    ),
                }
            }
            Some(c) if c.is_ascii_digit() => {
                let n = self.parse_number()?;
                Ok(Word::Number(n))
            }
            Some(c)
                if (c == '+' || c == '-')
                    && matches!(self.peek_at(1), Some(d) if d.is_ascii_digit()) =>
            {
                let n = self.parse_number()?;
                Ok(Word::Number(n))
            }
            Some(c) if is_word_char(c) => {
                let w = self.scan_word();
                if w.is_empty() {
                    return self.err(format!("unexpected '{}'", c));
                }
                let keyword = match w.as_str() {
                    "true" => Some(Scalar::Bool(true)),
                    "false" => Some(Scalar::Bool(false)),
                    "null" => Some(Scalar::Null),
                    _ => None,
                };
                match keyword {
                    Some(k) => Ok(Word::Keyword(k, w)),
                    None => Ok(Word::Str {
                        text: w,
                        quoted: false,
                    }),
                }
            }
            Some(c) => self.err(unexpected_message(c, "a value")),
        }
    }

    /// A run of bare-word characters.  A `/` is part of the word unless it starts a comment.
    fn scan_word(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c == '/' {
                if matches!(self.peek_at(1), Some('/') | Some('*')) {
                    break;
                }
            } else if !is_word_char(c) {
                break;
            }
            s.push(c);
            self.bump();
        }
        s
    }

    // ---- numbers -----------------------------------------------------------------------

    fn expect_number_end(&self, at: Pos) -> PResult<()> {
        match self.peek() {
            Some('/') if matches!(self.peek_at(1), Some('/') | Some('*')) => Ok(()),
            Some(c) if is_word_char(c) || c == '/' => Self::err_at(
                at,
                "this is not a valid number (quote it if it is meant to be text)",
            ),
            _ => Ok(()),
        }
    }

    fn parse_number(&mut self) -> PResult<Scalar> {
        let at = self.pos();
        let mut negative = false;
        match self.peek() {
            Some('+') => {
                self.bump();
            }
            Some('-') => {
                negative = true;
                self.bump();
            }
            _ => {}
        }
        if self.peek() == Some('0') && matches!(self.peek_at(1), Some('x') | Some('o') | Some('b'))
        {
            let radix: u32 = match self.peek_at(1) {
                Some('x') => 16,
                Some('o') => 8,
                _ => 2,
            };
            self.bump();
            self.bump();
            let mut digits = String::new();
            while let Some(c) = self.peek() {
                if c == '_' {
                    self.bump();
                } else if c.is_digit(radix) {
                    digits.push(c);
                    self.bump();
                } else {
                    break;
                }
            }
            if digits.is_empty() {
                return Self::err_at(at, "this number has no digits after its prefix");
            }
            self.expect_number_end(at)?;
            return match i64::from_str_radix(&digits, radix) {
                Ok(v) => Ok(Scalar::Int(if negative { -v } else { v })),
                Err(_) => Self::err_at(at, "this number is too large"),
            };
        }
        let mut text = String::new();
        let mut is_float = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                text.push(c);
                self.bump();
            } else if c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        if self.peek() == Some('.') && matches!(self.peek_at(1), Some(d) if d.is_ascii_digit()) {
            is_float = true;
            text.push('.');
            self.bump();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    text.push(c);
                    self.bump();
                } else if c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            let signed = matches!(self.peek_at(1), Some('+') | Some('-'));
            let digit_at = if signed { 2 } else { 1 };
            if matches!(self.peek_at(digit_at), Some(d) if d.is_ascii_digit()) {
                is_float = true;
                text.push('e');
                self.bump();
                if signed {
                    if let Some(sign) = self.bump() {
                        text.push(sign);
                    }
                }
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        text.push(c);
                        self.bump();
                    } else if c == '_' {
                        self.bump();
                    } else {
                        break;
                    }
                }
            }
        }
        self.expect_number_end(at)?;
        if is_float {
            return match text.parse::<f64>() {
                Ok(v) => Ok(Scalar::Float(if negative { -v } else { v })),
                Err(_) => Self::err_at(at, "this is not a valid number"),
            };
        }
        match text.parse::<i64>() {
            Ok(v) => Ok(Scalar::Int(if negative { -v } else { v })),
            Err(_) => match text.parse::<f64>() {
                Ok(v) => Ok(Scalar::Float(if negative { -v } else { v })),
                Err(_) => Self::err_at(at, "this is not a valid number"),
            },
        }
    }

    // ---- strings -----------------------------------------------------------------------

    /// `r"..."` or `r#"..."#`: the cursor is on the `r`.
    fn raw_string_ahead(&self) -> bool {
        if self.peek() != Some('r') {
            return false;
        }
        let mut n = 1usize;
        while self.peek_at(n) == Some('#') {
            n += 1;
        }
        self.peek_at(n) == Some('"')
    }

    /// `#"..."#`: the cursor is on the first `#`.
    fn v2_raw_string_ahead(&self) -> bool {
        if self.peek() != Some('#') {
            return false;
        }
        let mut n = 1usize;
        while self.peek_at(n) == Some('#') {
            n += 1;
        }
        self.peek_at(n) == Some('"')
    }

    fn parse_raw(&mut self) -> PResult<String> {
        let start = self.pos();
        if self.peek() == Some('r') {
            self.bump();
        }
        let mut hashes = 0usize;
        while self.peek() == Some('#') {
            self.bump();
            hashes += 1;
        }
        if self.peek() != Some('"') {
            return self.err("expected '\"'");
        }
        self.bump();
        let mut out = String::new();
        loop {
            let c = match self.bump() {
                Some(c) => c,
                None => return Self::err_at(start, "this raw string is never closed"),
            };
            if c == '"' {
                let mut n = 0usize;
                while n < hashes && self.peek_at(n) == Some('#') {
                    n += 1;
                }
                if n == hashes {
                    for _ in 0..hashes {
                        self.bump();
                    }
                    return Ok(out);
                }
            }
            out.push(c);
        }
    }

    /// The text after an opening quote starts like a Windows path (`C:\` or `\\`).
    fn path_like_ahead(&self) -> bool {
        match (self.peek(), self.peek_at(1), self.peek_at(2)) {
            (Some(d), Some(':'), Some('\\')) if d.is_ascii_alphabetic() => true,
            (Some('\\'), Some('\\'), _) => true,
            _ => false,
        }
    }

    fn parse_quoted(&mut self) -> PResult<String> {
        let start = self.pos();
        self.bump(); // the opening quote
        let path_mode = self.path_like_ahead();
        let mut out = String::new();
        loop {
            let at = self.pos();
            let c = match self.bump() {
                Some(c) => c,
                None => return Self::err_at(start, "this string is never closed (missing '\"')"),
            };
            match c {
                '"' => return Ok(out),
                '\\' if path_mode => {
                    // Windows command-line rules: backslashes are literal unless a run of
                    // them ends in a quote; 2n backslashes + quote = n backslashes and the
                    // quote ends the string, 2n+1 = n backslashes and a literal quote.
                    let mut run = 1usize;
                    while self.peek() == Some('\\') {
                        self.bump();
                        run += 1;
                    }
                    if self.peek() == Some('"') {
                        for _ in 0..(run / 2) {
                            out.push('\\');
                        }
                        if run % 2 == 1 {
                            self.bump();
                            out.push('"');
                        }
                    } else {
                        for _ in 0..run {
                            out.push('\\');
                        }
                    }
                }
                '\\' => {
                    let esc = match self.bump() {
                        Some(e) => e,
                        None => {
                            return Self::err_at(start, "this string is never closed (missing '\"')")
                        }
                    };
                    match esc {
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        '\\' => out.push('\\'),
                        '"' => out.push('"'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{0008}'),
                        'f' => out.push('\u{000C}'),
                        's' => out.push(' '),
                        'u' => out.push(self.parse_unicode_escape(at)?),
                        e if is_newline(e) || is_ws(e) => {
                            // A backslash before a line break joins the lines; the
                            // whitespace that follows is dropped.
                            if is_newline(e) {
                                if e == '\r' && self.peek() == Some('\n') {
                                    self.bump();
                                }
                                while matches!(self.peek(), Some(w) if is_ws(w) || is_newline(w)) {
                                    self.bump();
                                }
                            } else {
                                return Self::err_at(
                                    at,
                                    "a backslash in a string must be followed by an escape character",
                                );
                            }
                        }
                        other => {
                            return Self::err_at(
                                at,
                                format!(
                                    "'\\{}' is not an escape sequence; to write a backslash use \
                                     \\\\ (for example C:\\\\Program Files) or a raw string r\"...\"",
                                    other
                                ),
                            )
                        }
                    }
                }
                c => out.push(c),
            }
        }
    }

    fn parse_unicode_escape(&mut self, at: Pos) -> PResult<char> {
        if self.peek() != Some('{') {
            return Self::err_at(at, "\\u must be followed by {hex digits}, like \\u{1F600}");
        }
        self.bump();
        let mut digits = String::new();
        loop {
            match self.bump() {
                Some('}') => break,
                Some(c) if c.is_ascii_hexdigit() && digits.len() < 6 => digits.push(c),
                _ => {
                    return Self::err_at(at, "\\u{...} takes one to six hexadecimal digits")
                }
            }
        }
        if digits.is_empty() {
            return Self::err_at(at, "\\u{...} takes one to six hexadecimal digits");
        }
        let code = u32::from_str_radix(&digits, 16).unwrap_or(0xFFFF_FFFF);
        match char::from_u32(code) {
            Some(c) => Ok(c),
            None => Self::err_at(at, "this is not a valid Unicode character"),
        }
    }
}

fn word_to_value(word: Word, ty: Option<String>, pos: Pos) -> Value {
    let scalar = match word {
        Word::Str { text, quoted } => Scalar::Str { text, quoted },
        Word::Number(n) => n,
        Word::Keyword(k, _) => k,
    };
    Value { scalar, ty, pos }
}

fn unexpected_message(c: char, wanted: &str) -> String {
    match c {
        '{' => format!("expected {}, found '{{' (a children block needs a node before it)", wanted),
        '}' => format!("expected {}, found '}}'", wanted),
        '=' => format!("expected {}, found '=' (a property needs a name before the '=')", wanted),
        ')' | '(' | '[' | ']' => format!("expected {}, found '{}'", wanted, c),
        '\\' => format!(
            "expected {}, found '\\' (use a quoted string with \\\\ or a raw string r\"...\")",
            wanted
        ),
        _ => format!("expected {}, found '{}'", wanted, c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        match parse(text) {
            Ok(d) => d,
            Err(e) => panic!("parse error {} in {:?}", e, text),
        }
    }

    fn err(text: &str) -> SyntaxError {
        match parse(text) {
            Ok(d) => panic!("expected an error, got {:?} for {:?}", d, text),
            Err(e) => e,
        }
    }

    fn s(text: &str) -> Scalar {
        Scalar::Str {
            text: text.to_string(),
            quoted: true,
        }
    }

    fn bare(text: &str) -> Scalar {
        Scalar::Str {
            text: text.to_string(),
            quoted: false,
        }
    }

    #[test]
    fn empty_and_comment_only_documents() {
        assert!(doc("").nodes.is_empty());
        assert!(doc("\n\n   \n").nodes.is_empty());
        assert!(doc("// only a comment").nodes.is_empty());
        assert!(doc("/* block */ // line\n").nodes.is_empty());
    }

    #[test]
    fn nodes_arguments_and_properties() {
        let d = doc("node 1 \"two\" three=4 five=\"six\" true null\n");
        let n = &d.nodes[0];
        assert_eq!(n.name, "node");
        assert_eq!(n.args.len(), 4);
        assert_eq!(n.args[0].scalar, Scalar::Int(1));
        assert_eq!(n.args[1].scalar, s("two"));
        assert_eq!(n.args[2].scalar, Scalar::Bool(true));
        assert_eq!(n.args[3].scalar, Scalar::Null);
        assert_eq!(n.props.len(), 2);
        assert_eq!(n.props[0].key, "three");
        assert_eq!(n.props[0].value.scalar, Scalar::Int(4));
        assert_eq!(n.props[1].key, "five");
        assert_eq!(n.prop("five").map(|v| v.scalar.clone()), Some(s("six")));
    }

    #[test]
    fn children_semicolons_and_one_line_blocks() {
        let d = doc("a { b; c 1; d { e } }\nf");
        assert_eq!(d.nodes.len(), 2);
        let kids = d.nodes[0].kids();
        assert_eq!(kids.len(), 3);
        assert_eq!(kids[0].name, "b");
        assert_eq!(kids[1].args[0].scalar, Scalar::Int(1));
        assert_eq!(kids[2].kids()[0].name, "e");
        assert_eq!(d.nodes[1].name, "f");
        // An empty block is Some(empty), no block is None.
        let d = doc("a {}\nb");
        assert_eq!(d.nodes[0].children, Some(vec![]));
        assert_eq!(d.nodes[1].children, None);
    }

    #[test]
    fn multi_line_blocks_and_node_terminators() {
        let d = doc("outer {\n    inner 1\n    inner2 \"x\" // trailing comment\n}\nnext;other");
        assert_eq!(d.nodes.len(), 3);
        assert_eq!(d.nodes[0].kids().len(), 2);
        assert_eq!(d.nodes[1].name, "next");
        assert_eq!(d.nodes[2].name, "other");
    }

    #[test]
    fn crlf_and_bom_are_whitespace() {
        let d = doc("\u{FEFF}a 1\r\nb 2\r\n");
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.nodes[0].args[0].scalar, Scalar::Int(1));
        assert_eq!(d.nodes[1].pos, Pos { line: 2, col: 1 });
    }

    #[test]
    fn slashdash_comments_out_nodes_arguments_properties_and_blocks() {
        let d = doc("/-a 1\nb /-1 2 /-x=1 y=2 /-{ c }\n/-d { e }\nf");
        assert_eq!(d.nodes.len(), 2, "{:?}", d);
        let b = &d.nodes[0];
        assert_eq!(b.name, "b");
        assert_eq!(b.args.len(), 1);
        assert_eq!(b.args[0].scalar, Scalar::Int(2));
        assert_eq!(b.props.len(), 1);
        assert_eq!(b.props[0].key, "y");
        assert!(b.children.is_none(), "the slashdashed block is dropped");
        assert_eq!(d.nodes[1].name, "f");
        // A slashdashed section keeps the rest working (niri: /-output "eDP-1" { off }).
        let d = doc("/-output \"eDP-1\" {\n    off\n}\nlayout { gaps 8 }");
        assert_eq!(d.nodes.len(), 1);
        assert_eq!(d.nodes[0].name, "layout");
    }

    #[test]
    fn slashdash_inside_a_block() {
        let d = doc("a {\n    /-b\n    c\n}");
        assert_eq!(d.nodes[0].kids().len(), 1);
        assert_eq!(d.nodes[0].kids()[0].name, "c");
    }

    #[test]
    fn block_comments_nest() {
        let d = doc("a /* x /* y */ z */ 1 /* tail */\nb");
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.nodes[0].args.len(), 1);
        let e = err("a /* never closed");
        assert!(e.message.contains("never closed"), "{}", e);
        assert_eq!(e.pos, Pos { line: 1, col: 3 });
    }

    #[test]
    fn line_continuation() {
        let d = doc("a 1 \\\n  2 \\ // note\n  3\nb");
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.nodes[0].args.len(), 3);
        // A stray backslash is an error that points at it.
        let e = err("a 1 \\ 2");
        assert_eq!(e.pos, Pos { line: 1, col: 5 });
    }

    #[test]
    fn quoted_string_escapes() {
        let d = doc(r#"a "tab\there" "nl\nx" "q\"q" "bs\\" "slash\/" "sp\s" "u\u{1F600}" "bf\b\f" "#);
        let a = &d.nodes[0];
        assert_eq!(a.args[0].as_str(), Some("tab\there"));
        assert_eq!(a.args[1].as_str(), Some("nl\nx"));
        assert_eq!(a.args[2].as_str(), Some("q\"q"));
        assert_eq!(a.args[3].as_str(), Some("bs\\"));
        assert_eq!(a.args[4].as_str(), Some("slash/"));
        assert_eq!(a.args[5].as_str(), Some("sp "));
        assert_eq!(a.args[6].as_str(), Some("u\u{1F600}"));
        assert_eq!(a.args[7].as_str(), Some("bf\u{8}\u{c}"));
    }

    #[test]
    fn a_bad_escape_is_an_error_with_a_hint() {
        let e = err("a \"x\\qy\"");
        assert!(e.message.contains("'\\q'"), "{}", e);
        assert!(e.message.contains("raw string"), "{}", e);
        assert_eq!(e.pos, Pos { line: 1, col: 5 });
        let e = err("spawn-sh \"echo \\w\"");
        assert!(e.message.contains("not an escape"), "{}", e);
        let e = err("a \"\\u{110000}\"");
        assert!(e.message.contains("Unicode"), "{}", e);
        let e = err("a \"\\u12\"");
        assert!(e.message.contains("\\u"), "{}", e);
    }

    #[test]
    fn windows_paths_keep_their_backslashes() {
        // The old format read these literally; they must go on working.
        let d = doc(r#"a "C:\Program Files\x y\z.exe" "D:\tmp\new\bin" "\\server\share\f""#);
        let a = &d.nodes[0];
        assert_eq!(a.args[0].as_str(), Some(r"C:\Program Files\x y\z.exe"));
        assert_eq!(a.args[1].as_str(), Some(r"D:\tmp\new\bin"));
        assert_eq!(a.args[2].as_str(), Some(r"\\server\share\f"));
        // `\\"` at the end is one backslash and the end of the string; `\"` is a quote.
        let d = doc(r#"a "C:\Program Files\startup dir\\" "C:\say \"hi\" now""#);
        let a = &d.nodes[0];
        assert_eq!(a.args[0].as_str(), Some(r"C:\Program Files\startup dir\"));
        assert_eq!(a.args[1].as_str(), Some(r#"C:\say "hi" now"#));
        // Outside paths the escapes are KDL's.
        let d = doc(r#"a "echo \"x\" > out.txt""#);
        assert_eq!(d.nodes[0].args[0].as_str(), Some("echo \"x\" > out.txt"));
    }

    #[test]
    fn raw_strings_in_both_spellings() {
        let d = doc("a r\"C:\\x\\n\" r#\"say \"hi\"\"# #\"v2 \\ raw\"# r##\"a\"#b\"##");
        let a = &d.nodes[0];
        assert_eq!(a.args[0].as_str(), Some("C:\\x\\n"));
        assert_eq!(a.args[1].as_str(), Some("say \"hi\""));
        assert_eq!(a.args[2].as_str(), Some("v2 \\ raw"));
        assert_eq!(a.args[3].as_str(), Some("a\"#b"));
        assert!(a.args.iter().all(|v| v.is_quoted_str()));
        let e = err("a r#\"never closed");
        assert!(e.message.contains("raw string"), "{}", e);
        // A word that merely starts with r is a word.
        let d = doc("repeat right r");
        assert_eq!(d.nodes[0].name, "repeat");
        assert_eq!(d.nodes[0].args[0].scalar, bare("right"));
        assert_eq!(d.nodes[0].args[1].scalar, bare("r"));
    }

    #[test]
    fn numbers() {
        let d = doc("n 12 -3 +4 1_000 0x1F 0o17 0b101 -0x10 1.5 -2.25 1e3 1.5E-2 2_0.5");
        let a = &d.nodes[0].args;
        let v: Vec<Scalar> = a.iter().map(|x| x.scalar.clone()).collect();
        assert_eq!(
            v,
            vec![
                Scalar::Int(12),
                Scalar::Int(-3),
                Scalar::Int(4),
                Scalar::Int(1000),
                Scalar::Int(31),
                Scalar::Int(15),
                Scalar::Int(5),
                Scalar::Int(-16),
                Scalar::Float(1.5),
                Scalar::Float(-2.25),
                Scalar::Float(1000.0),
                Scalar::Float(0.015),
                Scalar::Float(20.5),
            ]
        );
        assert_eq!(a[0].as_f64(), Some(12.0));
        assert_eq!(a[8].as_i64(), None);
        // `12px` is neither a number nor a safe word.
        let e = err("n 12px");
        assert!(e.message.contains("not a valid number"), "{}", e);
        assert_eq!(e.pos, Pos { line: 1, col: 3 });
        let e = err("n 0x");
        assert!(e.message.contains("no digits"), "{}", e);
    }

    #[test]
    fn keywords_both_spellings_and_bare_words() {
        let d = doc("n true false null #true #false #null word wt.exe -x +y");
        let v: Vec<Scalar> = d.nodes[0].args.iter().map(|x| x.scalar.clone()).collect();
        assert_eq!(
            v,
            vec![
                Scalar::Bool(true),
                Scalar::Bool(false),
                Scalar::Null,
                Scalar::Bool(true),
                Scalar::Bool(false),
                Scalar::Null,
                bare("word"),
                bare("wt.exe"),
                bare("-x"),
                bare("+y"),
            ]
        );
        let e = err("n #bogus");
        assert!(e.message.contains("#bogus"), "{}", e);
    }

    #[test]
    fn bind_chords_are_node_names() {
        let d = doc(
            "Ctrl+Alt+Left focus-column-left\nMod+Shift+Bracket_Left { x; }\nCtrl+Alt+, y\nMod+1 z\nF5 w",
        );
        let names: Vec<&str> = d.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Ctrl+Alt+Left", "Mod+Shift+Bracket_Left", "Ctrl+Alt+,", "Mod+1", "F5"]
        );
        assert_eq!(d.nodes[0].args[0].scalar, bare("focus-column-left"));
        // A slash is part of a word unless it starts a comment.
        let d = doc("Ctrl+Alt+/ show-key-bindings // comment");
        assert_eq!(d.nodes[0].name, "Ctrl+Alt+/");
        assert_eq!(d.nodes[0].args[0].scalar, bare("show-key-bindings"));
    }

    #[test]
    fn quoted_node_names_and_property_keys() {
        let d = doc("\"Mod+T\" repeat=false \"a b\"=1");
        let n = &d.nodes[0];
        assert_eq!(n.name, "Mod+T");
        assert_eq!(n.props[0].key, "repeat");
        assert_eq!(n.props[0].value.scalar, Scalar::Bool(false));
        assert_eq!(n.props[1].key, "a b");
    }

    #[test]
    fn niri_bind_syntax() {
        let d = doc(
            "binds {\n    Mod+T hotkey-overlay-title=\"Open a Terminal: alacritty\" { spawn \"alacritty\"; }\n    Mod+Left repeat=false cooldown-ms=150 { focus-column-left; }\n    XF86AudioMute allow-when-locked=true { spawn-sh \"wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle\"; }\n}\n",
        );
        let binds = &d.nodes[0];
        let kids = binds.kids();
        assert_eq!(kids.len(), 3);
        assert_eq!(kids[0].name, "Mod+T");
        assert_eq!(
            kids[0].prop("hotkey-overlay-title").and_then(|v| v.as_str()),
            Some("Open a Terminal: alacritty")
        );
        assert_eq!(kids[0].kids()[0].name, "spawn");
        assert_eq!(kids[1].prop("repeat").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(kids[1].prop("cooldown-ms").and_then(|v| v.as_i64()), Some(150));
        assert_eq!(kids[2].kids()[0].args[0].as_str(), Some("wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"));
    }

    #[test]
    fn niri_window_rule_syntax() {
        let d = doc(
            "window-rule {\n    match app-id=r#\"firefox$\"# title=\"^Picture-in-Picture$\"\n    exclude app-id=\"x\" is-active=false\n    open-floating true\n    default-column-width { proportion 0.5; }\n}\n",
        );
        let rule = &d.nodes[0];
        let kids = rule.kids();
        assert_eq!(kids[0].name, "match");
        assert_eq!(kids[0].prop("app-id").and_then(|v| v.as_str()), Some("firefox$"));
        assert_eq!(kids[1].name, "exclude");
        assert_eq!(kids[1].prop("is-active").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(kids[3].kids()[0].args[0].as_f64(), Some(0.5));
    }

    #[test]
    fn wiri_legacy_line_forms() {
        // `key=value` as a whole line, with and without spaces.
        let d = doc("repeat_delay=600\nclass-name = \"Firefox\"\nborder_color=\"#333\"");
        assert_eq!(d.nodes[0].name, "repeat_delay");
        assert_eq!(d.nodes[0].args[0].scalar, Scalar::Int(600));
        assert_eq!(d.nodes[1].name, "class-name");
        assert_eq!(d.nodes[1].args[0].scalar, s("Firefox"));
        assert_eq!(d.nodes[2].args[0].scalar, s("#333"));
        // Bare words as values.
        let d = doc("hide-method hide\nCtrl+Alt+1 switch-workspace 1");
        assert_eq!(d.nodes[0].args[0].scalar, bare("hide"));
        assert_eq!(d.nodes[1].args[0].scalar, bare("switch-workspace"));
        assert_eq!(d.nodes[1].args[1].scalar, Scalar::Int(1));
    }

    #[test]
    fn type_annotations() {
        let d = doc("(t)node (u8)5 key=(f32)1.5 (str)\"x\"");
        let n = &d.nodes[0];
        assert_eq!(n.ty.as_deref(), Some("t"));
        assert_eq!(n.args[0].ty.as_deref(), Some("u8"));
        assert_eq!(n.props[0].value.ty.as_deref(), Some("f32"));
        assert_eq!(n.args[1].ty.as_deref(), Some("str"));
        let e = err("node (u8 5");
        assert!(e.message.contains("never closed"), "{}", e);
    }

    #[test]
    fn positions_are_line_and_column() {
        let d = doc("a\n  bb 1\n\tccc \"x\" k=2\n");
        assert_eq!(d.nodes[0].pos, Pos { line: 1, col: 1 });
        assert_eq!(d.nodes[1].pos, Pos { line: 2, col: 3 });
        assert_eq!(d.nodes[1].args[0].pos, Pos { line: 2, col: 6 });
        assert_eq!(d.nodes[2].pos, Pos { line: 3, col: 2 });
        assert_eq!(d.nodes[2].args[0].pos, Pos { line: 3, col: 6 });
        assert_eq!(d.nodes[2].props[0].key_pos, Pos { line: 3, col: 10 });
        // Columns count characters, not bytes.
        let d = doc("é \"ü\" b");
        assert_eq!(d.nodes[0].args[1].pos, Pos { line: 1, col: 7 });
    }

    #[test]
    fn syntax_errors_say_where() {
        let e = err("a {\n  b\n");
        assert_eq!(e.pos, Pos { line: 1, col: 3 });
        assert!(e.message.contains("never closed"), "{}", e);
        let e = err("a }\n");
        assert_eq!(e.pos, Pos { line: 1, col: 3 });
        let e = err("}\n");
        assert_eq!(e.pos, Pos { line: 1, col: 1 });
        let e = err("a \"open\nstill open");
        assert_eq!(e.pos, Pos { line: 1, col: 3 });
        assert!(e.message.contains("never closed"), "{}", e);
        let e = err("a { b } c");
        assert!(e.message.contains("nothing can follow") || e.message.contains("expected"), "{}", e);
        let e = err("a =");
        assert!(e.message.contains("expected a value"), "{}", e);
        let e = err("a b=");
        assert!(e.message.contains("expected a value"), "{}", e);
        let e = err("a 1 {} {}");
        assert!(e.message.contains("only one children block"), "{}", e);
        let e = err("a\n=5");
        assert_eq!(e.pos, Pos { line: 2, col: 1 });
        let e = err("/-");
        assert!(e.message.contains("expected a node"), "{}", e);
        let e = err("a /-");
        assert!(e.message.contains("after '/-'"), "{}", e);
    }

    #[test]
    fn a_node_needs_space_between_its_parts() {
        let e = err("a\"x\"");
        assert!(e.message.contains("space"), "{}", e);
        // But a block may follow the name directly.
        let d = doc("layout{ gaps 8 }");
        assert_eq!(d.nodes[0].kids()[0].name, "gaps");
    }

    #[test]
    fn a_trailing_comment_ends_the_node() {
        let d = doc("a 1 // note\nb 2 /* c */ 3 // d\n");
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.nodes[0].args.len(), 1);
        assert_eq!(d.nodes[1].args.len(), 2);
    }

    #[test]
    fn value_helpers() {
        let d = doc("n 1 2.5 \"s\" w true");
        let a = &d.nodes[0].args;
        assert_eq!(a[0].kind(), "integer");
        assert_eq!(a[1].kind(), "number");
        assert_eq!(a[2].kind(), "string");
        assert_eq!(a[3].kind(), "word");
        assert_eq!(a[4].kind(), "boolean");
        assert_eq!(a[0].to_text(), "1");
        assert_eq!(a[1].to_text(), "2.5");
        assert_eq!(a[2].to_text(), "s");
        assert_eq!(a[4].to_text(), "true");
        assert_eq!(a[3].as_str(), Some("w"));
        assert!(!a[3].is_quoted_str());
    }

    #[test]
    fn the_default_style_config_parses() {
        let text = "version \"1.0\"\ninput {\n    keyboard {\n        layout \"us\"\n        repeat-delay 250        // ms\n    }\n    focus-follows-mouse\n}\nlayout {\n    gaps 16\n    border { width 4; active-color \"#ffc87f\"; }\n    preset-column-widths {\n        proportion 0.33333\n        fixed 1920\n    }\n}\nspawn-at-startup \"wt.exe\" \"-d\" \"C:\\Users\\me\"\nworkspace \"web\"\n";
        let d = doc(text);
        let names: Vec<&str> = d.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["version", "input", "layout", "spawn-at-startup", "workspace"]);
        assert_eq!(d.nodes[3].args[2].as_str(), Some(r"C:\Users\me"));
    }
}
