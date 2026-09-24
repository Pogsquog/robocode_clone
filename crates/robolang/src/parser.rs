//! Recursive-descent parser for the robolang DSL.

use crate::ast::*;
use crate::lexer::{lex, SyntaxError, Tok, Token};

/// Maximum syntactic nesting (blocks, parentheses, unary operators, and
/// operator chains). Part of the sandbox: the parser, compiler and AST drop
/// all recurse over this depth, so it must stay well within a thread stack.
pub const MAX_NESTING: usize = 100;

pub fn parse(source: &str) -> Result<Program, SyntaxError> {
    let tokens = lex(source)?;
    let mut p = Parser {
        tokens,
        pos: 0,
        depth: 0,
    };
    p.program()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Current nesting depth; an upper bound on the depth of the AST being
    /// built.
    depth: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn line(&self) -> u32 {
        self.tokens[self.pos].line
    }

    fn bump(&mut self) -> Tok {
        let t = self.tokens[self.pos].tok.clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, want: &Tok, what: &str) -> Result<Tok, SyntaxError> {
        if self.peek() == want {
            Ok(self.bump())
        } else {
            Err(self.err(format!("expected {}, found {}", what, self.describe())))
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<String, SyntaxError> {
        match self.peek() {
            Tok::Ident(s) => {
                let s = s.clone();
                self.bump();
                Ok(s)
            }
            _ => Err(self.err(format!("expected {}, found {}", what, self.describe()))),
        }
    }

    fn describe(&self) -> String {
        match self.peek() {
            Tok::Eof => "end of file".to_string(),
            Tok::Num(n) => format!("number {}", n),
            Tok::Str(s) => format!("string \"{}\"", s),
            Tok::Ident(s) => format!("identifier '{}'", s),
            other => format!("{:?}", other),
        }
    }

    fn enter(&mut self) -> Result<(), SyntaxError> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            return Err(self.err(format!(
                "code is nested too deeply (max {} levels)",
                MAX_NESTING
            )));
        }
        Ok(())
    }

    fn err(&self, msg: String) -> SyntaxError {
        SyntaxError {
            msg,
            line: self.line(),
        }
    }

    fn program(&mut self) -> Result<Program, SyntaxError> {
        let mut funcs = Vec::new();
        while !matches!(self.peek(), Tok::Eof) {
            funcs.push(self.func_decl()?);
        }
        Ok(Program { funcs })
    }

    fn func_decl(&mut self) -> Result<FuncDecl, SyntaxError> {
        let line = self.line();
        self.expect(&Tok::Func, "'func'")?;
        let name = self.expect_ident("function name")?;
        self.expect(&Tok::LParen, "'('")?;
        let mut params = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                params.push(self.expect_ident("parameter name")?);
                if !matches!(self.peek(), Tok::Comma) {
                    break;
                }
                self.bump();
            }
        }
        self.expect(&Tok::RParen, "')'")?;
        let body = self.block()?;
        Ok(FuncDecl {
            name,
            params,
            body,
            line,
        })
    }

    fn block(&mut self) -> Result<Block, SyntaxError> {
        self.expect(&Tok::LBrace, "'{'")?;
        self.enter()?;
        let mut stmts = Vec::new();
        while !matches!(self.peek(), Tok::RBrace) {
            if matches!(self.peek(), Tok::Eof) {
                return Err(self.err("unterminated block, expected '}'".into()));
            }
            stmts.push(self.statement()?);
        }
        self.bump(); // '}'
        self.depth -= 1;
        Ok(stmts)
    }

    fn statement(&mut self) -> Result<Stmt, SyntaxError> {
        let line = self.line();
        match self.peek() {
            Tok::Var => {
                self.bump();
                let name = self.expect_ident("variable name")?;
                let init = if matches!(self.peek(), Tok::Assign) {
                    self.bump();
                    Some(self.expression()?)
                } else {
                    None
                };
                self.expect(&Tok::Semi, "';'")?;
                Ok(Stmt::Var { name, init, line })
            }
            Tok::If => {
                self.bump();
                self.expect(&Tok::LParen, "'('")?;
                let cond = self.expression()?;
                self.expect(&Tok::RParen, "')'")?;
                let then = self.block()?;
                let els = if matches!(self.peek(), Tok::Else) {
                    self.bump();
                    Some(self.block()?)
                } else {
                    None
                };
                Ok(Stmt::If {
                    cond,
                    then,
                    els,
                    line,
                })
            }
            Tok::While => {
                self.bump();
                self.expect(&Tok::LParen, "'('")?;
                let cond = self.expression()?;
                self.expect(&Tok::RParen, "')'")?;
                let body = self.block()?;
                Ok(Stmt::While { cond, body, line })
            }
            Tok::For => {
                self.bump();
                self.expect(&Tok::LParen, "'('")?;
                let init = if matches!(self.peek(), Tok::Semi) {
                    None
                } else {
                    Some(Box::new(self.simple_statement()?))
                };
                self.expect(&Tok::Semi, "';'")?;
                let cond = if matches!(self.peek(), Tok::Semi) {
                    None
                } else {
                    Some(self.expression()?)
                };
                self.expect(&Tok::Semi, "';'")?;
                let mut step = Vec::new();
                if !matches!(self.peek(), Tok::RParen) {
                    loop {
                        step.push(self.simple_statement()?);
                        if !matches!(self.peek(), Tok::Comma) {
                            break;
                        }
                        self.bump();
                    }
                }
                self.expect(&Tok::RParen, "')'")?;
                let body = self.block()?;
                Ok(Stmt::For {
                    init,
                    cond,
                    step,
                    body,
                    line,
                })
            }
            Tok::Return => {
                self.bump();
                let value = if matches!(self.peek(), Tok::Semi) {
                    None
                } else {
                    Some(self.expression()?)
                };
                self.expect(&Tok::Semi, "';'")?;
                Ok(Stmt::Return { value, line })
            }
            Tok::Break => {
                self.bump();
                self.expect(&Tok::Semi, "';'")?;
                Ok(Stmt::Break { line })
            }
            Tok::Continue => {
                self.bump();
                self.expect(&Tok::Semi, "';'")?;
                Ok(Stmt::Continue { line })
            }
            _ => {
                let st = self.simple_statement()?;
                self.expect(&Tok::Semi, "';'")?;
                Ok(st)
            }
        }
    }

    /// Assignment, `var` declaration, or expression statement, without the
    /// trailing semicolon (used by `for` clauses too).
    fn simple_statement(&mut self) -> Result<Stmt, SyntaxError> {
        let line = self.line();
        if matches!(self.peek(), Tok::Var) {
            self.bump();
            let name = self.expect_ident("variable name")?;
            let init = if matches!(self.peek(), Tok::Assign) {
                self.bump();
                Some(self.expression()?)
            } else {
                None
            };
            return Ok(Stmt::Var { name, init, line });
        }
        // Look ahead for `ident <assign-op>`.
        let is_assign = matches!(self.peek(), Tok::Ident(_))
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.tok),
                Some(Tok::Assign)
                    | Some(Tok::PlusAssign)
                    | Some(Tok::MinusAssign)
                    | Some(Tok::StarAssign)
                    | Some(Tok::SlashAssign)
            );
        if is_assign {
            let name = self.expect_ident("variable name")?;
            let op = match self.bump() {
                Tok::Assign => AssignOp::Set,
                Tok::PlusAssign => AssignOp::Add,
                Tok::MinusAssign => AssignOp::Sub,
                Tok::StarAssign => AssignOp::Mul,
                Tok::SlashAssign => AssignOp::Div,
                _ => unreachable!(),
            };
            let value = self.expression()?;
            Ok(Stmt::Assign {
                name,
                op,
                value,
                line,
            })
        } else {
            Ok(Stmt::Expr {
                expr: self.expression()?,
                line,
            })
        }
    }

    fn expression(&mut self) -> Result<Expr, SyntaxError> {
        self.enter()?;
        let e = self.logical_or()?;
        self.depth -= 1;
        Ok(e)
    }

    fn logical_or(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.logical_and()?;
        let base = self.depth;
        while matches!(self.peek(), Tok::OrOr) {
            self.bump();
            self.enter()?;
            let rhs = self.logical_and()?;
            lhs = Expr::Logical {
                op: LogOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        self.depth = base;
        Ok(lhs)
    }

    fn logical_and(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.equality()?;
        let base = self.depth;
        while matches!(self.peek(), Tok::AndAnd) {
            self.bump();
            self.enter()?;
            let rhs = self.equality()?;
            lhs = Expr::Logical {
                op: LogOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        self.depth = base;
        Ok(lhs)
    }

    fn equality(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.comparison()?;
        let base = self.depth;
        loop {
            let op = match self.peek() {
                Tok::Eq => BinOp::Eq,
                Tok::Ne => BinOp::Ne,
                _ => {
                    self.depth = base;
                    return Ok(lhs);
                }
            };
            let line = self.line();
            self.bump();
            self.enter()?;
            let rhs = self.comparison()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
    }

    fn comparison(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.term()?;
        let base = self.depth;
        loop {
            let op = match self.peek() {
                Tok::Lt => BinOp::Lt,
                Tok::Le => BinOp::Le,
                Tok::Gt => BinOp::Gt,
                Tok::Ge => BinOp::Ge,
                _ => {
                    self.depth = base;
                    return Ok(lhs);
                }
            };
            let line = self.line();
            self.bump();
            self.enter()?;
            let rhs = self.term()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
    }

    fn term(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.factor()?;
        let base = self.depth;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => {
                    self.depth = base;
                    return Ok(lhs);
                }
            };
            let line = self.line();
            self.bump();
            self.enter()?;
            let rhs = self.factor()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
    }

    fn factor(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.unary()?;
        let base = self.depth;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => BinOp::Mod,
                _ => {
                    self.depth = base;
                    return Ok(lhs);
                }
            };
            let line = self.line();
            self.bump();
            self.enter()?;
            let rhs = self.unary()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
    }

    fn unary(&mut self) -> Result<Expr, SyntaxError> {
        let line = self.line();
        match self.peek() {
            Tok::Minus => {
                self.bump();
                self.enter()?;
                let expr = self.unary()?;
                self.depth -= 1;
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(expr),
                    line,
                })
            }
            Tok::Not => {
                self.bump();
                self.enter()?;
                let expr = self.unary()?;
                self.depth -= 1;
                Ok(Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(expr),
                    line,
                })
            }
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> Result<Expr, SyntaxError> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Num(n) => {
                self.bump();
                Ok(Expr::Num(n))
            }
            Tok::True => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            Tok::False => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::LParen => {
                self.bump();
                let e = self.expression()?;
                self.expect(&Tok::RParen, "')'")?;
                Ok(e)
            }
            Tok::Ident(name) => {
                self.bump();
                if matches!(self.peek(), Tok::LParen) {
                    self.bump();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Tok::RParen) {
                        loop {
                            args.push(self.expression()?);
                            if !matches!(self.peek(), Tok::Comma) {
                                break;
                            }
                            self.bump();
                        }
                    }
                    self.expect(&Tok::RParen, "')'")?;
                    Ok(Expr::Call { name, args, line })
                } else {
                    Ok(Expr::Ident { name, line })
                }
            }
            _ => Err(self.err(format!("expected expression, found {}", self.describe()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_program() {
        let prog = parse("func main() { var i = 0; while (i < 3) { i += 1; } log(i); }").unwrap();
        assert_eq!(prog.funcs.len(), 1);
        assert_eq!(prog.funcs[0].name, "main");
    }

    #[test]
    fn parses_for_and_calls() {
        parse("func main() { for (var i = 0; i < 10; i += 1) { fire(1.5); } }").unwrap();
    }

    #[test]
    fn rejects_missing_semi() {
        assert!(parse("func main() { var x = 1 }").is_err());
    }

    #[test]
    fn func_decl_line_is_where_it_starts() {
        let prog = parse("\n\nfunc main() {\n\n}\n\nfunc f() { }").unwrap();
        assert_eq!(prog.funcs[0].line, 3);
        assert_eq!(prog.funcs[1].line, 7);
    }

    fn wrap(depth: usize, open: &str, inner: &str, close: &str) -> String {
        format!("{}{}{}", open.repeat(depth), inner, close.repeat(depth))
    }

    #[test]
    fn deep_nesting_is_rejected_not_a_stack_overflow() {
        let n = 200_000;
        let cases = [
            format!("func main() {{ var x = {}; }}", wrap(n, "(", "1", ")")),
            format!("func main() {{ var x = {}1; }}", "-".repeat(n)),
            format!("func main() {{ var x = {}; }}", wrap(n, "!", "1", "")),
            format!("func main() {{ var x = 1{}; }}", " + 1".repeat(n)),
            format!("func main() {{ var x = 1{}; }}", " && 1".repeat(n)),
            format!("func main() {{ {} }}", wrap(n, "if (1) { ", "", "}")),
        ];
        for src in &cases {
            let err = parse(src).unwrap_err();
            assert!(err.msg.contains("nested too deeply"), "{}", err.msg);
        }
    }

    #[test]
    fn nesting_just_under_the_limit_parses_and_compiles() {
        // Runs on a test thread (2 MB stack), so this also checks the limit
        // is comfortably safe for the parser and compiler.
        let src = format!(
            "func main() {{ var x = {}; }}",
            wrap(MAX_NESTING - 3, "(", "1", ")")
        );
        let prog = parse(&src).unwrap();
        crate::compile::compile_program(&prog).unwrap();
        let src = format!(
            "func main() {{ var x = 1{}; }}",
            " + 1".repeat(MAX_NESTING - 3)
        );
        crate::compile::compile_program(&parse(&src).unwrap()).unwrap();
    }

    #[test]
    fn rejects_bad_assign_target() {
        assert!(parse("func main() { 1 = 2; }").is_err());
    }
}
