use crate::ast::{CmpOp, Expr, Predicate};
use alefs_types::{Scalar, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "parse error: {}", self.message)
    }
}

impl std::error::Error for ParseError {}

pub fn parse_query(input: &str) -> Result<Expr, ParseError> {
    let mut p = Parser {
        tokens: tokenize(input)?,
        i: 0,
    };
    let expr = p.parse_expr()?;
    if p.peek().is_some() {
        return Err(ParseError {
            message: format!("unexpected token {:?}", p.peek()),
        });
    }
    Ok(expr)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    String(String),
    Int(i64),
    Float(f64),
    Op(String),
    LParen,
    RParen,
}

struct Parser {
    tokens: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.i)
    }

    fn bump(&mut self) -> Option<Tok> {
        if self.i < self.tokens.len() {
            let t = self.tokens[self.i].clone();
            self.i += 1;
            Some(t)
        } else {
            None
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        // term ((AND|OR) term)*  left-assoc equal precedence
        let mut left = self.parse_term()?;
        while let Some(Tok::Ident(op)) = self.peek().cloned() {
            if op != "AND" && op != "OR" {
                break;
            }
            self.bump();
            let right = self.parse_term()?;
            left = if op == "AND" {
                Expr::And(Box::new(left), Box::new(right))
            } else {
                Expr::Or(Box::new(left), Box::new(right))
            };
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        if let Some(Tok::Ident(n)) = self.peek().cloned() {
            if n == "NOT" {
                self.bump();
                let inner = self.parse_primary()?;
                return Ok(Expr::Not(Box::new(inner)));
            }
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), Some(Tok::LParen)) {
            self.bump();
            let e = self.parse_expr()?;
            match self.bump() {
                Some(Tok::RParen) => Ok(e),
                _ => Err(ParseError {
                    message: "expected )".into(),
                }),
            }
        } else {
            Ok(Expr::Pred(self.parse_predicate()?))
        }
    }

    fn parse_predicate(&mut self) -> Result<Predicate, ParseError> {
        let name = match self.bump() {
            Some(Tok::Ident(s)) => s,
            other => {
                return Err(ParseError {
                    message: format!("expected predicate, got {other:?}"),
                })
            }
        };
        match name.as_str() {
            "path" => Ok(Predicate::PathGlob(self.expect_path_or_string()?)),
            "type" => Ok(Predicate::TypeName(self.expect_ident_or_string()?)),
            "name" => Ok(Predicate::NameGlob(self.expect_string_like()?)),
            "value" => {
                let (op, s) = self.parse_cmp_scalar()?;
                Ok(Predicate::ValueCmp(op, s))
            }
            "has" => Ok(Predicate::HasKey(self.expect_string_like()?)),
            "contains" => Ok(Predicate::Contains(Value::Scalar(
                self.parse_scalar_value()?,
            ))),
            "at" => {
                let idx = self.expect_uint()?;
                let (op, s) = self.parse_cmp_scalar()?;
                Ok(Predicate::AtIndex(idx, op, s))
            }
            "key" => {
                let (op, s) = self.parse_cmp_scalar()?;
                Ok(Predicate::KeyCmp(op, s))
            }
            "size" => {
                let op = self.expect_cmp()?;
                let n = self.expect_int()?;
                Ok(Predicate::SizeCmp(op, n))
            }
            other => Err(ParseError {
                message: format!("unknown predicate {other}"),
            }),
        }
    }

    fn parse_cmp_scalar(&mut self) -> Result<(CmpOp, Scalar), ParseError> {
        let op = self.expect_cmp()?;
        let s = self.parse_scalar_value()?;
        Ok((op, s))
    }

    fn parse_scalar_value(&mut self) -> Result<Scalar, ParseError> {
        match self.bump() {
            Some(Tok::String(s)) => Ok(Scalar::String(s)),
            Some(Tok::Int(n)) => Ok(Scalar::Int(n)),
            Some(Tok::Float(f)) => Ok(Scalar::from_f64(f)),
            Some(Tok::Ident(s)) if s == "null" => Ok(Scalar::Null),
            Some(Tok::Ident(s)) if s == "true" => Ok(Scalar::Bool(true)),
            Some(Tok::Ident(s)) if s == "false" => Ok(Scalar::Bool(false)),
            Some(Tok::Ident(s)) => Ok(Scalar::String(s)),
            other => Err(ParseError {
                message: format!("expected scalar, got {other:?}"),
            }),
        }
    }

    fn expect_cmp(&mut self) -> Result<CmpOp, ParseError> {
        match self.bump() {
            Some(Tok::Op(s)) => match s.as_str() {
                "=" => Ok(CmpOp::Eq),
                "!=" => Ok(CmpOp::Ne),
                "<" => Ok(CmpOp::Lt),
                "<=" => Ok(CmpOp::Le),
                ">" => Ok(CmpOp::Gt),
                ">=" => Ok(CmpOp::Ge),
                other => Err(ParseError {
                    message: format!("bad cmp {other}"),
                }),
            },
            other => Err(ParseError {
                message: format!("expected comparator, got {other:?}"),
            }),
        }
    }

    fn expect_string_like(&mut self) -> Result<String, ParseError> {
        match self.bump() {
            Some(Tok::String(s)) | Some(Tok::Ident(s)) => Ok(s),
            other => Err(ParseError {
                message: format!("expected string, got {other:?}"),
            }),
        }
    }

    fn expect_ident_or_string(&mut self) -> Result<String, ParseError> {
        self.expect_string_like()
    }

    fn expect_path_or_string(&mut self) -> Result<String, ParseError> {
        match self.bump() {
            Some(Tok::String(s)) => Ok(s),
            Some(Tok::Ident(s)) => Ok(s),
            Some(Tok::Op(s)) if s.starts_with('/') => Ok(s),
            // path tokens may be glued - tokenizer handles /paths
            other => Err(ParseError {
                message: format!("expected path, got {other:?}"),
            }),
        }
    }

    fn expect_int(&mut self) -> Result<i64, ParseError> {
        match self.bump() {
            Some(Tok::Int(n)) => Ok(n),
            other => Err(ParseError {
                message: format!("expected int, got {other:?}"),
            }),
        }
    }

    fn expect_uint(&mut self) -> Result<u64, ParseError> {
        let n = self.expect_int()?;
        if n < 0 {
            return Err(ParseError {
                message: "index must be >= 0".into(),
            });
        }
        Ok(n as u64)
    }
}

fn tokenize(input: &str) -> Result<Vec<Tok>, ParseError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                tokens.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Tok::RParen);
                i += 1;
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        s.push(chars[i]);
                    } else {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(ParseError {
                        message: "unterminated string".into(),
                    });
                }
                i += 1;
                tokens.push(Tok::String(s));
            }
            '=' | '!' | '<' | '>' => {
                let mut op = String::new();
                op.push(c);
                i += 1;
                if i < chars.len() && chars[i] == '=' {
                    op.push('=');
                    i += 1;
                }
                tokens.push(Tok::Op(op));
            }
            '/' => {
                // path token
                let mut s = String::new();
                while i < chars.len() {
                    let ch = chars[i];
                    if ch.is_ascii_whitespace() || ch == '(' || ch == ')' {
                        break;
                    }
                    s.push(ch);
                    i += 1;
                }
                tokens.push(Tok::Ident(s));
            }
            '-' | '0'..='9' => {
                let start = i;
                if chars[i] == '-' {
                    i += 1;
                }
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                if i < chars.len() && chars[i] == '.' {
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    let text: String = chars[start..i].iter().collect();
                    let f: f64 = text.parse().map_err(|_| ParseError {
                        message: format!("bad float {text}"),
                    })?;
                    tokens.push(Tok::Float(f));
                } else {
                    let text: String = chars[start..i].iter().collect();
                    let n: i64 = text.parse().map_err(|_| ParseError {
                        message: format!("bad int {text}"),
                    })?;
                    tokens.push(Tok::Int(n));
                }
            }
            _ if c.is_ascii_alphabetic() || c == '_' || c == '*' || c == '?' => {
                let mut s = String::new();
                while i < chars.len() {
                    let ch = chars[i];
                    if ch.is_ascii_alphanumeric()
                        || ch == '_'
                        || ch == '*'
                        || ch == '?'
                        || ch == '.'
                        || ch == '-'
                    {
                        s.push(ch);
                        i += 1;
                    } else {
                        break;
                    }
                }
                tokens.push(Tok::Ident(s));
            }
            other => {
                return Err(ParseError {
                    message: format!("unexpected char {other}"),
                })
            }
        }
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let e = parse_query(r#"path /users/** AND type hash AND has "email""#).unwrap();
        assert!(matches!(e, Expr::And(_, _)));
    }

    #[test]
    fn parse_not_or() {
        let e = parse_query(r#"NOT type scalar OR name "*.tmp""#).unwrap();
        assert!(matches!(e, Expr::Or(_, _)));
    }

    #[test]
    fn parse_size() {
        let e = parse_query("size > 0").unwrap();
        match e {
            Expr::Pred(Predicate::SizeCmp(CmpOp::Gt, 0)) => {}
            other => panic!("{other:?}"),
        }
    }
}
