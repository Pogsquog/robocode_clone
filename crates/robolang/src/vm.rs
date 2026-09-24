//! The robolang stack VM.
//!
//! Executes bytecode with hard resource limits (instruction budget per run,
//! value-stack cap, call-depth cap) and supports *suspension*: when robot
//! code performs a blocking operation (`ahead`, `turn_gun`, `await_tick`,
//! ...) the VM freezes at that instruction and the engine resumes it later,
//! pushing a result value. All cross-tick robot state lives in the VM's
//! globals and stack frames, which are simply preserved.

use crate::compile::{Op, Program};
use crate::host::{BlockRequest, Host, HostOutcome};
use crate::value::{Value, MAX_STRING_LEN};
use std::rc::Rc;

/// Hard limits; part of the sandbox.
pub const MAX_FRAMES: usize = 96;
pub const MAX_STACK: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
pub enum RunOutcome {
    /// `main` returned; the program is finished. The robot will idle for the
    /// rest of the battle (its current intents persist).
    Halted,
    /// A blocking operation was reached; the VM is paused at that instruction.
    /// The engine must call `resume()` once the request completes.
    Blocked(BlockRequest),
    /// The instruction budget for this run was exhausted. The VM is paused
    /// mid-execution; call `run()` again to continue.
    BudgetExceeded,
    /// A runtime fault (bad types, division by zero, host error). The
    /// offending robot forfeits the battle.
    Fault(String),
}

struct Frame {
    caller_func: usize,
    ret_ip: usize,
    base: usize,
}

pub struct Vm {
    prog: Rc<Program>,
    globals: Vec<Value>,
    /// Static arrays: sized by the program, zero-filled at start.
    arrays: Vec<Vec<f64>>,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    ip: usize,
    cur_func: usize,
    resume: Option<Value>,
    /// Paused on a blocking host call; `resume()` must be called before the
    /// next `run()`.
    blocked: bool,
    halted: bool,
    /// Instructions executed over the VM's lifetime (see `ops_executed`).
    ops_total: u64,
}

impl Vm {
    pub fn new(prog: Rc<Program>) -> Vm {
        let n_globals = prog.n_globals;
        let arrays = prog.arrays.iter().map(|a| vec![0.0; a.len]).collect();
        let mut vm = Vm {
            prog,
            globals: vec![Value::Null; n_globals],
            arrays,
            stack: Vec::new(),
            frames: Vec::new(),
            ip: 0,
            cur_func: 0,
            resume: None,
            blocked: false,
            halted: false,
            ops_total: 0,
        };
        vm.enter_func(0);
        vm
    }

    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// Total instructions executed so far. Lets a host charge several `run`
    /// calls within one tick against a single budget.
    pub fn ops_executed(&self) -> u64 {
        self.ops_total
    }

    /// Deliver the result of a completed blocking operation. The next `run`
    /// pushes this value and continues past the blocking instruction.
    pub fn resume(&mut self, result: Value) {
        self.resume = Some(result);
    }

    fn enter_func(&mut self, func: usize) {
        let fc = &self.prog.funcs[func];
        let nlocals = fc.nlocals;
        let base = self.stack.len().saturating_sub(fc.nparams);
        // Ensure all local slots (args + declared vars) exist.
        while self.stack.len() < base + nlocals {
            self.stack.push(Value::Null);
        }
        self.frames.push(Frame {
            caller_func: self.cur_func,
            ret_ip: self.ip,
            base,
        });
        self.cur_func = func;
        self.ip = 0;
    }

    fn push(&mut self, v: Value) -> Result<(), String> {
        if self.stack.len() >= MAX_STACK {
            return Err("value stack overflow (too much recursion or nesting)".into());
        }
        self.stack.push(v);
        Ok(())
    }

    fn pop(&mut self) -> Value {
        self.stack.pop().expect("value stack underflow")
    }

    pub fn run(&mut self, host: &mut dyn Host, budget: u32) -> RunOutcome {
        if self.blocked {
            let Some(v) = self.resume.take() else {
                return RunOutcome::Fault(
                    "internal error: run() while blocked without resume()".into(),
                );
            };
            self.blocked = false;
            // The blocking call's result becomes its return value.
            if self.push(v).is_err() {
                return RunOutcome::Fault("value stack overflow on resume".into());
            }
        }
        let mut executed: u32 = 0;
        loop {
            if self.halted {
                return RunOutcome::Halted;
            }
            if executed >= budget {
                return RunOutcome::BudgetExceeded;
            }
            executed += 1;
            self.ops_total += 1;
            let op = self.prog.funcs[self.cur_func].code[self.ip];
            self.ip += 1;
            match self.exec_op(op, host) {
                Ok(None) => {}
                Ok(Some(outcome)) => return outcome,
                Err(msg) => return RunOutcome::Fault(msg),
            }
        }
    }

    /// Ok(Some(..)) means "stop this run now" (blocked/budget is handled
    /// outside, halt). Ok(None) means "keep going".
    fn exec_op(&mut self, op: Op, host: &mut dyn Host) -> Result<Option<RunOutcome>, String> {
        match op {
            Op::Const(i) => {
                let v = self.prog.consts[i as usize].clone();
                self.push(v)?;
            }
            Op::Pop => {
                self.pop();
            }
            Op::GetGlobal(i) => {
                let v = self.globals[i as usize].clone();
                self.push(v)?;
            }
            Op::SetGlobal(i) => {
                let v = self.pop();
                self.globals[i as usize] = v;
            }
            Op::GetLocal(i) => {
                let base = self.frames.last().expect("frame").base;
                let v = self.stack[base + i as usize].clone();
                self.push(v)?;
            }
            Op::SetLocal(i) => {
                let base = self.frames.last().expect("frame").base;
                let v = self.pop();
                self.stack[base + i as usize] = v;
            }
            Op::Add => {
                let b = self.pop();
                let a = self.pop();
                let v = match (&a, &b) {
                    (Value::Num(x), Value::Num(y)) => Value::Num(x + y),
                    (Value::Str(_), _) | (_, Value::Str(_)) => {
                        let (x, y) = (a.to_display(), b.to_display());
                        if x.len() + y.len() > MAX_STRING_LEN {
                            return Err(format!(
                                "string too long ({} bytes; the limit is {})",
                                x.len() + y.len(),
                                MAX_STRING_LEN
                            ));
                        }
                        Value::Str(x + &y)
                    }
                    _ => {
                        return Err(format!(
                            "cannot add {} and {}",
                            a.type_name(),
                            b.type_name()
                        ))
                    }
                };
                self.push(v)?;
            }
            Op::Sub => {
                let b = self.pop();
                let a = self.pop();
                let (x, y) = two_nums(&a, &b, "subtract")?;
                self.push(Value::Num(x - y))?;
            }
            Op::Mul => {
                let b = self.pop();
                let a = self.pop();
                let (x, y) = two_nums(&a, &b, "multiply")?;
                self.push(Value::Num(x * y))?;
            }
            Op::Div => {
                let b = self.pop();
                let a = self.pop();
                let (x, y) = two_nums(&a, &b, "divide")?;
                if y == 0.0 {
                    return Err("division by zero".into());
                }
                self.push(Value::Num(x / y))?;
            }
            Op::Mod => {
                let b = self.pop();
                let a = self.pop();
                let (x, y) = two_nums(&a, &b, "take modulo with")?;
                if y == 0.0 {
                    return Err("modulo by zero".into());
                }
                self.push(Value::Num(x % y))?;
            }
            Op::Neg => {
                let a = self.pop();
                let n = a
                    .as_num()
                    .ok_or_else(|| format!("cannot negate a {}", a.type_name()))?;
                self.push(Value::Num(-n))?;
            }
            Op::Not => {
                let a = self.pop();
                self.push(Value::Bool(!a.truthy()))?;
            }
            Op::Eq | Op::Ne => {
                let b = self.pop();
                let a = self.pop();
                let eq = values_equal(&a, &b);
                self.push(Value::Bool(if matches!(op, Op::Eq) { eq } else { !eq }))?;
            }
            Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let b = self.pop();
                let a = self.pop();
                let r = compare(&a, &b, op)?;
                self.push(Value::Bool(r))?;
            }
            Op::Jump(t) => {
                self.ip = t as usize;
            }
            Op::JumpIfFalse(t) => {
                let v = self.pop();
                if !v.truthy() {
                    self.ip = t as usize;
                }
            }
            Op::JumpIfTrue(t) => {
                let v = self.pop();
                if v.truthy() {
                    self.ip = t as usize;
                }
            }
            Op::Call(f, _) => {
                if self.frames.len() >= MAX_FRAMES {
                    return Err("call stack overflow (recursion too deep)".into());
                }
                self.enter_func(f as usize);
            }
            Op::HostCall(hf_index, nargs) => {
                let hf = crate::host::HostFn::from_index(hf_index)
                    .ok_or("invalid host function index")?;
                let mut args = Vec::with_capacity(nargs as usize);
                for _ in 0..nargs {
                    args.push(self.pop());
                }
                args.reverse();
                match host.call(hf, args) {
                    Ok(HostOutcome::Value(v)) => self.push(v)?,
                    Ok(HostOutcome::Block(req)) => {
                        // ip already points past this call; resume() supplies
                        // its result.
                        self.blocked = true;
                        return Ok(Some(RunOutcome::Blocked(req)));
                    }
                    Err(msg) => return Err(msg),
                }
            }
            Op::Dup => {
                let v = self.stack.last().cloned().expect("value stack underflow");
                self.push(v)?;
            }
            Op::GetElem(a) => {
                let idx = self.pop();
                let i = self.elem_index(a, &idx)?;
                self.push(Value::Num(self.arrays[a as usize][i]))?;
            }
            Op::SetElem(a) => {
                let v = self.pop();
                let idx = self.pop();
                let i = self.elem_index(a, &idx)?;
                let Value::Num(n) = v else {
                    return Err(format!(
                        "array '{}' holds numbers; cannot store a {}",
                        self.prog.arrays[a as usize].name,
                        v.type_name()
                    ));
                };
                self.arrays[a as usize][i] = n;
            }
            Op::Return => {
                let v = self.pop();
                if self.frames.len() == 1 {
                    self.halted = true;
                    return Ok(Some(RunOutcome::Halted));
                }
                let f = self.frames.pop().expect("frame");
                self.stack.truncate(f.base);
                self.push(v)?;
                self.cur_func = f.caller_func;
                self.ip = f.ret_ip;
            }
        }
        Ok(None)
    }
}

impl Vm {
    /// Validate an array index: a whole number within bounds.
    fn elem_index(&self, a: u16, idx: &Value) -> Result<usize, String> {
        let info = &self.prog.arrays[a as usize];
        match idx {
            Value::Num(n) if n.fract() == 0.0 && *n >= 0.0 && *n < info.len as f64 => {
                Ok(*n as usize)
            }
            Value::Num(n) => Err(format!(
                "index {} is out of range for '{}' (valid: whole numbers 0 to {})",
                Value::Num(*n),
                info.name,
                info.len - 1
            )),
            other => Err(format!(
                "array index must be a number, not a {} (indexing '{}')",
                other.type_name(),
                info.name
            )),
        }
    }
}

fn two_nums(a: &Value, b: &Value, what: &str) -> Result<(f64, f64), String> {
    match (a.as_num(), b.as_num()) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!(
            "cannot {} {} and {}",
            what,
            a.type_name(),
            b.type_name()
        )),
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Num(x), Value::Num(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        _ => false,
    }
}

fn compare(a: &Value, b: &Value, op: Op) -> Result<bool, String> {
    let o = match (a, b) {
        // Any ordering against NaN is false, as in IEEE 754.
        (Value::Num(x), Value::Num(y)) => match x.partial_cmp(y) {
            Some(o) => o,
            None => return Ok(false),
        },
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        _ => {
            return Err(format!(
                "cannot compare {} with {}",
                a.type_name(),
                b.type_name()
            ))
        }
    };
    Ok(match op {
        Op::Lt => o.is_lt(),
        Op::Le => o.is_le(),
        Op::Gt => o.is_gt(),
        Op::Ge => o.is_ge(),
        _ => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile_program;
    use crate::parser::parse;
    use std::cell::RefCell;

    /// Mock host: records calls, can block once on a chosen function.
    struct MockHost {
        calls: RefCell<Vec<(String, Vec<Value>)>>,
        block_on: Option<&'static str>,
        blocked: RefCell<bool>,
    }

    impl MockHost {
        fn new() -> MockHost {
            MockHost {
                calls: RefCell::new(Vec::new()),
                block_on: None,
                blocked: RefCell::new(false),
            }
        }
    }

    impl Host for MockHost {
        fn call(
            &mut self,
            f: crate::host::HostFn,
            args: Vec<Value>,
        ) -> Result<HostOutcome, String> {
            let name = crate::host::HOST_FN_TABLE
                .iter()
                .find(|(hf, _, _)| *hf == f)
                .map(|(_, n, _)| *n)
                .unwrap_or("?");
            self.calls
                .borrow_mut()
                .push((name.to_string(), args.clone()));
            if self.block_on == Some(name) && !*self.blocked.borrow() {
                *self.blocked.borrow_mut() = true;
                return Ok(HostOutcome::Block(match name {
                    "ahead" => BlockRequest::Ahead(10.0),
                    "await_tick" => BlockRequest::AwaitTick,
                    _ => BlockRequest::AwaitTick,
                }));
            }
            Ok(HostOutcome::Value(Value::Num(1.0)))
        }
    }

    fn compile(src: &str) -> Rc<Program> {
        let ast = parse(src).expect("parse");
        compile_program(&ast).expect("compile")
    }

    fn run_to_halt(src: &str, host: &mut dyn Host) -> Vec<Value> {
        let prog = compile(src);
        let mut vm = Vm::new(prog);
        let mut budget = 100_000u32;
        loop {
            match vm.run(host, budget) {
                RunOutcome::Halted => break,
                RunOutcome::Blocked(_) => {
                    vm.resume(Value::Null);
                    continue;
                }
                RunOutcome::BudgetExceeded => {
                    budget = 100_000;
                    continue;
                }
                RunOutcome::Fault(e) => panic!("fault: {}", e),
            }
        }
        // Return the final main-return value: we can't easily capture it;
        // instead tests inspect via log calls.
        Vec::new()
    }

    fn logged(src: &str) -> Vec<String> {
        struct LogHost {
            logs: Vec<String>,
        }
        impl Host for LogHost {
            fn call(
                &mut self,
                f: crate::host::HostFn,
                args: Vec<Value>,
            ) -> Result<HostOutcome, String> {
                if f == crate::host::HostFn::Log {
                    self.logs.push(args[0].to_display());
                }
                Ok(HostOutcome::Value(Value::Null))
            }
        }
        let prog = compile(src);
        let mut vm = Vm::new(prog);
        let mut host = LogHost { logs: Vec::new() };
        let mut budget = 100_000u32;
        let mut done = false;
        while !done {
            match vm.run(&mut host, budget) {
                RunOutcome::Halted => done = true,
                RunOutcome::Blocked(_) => vm.resume(Value::Null),
                RunOutcome::BudgetExceeded => budget = 100_000,
                RunOutcome::Fault(e) => panic!("fault: {}", e),
            }
        }
        host.logs
    }

    #[test]
    fn arithmetic_and_globals() {
        let logs = logged(
            "func main() { var x = 2; var y = 3; log(x * y + 1); log(x / 2); x += 5; log(x); }",
        );
        assert_eq!(logs, vec!["7", "1", "7"]);
    }

    #[test]
    fn control_flow() {
        let logs = logged(
            "func main() {\
                var s = 0;\
                for (var i = 0; i < 5; i += 1) {\
                    if (i == 2) { continue; }\
                    if (i == 4) { break; }\
                    s += i;\
                }\
                log(s);\
                var n = 0;\
                while (n < 3) { n += 1; }\
                log(n);\
            }",
        );
        assert_eq!(logs, vec!["4", "3"]);
    }

    #[test]
    fn functions_and_recursion() {
        let logs = logged(
            "func fib(n) { if (n < 2) { return n; } return fib(n - 1) + fib(n - 2); }\
             func main() { log(fib(10)); }",
        );
        assert_eq!(logs, vec!["55"]);
    }

    #[test]
    fn globals_shared_between_functions() {
        let logs = logged(
            "func bump() { counter += 1; return counter; }\
             func main() { var counter = 10; log(bump()); log(bump()); log(counter); }",
        );
        assert_eq!(logs, vec!["11", "12", "12"]);
    }

    #[test]
    fn blocking_resumes_with_value() {
        struct BlockingHost;
        impl Host for BlockingHost {
            fn call(
                &mut self,
                f: crate::host::HostFn,
                _args: Vec<Value>,
            ) -> Result<HostOutcome, String> {
                if f == crate::host::HostFn::AwaitTick {
                    return Ok(HostOutcome::Block(BlockRequest::AwaitTick));
                }
                Ok(HostOutcome::Value(Value::Num(42.0)))
            }
        }
        let prog =
            compile("func main() { var a = await_tick(); var b = await_tick(); log(a + b); }");
        let mut vm = Vm::new(prog);
        let mut host = BlockingHost;
        let out = vm.run(&mut host, 1000);
        assert!(matches!(out, RunOutcome::Blocked(BlockRequest::AwaitTick)));
        vm.resume(Value::Num(1.0));
        let out = vm.run(&mut host, 1000);
        assert!(
            matches!(out, RunOutcome::Blocked(BlockRequest::AwaitTick)),
            "got {:?}",
            out
        );
        vm.resume(Value::Num(2.0));
        let out = vm.run(&mut host, 1000);
        assert!(matches!(out, RunOutcome::Halted), "got {:?}", out);
    }

    #[test]
    fn budget_exceeded_is_resumable() {
        struct CountingHost;
        impl Host for CountingHost {
            fn call(
                &mut self,
                _f: crate::host::HostFn,
                _args: Vec<Value>,
            ) -> Result<HostOutcome, String> {
                Ok(HostOutcome::Value(Value::Null))
            }
        }
        let prog = compile("func main() { var x = 0; while (true) { x += 1; } }");
        let mut vm = Vm::new(prog);
        let mut host = CountingHost;
        // Tiny budgets: the loop must grind forward without losing state.
        for _ in 0..10 {
            match vm.run(&mut host, 5) {
                RunOutcome::BudgetExceeded => {}
                other => panic!("expected budget exhaustion, got {:?}", other),
            }
        }
        // And it must halt when a finite loop is used.
        let prog2 =
            compile("func main() { var x = 0; for (var i = 0; i < 50; i += 1) { x += 2; } }");
        let mut vm2 = Vm::new(prog2);
        let mut done = false;
        let mut rounds = 0;
        while !done {
            match vm2.run(&mut host, 10) {
                RunOutcome::Halted => done = true,
                RunOutcome::BudgetExceeded => rounds += 1,
                other => panic!("unexpected {:?}", other),
            }
        }
        assert!(rounds > 0);
        let _ = run_to_halt("func main() { }", &mut MockHost::new());
    }

    #[test]
    fn faults() {
        let prog = compile("func main() { var x = 1 / 0; }");
        let mut vm = Vm::new(prog);
        struct NopHost;
        impl Host for NopHost {
            fn call(
                &mut self,
                _f: crate::host::HostFn,
                _args: Vec<Value>,
            ) -> Result<HostOutcome, String> {
                Ok(HostOutcome::Value(Value::Null))
            }
        }
        let out = vm.run(&mut NopHost, 1000);
        assert!(
            matches!(out, RunOutcome::Fault(ref m) if m.contains("zero")),
            "{:?}",
            out
        );
    }

    #[test]
    fn var_without_initializer_is_null_each_time() {
        let logs = logged(
            "func f() { for (var i = 0; i < 2; i += 1) { var x; log(x); x = 5; } } \
             func main() { f(); for (var i = 0; i < 2; i += 1) { var y; log(y); y = 5; } }",
        );
        assert_eq!(logs, vec!["null", "null", "null", "null"]);
    }

    #[test]
    fn string_concat() {
        let logs = logged("func main() { var s = \"a\" + 1 + true; log(s); log(\"x\" == \"x\"); }");
        assert_eq!(logs, vec!["a1true", "true"]);
    }

    #[test]
    fn nan_comparisons_are_false_not_a_panic() {
        for op in [Op::Lt, Op::Le, Op::Gt, Op::Ge] {
            assert_eq!(
                compare(&Value::Num(f64::NAN), &Value::Num(1.0), op),
                Ok(false)
            );
        }
    }

    #[test]
    fn run_without_resume_faults_instead_of_panicking() {
        struct BlockHost;
        impl Host for BlockHost {
            fn call(
                &mut self,
                _f: crate::host::HostFn,
                _args: Vec<Value>,
            ) -> Result<HostOutcome, String> {
                Ok(HostOutcome::Block(BlockRequest::AwaitTick))
            }
        }
        let mut vm = Vm::new(compile("func main() { ahead(5); }"));
        assert!(matches!(
            vm.run(&mut BlockHost, 100),
            RunOutcome::Blocked(_)
        ));
        assert!(matches!(vm.run(&mut BlockHost, 100), RunOutcome::Fault(_)));
    }

    #[test]
    fn string_growth_is_capped() {
        let prog = compile("func main() { var s = \"ab\"; while (true) { s = s + s; } }");
        let mut vm = Vm::new(prog);
        let out = vm.run(&mut NullHost, 100_000);
        assert!(
            matches!(out, RunOutcome::Fault(ref m) if m.contains("string too long")),
            "{:?}",
            out
        );
        // Right up to the limit is fine.
        let logs = logged(&format!(
            "func main() {{ var s = \"{}\"; log(s + \"b\"); }}",
            "a".repeat(MAX_STRING_LEN - 1)
        ));
        assert_eq!(logs[0].len(), MAX_STRING_LEN);
    }

    struct NullHost;
    impl Host for NullHost {
        fn call(
            &mut self,
            _f: crate::host::HostFn,
            _args: Vec<Value>,
        ) -> Result<HostOutcome, String> {
            Ok(HostOutcome::Value(Value::Null))
        }
    }

    #[test]
    fn arrays_store_load_and_persist_across_functions() {
        let logs = logged(
            "func push_hist(v) { hist[head] = v; head = (head + 1) % len(hist); } \
             func avg() { var s = 0; for (var i = 0; i < len(hist); i += 1) { s += hist[i]; } \
                          return s / len(hist); } \
             func main() { var hist[4]; var head = 0; \
                log(hist[3]); \
                for (var k = 1; k <= 6; k += 1) { push_hist(k); } \
                log(hist[0] + \",\" + hist[1] + \",\" + hist[2] + \",\" + hist[3]); \
                log(avg()); }",
        );
        assert_eq!(logs, vec!["0", "5,6,3,4", "4.5"]);
    }

    #[test]
    fn compound_element_assignment_evaluates_index_once() {
        let logs = logged(
            "func next() { calls += 1; return 1; } \
             func main() { var a[3]; var calls = 0; a[next()] += 5; a[next()] *= 3; \
                           log(a[1]); log(calls); }",
        );
        assert_eq!(logs, vec!["15", "2"]);
    }

    #[test]
    fn bad_array_access_faults() {
        let cases = [
            ("func main() { var a[4]; var x = a[4]; }", "out of range"),
            (
                "func main() { var a[4]; var x = a[0 - 1]; }",
                "out of range",
            ),
            ("func main() { var a[4]; var x = a[1.5]; }", "out of range"),
            (
                "func main() { var a[4]; a[\"1\"] = 2; }",
                "must be a number",
            ),
            ("func main() { var a[4]; a[0] = \"hi\"; }", "holds numbers"),
            ("func main() { var a[4]; a[0] = true; }", "holds numbers"),
            ("func main() { var a[4]; a[0] += \"x\"; }", "holds numbers"),
        ];
        for (src, want) in cases {
            let mut vm = Vm::new(compile(src));
            match vm.run(&mut NullHost, 1000) {
                RunOutcome::Fault(m) => assert!(m.contains(want), "{}: {}", src, m),
                other => panic!("{}: expected fault, got {:?}", src, other),
            }
        }
    }

    #[test]
    fn logical_short_circuit() {
        let logs = logged(
            "func side() { log(\"side\"); return true; }\
             func main() { var a = false && side(); var b = true || side(); log(a); log(b); }",
        );
        assert_eq!(logs, vec!["false", "true"]);
    }
}
