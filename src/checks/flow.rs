// ── DJ050: PossiblyUsedBeforeAssignment ───────────────────────────────────
//
// Control flow analysis that detects variables which may be used before being
// assigned on some code path. Implements a simple reaching-definitions
// approach: walk a function body tracking the set of "definitely assigned"
// variables, narrowing the set to the intersection at branch join points.
//
// Scope: one function at a time. Module-level and class-body code is skipped
// intentionally (too many false positives from conditional imports and class
// attributes).

use ruff_python_ast::*;
use ruff_text_size::Ranged;
use std::collections::HashSet;
use thorn_api::{AstCheck, CheckContext, Diagnostic};

// ── Python built-in names we never flag ──────────────────────────────────

const BUILTINS: &[&str] = &[
    // constants
    "True",
    "False",
    "None",
    "NotImplemented",
    "Ellipsis",
    "__debug__",
    "__name__",
    "__file__",
    "__doc__",
    "__package__",
    "__spec__",
    "__loader__",
    "__builtins__",
    "__all__",
    "__version__",
    "__author__",
    "__annotations__",
    "__dict__",
    "__slots__",
    "__class__",
    // common builtins
    "print",
    "len",
    "range",
    "enumerate",
    "zip",
    "map",
    "filter",
    "sorted",
    "reversed",
    "list",
    "dict",
    "set",
    "tuple",
    "str",
    "int",
    "float",
    "bool",
    "bytes",
    "bytearray",
    "type",
    "object",
    "super",
    "isinstance",
    "issubclass",
    "hasattr",
    "getattr",
    "setattr",
    "delattr",
    "callable",
    "iter",
    "next",
    "open",
    "input",
    "id",
    "hash",
    "repr",
    "abs",
    "round",
    "min",
    "max",
    "sum",
    "all",
    "any",
    "pow",
    "divmod",
    "hex",
    "oct",
    "bin",
    "ord",
    "chr",
    "vars",
    "dir",
    "locals",
    "globals",
    "exec",
    "eval",
    "compile",
    "breakpoint",
    "format",
    "staticmethod",
    "classmethod",
    "property",
    "slice",
    "memoryview",
    "frozenset",
    "complex",
    "NotImplementedError",
    "Exception",
    "ValueError",
    "TypeError",
    "AttributeError",
    "KeyError",
    "IndexError",
    "RuntimeError",
    "StopIteration",
    "StopAsyncIteration",
    "GeneratorExit",
    "SystemExit",
    "KeyboardInterrupt",
    "OSError",
    "IOError",
    "FileNotFoundError",
    "PermissionError",
    "ImportError",
    "ModuleNotFoundError",
    "NameError",
    "UnboundLocalError",
    "RecursionError",
    "OverflowError",
    "ZeroDivisionError",
    "FloatingPointError",
    "ArithmeticError",
    "LookupError",
    "MemoryError",
    "BufferError",
    "AssertionError",
    "UnicodeError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeTranslateError",
    "SyntaxError",
    "IndentationError",
    "TabError",
    "SystemError",
    "ReferenceError",
    "EOFError",
    "ConnectionError",
    "BrokenPipeError",
    "ConnectionAbortedError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "BlockingIOError",
    "ChildProcessError",
    "FileExistsError",
    "IsADirectoryError",
    "NotADirectoryError",
    "InterruptedError",
    "ProcessLookupError",
    "TimeoutError",
    "Warning",
    "DeprecationWarning",
    "PendingDeprecationWarning",
    "RuntimeWarning",
    "SyntaxWarning",
    "ResourceWarning",
    "FutureWarning",
    "ImportWarning",
    "UnicodeWarning",
    "BytesWarning",
    "UserWarning",
    "BaseException",
    "BaseExceptionGroup",
    "ExceptionGroup",
    "enumerate",
    "zip",
    "map",
    "reversed",
    "NotImplemented",
    // self/cls are always OK
    "self",
    "cls",
];

fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

// ── Public check ─────────────────────────────────────────────────────────

pub struct PossiblyUsedBeforeAssignment;

impl AstCheck for PossiblyUsedBeforeAssignment {
    fn code(&self) -> &'static str {
        "DJ050"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.filename.contains("/tests/") || ctx.filename.contains("\\tests\\") {
            return vec![];
        }
        // Collect all module-level names (imports, assignments, function/class defs)
        // so that functions can reference them without false positives.
        let module_names = collect_module_level_names(&ctx.module.body);
        let mut diags = Vec::new();
        for stmt in &ctx.module.body {
            visit_stmt_for_functions(stmt, ctx.filename, &module_names, &mut diags);
        }
        diags
    }
}

// ── Module-level name collection ─────────────────────────────────────────

/// Collect all names defined at module level: imports, assignments, function
/// defs, class defs. These are always available inside functions.
fn collect_module_level_names(body: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in body {
        match stmt {
            Stmt::Import(imp) => {
                for alias in &imp.names {
                    let name = alias
                        .asname
                        .as_ref()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| alias.name.split('.').next().unwrap_or("").to_string());
                    names.insert(name);
                }
            }
            Stmt::ImportFrom(imp) => {
                for alias in &imp.names {
                    let name = alias
                        .asname
                        .as_ref()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| alias.name.to_string());
                    names.insert(name);
                }
            }
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        names.insert(n.id.to_string());
                    }
                }
            }
            Stmt::AnnAssign(ann) => {
                if let Expr::Name(n) = ann.target.as_ref() {
                    names.insert(n.id.to_string());
                }
            }
            Stmt::FunctionDef(f) => {
                names.insert(f.name.to_string());
            }
            Stmt::ClassDef(c) => {
                names.insert(c.name.to_string());
            }
            Stmt::If(if_stmt) => {
                // Conditional imports at module level — collect from all branches
                collect_module_level_names_from_body(&if_stmt.body, &mut names);
                for clause in &if_stmt.elif_else_clauses {
                    collect_module_level_names_from_body(&clause.body, &mut names);
                }
            }
            Stmt::Try(try_stmt) => {
                collect_module_level_names_from_body(&try_stmt.body, &mut names);
                for handler in &try_stmt.handlers {
                    let ExceptHandler::ExceptHandler(h) = handler;
                    collect_module_level_names_from_body(&h.body, &mut names);
                }
            }
            _ => {}
        }
    }
    names
}

fn collect_module_level_names_from_body(body: &[Stmt], names: &mut HashSet<String>) {
    for s in body {
        match s {
            Stmt::Import(imp) => {
                for alias in &imp.names {
                    let name = alias
                        .asname
                        .as_ref()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| alias.name.split('.').next().unwrap_or("").to_string());
                    names.insert(name);
                }
            }
            Stmt::ImportFrom(imp) => {
                for alias in &imp.names {
                    let name = alias
                        .asname
                        .as_ref()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| alias.name.to_string());
                    names.insert(name);
                }
            }
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        names.insert(n.id.to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

// ── Recursively find function definitions ────────────────────────────────

/// Walk the AST looking for function definitions. When one is found, analyze
/// it for possibly-used-before-assignment. Recurse into nested functions.
fn visit_stmt_for_functions(
    stmt: &Stmt,
    filename: &str,
    module_names: &HashSet<String>,
    diags: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::FunctionDef(f) => {
            analyze_function(f, filename, module_names, diags);
            // Also recurse into the function body for nested function defs
            for s in &f.body {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
        }
        Stmt::ClassDef(cls) => {
            // Skip class-level code but recurse into methods
            for s in &cls.body {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
        }
        Stmt::If(s) => {
            for s in &s.body {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
            for clause in &s.elif_else_clauses {
                for s in &clause.body {
                    visit_stmt_for_functions(s, filename, module_names, diags);
                }
            }
        }
        Stmt::For(s) => {
            for s in &s.body {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
            for s in &s.orelse {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
        }
        Stmt::While(s) => {
            for s in &s.body {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
            for s in &s.orelse {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
        }
        Stmt::With(s) => {
            for s in &s.body {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
        }
        Stmt::Try(s) => {
            for s in &s.body {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
            for handler in &s.handlers {
                let ExceptHandler::ExceptHandler(eh) = handler;
                for s in &eh.body {
                    visit_stmt_for_functions(s, filename, module_names, diags);
                }
            }
            for s in &s.orelse {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
            for s in &s.finalbody {
                visit_stmt_for_functions(s, filename, module_names, diags);
            }
        }
        _ => {}
    }
}

// ── Function-level analysis ───────────────────────────────────────────────

fn analyze_function(
    func: &StmtFunctionDef,
    filename: &str,
    module_names: &HashSet<String>,
    diags: &mut Vec<Diagnostic>,
) {
    // Start with module-level names (imports, globals) as definitely defined
    let mut defined: HashSet<String> = module_names.clone();

    // Collect all parameter names as definitely defined
    collect_params(&func.parameters, &mut defined);

    analyze_body(&func.body, &mut defined, filename, diags);
}

fn collect_params(params: &Parameters, defined: &mut HashSet<String>) {
    // positional-only args
    for p in &params.posonlyargs {
        defined.insert(p.parameter.name.as_str().to_string());
    }
    // regular args
    for p in &params.args {
        defined.insert(p.parameter.name.as_str().to_string());
    }
    // keyword-only args
    for p in &params.kwonlyargs {
        defined.insert(p.parameter.name.as_str().to_string());
    }
    // *args
    if let Some(vararg) = &params.vararg {
        defined.insert(vararg.name.as_str().to_string());
    }
    // **kwargs
    if let Some(kwarg) = &params.kwarg {
        defined.insert(kwarg.name.as_str().to_string());
    }
}

// ── Statement-level analysis ──────────────────────────────────────────────

fn analyze_body(
    body: &[Stmt],
    defined: &mut HashSet<String>,
    filename: &str,
    diags: &mut Vec<Diagnostic>,
) {
    for stmt in body {
        analyze_stmt(stmt, defined, filename, diags);
    }
}

fn analyze_stmt(
    stmt: &Stmt,
    defined: &mut HashSet<String>,
    filename: &str,
    diags: &mut Vec<Diagnostic>,
) {
    match stmt {
        // ── Simple assignment: check RHS, then define LHS ─────────────────
        Stmt::Assign(s) => {
            check_expr(s.value.as_ref(), defined, filename, diags);
            for target in &s.targets {
                collect_targets(target, defined);
            }
        }

        // ── Annotated assignment: x: int = ... ───────────────────────────
        Stmt::AnnAssign(s) => {
            if let Some(value) = &s.value {
                check_expr(value.as_ref(), defined, filename, diags);
            }
            // Only define the target when there is a value
            if s.value.is_some() {
                collect_targets(&s.target, defined);
            }
        }

        // ── Augmented assignment: x += 1 (x must already be defined) ─────
        Stmt::AugAssign(s) => {
            // The target is both read and written; treat as a use
            check_expr(&s.target, defined, filename, diags);
            check_expr(&s.value, defined, filename, diags);
            // After the augmented assign, the target is definitely defined
            collect_targets(&s.target, defined);
        }

        // ── Import: adds names to defined set ─────────────────────────────
        Stmt::Import(s) => {
            for alias in &s.names {
                let name = alias.asname.as_ref().unwrap_or(&alias.name);
                // Only the top-level module name is bound for bare imports
                let top = name.as_str().split('.').next().unwrap_or(name.as_str());
                defined.insert(top.to_string());
            }
        }

        // ── From import: adds each imported name ──────────────────────────
        Stmt::ImportFrom(s) => {
            for alias in &s.names {
                // `from x import y as z` → z is defined; `from x import y` → y
                let name = alias.asname.as_ref().unwrap_or(&alias.name);
                defined.insert(name.as_str().to_string());
            }
        }

        // ── global/nonlocal: treat as definitely defined ──────────────────
        Stmt::Global(s) => {
            for name in &s.names {
                defined.insert(name.as_str().to_string());
            }
        }
        Stmt::Nonlocal(s) => {
            for name in &s.names {
                defined.insert(name.as_str().to_string());
            }
        }

        // ── for x in iterable: loop body might not execute ────────────────
        Stmt::For(s) => {
            // Check the iterable
            check_expr(&s.iter, defined, filename, diags);

            // The loop variable is defined inside the loop — but since the
            // loop body may not execute, we analyze the body with the loop
            // variable defined, then do NOT add loop-body-only variables to
            // the outer defined set. However the loop variable itself IS
            // available after the loop if the loop ran (Python semantics).
            //
            // Conservative approach: the loop variable leaks out (like Python)
            // but other variables assigned only in the body do not.
            let mut body_defined = defined.clone();
            collect_targets(&s.target, &mut body_defined);
            analyze_body(&s.body, &mut body_defined, filename, diags);

            // orelse (for...else) runs when loop completes without break
            let mut else_defined = defined.clone();
            analyze_body(&s.orelse, &mut else_defined, filename, diags);

            // After the for statement: only keep what was defined before,
            // plus the loop variable itself (it leaks in Python)
            collect_targets(&s.target, defined);
            // Variables from body/orelse that weren't defined before are NOT guaranteed
        }

        // ── while: same conservative treatment as for ─────────────────────
        Stmt::While(s) => {
            check_expr(&s.test, defined, filename, diags);

            let mut body_defined = defined.clone();
            analyze_body(&s.body, &mut body_defined, filename, diags);

            let mut else_defined = defined.clone();
            analyze_body(&s.orelse, &mut else_defined, filename, diags);
            // Do not widen defined set from loop body
        }

        // ── if/elif/else ─────────────────────────────────────────────────
        Stmt::If(s) => {
            check_expr(&s.test, defined, filename, diags);

            let pre_if = defined.clone();

            // Analyze the if-body
            let mut if_defined = defined.clone();
            analyze_body(&s.body, &mut if_defined, filename, diags);

            // Analyse each elif/else clause
            let has_else = s.elif_else_clauses.last().is_some_and(|c| c.test.is_none());

            if s.elif_else_clauses.is_empty() {
                // No else: variables assigned only in the if-body are NOT guaranteed
                // Keep only what was defined before
                *defined = pre_if;
            } else {
                // Collect the "defined" sets for each elif/else branch
                // Start with what we had going into the if
                let mut branch_defined_sets: Vec<HashSet<String>> = vec![if_defined];

                for clause in &s.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        // This is an elif clause
                        check_expr(test, defined, filename, diags);
                    }
                    let mut branch_defined = pre_if.clone();
                    analyze_body(&clause.body, &mut branch_defined, filename, diags);
                    branch_defined_sets.push(branch_defined);
                }

                if has_else {
                    // All branches are covered → intersection is guaranteed
                    let mut intersection = branch_defined_sets[0].clone();
                    for set in &branch_defined_sets[1..] {
                        intersection.retain(|k| set.contains(k));
                    }
                    *defined = intersection;
                } else {
                    // No else → not all paths covered; keep only pre-if
                    *defined = pre_if;
                }
            }
        }

        // ── try/except/finally ────────────────────────────────────────────
        //
        // Variables assigned in the try body are NOT guaranteed (an exception
        // might skip them). Variables in the finally block ARE guaranteed.
        // Variables in handlers are NOT guaranteed (handler may not run).
        Stmt::Try(s) => {
            let pre_try = defined.clone();

            // Analyze try body — but assignments there are NOT guaranteed
            let mut try_defined = defined.clone();
            analyze_body(&s.body, &mut try_defined, filename, diags);

            // Analyze each except handler
            for handler in &s.handlers {
                let ExceptHandler::ExceptHandler(eh) = handler;
                let mut handler_defined = pre_try.clone();
                // The `as e` variable is defined within the handler
                if let Some(name) = &eh.name {
                    handler_defined.insert(name.as_str().to_string());
                }
                if let Some(exc_type) = &eh.type_ {
                    check_expr(exc_type, &handler_defined, filename, diags);
                }
                analyze_body(&eh.body, &mut handler_defined, filename, diags);
            }

            // orelse (try...else) runs only if try completed without exception
            // Treat it like the try body (conservative)
            let mut else_defined = pre_try.clone();
            analyze_body(&s.orelse, &mut else_defined, filename, diags);

            // finally always runs → its assignments ARE guaranteed
            analyze_body(&s.finalbody, defined, filename, diags);
            // Assignments from try/except/else are NOT guaranteed; only finally propagates
        }

        // ── with ... as x ────────────────────────────────────────────────
        Stmt::With(s) => {
            for item in &s.items {
                check_expr(&item.context_expr, defined, filename, diags);
                if let Some(var) = &item.optional_vars {
                    collect_targets(var, defined);
                }
            }
            analyze_body(&s.body, defined, filename, diags);
        }

        // ── Expression statement (e.g., function calls) ───────────────────
        Stmt::Expr(s) => {
            check_expr(&s.value, defined, filename, diags);
        }

        // ── Return ────────────────────────────────────────────────────────
        Stmt::Return(s) => {
            if let Some(value) = &s.value {
                check_expr(value, defined, filename, diags);
            }
        }

        // ── Delete ───────────────────────────────────────────────────────
        Stmt::Delete(s) => {
            for target in &s.targets {
                check_expr(target, defined, filename, diags);
            }
        }

        // ── Assert ───────────────────────────────────────────────────────
        Stmt::Assert(s) => {
            check_expr(&s.test, defined, filename, diags);
            if let Some(msg) = &s.msg {
                check_expr(msg, defined, filename, diags);
            }
        }

        // ── Raise ────────────────────────────────────────────────────────
        Stmt::Raise(s) => {
            if let Some(exc) = &s.exc {
                check_expr(exc, defined, filename, diags);
            }
            if let Some(cause) = &s.cause {
                check_expr(cause, defined, filename, diags);
            }
        }

        // ── Nested function/class definition: the name is defined ─────────
        // (We don't analyze the nested function body here — visit_stmt_for_functions
        // handles that separately with proper scoping.)
        Stmt::FunctionDef(f) => {
            // Check decorators in the current scope
            for dec in &f.decorator_list {
                check_expr(&dec.expression, defined, filename, diags);
            }
            defined.insert(f.name.as_str().to_string());
        }
        Stmt::ClassDef(cls) => {
            for dec in &cls.decorator_list {
                check_expr(&dec.expression, defined, filename, diags);
            }
            defined.insert(cls.name.as_str().to_string());
        }

        // ── Everything else (pass, break, continue, type aliases, etc.) ───
        _ => {}
    }
}

// ── Expression-level use checker ─────────────────────────────────────────

/// Walk an expression tree. For every `Name` node encountered, flag it if
/// the name is not in the `defined` set and is not a builtin/special name.
fn check_expr(expr: &Expr, defined: &HashSet<String>, filename: &str, diags: &mut Vec<Diagnostic>) {
    match expr {
        Expr::Name(n) => {
            let name = n.id.as_str();
            if should_flag_name(name, defined) {
                diags.push(
                    Diagnostic::new(
                        "DJ050",
                        format!("'{name}' may be used before assignment"),
                        filename,
                    )
                    .with_range(n.range())
                    .with_level(thorn_api::Level::Fix),
                );
            }
        }

        // Don't descend into comprehensions/lambdas — they create new scopes
        Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Lambda(_) => {
            // Skip — inner bindings (e.g. `x for x in ...`) must not be
            // treated as definitions in the outer scope, and uses inside the
            // comprehension are in their own scope.
        }

        // ── Compound expressions ─────────────────────────────────────────
        Expr::BoolOp(e) => {
            for val in &e.values {
                check_expr(val, defined, filename, diags);
            }
        }
        Expr::BinOp(e) => {
            check_expr(&e.left, defined, filename, diags);
            check_expr(&e.right, defined, filename, diags);
        }
        Expr::UnaryOp(e) => {
            check_expr(&e.operand, defined, filename, diags);
        }
        Expr::If(e) => {
            check_expr(&e.test, defined, filename, diags);
            check_expr(&e.body, defined, filename, diags);
            check_expr(&e.orelse, defined, filename, diags);
        }
        Expr::Compare(e) => {
            check_expr(&e.left, defined, filename, diags);
            for cmp in &e.comparators {
                check_expr(cmp, defined, filename, diags);
            }
        }
        Expr::Call(e) => {
            check_expr(&e.func, defined, filename, diags);
            for arg in &e.arguments.args {
                check_expr(arg, defined, filename, diags);
            }
            for kw in &e.arguments.keywords {
                check_expr(&kw.value, defined, filename, diags);
            }
        }
        Expr::Attribute(e) => {
            // Only check the object, not the attribute name itself
            check_expr(&e.value, defined, filename, diags);
        }
        Expr::Subscript(e) => {
            check_expr(&e.value, defined, filename, diags);
            check_expr(&e.slice, defined, filename, diags);
        }
        Expr::Starred(e) => {
            check_expr(&e.value, defined, filename, diags);
        }
        Expr::Tuple(e) => {
            for elt in &e.elts {
                check_expr(elt, defined, filename, diags);
            }
        }
        Expr::List(e) => {
            for elt in &e.elts {
                check_expr(elt, defined, filename, diags);
            }
        }
        Expr::Set(e) => {
            for elt in &e.elts {
                check_expr(elt, defined, filename, diags);
            }
        }
        Expr::Dict(e) => {
            for item in &e.items {
                if let Some(key) = &item.key {
                    check_expr(key, defined, filename, diags);
                }
                check_expr(&item.value, defined, filename, diags);
            }
        }
        Expr::Await(e) => {
            check_expr(&e.value, defined, filename, diags);
        }
        Expr::Yield(e) => {
            if let Some(val) = &e.value {
                check_expr(val, defined, filename, diags);
            }
        }
        Expr::YieldFrom(e) => {
            check_expr(&e.value, defined, filename, diags);
        }
        Expr::FString(e) => {
            // Walk interpolated expressions inside f-strings
            for part in &e.value {
                match part {
                    FStringPart::FString(inner) => {
                        for inner_part in &inner.elements {
                            if let ruff_python_ast::FStringElement::Expression(fv) = inner_part {
                                check_expr(&fv.expression, defined, filename, diags);
                            }
                        }
                    }
                    FStringPart::Literal(_) => {}
                }
            }
        }
        Expr::Named(e) => {
            // walrus operator: (x := expr) — check rhs, define lhs
            check_expr(&e.value, defined, filename, diags);
            // Note: we can't mutate `defined` here since it's a shared ref.
            // Walrus assignments are rare; skip the definition to be safe.
        }

        // Literals — nothing to check
        Expr::NumberLiteral(_)
        | Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_) => {}

        // IpyEscapeCommand and other non-standard nodes
        _ => {}
    }
}

// ── Assignment target collector ───────────────────────────────────────────

/// Extract all names that a target expression binds and insert them into `defined`.
fn collect_targets(expr: &Expr, defined: &mut HashSet<String>) {
    match expr {
        Expr::Name(n) => {
            defined.insert(n.id.as_str().to_string());
        }
        Expr::Tuple(t) => {
            for elt in &t.elts {
                collect_targets(elt, defined);
            }
        }
        Expr::List(l) => {
            for elt in &l.elts {
                collect_targets(elt, defined);
            }
        }
        Expr::Starred(s) => {
            collect_targets(&s.value, defined);
        }
        // Subscript/Attribute targets (e.g. self.x = ...) — no new local binding
        _ => {}
    }
}

// ── Name filter ──────────────────────────────────────────────────────────

fn should_flag_name(name: &str, defined: &HashSet<String>) -> bool {
    if defined.contains(name) {
        return false;
    }
    if is_builtin(name) {
        return false;
    }
    // Skip names starting with _ (intentionally unused / dunder)
    if name.starts_with('_') {
        return false;
    }
    // Skip all-uppercase names (constants assumed to be module-level)
    if name
        .chars()
        .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
        && name.len() > 1
    {
        return false;
    }
    true
}
