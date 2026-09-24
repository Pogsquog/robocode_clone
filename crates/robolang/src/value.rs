//! Runtime values of the robolang DSL.

use std::fmt;

/// Longest string a program can create, in bytes. Part of the sandbox:
/// together with the stack, local and global caps it bounds the memory
/// strings can use, so concatenation cannot exhaust host memory.
pub const MAX_STRING_LEN: usize = 256;

/// A robolang value. The language is small on purpose: numbers, booleans and
/// strings. `Null` is the result of void operations (e.g. `turn_body(30)`).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Num(f64),
    Bool(bool),
    Str(String),
}

impl Value {
    /// Permissive truthiness used by `if`/`while`/`&&`/`||`:
    /// null, 0, false and the empty string are falsy, everything else truthy.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Num(n) => *n != 0.0,
            Value::Bool(b) => *b,
            Value::Str(s) => !s.is_empty(),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Num(_) => "number",
            Value::Bool(_) => "bool",
            Value::Str(_) => "string",
        }
    }

    /// Human-oriented rendering (used by `log` and string concatenation).
    pub fn to_display(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    let s = format!("{:.6}", n);
                    s.trim_end_matches('0').trim_end_matches('.').to_string()
                }
            }
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => s.clone(),
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_display())
    }
}
