//! Script errors with the file, line and column they came from.

use rhai::{Dynamic, EvalAltResult, ParseError, Position};

/// What went wrong, roughly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The text does not parse.
    Syntax,
    /// A statement failed while running (a bad argument, a command that was refused, `throw`).
    Runtime,
    /// The script ran out of operations or time; it was stopped.
    Limit,
}

/// An error with its place in the script. `Display` is `file:line:column: message`, the form the
/// console and the command line print and the editor turns into a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    pub file: String,
    /// 1-based line, when the error has a place in the text.
    pub line: Option<usize>,
    /// 1-based column.
    pub column: Option<usize>,
    pub message: String,
    pub kind: ErrorKind,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(l), Some(c)) => write!(f, "{}:{l}:{c}: {}", self.file, self.message),
            (Some(l), None) => write!(f, "{}:{l}: {}", self.file, self.message),
            _ => write!(f, "{}: {}", self.file, self.message),
        }
    }
}

impl std::error::Error for ScriptError {}

fn place(p: Position) -> (Option<usize>, Option<usize>) {
    (p.line(), p.position())
}

pub(crate) fn from_parse(file: &str, e: &ParseError) -> ScriptError {
    let (line, column) = place(e.position());
    ScriptError { file: file.to_string(), line, column, message: e.err_type().to_string(), kind: ErrorKind::Syntax }
}

/// The innermost error of a chain of function-call errors, with the call sites that led to it.
pub(crate) fn from_eval(file: &str, e: &EvalAltResult, limit_secs: f64) -> ScriptError {
    let mut calls: Vec<String> = Vec::new();
    let mut cur = e;
    while let EvalAltResult::ErrorInFunctionCall(name, _, inner, pos) = cur {
        if let Some(l) = pos.line() {
            calls.push(format!("in {name}(), called from line {l}"));
        }
        cur = inner;
    }
    let (line, column) = place(cur.position());
    let (message, kind) = match cur {
        EvalAltResult::ErrorTooManyOperations(_) => {
            ("the script ran too many operations and was stopped (is there an endless loop?)".to_string(), ErrorKind::Limit)
        }
        EvalAltResult::ErrorTerminated(token, _) => {
            (token.clone().into_string().unwrap_or_else(|_| format!("the script ran longer than {limit_secs} s and was stopped")), ErrorKind::Limit)
        }
        EvalAltResult::ErrorStackOverflow(_) => ("the script nests calls or expressions too deeply".to_string(), ErrorKind::Limit),
        EvalAltResult::ErrorDataTooLarge(what, _) => (format!("{what} is too large for a script (see the limits in the documentation)"), ErrorKind::Limit),
        EvalAltResult::ErrorRuntime(v, _) => (v.to_string(), ErrorKind::Runtime),
        other => (strip_position(&other.to_string()), ErrorKind::Runtime),
    };
    let message = if calls.is_empty() { message } else { format!("{message} ({})", calls.join("; ")) };
    ScriptError { file: file.to_string(), line, column, message, kind }
}

/// Rhai appends " (line N, position M)" to its messages; we print the place ourselves.
fn strip_position(s: &str) -> String {
    match s.rfind(" (line ") {
        Some(i) if s.ends_with(')') => s[..i].to_string(),
        _ => s.to_string(),
    }
}

/// A runtime error from a host function. The engine fills in the place of the call.
pub(crate) fn rt(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(message.into()), Position::NONE).into()
}

/// A limit was reached. Unlike a runtime error a script cannot catch this with `try`/`catch`.
pub(crate) fn limit(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorTerminated(Dynamic::from(message.into()), Position::NONE).into()
}

pub(crate) type R<T> = Result<T, Box<EvalAltResult>>;
