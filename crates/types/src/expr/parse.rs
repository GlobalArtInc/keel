use serde_json::{Number, Value};

use super::Expr;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message} at offset {offset}")]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

impl ParseError {
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }
}

pub(super) fn parse(source: &str) -> Result<Expr, ParseError> {
    let tokens = lex(source)?;
    let mut parser = Parser {
        tokens,
        position: 0,
        end: source.len(),
    };
    let expr = parser.or()?;
    match parser.peek() {
        None => Ok(expr),
        Some(token) => Err(ParseError::new(
            token.offset,
            "unexpected token after the expression",
        )),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Variable(String),
    Str(String),
    Num(Number),
    True,
    False,
    Null,
    In,
    Matches,
    Empty,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Bang,
    And,
    Or,
    Eq,
    Ne,
    Gt,
    Lt,
}

#[derive(Clone, Debug)]
struct Spanned {
    offset: usize,
    token: Token,
}

fn lex(source: &str) -> Result<Vec<Spanned>, ParseError> {
    let mut tokens = Vec::new();
    let mut offset = 0;
    while offset < source.len() {
        let rest = &source[offset..];
        let byte = rest.as_bytes()[0];
        let next = rest.as_bytes().get(1).copied();
        let lexed = match byte {
            b' ' | b'\t' | b'\n' | b'\r' => {
                offset += 1;
                continue;
            }
            b'(' => Ok((Token::LParen, 1)),
            b')' => Ok((Token::RParen, 1)),
            b'[' => Ok((Token::LBracket, 1)),
            b']' => Ok((Token::RBracket, 1)),
            b',' => Ok((Token::Comma, 1)),
            b'>' => Ok((Token::Gt, 1)),
            b'<' => Ok((Token::Lt, 1)),
            b'!' if next == Some(b'=') => Ok((Token::Ne, 2)),
            b'!' => Ok((Token::Bang, 1)),
            b'=' if next == Some(b'=') => Ok((Token::Eq, 2)),
            b'&' if next == Some(b'&') => Ok((Token::And, 2)),
            b'|' if next == Some(b'|') => Ok((Token::Or, 2)),
            b'\'' | b'"' => lex_string(rest),
            b'-' | b'0'..=b'9' => lex_number(rest),
            b'$' | b'_' | b'a'..=b'z' | b'A'..=b'Z' => lex_word(rest),
            _ => Err(ParseError::new(0, "unexpected character")),
        };
        let (token, len) =
            lexed.map_err(|error| ParseError::new(offset + error.offset, error.message))?;
        tokens.push(Spanned { offset, token });
        offset += len;
    }
    Ok(tokens)
}

fn lex_string(rest: &str) -> Result<(Token, usize), ParseError> {
    let bytes = rest.as_bytes();
    let quote = bytes[0];
    let mut text = String::new();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                let escaped = bytes
                    .get(i + 1)
                    .ok_or_else(|| ParseError::new(i, "unterminated escape"))?;
                match escaped {
                    b'\\' | b'\'' | b'"' => text.push(*escaped as char),
                    _ => return Err(ParseError::new(i, "unknown escape")),
                }
                i += 2;
            }
            byte if byte == quote => return Ok((Token::Str(text), i + 1)),
            _ => {
                let ch = rest[i..]
                    .chars()
                    .next()
                    .ok_or_else(|| ParseError::new(i, "unterminated string"))?;
                text.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    Err(ParseError::new(0, "unterminated string"))
}

fn lex_number(rest: &str) -> Result<(Token, usize), ParseError> {
    let bytes = rest.as_bytes();
    let mut i = usize::from(bytes[0] == b'-');
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_start {
        return Err(ParseError::new(0, "expected digits"));
    }
    let mut is_float = false;
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        is_float = true;
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    let text = &rest[..i];
    let number = if is_float {
        let value: f64 = text
            .parse()
            .map_err(|_| ParseError::new(0, "invalid number"))?;
        Number::from_f64(value).ok_or_else(|| ParseError::new(0, "invalid number"))?
    } else {
        let value: i64 = text
            .parse()
            .map_err(|_| ParseError::new(0, "integer out of range"))?;
        Number::from(value)
    };
    Ok((Token::Num(number), i))
}

fn lex_word(rest: &str) -> Result<(Token, usize), ParseError> {
    let bytes = rest.as_bytes();
    let system = bytes[0] == b'$';
    let mut i = usize::from(system);
    let mut segments = 0;
    loop {
        let segment_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        if i == segment_start || bytes[segment_start].is_ascii_digit() {
            return Err(ParseError::new(segment_start, "expected a name"));
        }
        segments += 1;
        if bytes.get(i) == Some(&b'.') {
            i += 1;
            continue;
        }
        break;
    }
    let word = &rest[..i];
    if system || segments > 1 {
        return Ok((Token::Variable(word.to_owned()), i));
    }
    let token = match word {
        "in" => Token::In,
        "matches" => Token::Matches,
        "empty" => Token::Empty,
        "true" => Token::True,
        "false" => Token::False,
        "null" => Token::Null,
        _ => Token::Variable(word.to_owned()),
    };
    Ok((token, i))
}

struct Parser {
    tokens: Vec<Spanned>,
    position: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Spanned> {
        self.tokens.get(self.position)
    }

    fn offset(&self) -> usize {
        self.peek().map_or(self.end, |token| token.offset)
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self
            .peek()
            .is_some_and(|spanned| &spanned.token == expected)
        {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: Token, message: &str) -> Result<(), ParseError> {
        if self.eat(&expected) {
            Ok(())
        } else {
            Err(ParseError::new(self.offset(), message))
        }
    }

    fn next(&mut self) -> Option<Token> {
        let token = self
            .tokens
            .get(self.position)
            .map(|spanned| spanned.token.clone());
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    fn or(&mut self) -> Result<Expr, ParseError> {
        let first = self.and()?;
        let mut rest = Vec::new();
        while self.eat(&Token::Or) {
            rest.push(self.and()?);
        }
        Ok(combine(first, rest, Expr::Or))
    }

    fn and(&mut self) -> Result<Expr, ParseError> {
        let first = self.unary()?;
        let mut rest = Vec::new();
        while self.eat(&Token::And) {
            rest.push(self.unary()?);
        }
        Ok(combine(first, rest, Expr::And))
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.eat(&Token::Bang) {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let offset = self.offset();
        match self.next() {
            Some(Token::LParen) => {
                let inner = self.or()?;
                self.expect(Token::RParen, "expected ')'")?;
                Ok(inner)
            }
            Some(Token::Empty) => {
                self.expect(Token::LParen, "expected '(' after empty")?;
                let variable = self.variable()?;
                self.expect(Token::RParen, "expected ')'")?;
                Ok(Expr::Empty(variable))
            }
            Some(Token::Variable(variable)) => self.comparison(variable),
            _ => Err(ParseError::new(offset, "expected an expression")),
        }
    }

    fn variable(&mut self) -> Result<String, ParseError> {
        let offset = self.offset();
        match self.next() {
            Some(Token::Variable(variable)) => Ok(variable),
            _ => Err(ParseError::new(offset, "expected a variable")),
        }
    }

    fn comparison(&mut self, variable: String) -> Result<Expr, ParseError> {
        let Some(operator) = self.peek().map(|spanned| spanned.token.clone()) else {
            return Ok(Expr::Truthy(variable));
        };
        let expr = match operator {
            Token::Eq => {
                self.position += 1;
                Expr::Eq(variable, self.literal()?)
            }
            Token::Ne => {
                self.position += 1;
                Expr::Ne(variable, self.literal()?)
            }
            Token::Gt => {
                self.position += 1;
                Expr::Gt(variable, self.literal()?)
            }
            Token::Lt => {
                self.position += 1;
                Expr::Lt(variable, self.literal()?)
            }
            Token::In => {
                self.position += 1;
                Expr::In(variable, self.list()?)
            }
            Token::Matches => {
                self.position += 1;
                let offset = self.offset();
                match self.next() {
                    Some(Token::Str(pattern)) => Expr::Matches(variable, pattern),
                    _ => return Err(ParseError::new(offset, "expected a pattern string")),
                }
            }
            _ => Expr::Truthy(variable),
        };
        Ok(expr)
    }

    fn literal(&mut self) -> Result<Value, ParseError> {
        let offset = self.offset();
        match self.next() {
            Some(Token::Str(text)) => Ok(Value::String(text)),
            Some(Token::Num(number)) => Ok(Value::Number(number)),
            Some(Token::True) => Ok(Value::Bool(true)),
            Some(Token::False) => Ok(Value::Bool(false)),
            Some(Token::Null) => Ok(Value::Null),
            Some(Token::LBracket) => Ok(Value::Array(self.items()?)),
            _ => Err(ParseError::new(offset, "expected a literal")),
        }
    }

    fn list(&mut self) -> Result<Vec<Value>, ParseError> {
        self.expect(Token::LBracket, "expected '['")?;
        self.items()
    }

    fn items(&mut self) -> Result<Vec<Value>, ParseError> {
        let mut items = Vec::new();
        if self.eat(&Token::RBracket) {
            return Ok(items);
        }
        loop {
            items.push(self.literal()?);
            if self.eat(&Token::Comma) {
                continue;
            }
            self.expect(Token::RBracket, "expected ',' or ']'")?;
            return Ok(items);
        }
    }
}

fn combine(first: Expr, mut rest: Vec<Expr>, wrap: fn(Vec<Expr>) -> Expr) -> Expr {
    if rest.is_empty() {
        first
    } else {
        rest.insert(0, first);
        wrap(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ast(source: &str) -> Value {
        serde_json::to_value(Expr::parse(source).unwrap()).unwrap()
    }

    #[test]
    fn parses_the_examples_of_the_design() {
        assert_eq!(
            ast("kind == 'slave' && !dnssec"),
            json!({ "and": [ { "eq": ["kind", "slave"] }, { "not": { "truthy": "dnssec" } } ] })
        );
        assert_eq!(ast("$mode == 'edit'"), json!({ "eq": ["$mode", "edit"] }));
        assert_eq!(
            ast("status == 'suspended'"),
            json!({ "eq": ["status", "suspended"] })
        );
    }

    #[test]
    fn and_binds_tighter_than_or_and_parentheses_override() {
        assert_eq!(
            ast("a || b && c"),
            json!({ "or": [ { "truthy": "a" }, { "and": [ { "truthy": "b" }, { "truthy": "c" } ] } ] })
        );
        assert_eq!(
            ast("(a || b) && c"),
            json!({ "and": [ { "or": [ { "truthy": "a" }, { "truthy": "b" } ] }, { "truthy": "c" } ] })
        );
        assert_eq!(
            ast("a && b && c"),
            json!({ "and": [ { "truthy": "a" }, { "truthy": "b" }, { "truthy": "c" } ] })
        );
    }

    #[test]
    fn negation_applies_to_the_comparison_that_follows() {
        assert_eq!(ast("!a == 'x'"), json!({ "not": { "eq": ["a", "x"] } }));
        assert_eq!(ast("!!a"), json!({ "not": { "not": { "truthy": "a" } } }));
        assert_eq!(
            ast("!(a && b)"),
            json!({ "not": { "and": [ { "truthy": "a" }, { "truthy": "b" } ] } })
        );
    }

    #[test]
    fn literals_cover_strings_numbers_booleans_null_and_lists() {
        assert_eq!(ast("a == \"x\""), json!({ "eq": ["a", "x"] }));
        assert_eq!(ast("a == 'it\\'s'"), json!({ "eq": ["a", "it's"] }));
        assert_eq!(ast("a == 'wéb'"), json!({ "eq": ["a", "wéb"] }));
        assert_eq!(ast("a == -12"), json!({ "eq": ["a", -12] }));
        assert_eq!(ast("a > 1.5"), json!({ "gt": ["a", 1.5] }));
        assert_eq!(ast("a == true"), json!({ "eq": ["a", true] }));
        assert_eq!(ast("a != null"), json!({ "ne": ["a", null] }));
        assert_eq!(ast("a == []"), json!({ "eq": ["a", []] }));
        assert_eq!(
            ast("a in ['x', 2, false]"),
            json!({ "in": ["a", ["x", 2, false]] })
        );
        assert_eq!(ast("a matches '^x'"), json!({ "matches": ["a", "^x"] }));
        assert_eq!(ast("empty(a)"), json!({ "empty": "a" }));
        assert_eq!(
            ast("$screen.params.zone_id == 'z'"),
            json!({ "eq": ["$screen.params.zone_id", "z"] })
        );
        assert_eq!(
            ast("owner.login == 'a'"),
            json!({ "eq": ["owner.login", "a"] })
        );
    }

    #[test]
    fn rejects_what_the_language_does_not_have() {
        for source in [
            "",
            "status ==",
            "'x' == status",
            "status >= 1",
            "status = 'x'",
            "a &&",
            "a & b",
            "empty status",
            "(a",
            "[1] in x",
            "a == 'unterminated",
            "a == 'bad\\n'",
            "a in 'x'",
            "a matches x",
            "a == b",
            "1 == 1",
            "a.",
            "a.1",
            "$",
            "a == 99999999999999999999",
            "a b",
            "a == 'x' 'y'",
            "a in [1,]",
            "a == 'x' ==",
        ] {
            assert!(Expr::parse(source).is_err(), "{source:?} should not parse");
        }
    }

    #[test]
    fn errors_carry_the_offset_of_the_problem() {
        let error = Expr::parse("status == ").unwrap_err();
        assert_eq!(error.offset, 10);
        assert_eq!(error.to_string(), "expected a literal at offset 10");
        let error = Expr::parse("a ^ b").unwrap_err();
        assert_eq!(error.offset, 2);
        let error = Expr::parse("a == 'x\\q'").unwrap_err();
        assert_eq!(error.offset, 7);
        assert_eq!(error.message, "unknown escape");
        let error = Expr::parse("é").unwrap_err();
        assert_eq!(error.offset, 0);
    }
}
