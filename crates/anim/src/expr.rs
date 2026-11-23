//! A tiny expression language for numbers: `sin(time * 2) * 0.5 + @3.translate.x`.
//!
//! It only computes: there are no statements, no loops, no assignment and no access to anything
//! but the variables its caller resolves, so an expression from a hostile file can do no more
//! than return a number. Length, nesting and node count are capped.
//!
//! ```text
//! expr    := ternary
//! ternary := or ('?' ternary ':' ternary)?
//! or      := and ('||' and)*
//! and     := compare ('&&' compare)*
//! compare := sum (('<' | '<=' | '>' | '>=' | '==' | '!=') sum)?
//! sum     := product (('+' | '-') product)*
//! product := unary (('*' | '/' | '%') unary)*
//! unary   := ('-' | '!') unary | power
//! power   := atom ('^' unary)?
//! atom    := number | name | name '(' expr (',' expr)* ')' | '(' expr ')'
//! name    := [A-Za-z_@][A-Za-z0-9_.@]*
//! ```
//!
//! Comparisons and logic give 1 or 0. A result that is not finite (division by zero, the square
//! root of a negative number) is an error, never a NaN handed on.

use std::fmt;

/// Longest source text accepted.
pub const MAX_SOURCE: usize = 512;
/// Deepest nesting of brackets and operators.
pub const MAX_DEPTH: usize = 32;
/// Most nodes in one expression.
pub const MAX_NODES: usize = 512;

#[derive(Clone, Debug, PartialEq)]
pub struct ExprError(pub String);

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExprError {}

fn err<T>(m: impl Into<String>) -> Result<T, ExprError> {
    Err(ExprError(m.into()))
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Num(f64),
    Var(String),
    Neg(Box<Node>),
    Not(Box<Node>),
    Bin(u8, Box<Node>, Box<Node>),
    If(Box<Node>, Box<Node>, Box<Node>),
    Call(Func, Vec<Node>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Func {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sqrt,
    Abs,
    Floor,
    Ceil,
    Round,
    Min,
    Max,
    Clamp,
    Lerp,
    Pow,
    Exp,
    Ln,
    Sign,
    Step,
    Smoothstep,
    Deg,
    Rad,
}

/// Name, function and number of arguments.
const FUNCS: &[(&str, Func, usize)] = &[
    ("sin", Func::Sin, 1),
    ("cos", Func::Cos, 1),
    ("tan", Func::Tan, 1),
    ("asin", Func::Asin, 1),
    ("acos", Func::Acos, 1),
    ("atan", Func::Atan, 1),
    ("atan2", Func::Atan2, 2),
    ("sqrt", Func::Sqrt, 1),
    ("abs", Func::Abs, 1),
    ("floor", Func::Floor, 1),
    ("ceil", Func::Ceil, 1),
    ("round", Func::Round, 1),
    ("min", Func::Min, 2),
    ("max", Func::Max, 2),
    ("clamp", Func::Clamp, 3),
    ("lerp", Func::Lerp, 3),
    ("pow", Func::Pow, 2),
    ("exp", Func::Exp, 1),
    ("ln", Func::Ln, 1),
    ("sign", Func::Sign, 1),
    ("step", Func::Step, 2),
    ("smoothstep", Func::Smoothstep, 3),
    ("deg", Func::Deg, 1),
    ("rad", Func::Rad, 1),
];

/// The function names, for help texts.
pub fn function_names() -> Vec<&'static str> {
    FUNCS.iter().map(|f| f.0).collect()
}

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    root: Node,
    variables: Vec<String>,
    nodes: usize,
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    nodes: usize,
    variables: Vec<String>,
}

impl Parser<'_> {
    fn skip(&mut self) {
        while self.src.get(self.pos).is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.src.get(self.pos).copied()
    }
    fn eat(&mut self, token: &str) -> bool {
        self.skip();
        let t = token.as_bytes();
        if self.src.get(self.pos..self.pos + t.len()) == Some(t) {
            self.pos += t.len();
            true
        } else {
            false
        }
    }
    fn node(&mut self, n: Node) -> Result<Node, ExprError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES { err("the expression is too long") } else { Ok(n) }
    }
    fn deeper(depth: usize) -> Result<usize, ExprError> {
        if depth >= MAX_DEPTH { err("the expression is nested too deeply") } else { Ok(depth + 1) }
    }

    fn ternary(&mut self, depth: usize) -> Result<Node, ExprError> {
        let d = Self::deeper(depth)?;
        let cond = self.or(d)?;
        if !self.eat("?") {
            return Ok(cond);
        }
        let yes = self.ternary(d)?;
        if !self.eat(":") {
            return err("expected ':' after '?'");
        }
        let no = self.ternary(d)?;
        self.node(Node::If(Box::new(cond), Box::new(yes), Box::new(no)))
    }
    fn or(&mut self, d: usize) -> Result<Node, ExprError> {
        let mut left = self.and(d)?;
        while self.eat("||") {
            let right = self.and(d)?;
            left = self.node(Node::Bin(b'|', Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }
    fn and(&mut self, d: usize) -> Result<Node, ExprError> {
        let mut left = self.compare(d)?;
        while self.eat("&&") {
            let right = self.compare(d)?;
            left = self.node(Node::Bin(b'&', Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }
    fn compare(&mut self, d: usize) -> Result<Node, ExprError> {
        let left = self.sum(d)?;
        // Two-character operators first.
        for (token, op) in [("<=", b'l'), (">=", b'g'), ("==", b'='), ("!=", b'n'), ("<", b'<'), (">", b'>')] {
            if self.eat(token) {
                let right = self.sum(d)?;
                return self.node(Node::Bin(op, Box::new(left), Box::new(right)));
            }
        }
        Ok(left)
    }
    fn sum(&mut self, d: usize) -> Result<Node, ExprError> {
        let mut left = self.product(d)?;
        loop {
            let op = match self.peek() {
                Some(b'+') => b'+',
                Some(b'-') => b'-',
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.product(d)?;
            left = self.node(Node::Bin(op, Box::new(left), Box::new(right)))?;
        }
    }
    fn product(&mut self, d: usize) -> Result<Node, ExprError> {
        let mut left = self.unary(d)?;
        loop {
            let op = match self.peek() {
                Some(b'*') => b'*',
                Some(b'/') => b'/',
                Some(b'%') => b'%',
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.unary(d)?;
            left = self.node(Node::Bin(op, Box::new(left), Box::new(right)))?;
        }
    }
    fn unary(&mut self, depth: usize) -> Result<Node, ExprError> {
        let d = Self::deeper(depth)?;
        match self.peek() {
            Some(b'-') => {
                self.pos += 1;
                let inner = self.unary(d)?;
                self.node(Node::Neg(Box::new(inner)))
            }
            Some(b'!') if self.src.get(self.pos + 1) != Some(&b'=') => {
                self.pos += 1;
                let inner = self.unary(d)?;
                self.node(Node::Not(Box::new(inner)))
            }
            _ => {
                let base = self.atom(d)?;
                if self.eat("^") {
                    let exponent = self.unary(d)?;
                    return self.node(Node::Bin(b'^', Box::new(base), Box::new(exponent)));
                }
                Ok(base)
            }
        }
    }
    fn atom(&mut self, d: usize) -> Result<Node, ExprError> {
        let Some(c) = self.peek() else { return err("the expression ends too early") };
        if c == b'(' {
            self.pos += 1;
            let inner = self.ternary(d)?;
            return if self.eat(")") { Ok(inner) } else { err("expected ')'") };
        }
        if c.is_ascii_digit() || c == b'.' {
            let start = self.pos;
            while self.src.get(self.pos).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                self.pos += 1;
            }
            // An exponent: 1e-3.
            if self.src.get(self.pos).is_some_and(|c| *c == b'e' || *c == b'E') {
                let mark = self.pos;
                self.pos += 1;
                if self.src.get(self.pos).is_some_and(|c| *c == b'+' || *c == b'-') {
                    self.pos += 1;
                }
                if self.src.get(self.pos).is_some_and(u8::is_ascii_digit) {
                    while self.src.get(self.pos).is_some_and(u8::is_ascii_digit) {
                        self.pos += 1;
                    }
                } else {
                    self.pos = mark;
                }
            }
            let text = self.src.get(start..self.pos).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("");
            return match text.parse::<f64>() {
                Ok(v) if v.is_finite() => self.node(Node::Num(v)),
                _ => err(format!("{text:?} is not a number")),
            };
        }
        if c.is_ascii_alphabetic() || c == b'_' || c == b'@' {
            let start = self.pos;
            while self.src.get(self.pos).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(*c, b'_' | b'.' | b'@')) {
                self.pos += 1;
            }
            let name = self.src.get(start..self.pos).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("").to_string();
            if self.eat("(") {
                let Some((_, func, arity)) = FUNCS.iter().find(|f| f.0 == name) else { return err(format!("no function {name:?}")) };
                let mut args = Vec::new();
                if !self.eat(")") {
                    loop {
                        if args.len() >= 4 {
                            return err(format!("{name} takes {arity} values"));
                        }
                        args.push(self.ternary(d)?);
                        if self.eat(")") {
                            break;
                        }
                        if !self.eat(",") {
                            return err("expected ',' or ')'");
                        }
                    }
                }
                if args.len() != *arity {
                    return err(format!("{name} takes {arity} values, not {}", args.len()));
                }
                return self.node(Node::Call(*func, args));
            }
            return match name.as_str() {
                "pi" => self.node(Node::Num(std::f64::consts::PI)),
                "tau" => self.node(Node::Num(std::f64::consts::TAU)),
                "e" => self.node(Node::Num(std::f64::consts::E)),
                _ => {
                    if !self.variables.contains(&name) {
                        self.variables.push(name.clone());
                    }
                    self.node(Node::Var(name))
                }
            };
        }
        err(format!("unexpected {:?}", c as char))
    }
}

impl Expr {
    /// Parse `source`.
    pub fn parse(source: &str) -> Result<Expr, ExprError> {
        if source.len() > MAX_SOURCE {
            return err(format!("an expression holds at most {MAX_SOURCE} characters"));
        }
        if source.trim().is_empty() {
            return err("the expression is empty");
        }
        let mut p = Parser { src: source.as_bytes(), pos: 0, nodes: 0, variables: Vec::new() };
        let root = p.ternary(0)?;
        p.skip();
        if p.pos != p.src.len() {
            let rest = source.get(p.pos..).unwrap_or("");
            return err(format!("unexpected {:?}", rest.chars().next().unwrap_or(' ')));
        }
        Ok(Expr { root, variables: p.variables, nodes: p.nodes.max(1) })
    }

    /// The names the expression reads, each once, in order of first use.
    pub fn variables(&self) -> &[String] {
        &self.variables
    }

    /// How many nodes the expression has: the most steps one [`Expr::eval`] takes (at least 1).
    /// Callers that run many expressions add these up to cap their work.
    pub fn cost(&self) -> usize {
        self.nodes
    }

    /// Compute the value; `lookup` gives the value of a name (or `None` when there is no such
    /// name, which is an error).
    pub fn eval(&self, lookup: &dyn Fn(&str) -> Option<f64>) -> Result<f64, ExprError> {
        let v = eval(&self.root, lookup)?;
        if v.is_finite() { Ok(v) } else { err("the result is not a number") }
    }
}

fn truth(v: f64) -> bool {
    v != 0.0
}
fn flag(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

// Recursion is bounded by `MAX_DEPTH` at parse time.
fn eval(n: &Node, lookup: &dyn Fn(&str) -> Option<f64>) -> Result<f64, ExprError> {
    Ok(match n {
        Node::Num(v) => *v,
        Node::Var(name) => match lookup(name) {
            Some(v) if v.is_finite() => v,
            Some(_) => return err(format!("{name} is not a number")),
            None => return err(format!("nothing is called {name:?}")),
        },
        Node::Neg(a) => -eval(a, lookup)?,
        Node::Not(a) => flag(!truth(eval(a, lookup)?)),
        Node::If(c, a, b) => {
            if truth(eval(c, lookup)?) {
                eval(a, lookup)?
            } else {
                eval(b, lookup)?
            }
        }
        Node::Bin(op, a, b) => {
            let x = eval(a, lookup)?;
            // Logic stops early, like everywhere else.
            match op {
                b'&' if !truth(x) => return Ok(0.0),
                b'|' if truth(x) => return Ok(1.0),
                _ => {}
            }
            let y = eval(b, lookup)?;
            match op {
                b'+' => x + y,
                b'-' => x - y,
                b'*' => x * y,
                b'/' => x / y,
                b'%' => x.rem_euclid(y),
                b'^' => x.powf(y),
                b'<' => flag(x < y),
                b'>' => flag(x > y),
                b'l' => flag(x <= y),
                b'g' => flag(x >= y),
                b'=' => flag(x == y),
                b'n' => flag(x != y),
                _ => flag(truth(y)),
            }
        }
        Node::Call(f, args) => {
            let mut v = [0.0f64; 3];
            for (slot, a) in v.iter_mut().zip(args) {
                *slot = eval(a, lookup)?;
            }
            let [a, b, c] = v;
            match f {
                Func::Sin => a.sin(),
                Func::Cos => a.cos(),
                Func::Tan => a.tan(),
                Func::Asin => a.asin(),
                Func::Acos => a.acos(),
                Func::Atan => a.atan(),
                Func::Atan2 => a.atan2(b),
                Func::Sqrt => a.sqrt(),
                Func::Abs => a.abs(),
                Func::Floor => a.floor(),
                Func::Ceil => a.ceil(),
                Func::Round => a.round(),
                Func::Min => a.min(b),
                Func::Max => a.max(b),
                Func::Clamp => a.max(b.min(c)).min(c.max(b)),
                Func::Lerp => a + (b - a) * c,
                Func::Pow => a.powf(b),
                Func::Exp => a.exp(),
                Func::Ln => a.ln(),
                Func::Sign => {
                    if a > 0.0 {
                        1.0
                    } else if a < 0.0 {
                        -1.0
                    } else {
                        0.0
                    }
                }
                Func::Step => flag(b >= a),
                Func::Smoothstep => {
                    let t = if (b - a).abs() < f64::EPSILON { flag(c >= b) } else { ((c - a) / (b - a)).clamp(0.0, 1.0) };
                    t * t * (3.0 - 2.0 * t)
                }
                Func::Deg => a.to_degrees(),
                Func::Rad => a.to_radians(),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn value(src: &str) -> Result<f64, ExprError> {
        Expr::parse(src)?.eval(&|name| match name {
            "frame" => Some(12.0),
            "@3.translate.x" => Some(2.5),
            "self.rotate.z" => Some(90.0),
            _ => None,
        })
    }

    #[test]
    fn arithmetic_precedence_and_functions() {
        for (src, want) in [
            ("1 + 2 * 3", 7.0),
            ("(1 + 2) * 3", 9.0),
            ("-2 ^ 2", -4.0),
            ("2 ^ 3 ^ 2", 512.0),
            ("2 ^ -1", 0.5),
            ("10 % 4", 2.0),
            ("-1 % 4", 3.0),
            ("1e3 + .5", 1000.5),
            ("frame / 4", 3.0),
            ("@3.translate.x * 2", 5.0),
            ("sin(rad(self.rotate.z))", 1.0),
            ("clamp(5, 0, 1) + clamp(-5, 0, 1) + clamp(0.5, 1, 0)", 1.5),
            ("lerp(10, 20, 0.25)", 12.5),
            ("min(3, 4) + max(3, 4)", 7.0),
            ("frame > 10 ? 1 : 2", 1.0),
            ("frame < 10 ? 1 : frame == 12 ? 5 : 2", 5.0),
            ("1 < 2 && 2 <= 2 && 3 >= 3 && 1 != 2", 1.0),
            ("0 || !1", 0.0),
            ("0 && nothing", 0.0),
            ("smoothstep(0, 10, 5)", 0.5),
            ("step(5, frame)", 1.0),
            ("sign(-3) + abs(-3) + floor(1.7) + ceil(1.2) + round(1.5)", 7.0),
            ("atan2(1, 1) * 4 - pi", 0.0),
            ("ln(e) + exp(0) + sqrt(16) + pow(2, 3) + deg(tau)", 374.0),
        ] {
            let got = value(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            assert!((got - want).abs() < 1e-9, "{src} = {got}, not {want}");
        }
        assert_eq!(Expr::parse("@3.translate.x + frame + frame").unwrap().variables(), ["@3.translate.x", "frame"]);
        assert_eq!(Expr::parse("1 + 2 * 3").unwrap().cost(), 5, "three numbers and two operators");
        assert_eq!(Expr::parse("(((7)))").unwrap().cost(), 1, "brackets add no work");
    }

    #[test]
    fn errors_are_messages_not_panics() {
        for src in ["", "  ", "1 +", "(1", "1 2", "foo(1)", "sin()", "sin(1, 2)", "min(1)", "1 ? 2", "$", "1..2", "nothing + 1", "1 / 0", "sqrt(-1)", "ln(0)"] {
            assert!(value(src).is_err(), "{src:?} should fail");
        }
        assert!(Expr::parse(&"1+".repeat(MAX_SOURCE)).is_err(), "too long");
        assert!(Expr::parse(&format!("{}1{}", "(".repeat(100), ")".repeat(100))).is_err(), "too deep");
        assert!(Expr::parse(&format!("{}1", "-".repeat(100))).is_err(), "too deep");
        assert!(Expr::parse(&format!("1{}", "+1".repeat(250))).is_ok());
    }

    proptest! {
        #[test]
        fn any_text_parses_or_fails_cleanly(src in "[-+*/%^()<>=!?:,.@ 0-9a-z]{0,80}") {
            if let Ok(e) = Expr::parse(&src)
                && let Ok(v) = e.eval(&|_| Some(1.5))
            {
                prop_assert!(v.is_finite());
            }
        }

        #[test]
        fn sums_and_products_match_rust(a in -1e3f64..1e3, b in -1e3f64..1e3, c in 0.1f64..1e3) {
            let got = Expr::parse(&format!("{a} + {b} * {c} - {a} / {c}")).unwrap().eval(&|_| None).unwrap();
            prop_assert!((got - (a + b * c - a / c)).abs() < 1e-6);
        }
    }
}
