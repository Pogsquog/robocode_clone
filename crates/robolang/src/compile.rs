//! Bytecode compiler: AST -> flat stack bytecode.
//!
//! Scoping rules:
//! - Every `var` declared anywhere in `main` is hoisted to a *global* and is
//!   visible from all functions (and persists across ticks). Declaring the
//!   same name twice in `main` refers to the same global.
//! - `var x;` without an initializer sets `x` to null each time it runs.
//! - `var a[N];` declares a fixed-size, zero-filled array of numbers. Arrays
//!   are static: declared only at the top level of `main`, sized by a
//!   literal, allocated once when the robot starts, and usable from every
//!   function by indexing (`a[i]`). They cannot be copied, passed or
//!   returned, so their total memory is known at compile time.
//! - `var` declared inside any other function (and `for` loop variables there)
//!   are function-locals. Parameters are locals.
//! - Names resolve innermost-scope-first, then globals, else compile error.

use crate::ast::*;
use crate::host::host_lookup;
use crate::value::Value;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Copy, Debug)]
pub enum Op {
    Const(u16),
    Pop,
    GetGlobal(u16),
    SetGlobal(u16),
    GetLocal(u16),
    SetLocal(u16),
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Not,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Jump(u16),
    JumpIfFalse(u16),
    JumpIfTrue(u16),
    Call(u16, u8),
    HostCall(u16, u8),
    Return,
    /// Duplicate the top of the stack.
    Dup,
    /// Pop an index, push that element of array N.
    GetElem(u16),
    /// Pop a value then an index; store the value in array N.
    SetElem(u16),
}

#[derive(Clone, Debug)]
pub struct FuncCode {
    pub name: String,
    pub nparams: usize,
    pub nlocals: usize,
    pub code: Vec<Op>,
}

/// A compiled robot program. Rc-shared between robots running the same source.
#[derive(Clone, Debug)]
pub struct Program {
    /// Function 0 is always `main`.
    pub funcs: Vec<FuncCode>,
    pub consts: Vec<Value>,
    pub n_globals: usize,
    /// Static arrays, indexed by the id in `GetElem`/`SetElem`.
    pub arrays: Vec<ArrayInfo>,
}

#[derive(Clone, Debug)]
pub struct ArrayInfo {
    pub name: String,
    pub len: usize,
}

#[derive(Clone, Debug)]
pub struct CompileError {
    pub msg: String,
    pub line: u32,
}

const MAX_GLOBALS: usize = 256;
const MAX_LOCALS: usize = 256;
const MAX_FUNCS: usize = 64;
const MAX_CONSTS: usize = 4096;
const MAX_CODE_LEN: usize = u16::MAX as usize;
/// Total elements across all of a program's arrays. Part of the sandbox.
pub const MAX_ARRAY_ELEMENTS: usize = 16384;

enum VarRef {
    Global(u16),
    Local(u16),
    Array(u16),
}

struct ProgramParts {
    consts: Vec<Value>,
    const_map: HashMap<String, u16>,
    func_index: HashMap<String, u16>,
    /// nparams per function index (from the AST; needed for arity checks on
    /// calls to functions compiled later).
    func_params: HashMap<u16, usize>,
    globals: HashMap<String, u16>,
    arrays: HashMap<String, u16>,
    array_info: Vec<ArrayInfo>,
}

struct FuncCompiler<'a> {
    prog: &'a mut ProgramParts,
    is_main: bool,
    scopes: Vec<HashMap<String, u16>>,
    nlocals: usize,
    code: Vec<Op>,
    loops: Vec<LoopCtx>,
}

#[derive(Default)]
struct LoopCtx {
    breaks: Vec<u16>,
    continues: Vec<u16>,
}

pub fn compile_program(ast: &crate::ast::Program) -> Result<Rc<Program>, CompileError> {
    if ast.funcs.len() > MAX_FUNCS {
        return Err(CompileError {
            msg: format!("too many functions (max {})", MAX_FUNCS),
            line: 0,
        });
    }
    if ast.funcs.is_empty() || !ast.funcs.iter().any(|f| f.name == "main") {
        return Err(CompileError {
            msg: "program must define 'func main()'".into(),
            line: 0,
        });
    }

    // Order functions: main first (index 0), then the rest in source order.
    let mut ordered: Vec<&FuncDecl> = Vec::with_capacity(ast.funcs.len());
    let mut func_index: HashMap<String, u16> = HashMap::new();
    let mut func_params: HashMap<u16, usize> = HashMap::new();

    let insert =
        |i: u16, f: &FuncDecl, func_index: &mut HashMap<String, u16>| -> Result<(), CompileError> {
            if func_index.insert(f.name.clone(), i).is_some() {
                return Err(CompileError {
                    msg: format!("duplicate function '{}'", f.name),
                    line: f.line,
                });
            }
            Ok(())
        };

    let main = ast
        .funcs
        .iter()
        .find(|f| f.name == "main")
        .ok_or(CompileError {
            msg: "program must define 'func main()'".into(),
            line: 0,
        })?;
    insert(0, main, &mut func_index)?;
    func_params.insert(0, main.params.len());
    ordered.push(main);
    let mut next = 1u16;
    for f in ast.funcs.iter() {
        if f.name == "main" {
            continue;
        }
        insert(next, f, &mut func_index)?;
        func_params.insert(next, f.params.len());
        ordered.push(f);
        next += 1;
    }

    // Arrays (top level of main), then all other `var` names declared in
    // main, become globals. They share one namespace.
    let (arrays, array_info) = hoist_arrays(&main.body)?;
    let mut globals: HashMap<String, u16> = HashMap::new();
    hoist_globals(&main.body, &arrays, &mut globals)?;

    let mut parts = ProgramParts {
        consts: Vec::new(),
        const_map: HashMap::new(),
        func_index,
        func_params,
        globals,
        arrays,
        array_info,
    };

    let mut funcs = Vec::new();
    for f in ordered {
        funcs.push(compile_func(&mut parts, f)?);
    }

    Ok(Rc::new(Program {
        funcs,
        consts: parts.consts,
        n_globals: parts.globals.len(),
        arrays: parts.array_info,
    }))
}

fn hoist_arrays(body: &[Stmt]) -> Result<(HashMap<String, u16>, Vec<ArrayInfo>), CompileError> {
    let mut ids = HashMap::new();
    let mut info: Vec<ArrayInfo> = Vec::new();
    let mut total = 0usize;
    for s in body {
        let Stmt::ArrayDecl { name, size, line } = s else {
            continue;
        };
        let err = |msg: String| CompileError { msg, line: *line };
        if ids.contains_key(name) {
            return Err(err(format!("duplicate array '{}'", name)));
        }
        if size.fract() != 0.0 || *size < 1.0 {
            return Err(err(format!(
                "array size must be a whole number of at least 1, got {}",
                Value::Num(*size)
            )));
        }
        total = total.saturating_add(*size as usize);
        if total > MAX_ARRAY_ELEMENTS {
            return Err(err(format!(
                "arrays are too large: {} elements in total (max {})",
                total, MAX_ARRAY_ELEMENTS
            )));
        }
        ids.insert(name.clone(), info.len() as u16);
        info.push(ArrayInfo {
            name: name.clone(),
            len: *size as usize,
        });
    }
    Ok((ids, info))
}

fn hoist_globals(
    body: &[Stmt],
    arrays: &HashMap<String, u16>,
    globals: &mut HashMap<String, u16>,
) -> Result<(), CompileError> {
    fn walk(
        stmts: &[Stmt],
        top: bool,
        arrays: &HashMap<String, u16>,
        globals: &mut HashMap<String, u16>,
    ) -> Result<(), CompileError> {
        for s in stmts {
            match s {
                Stmt::ArrayDecl { line, .. } if !top => {
                    return Err(CompileError {
                        msg: "arrays must be declared at the top level of main".into(),
                        line: *line,
                    });
                }
                Stmt::Var { name, line, .. } => {
                    if arrays.contains_key(name) {
                        return Err(CompileError {
                            msg: format!("'{}' is already declared as an array", name),
                            line: *line,
                        });
                    }
                    if globals.contains_key(name) {
                        continue; // redeclaration: same global
                    }
                    if globals.len() >= MAX_GLOBALS {
                        return Err(CompileError {
                            msg: format!("too many global variables (max {})", MAX_GLOBALS),
                            line: *line,
                        });
                    }
                    globals.insert(name.clone(), globals.len() as u16);
                }
                Stmt::If { then, els, .. } => {
                    walk(then, false, arrays, globals)?;
                    if let Some(e) = els {
                        walk(e, false, arrays, globals)?;
                    }
                }
                Stmt::While { body, .. } => walk(body, false, arrays, globals)?,
                Stmt::For {
                    init, step, body, ..
                } => {
                    if let Some(init) = init {
                        walk(std::slice::from_ref(init), false, arrays, globals)?;
                    }
                    walk(step, false, arrays, globals)?;
                    walk(body, false, arrays, globals)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    walk(body, true, arrays, globals)
}

fn compile_func(parts: &mut ProgramParts, decl: &FuncDecl) -> Result<FuncCode, CompileError> {
    let is_main = decl.name == "main";
    if is_main && !decl.params.is_empty() {
        return Err(CompileError {
            msg: "'main' must not take parameters".into(),
            line: decl.line,
        });
    }
    let mut fc = FuncCompiler::new(parts, is_main);
    for p in &decl.params {
        fc.declare_local(p, decl.line)?;
    }
    fc.block(&decl.body)?;
    // Implicit `return null;` at the end of every function.
    let z = fc.const_idx(Value::Null)?;
    fc.emit(Op::Const(z))?;
    fc.emit(Op::Return)?;
    Ok(FuncCode {
        name: decl.name.clone(),
        nparams: decl.params.len(),
        nlocals: fc.nlocals,
        code: fc.code,
    })
}

impl<'a> FuncCompiler<'a> {
    fn new(prog: &'a mut ProgramParts, is_main: bool) -> FuncCompiler<'a> {
        FuncCompiler {
            prog,
            is_main,
            scopes: vec![HashMap::new()],
            nlocals: 0,
            code: Vec::new(),
            loops: Vec::new(),
        }
    }

    fn emit(&mut self, op: Op) -> Result<u16, CompileError> {
        if self.code.len() >= MAX_CODE_LEN {
            return Err(CompileError {
                msg: "function body too large".into(),
                line: 0,
            });
        }
        self.code.push(op);
        Ok((self.code.len() - 1) as u16)
    }

    fn here(&self) -> u16 {
        self.code.len() as u16
    }

    fn patch(&mut self, at: u16) {
        let target = self.here();
        match &mut self.code[at as usize] {
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => *t = target,
            _ => panic!("patch target is not a jump"),
        }
    }

    fn const_idx(&mut self, v: Value) -> Result<u16, CompileError> {
        let key = match &v {
            Value::Num(n) => format!("n{}", n.to_bits()),
            Value::Str(s) => format!("s{}", s),
            Value::Bool(b) => format!("b{}", b),
            Value::Null => "z".into(),
        };
        if let Some(i) = self.prog.const_map.get(&key) {
            return Ok(*i);
        }
        if self.prog.consts.len() >= MAX_CONSTS {
            return Err(CompileError {
                msg: "too many constants".into(),
                line: 0,
            });
        }
        self.prog.consts.push(v);
        let i = (self.prog.consts.len() - 1) as u16;
        self.prog.const_map.insert(key, i);
        Ok(i)
    }

    fn declare_local(&mut self, name: &str, line: u32) -> Result<u16, CompileError> {
        // `var` in main is hoisted to a global; nothing to declare locally.
        if self.is_main {
            return Ok(0);
        }
        if self.nlocals >= MAX_LOCALS {
            return Err(CompileError {
                msg: format!("too many local variables (max {})", MAX_LOCALS),
                line,
            });
        }
        let slot = self.nlocals as u16;
        self.nlocals += 1;
        self.scopes
            .last_mut()
            .expect("scope stack")
            .insert(name.to_string(), slot);
        Ok(slot)
    }

    fn resolve(&self, name: &str) -> Option<VarRef> {
        for scope in self.scopes.iter().rev() {
            if let Some(slot) = scope.get(name) {
                return Some(VarRef::Local(*slot));
            }
        }
        if let Some(g) = self.prog.globals.get(name) {
            return Some(VarRef::Global(*g));
        }
        self.prog.arrays.get(name).map(|a| VarRef::Array(*a))
    }

    /// Resolve `name` as an array for indexing.
    fn resolve_array(&self, name: &str, line: u32) -> Result<u16, CompileError> {
        match self.resolve(name) {
            Some(VarRef::Array(a)) => Ok(a),
            Some(_) => Err(CompileError {
                msg: format!("'{}' is not an array", name),
                line,
            }),
            None => Err(CompileError {
                msg: format!("undefined array '{}'", name),
                line,
            }),
        }
    }

    fn array_misuse(name: &str, line: u32) -> CompileError {
        CompileError {
            msg: format!("'{}' is an array; use an element such as {}[i]", name, name),
            line,
        }
    }

    fn block(&mut self, stmts: &[Stmt]) -> Result<(), CompileError> {
        self.scopes.push(HashMap::new());
        let result = self.stmts(stmts);
        self.scopes.pop();
        result
    }

    fn stmts(&mut self, stmts: &[Stmt]) -> Result<(), CompileError> {
        for s in stmts {
            self.stmt(s)?;
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> Result<(), CompileError> {
        match s {
            Stmt::Var { name, init, line } => {
                if self.is_main {
                    let g = self
                        .prog
                        .globals
                        .get(name)
                        .copied()
                        .ok_or_else(|| CompileError {
                            msg: format!("internal error: global '{}' not hoisted", name),
                            line: *line,
                        })?;
                    self.var_init(init)?;
                    self.emit(Op::SetGlobal(g))?;
                } else {
                    // Compile the initializer before declaring, so
                    // `var x = x;` reads any outer `x`.
                    self.var_init(init)?;
                    let slot = self.declare_local(name, *line)?;
                    self.emit(Op::SetLocal(slot))?;
                }
            }
            Stmt::ArrayDecl { line, .. } => {
                // Allocated at startup (see hoist_arrays); no code here.
                if !self.is_main {
                    return Err(CompileError {
                        msg: "arrays can only be declared in main".into(),
                        line: *line,
                    });
                }
            }
            Stmt::Assign {
                target,
                op,
                value,
                line,
            } => {
                let binop = match op {
                    AssignOp::Set => None,
                    AssignOp::Add => Some(Op::Add),
                    AssignOp::Sub => Some(Op::Sub),
                    AssignOp::Mul => Some(Op::Mul),
                    AssignOp::Div => Some(Op::Div),
                };
                match target {
                    Target::Var(name) => {
                        let r = self.resolve(name).ok_or_else(|| CompileError {
                            msg: format!("undefined variable '{}'", name),
                            line: *line,
                        })?;
                        if binop.is_some() {
                            match r {
                                VarRef::Global(g) => self.emit(Op::GetGlobal(g))?,
                                VarRef::Local(l) => self.emit(Op::GetLocal(l))?,
                                VarRef::Array(_) => return Err(Self::array_misuse(name, *line)),
                            };
                        }
                        self.expr(value)?;
                        if let Some(o) = binop {
                            self.emit(o)?;
                        }
                        match r {
                            VarRef::Global(g) => self.emit(Op::SetGlobal(g))?,
                            VarRef::Local(l) => self.emit(Op::SetLocal(l))?,
                            VarRef::Array(_) => return Err(Self::array_misuse(name, *line)),
                        };
                    }
                    Target::Index { name, index } => {
                        let a = self.resolve_array(name, *line)?;
                        // Stack: [index] or, for compound ops, [index, old].
                        self.expr(index)?;
                        if binop.is_some() {
                            self.emit(Op::Dup)?;
                            self.emit(Op::GetElem(a))?;
                        }
                        self.expr(value)?;
                        if let Some(o) = binop {
                            self.emit(o)?;
                        }
                        self.emit(Op::SetElem(a))?;
                    }
                }
            }
            Stmt::Expr { expr, .. } => {
                self.expr(expr)?;
                self.emit(Op::Pop)?;
            }
            Stmt::If {
                cond, then, els, ..
            } => {
                self.expr(cond)?;
                let jf = self.emit(Op::JumpIfFalse(0))?;
                self.block(then)?;
                if let Some(els) = els {
                    let jend = self.emit(Op::Jump(0))?;
                    self.patch(jf);
                    self.block(els)?;
                    self.patch(jend);
                } else {
                    self.patch(jf);
                }
            }
            Stmt::While { cond, body, .. } => {
                let start = self.here();
                self.expr(cond)?;
                let jend = self.emit(Op::JumpIfFalse(0))?;
                self.loops.push(LoopCtx::default());
                self.block(body)?;
                let ctx = self.loops.pop().expect("loop ctx");
                for c in ctx.continues {
                    match &mut self.code[c as usize] {
                        Op::Jump(t) => *t = start,
                        _ => unreachable!(),
                    }
                }
                self.emit(Op::Jump(start))?;
                self.patch(jend);
                for b in ctx.breaks {
                    self.patch(b);
                }
            }
            Stmt::For {
                init,
                cond,
                step,
                body,
                ..
            } => {
                self.scopes.push(HashMap::new());
                let res = (|| -> Result<(), CompileError> {
                    if let Some(init) = init {
                        self.stmt(init)?;
                    }
                    let start = self.here();
                    let jend = if let Some(c) = cond {
                        self.expr(c)?;
                        Some(self.emit(Op::JumpIfFalse(0))?)
                    } else {
                        None
                    };
                    self.loops.push(LoopCtx::default());
                    self.block(body)?;
                    let ctx = self.loops.pop().expect("loop ctx");
                    let step_target = self.here();
                    self.stmts(step)?;
                    self.emit(Op::Jump(start))?;
                    if let Some(jend) = jend {
                        self.patch(jend);
                    }
                    for b in ctx.breaks {
                        self.patch(b);
                    }
                    for c in ctx.continues {
                        match &mut self.code[c as usize] {
                            Op::Jump(t) => *t = step_target,
                            _ => unreachable!(),
                        }
                    }
                    Ok(())
                })();
                self.scopes.pop();
                res?;
            }
            Stmt::Return { value, line } => {
                match value {
                    Some(e) => self.expr(e)?,
                    None => {
                        let z = self.const_idx(Value::Null)?;
                        self.emit(Op::Const(z))?;
                    }
                }
                self.emit(Op::Return)?;
                let _ = line;
            }
            Stmt::Break { line } => {
                if self.loops.is_empty() {
                    return Err(CompileError {
                        msg: "'break' outside of a loop".into(),
                        line: *line,
                    });
                }
                let j = self.emit(Op::Jump(0))?;
                self.loops.last_mut().expect("loop ctx").breaks.push(j);
            }
            Stmt::Continue { line } => {
                if self.loops.is_empty() {
                    return Err(CompileError {
                        msg: "'continue' outside of a loop".into(),
                        line: *line,
                    });
                }
                let j = self.emit(Op::Jump(0))?;
                self.loops.last_mut().expect("loop ctx").continues.push(j);
            }
        }
        Ok(())
    }

    /// Push a `var` initializer, or null when there is none.
    fn var_init(&mut self, init: &Option<Expr>) -> Result<(), CompileError> {
        match init {
            Some(e) => self.expr(e),
            None => {
                let z = self.const_idx(Value::Null)?;
                self.emit(Op::Const(z))?;
                Ok(())
            }
        }
    }

    fn expr(&mut self, e: &Expr) -> Result<(), CompileError> {
        match e {
            Expr::Num(n) => {
                let i = self.const_idx(Value::Num(*n))?;
                self.emit(Op::Const(i))?;
            }
            Expr::Bool(b) => {
                let i = self.const_idx(Value::Bool(*b))?;
                self.emit(Op::Const(i))?;
            }
            Expr::Str(s) => {
                let i = self.const_idx(Value::Str(s.clone()))?;
                self.emit(Op::Const(i))?;
            }
            Expr::Ident { name, line } => match self.resolve(name) {
                Some(VarRef::Global(g)) => {
                    self.emit(Op::GetGlobal(g))?;
                }
                Some(VarRef::Local(l)) => {
                    self.emit(Op::GetLocal(l))?;
                }
                Some(VarRef::Array(_)) => return Err(Self::array_misuse(name, *line)),
                None => {
                    return Err(CompileError {
                        msg: format!("undefined variable '{}'", name),
                        line: *line,
                    })
                }
            },
            Expr::Unary { op, expr, .. } => {
                self.expr(expr)?;
                match op {
                    UnOp::Neg => {
                        self.emit(Op::Neg)?;
                    }
                    UnOp::Not => {
                        self.emit(Op::Not)?;
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                self.expr(lhs)?;
                self.expr(rhs)?;
                let o = match op {
                    BinOp::Add => Op::Add,
                    BinOp::Sub => Op::Sub,
                    BinOp::Mul => Op::Mul,
                    BinOp::Div => Op::Div,
                    BinOp::Mod => Op::Mod,
                    BinOp::Eq => Op::Eq,
                    BinOp::Ne => Op::Ne,
                    BinOp::Lt => Op::Lt,
                    BinOp::Le => Op::Le,
                    BinOp::Gt => Op::Gt,
                    BinOp::Ge => Op::Ge,
                };
                self.emit(o)?;
            }
            Expr::Logical { op, lhs, rhs } => {
                self.expr(lhs)?;
                match op {
                    LogOp::And => {
                        // Result: rhs if lhs truthy, else false.
                        let jf = self.emit(Op::JumpIfFalse(0))?;
                        self.expr(rhs)?;
                        let jend = self.emit(Op::Jump(0))?;
                        self.patch(jf);
                        let f = self.const_idx(Value::Bool(false))?;
                        self.emit(Op::Const(f))?;
                        self.patch(jend);
                    }
                    LogOp::Or => {
                        // Result: rhs if lhs falsy, else true.
                        let jt = self.emit(Op::JumpIfTrue(0))?;
                        self.expr(rhs)?;
                        let jend = self.emit(Op::Jump(0))?;
                        self.patch(jt);
                        let t = self.const_idx(Value::Bool(true))?;
                        self.emit(Op::Const(t))?;
                        self.patch(jend);
                    }
                }
            }
            Expr::Index { name, index, line } => {
                let a = self.resolve_array(name, *line)?;
                self.expr(index)?;
                self.emit(Op::GetElem(a))?;
            }
            Expr::Call { name, args, line }
                if name == "len" && !self.prog.func_index.contains_key(name) =>
            {
                // Built-in, resolved at compile time: array sizes are static.
                let a = match args.as_slice() {
                    [Expr::Ident { name: arr, .. }] => self.resolve_array(arr, *line)?,
                    _ => {
                        return Err(CompileError {
                            msg: "len() takes the name of an array, e.g. len(history)".into(),
                            line: *line,
                        })
                    }
                };
                let n = self.prog.array_info[a as usize].len as f64;
                let i = self.const_idx(Value::Num(n))?;
                self.emit(Op::Const(i))?;
            }
            Expr::Call { name, args, line } => {
                if let Some(fidx) = self.prog.func_index.get(name).copied() {
                    let nparams = self.prog.func_params.get(&fidx).copied().unwrap_or(0);
                    if args.len() != nparams {
                        return Err(CompileError {
                            msg: format!(
                                "function '{}' expects {} argument(s), got {}",
                                name,
                                nparams,
                                args.len()
                            ),
                            line: *line,
                        });
                    }
                    for a in args {
                        self.expr(a)?;
                    }
                    self.emit(Op::Call(fidx, nparams as u8))?;
                } else if let Some((hf, arity)) = host_lookup(name) {
                    if args.len() != arity as usize {
                        return Err(CompileError {
                            msg: format!(
                                "'{}' expects {} argument(s), got {}",
                                name,
                                arity,
                                args.len()
                            ),
                            line: *line,
                        });
                    }
                    for a in args {
                        self.expr(a)?;
                    }
                    self.emit(Op::HostCall(hf.index(), arity))?;
                    let _ = hf;
                } else {
                    return Err(CompileError {
                        msg: format!("unknown function '{}'", name),
                        line: *line,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Convenience wrapper for tests: name of a host function by index.
#[cfg(test)]
pub fn host_fn_name(index: u16) -> Option<&'static str> {
    crate::host::HostFn::from_index(index).and_then(|f| {
        crate::host::HOST_FN_TABLE
            .iter()
            .find(|(hf, _, _)| *hf == f)
            .map(|(_, n, _)| *n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn compile(src: &str) -> Result<Rc<Program>, String> {
        compile_program(&parse(src).map_err(|e| e.msg)?).map_err(|e| e.msg)
    }

    #[test]
    fn compiles_simple() {
        let p = compile("func main() { var x = 1; while (x < 10) { x += 1; } }").unwrap();
        assert_eq!(p.n_globals, 1);
        assert_eq!(p.funcs[0].name, "main");
    }

    #[test]
    fn hoists_main_vars_to_globals() {
        let p = compile("func main() { if (true) { var a = 1; } } func f() { return a; }").unwrap();
        assert_eq!(p.n_globals, 1);
    }

    #[test]
    fn rejects_undefined_var() {
        assert!(compile("func main() { var x = y; }").is_err());
    }

    #[test]
    fn rejects_missing_main() {
        assert!(compile("func other() { return 1; }").is_err());
    }

    #[test]
    fn rejects_bad_arity() {
        assert!(compile("func main() { fire(); }").is_err());
        assert!(compile("func f(a) { return a; } func main() { f(1, 2); }").is_err());
    }

    #[test]
    fn main_may_redeclare_a_global() {
        let p = compile(
            "func main() { if (true) { var a = 1; } else { var a = 2; } \
             for (var i = 0; i < 2; i += 1) { } for (var i = 0; i < 2; i += 1) { } }",
        )
        .unwrap();
        assert_eq!(p.n_globals, 2);
    }

    #[test]
    fn var_in_for_step_in_main_is_hoisted() {
        compile("func main() { for (var i = 0; i < 2; var j = 1) { i += 1; } }").unwrap();
    }

    #[test]
    fn compiles_arrays() {
        let p = compile(
            "func main() { var a[4]; var b[10]; var n = len(a) + len(b); a[1] = 2; b[a[1]] += 3; } \
             func f(i) { return a[i]; }",
        )
        .unwrap();
        assert_eq!(p.arrays.len(), 2);
        assert_eq!((p.arrays[1].name.as_str(), p.arrays[1].len), ("b", 10));
        assert_eq!(p.n_globals, 1, "arrays are not scalar globals");
    }

    #[test]
    fn rejects_array_misuse() {
        let cases = [
            (
                "func main() { if (true) { var a[4]; } }",
                "top level of main",
            ),
            (
                "func main() { while (true) { var a[4]; } }",
                "top level of main",
            ),
            (
                "func f() { var a[4]; } func main() { }",
                "only be declared in main",
            ),
            ("func main() { var a[4]; var a[4]; }", "duplicate array"),
            (
                "func main() { var a[4]; var a = 1; }",
                "already declared as an array",
            ),
            ("func main() { var a[0]; }", "at least 1"),
            ("func main() { var a[2.5]; }", "whole number"),
            ("func main() { var a[16384]; var b[1]; }", "too large"),
            ("func main() { var a[4]; var x = a; }", "is an array"),
            ("func main() { var a[4]; a = 1; }", "is an array"),
            ("func main() { var a[4]; a += 1; }", "is an array"),
            ("func main() { var a[4]; log(a); }", "is an array"),
            ("func main() { var x = 1; x[0] = 1; }", "not an array"),
            ("func main() { var x = y[0]; }", "undefined array"),
            (
                "func main() { var a[4]; var n = len(a + 1); }",
                "len() takes",
            ),
            ("func main() { var n = len(z); }", "undefined array"),
            (
                "func f(a) { return a[0]; } func main() { var a[4]; }",
                "not an array",
            ),
        ];
        for (src, want) in cases {
            let err = compile(src).unwrap_err();
            assert!(
                err.contains(want),
                "{}: got '{}', want '{}'",
                src,
                err,
                want
            );
        }
        // The largest allowed total is fine.
        compile(&format!("func main() {{ var a[{}]; }}", MAX_ARRAY_ELEMENTS)).unwrap();
    }

    #[test]
    fn rejects_break_outside_loop() {
        assert!(compile("func main() { break; }").is_err());
    }
}
