//! Lexer for the robolang DSL.

#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxError {
    pub msg: String,
    pub line: u32,
}

impl SyntaxError {
    fn new(msg: impl Into<String>, line: u32) -> SyntaxError {
        SyntaxError {
            msg: msg.into(),
            line,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Func,
    Var,
    If,
    Else,
    While,
    For,
    Return,
    Break,
    Continue,
    True,
    False,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Not,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semi,
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
}

struct Lexer<'a> {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    _src: &'a str,
}

pub fn lex(source: &str) -> Result<Vec<Token>, SyntaxError> {
    let mut lx = Lexer {
        chars: source.chars().collect(),
        pos: 0,
        line: 1,
        _src: source,
    };
    let mut out = Vec::new();
    loop {
        lx.skip_ws_and_comments()?;
        let line = lx.line;
        if lx.pos >= lx.chars.len() {
            out.push(Token {
                tok: Tok::Eof,
                line,
            });
            return Ok(out);
        }
        let c = lx.peek();
        let tok = if c.is_ascii_digit() {
            lx.num()?
        } else if c == '"' {
            Tok::Str(lx.string()?)
        } else if c.is_ascii_alphabetic() || c == '_' {
            lx.ident()
        } else {
            lx.op()?
        };
        out.push(Token { tok, line });
    }
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> char {
        self.chars[self.pos]
    }

    fn bump(&mut self) -> char {
        let c = self.chars[self.pos];
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
        }
        c
    }

    fn skip_ws_and_comments(&mut self) -> Result<(), SyntaxError> {
        loop {
            while self.pos < self.chars.len() && self.peek().is_whitespace() {
                self.bump();
            }
            if self.pos + 1 < self.chars.len() && self.peek() == '/' {
                let next = self.chars[self.pos + 1];
                if next == '/' {
                    while self.pos < self.chars.len() && self.peek() != '\n' {
                        self.bump();
                    }
                    continue;
                } else if next == '*' {
                    self.bump();
                    self.bump();
                    let mut closed = false;
                    while self.pos < self.chars.len() {
                        if self.peek() == '*'
                            && self.pos + 1 < self.chars.len()
                            && self.chars[self.pos + 1] == '/'
                        {
                            self.bump();
                            self.bump();
                            closed = true;
                            break;
                        }
                        self.bump();
                    }
                    if !closed {
                        return Err(SyntaxError::new("unterminated block comment", self.line));
                    }
                    continue;
                }
            }
            break;
        }
        Ok(())
    }

    fn num(&mut self) -> Result<Tok, SyntaxError> {
        let mut s = String::new();
        while self.pos < self.chars.len() && self.peek().is_ascii_digit() {
            s.push(self.bump());
        }
        if self.pos < self.chars.len()
            && self.peek() == '.'
            && self.pos + 1 < self.chars.len()
            && self.chars[self.pos + 1].is_ascii_digit()
        {
            s.push(self.bump());
            while self.pos < self.chars.len() && self.peek().is_ascii_digit() {
                s.push(self.bump());
            }
        }
        s.parse::<f64>()
            .map(Tok::Num)
            .map_err(|_| SyntaxError::new(format!("invalid number '{}'", s), self.line))
    }

    fn string(&mut self) -> Result<String, SyntaxError> {
        self.bump(); // opening quote
        let mut s = String::new();
        loop {
            if self.pos >= self.chars.len() {
                return Err(SyntaxError::new("unterminated string", self.line));
            }
            let c = self.bump();
            match c {
                '"' => {
                    if s.len() > crate::value::MAX_STRING_LEN {
                        return Err(SyntaxError::new(
                            format!(
                                "string literal too long ({} bytes; the limit is {})",
                                s.len(),
                                crate::value::MAX_STRING_LEN
                            ),
                            self.line,
                        ));
                    }
                    return Ok(s);
                }
                '\\' => {
                    if self.pos >= self.chars.len() {
                        return Err(SyntaxError::new("unterminated escape", self.line));
                    }
                    match self.bump() {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        other => {
                            return Err(SyntaxError::new(
                                format!("unknown escape '\\{}'", other),
                                self.line,
                            ))
                        }
                    }
                }
                other => s.push(other),
            }
        }
    }

    fn ident(&mut self) -> Tok {
        let mut s = String::new();
        while self.pos < self.chars.len()
            && (self.peek().is_ascii_alphanumeric() || self.peek() == '_')
        {
            s.push(self.bump());
        }
        match s.as_str() {
            "func" => Tok::Func,
            "var" => Tok::Var,
            "if" => Tok::If,
            "else" => Tok::Else,
            "while" => Tok::While,
            "for" => Tok::For,
            "return" => Tok::Return,
            "break" => Tok::Break,
            "continue" => Tok::Continue,
            "true" => Tok::True,
            "false" => Tok::False,
            _ => Tok::Ident(s),
        }
    }

    fn op(&mut self) -> Result<Tok, SyntaxError> {
        let c = self.bump();
        let two = |lx: &mut Self, expect: char| -> bool {
            if lx.pos < lx.chars.len() && lx.peek() == expect {
                lx.bump();
                true
            } else {
                false
            }
        };
        Ok(match c {
            '+' => {
                if two(self, '=') {
                    Tok::PlusAssign
                } else {
                    Tok::Plus
                }
            }
            '-' => {
                if two(self, '=') {
                    Tok::MinusAssign
                } else {
                    Tok::Minus
                }
            }
            '*' => {
                if two(self, '=') {
                    Tok::StarAssign
                } else {
                    Tok::Star
                }
            }
            '/' => {
                if two(self, '=') {
                    Tok::SlashAssign
                } else {
                    Tok::Slash
                }
            }
            '%' => Tok::Percent,
            '=' => {
                if two(self, '=') {
                    Tok::Eq
                } else {
                    Tok::Assign
                }
            }
            '!' => {
                if two(self, '=') {
                    Tok::Ne
                } else {
                    Tok::Not
                }
            }
            '<' => {
                if two(self, '=') {
                    Tok::Le
                } else {
                    Tok::Lt
                }
            }
            '>' => {
                if two(self, '=') {
                    Tok::Ge
                } else {
                    Tok::Gt
                }
            }
            '&' => {
                if two(self, '&') {
                    Tok::AndAnd
                } else {
                    return Err(SyntaxError::new(
                        "unexpected '&', did you mean '&&'?",
                        self.line,
                    ));
                }
            }
            '|' => {
                if two(self, '|') {
                    Tok::OrOr
                } else {
                    return Err(SyntaxError::new(
                        "unexpected '|', did you mean '||'?",
                        self.line,
                    ));
                }
            }
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '{' => Tok::LBrace,
            '}' => Tok::RBrace,
            ',' => Tok::Comma,
            ';' => Tok::Semi,
            other => {
                return Err(SyntaxError::new(
                    format!("unexpected character '{}'", other),
                    self.line,
                ))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn basic_tokens() {
        assert_eq!(
            toks("var x = 1.5; // hi"),
            vec![
                Tok::Var,
                Tok::Ident("x".into()),
                Tok::Assign,
                Tok::Num(1.5),
                Tok::Semi,
                Tok::Eof
            ]
        );
    }

    #[test]
    fn operators() {
        assert_eq!(
            toks("a <= b != c && !d || e"),
            vec![
                Tok::Ident("a".into()),
                Tok::Le,
                Tok::Ident("b".into()),
                Tok::Ne,
                Tok::Ident("c".into()),
                Tok::AndAnd,
                Tok::Not,
                Tok::Ident("d".into()),
                Tok::OrOr,
                Tok::Ident("e".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn strings_and_comments() {
        assert_eq!(
            toks("/* multi\nline */ \"a\\\"b\\n\""),
            vec![Tok::Str("a\"b\n".into()), Tok::Eof]
        );
    }

    #[test]
    fn rejects_overlong_string_literal() {
        let src = format!("\"{}\"", "a".repeat(crate::value::MAX_STRING_LEN + 1));
        assert!(lex(&src).unwrap_err().msg.contains("too long"));
        let ok = format!("\"{}\"", "a".repeat(crate::value::MAX_STRING_LEN));
        assert!(lex(&ok).is_ok());
    }

    #[test]
    fn error_on_bad_char() {
        assert!(lex("var a = `;").is_err());
    }
}
