//! Expression evaluator for the WordStar-style Calculator dialog.
//!
//! Supports the symbols WordStar 7's calculator offers: the binary operators
//! `+ - * / % ^` and the prefix functions `sqr int log ln exp sin cos tan atn`.
//! Trigonometric functions work in radians. `%` is "percent": `a % b = a*b/100`
//! (i.e. *b* percent of *a*). Parentheses are supported for grouping.
//!
//! Grammar, in increasing precedence:
//! ```text
//! expr    = term (('+' | '-') term)*
//! term    = factor (('*' | '/' | '%') factor)*
//! factor  = '-' factor | func factor | power      // unary minus / functions
//! power   = primary ('^' factor)?                 // '^' is right-associative
//! primary = number | '(' expr ')'
//! ```

/// Evaluate `input`, returning the numeric result or a human-readable error.
pub fn eval(input: &str) -> Result<f64, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err("Enter an expression".into());
    }
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
    };
    let value = parser.expr()?;
    if parser.pos != tokens.len() {
        return Err("Unexpected symbol".into());
    }
    if value.is_nan() {
        return Err("Result is undefined".into());
    }
    if value.is_infinite() {
        return Err("Result is too large".into());
    }
    Ok(value)
}

/// Format a result the way a calculator display would: whole numbers without a
/// decimal point, others trimmed of trailing zeros.
pub fn format_result(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    let s = format!("{value:.10}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[derive(Clone, Copy, PartialEq)]
enum Func {
    Sqr,
    Int,
    Log,
    Ln,
    Exp,
    Sin,
    Cos,
    Tan,
    Atn,
}

#[derive(Clone, Copy, PartialEq)]
enum Token {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
    Func(Func),
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => i += 1,
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
            }
            '^' => {
                tokens.push(Token::Caret);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let value: f64 = text
                    .parse()
                    .map_err(|_| format!("Invalid number '{text}'"))?;
                tokens.push(Token::Num(value));
            }
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let name: String = chars[start..i].iter().collect::<String>().to_ascii_lowercase();
                let func = match name.as_str() {
                    "sqr" => Func::Sqr,
                    "int" => Func::Int,
                    "log" => Func::Log,
                    "ln" => Func::Ln,
                    "exp" => Func::Exp,
                    "sin" => Func::Sin,
                    "cos" => Func::Cos,
                    "tan" => Func::Tan,
                    "atn" => Func::Atn,
                    _ => return Err(format!("Unknown symbol '{name}'")),
                };
                tokens.push(Token::Func(func));
            }
            other => return Err(format!("Unexpected character '{other}'")),
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.pos).copied()
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut acc = self.term()?;
        while let Some(tok) = self.peek() {
            match tok {
                Token::Plus => {
                    self.pos += 1;
                    acc += self.term()?;
                }
                Token::Minus => {
                    self.pos += 1;
                    acc -= self.term()?;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut acc = self.factor()?;
        while let Some(tok) = self.peek() {
            match tok {
                Token::Star => {
                    self.pos += 1;
                    acc *= self.factor()?;
                }
                Token::Slash => {
                    self.pos += 1;
                    let rhs = self.factor()?;
                    if rhs == 0.0 {
                        return Err("Division by zero".into());
                    }
                    acc /= rhs;
                }
                Token::Percent => {
                    self.pos += 1;
                    acc = acc * self.factor()? / 100.0;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn factor(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(Token::Minus) => {
                self.pos += 1;
                Ok(-self.factor()?)
            }
            Some(Token::Func(func)) => {
                self.pos += 1;
                apply(func, self.factor()?)
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.primary()?;
        if self.peek() == Some(Token::Caret) {
            self.pos += 1;
            Ok(base.powf(self.factor()?))
        } else {
            Ok(base)
        }
    }

    fn primary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(Token::Num(n)) => {
                self.pos += 1;
                Ok(n)
            }
            Some(Token::LParen) => {
                self.pos += 1;
                let value = self.expr()?;
                if self.peek() != Some(Token::RParen) {
                    return Err("Missing ')'".into());
                }
                self.pos += 1;
                Ok(value)
            }
            _ => Err("Expected a number".into()),
        }
    }
}

fn apply(func: Func, x: f64) -> Result<f64, String> {
    Ok(match func {
        Func::Sqr => {
            if x < 0.0 {
                return Err("Cannot take the square root of a negative number".into());
            }
            x.sqrt()
        }
        Func::Int => x.trunc(),
        Func::Log => {
            if x <= 0.0 {
                return Err("Log needs a positive number".into());
            }
            x.log10()
        }
        Func::Ln => {
            if x <= 0.0 {
                return Err("Ln needs a positive number".into());
            }
            x.ln()
        }
        Func::Exp => x.exp(),
        Func::Sin => x.sin(),
        Func::Cos => x.cos(),
        Func::Tan => x.tan(),
        Func::Atn => x.atan(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(expr: &str) -> f64 {
        eval(expr).unwrap_or_else(|e| panic!("{expr:?}: {e}"))
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ok("2+3"), 5.0);
        assert_eq!(ok("2+3*4"), 14.0);
        assert_eq!(ok("(2+3)*4"), 20.0);
        assert_eq!(ok("10/4"), 2.5);
        assert_eq!(ok("100-1-2"), 97.0); // left-associative
    }

    #[test]
    fn power_is_right_associative_below_unary_minus() {
        assert_eq!(ok("2^10"), 1024.0);
        assert_eq!(ok("2^3^2"), 512.0); // 2^(3^2)
        assert_eq!(ok("-2^2"), -4.0); // -(2^2)
    }

    #[test]
    fn functions() {
        assert_eq!(ok("sqr 144"), 12.0);
        assert_eq!(ok("sqr(9+16)"), 5.0);
        assert_eq!(ok("int 3.7"), 3.0);
        assert_eq!(ok("int -3.7"), -3.0);
        assert_eq!(ok("log 1000"), 3.0);
        assert_eq!(ok("ln 1"), 0.0);
        assert_eq!(ok("exp 0"), 1.0);
        assert_eq!(ok("sin 0"), 0.0);
        assert!((ok("cos 0") - 1.0).abs() < 1e-12);
        assert!(ok("atn 1").abs() > 0.0);
    }

    #[test]
    fn percent_and_errors() {
        assert_eq!(ok("200 % 5"), 10.0); // 5% of 200
        assert!(eval("1/0").is_err());
        assert!(eval("sqr -1").is_err());
        assert!(eval("log 0").is_err());
        assert!(eval("2 +").is_err());
        assert!(eval("xyz").is_err());
    }

    #[test]
    fn formatting() {
        assert_eq!(format_result(0.0), "0");
        assert_eq!(format_result(12.0), "12");
        assert_eq!(format_result(2.5), "2.5");
        assert_eq!(format_result(-3.0), "-3");
    }
}
