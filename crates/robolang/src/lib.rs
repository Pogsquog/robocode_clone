//! robolang: the sandboxed robot-AI language.
//!
//! Pipeline: source -> lexer -> parser -> AST -> bytecode compiler -> stack VM.
//! The VM can only touch the outside world through the [`host::Host`] trait,
//! which makes robot programs safe to share and run by construction.

pub mod ast;
pub mod compile;
pub mod host;
pub mod lexer;
pub mod parser;
pub mod value;
pub mod vm;

pub use compile::{CompileError, Program};
pub use host::{BlockRequest, Host, HostFn, HostOutcome};
pub use value::Value;
pub use vm::{RunOutcome, Vm};

/// Current language version. A shared robot may declare the version it was
/// written for on its first non-blank line:
/// `// robolang 1`
/// Engines reject programs that declare a newer version, so incompatible
/// bots fail loudly instead of misbehaving.
pub const LANGUAGE_VERSION: u32 = 1;

fn check_version(source: &str) -> Result<(), CompileError> {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut words = trimmed.split_whitespace();
        if words.next() == Some("//") {
            if words.next() == Some("robolang") {
                match words.next().and_then(|v| v.parse::<u32>().ok()) {
                    Some(v) if v > LANGUAGE_VERSION => {
                        return Err(CompileError {
                            msg: format!(
                                "robot requires language version {}, this engine speaks version {}",
                                v, LANGUAGE_VERSION
                            ),
                            line: 1,
                        })
                    }
                    _ => {}
                }
            }
        }
        break; // only the first non-blank line is a pragma line
    }
    Ok(())
}

/// Compile robot source code into a shareable [`Program`].
pub fn compile(source: &str) -> Result<std::rc::Rc<Program>, CompileError> {
    check_version(source)?;
    let ast = parser::parse(source).map_err(|e| CompileError {
        msg: e.msg,
        line: e.line,
    })?;
    compile::compile_program(&ast)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_current_version_pragma_and_no_pragma() {
        assert!(compile("// robolang 1\nfunc main() { await_tick(); }").is_ok());
        assert!(compile("\n\nfunc main() { }").is_ok());
    }

    #[test]
    fn rejects_newer_version_pragma() {
        let err = compile("// robolang 99\nfunc main() { }").unwrap_err();
        assert!(err.msg.contains("version 99"), "{}", err.msg);
    }
}
