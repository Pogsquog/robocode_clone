//! AST for the robolang DSL.

#[derive(Clone, Debug)]
pub struct Program {
    pub funcs: Vec<FuncDecl>,
}

#[derive(Clone, Debug)]
pub struct FuncDecl {
    pub name: String,
    pub params: Vec<String>,
    pub body: Block,
    pub line: u32,
}

pub type Block = Vec<Stmt>;

#[derive(Clone, Debug)]
pub enum Stmt {
    Var {
        name: String,
        init: Option<Expr>,
        line: u32,
    },
    Assign {
        name: String,
        op: AssignOp,
        value: Expr,
        line: u32,
    },
    Expr {
        expr: Expr,
        line: u32,
    },
    If {
        cond: Expr,
        then: Block,
        els: Option<Block>,
        line: u32,
    },
    While {
        cond: Expr,
        body: Block,
        line: u32,
    },
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Vec<Stmt>,
        body: Block,
        line: u32,
    },
    Return {
        value: Option<Expr>,
        line: u32,
    },
    Break {
        line: u32,
    },
    Continue {
        line: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64),
    Bool(bool),
    Str(String),
    Ident {
        name: String,
        line: u32,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        line: u32,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        line: u32,
    },
    Logical {
        op: LogOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<Expr>,
        line: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LogOp {
    And,
    Or,
}
