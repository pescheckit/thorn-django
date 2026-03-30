// ── DJ053-DJ067: Code quality checks ─────────────────────────────────────
//
// Checks for code quality: pointless statements, redefined builtins, broad
// exception handling, forgotten debug statements, singleton comparisons,
// and verbose super() calls.

use thorn_api::ast::*;
use thorn_api::visitor::{walk_expr, walk_stmt, Visitor};
use thorn_api::{AstCheck, CheckContext, Diagnostic};

use super::common::{is_builtin, text_range};

// ── DJ053: PointlessStatement ─────────────────────────────────────────────
//
// A bare expression statement (not a call, yield, await, or docstring) has
// no effect and is almost certainly a programmer error.

pub struct PointlessStatement;

impl AstCheck for PointlessStatement {
    fn code(&self) -> &'static str {
        "DJ053"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = PointlessStatementVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.check_body(&ctx.module.body);
        v.diags
    }
}

struct PointlessStatementVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> PointlessStatementVisitor<'a> {
    fn check_body(&mut self, body: &[Stmt]) {
        for (idx, stmt) in body.iter().enumerate() {
            if let Stmt::Expr(expr_stmt) = stmt {
                let is_docstring_position = idx == 0;
                if !self.is_meaningful_expr(expr_stmt.value.as_ref(), is_docstring_position) {
                    self.diags.push(
                        Diagnostic::new("DJ053", "Statement has no effect.", self.filename)
                            .with_range(text_range(expr_stmt.range())),
                    );
                }
            }
            self.recurse_stmt(stmt);
        }
    }

    fn is_meaningful_expr(&self, expr: &Expr, is_docstring_position: bool) -> bool {
        match expr {
            Expr::Call(_) | Expr::Yield(_) | Expr::YieldFrom(_) | Expr::Await(_) => true,
            Expr::StringLiteral(_) if is_docstring_position => true,
            _ => false,
        }
    }

    fn recurse_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::FunctionDef(f) => self.check_body(&f.body),
            Stmt::ClassDef(c) => self.check_body(&c.body),
            Stmt::If(s) => {
                self.check_body(&s.body);
                for clause in &s.elif_else_clauses {
                    self.check_body(&clause.body);
                }
            }
            Stmt::For(s) => {
                self.check_body(&s.body);
                self.check_body(&s.orelse);
            }
            Stmt::While(s) => {
                self.check_body(&s.body);
                self.check_body(&s.orelse);
            }
            Stmt::With(s) => self.check_body(&s.body),
            Stmt::Try(s) => {
                self.check_body(&s.body);
                for handler in &s.handlers {
                    self.check_body(&handler.body);
                }
                self.check_body(&s.orelse);
                self.check_body(&s.finalbody);
            }
            _ => {}
        }
    }
}

// ── DJ059: RedefinedBuiltin ───────────────────────────────────────────────
//
// Assigning to, or defining a function/class with, the same name as a Python
// builtin makes the builtin inaccessible in that scope.
//
// Fix: `self` and `cls` are NOT flagged here — they are conventional method
// parameter names that happen to share names that look built-in but are not
// in PYTHON_BUILTINS. The `should_flag` method explicitly skips them.

pub struct RedefinedBuiltin;

impl AstCheck for RedefinedBuiltin {
    fn code(&self) -> &'static str {
        "DJ059"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = RedefinedBuiltinVisitor {
            diags: vec![],
            filename: ctx.filename,
            in_checked_scope: true,
            enclosing_function: String::new(),
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct RedefinedBuiltinVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    /// Whether we are currently inside a scope where we flag redefinitions.
    in_checked_scope: bool,
    /// Name of the immediately enclosing function, used to exempt `__init__`.
    enclosing_function: String,
}

impl<'a> Visitor for RedefinedBuiltinVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assign(s) if self.in_checked_scope => {
                for target in &s.targets {
                    self.check_assign_target(target, s.range());
                }
                walk_stmt(self, stmt);
            }
            Stmt::AnnAssign(s) if self.in_checked_scope => {
                self.check_assign_target(s.target.as_ref(), s.range());
                walk_stmt(self, stmt);
            }

            Stmt::FunctionDef(f) => {
                if self.in_checked_scope {
                    let name = f.name.as_str();
                    if self.should_flag(name) {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ059",
                                format!("Redefining built-in '{name}'."),
                                self.filename,
                            )
                            .with_range(text_range(f.range())),
                        );
                    }
                }
                let prev_fn = self.enclosing_function.clone();
                let prev_scope = self.in_checked_scope;
                self.enclosing_function = f.name.as_str().to_string();
                self.in_checked_scope = f.name.as_str() != "__init__";
                walk_stmt(self, stmt);
                self.in_checked_scope = prev_scope;
                self.enclosing_function = prev_fn;
            }

            Stmt::ClassDef(c) => {
                if self.in_checked_scope {
                    let name = c.name.as_str();
                    if self.should_flag(name) {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ059",
                                format!("Redefining built-in '{name}'."),
                                self.filename,
                            )
                            .with_range(text_range(c.range())),
                        );
                    }
                }
                let prev = self.in_checked_scope;
                self.in_checked_scope = false;
                walk_stmt(self, stmt);
                self.in_checked_scope = prev;
            }

            _ => {
                walk_stmt(self, stmt);
            }
        }
    }
}

impl<'a> RedefinedBuiltinVisitor<'a> {
    fn should_flag(&self, name: &str) -> bool {
        // Skip _ prefixed names (intentional shadowing pattern).
        if name.starts_with('_') {
            return false;
        }
        // self/cls are method parameter conventions, not builtins.
        // They are included in PYTHON_BUILTINS for flow analysis (DJ050)
        // but must never trigger DJ059.
        if name == "self" || name == "cls" {
            return false;
        }
        is_builtin(name)
    }

    fn check_assign_target(&mut self, target: &Expr, range: thorn_api::ByteRange) {
        match target {
            Expr::Name(n) => {
                let name = n.id.as_str();
                if self.should_flag(name) {
                    self.diags.push(
                        Diagnostic::new(
                            "DJ059",
                            format!("Redefining built-in '{name}'."),
                            self.filename,
                        )
                        .with_range(text_range(range)),
                    );
                }
            }
            Expr::Tuple(t) => {
                for elt in &t.elts {
                    self.check_assign_target(elt, range);
                }
            }
            Expr::List(l) => {
                for elt in &l.elts {
                    self.check_assign_target(elt, range);
                }
            }
            Expr::Starred(s) => {
                self.check_assign_target(s.value.as_ref(), range);
            }
            _ => {}
        }
    }
}

// ── DJ061: BroadExceptionCaught ───────────────────────────────────────────

pub struct BroadExceptionCaught;

impl AstCheck for BroadExceptionCaught {
    fn code(&self) -> &'static str {
        "DJ061"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = BroadExceptionCaughtVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct BroadExceptionCaughtVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor for BroadExceptionCaughtVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::Try(try_stmt) = stmt {
            for handler in &try_stmt.handlers {
                if let Some(exc_type) = &handler.type_ {
                    if is_broad_exception(exc_type) {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ061",
                                "Catching too general exception 'Exception'. Prefer catching a more specific exception.",
                                self.filename,
                            )
                            .with_range(text_range(handler.range())),
                        );
                    }
                }
            }
        }
        walk_stmt(self, stmt);
    }
}

fn is_broad_exception(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "Exception",
        Expr::Attribute(a) => a.attr.as_str() == "Exception",
        _ => false,
    }
}

// ── DJ063: ForgottenDebugStatement ────────────────────────────────────────

pub struct ForgottenDebugStatement;

impl AstCheck for ForgottenDebugStatement {
    fn code(&self) -> &'static str {
        "DJ063"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = ForgottenDebugVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct ForgottenDebugVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor for ForgottenDebugVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if is_debug_call(call.func.as_ref()) {
                self.diags.push(
                    Diagnostic::new(
                        "DJ063",
                        "Forgotten debug statement. Remove before committing.",
                        self.filename,
                    )
                    .with_range(text_range(call.range())),
                );
            }
        }
        walk_expr(self, expr);
    }
}

fn is_debug_call(func: &Expr) -> bool {
    match func {
        Expr::Name(n) => n.id.as_str() == "breakpoint",
        Expr::Attribute(a) => {
            let method = a.attr.as_str();
            matches!(method, "set_trace" | "embed" | "interact")
        }
        _ => false,
    }
}

// ── DJ065: SingletonComparison ────────────────────────────────────────────

pub struct SingletonComparison;

impl AstCheck for SingletonComparison {
    fn code(&self) -> &'static str {
        "DJ065"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = SingletonComparisonVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct SingletonComparisonVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor for SingletonComparisonVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Compare(cmp) = expr {
            let operands: Vec<&Expr> = std::iter::once(cmp.left.as_ref())
                .chain(cmp.comparators.iter())
                .collect();

            for (i, op) in cmp.ops.iter().enumerate() {
                let lhs = operands[i];
                let rhs = operands[i + 1];

                if !matches!(op, CmpOp::Eq | CmpOp::NotEq) {
                    continue;
                }

                if let Some(msg) = singleton_message(lhs, rhs) {
                    self.diags.push(
                        Diagnostic::new("DJ065", msg, self.filename)
                            .with_range(text_range(cmp.range())),
                    );
                }
            }
        }
        walk_expr(self, expr);
    }
}

fn singleton_message(lhs: &Expr, rhs: &Expr) -> Option<&'static str> {
    let lhs_kind = singleton_kind(lhs);
    let rhs_kind = singleton_kind(rhs);

    let kind = lhs_kind.or(rhs_kind)?;
    Some(match kind {
        SingletonKind::None => "Use 'is None' / 'is not None' instead of '== None' / '!= None'.",
        SingletonKind::Bool => {
            "Use 'is True'/'is False' or test the value directly instead of '== True'/'== False'."
        }
    })
}

enum SingletonKind {
    None,
    Bool,
}

fn singleton_kind(expr: &Expr) -> Option<SingletonKind> {
    match expr {
        Expr::NoneLiteral(_) => Some(SingletonKind::None),
        Expr::BooleanLiteral(_) => Some(SingletonKind::Bool),
        _ => None,
    }
}

// ── DJ067: SuperWithArguments ─────────────────────────────────────────────

pub struct SuperWithArguments;

impl AstCheck for SuperWithArguments {
    fn code(&self) -> &'static str {
        "DJ067"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = SuperWithArgumentsVisitor {
            diags: vec![],
            filename: ctx.filename,
            class_name_stack: vec![],
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct SuperWithArgumentsVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    class_name_stack: Vec<String>,
}

impl<'a> Visitor for SuperWithArgumentsVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            self.class_name_stack.push(cls.name.as_str().to_string());
            walk_stmt(self, stmt);
            self.class_name_stack.pop();
        } else {
            walk_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if let Expr::Name(n) = call.func.as_ref() {
                if n.id.as_str() == "super" && call.arguments.args.len() == 2 {
                    let first = &call.arguments.args[0];
                    let second = &call.arguments.args[1];

                    let first_matches = self
                        .class_name_stack
                        .last()
                        .map(|cls| {
                            if let Expr::Name(fn_) = first {
                                fn_.id.as_str() == cls.as_str()
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);

                    let second_is_self_or_cls = matches!(second, Expr::Name(n)
                        if n.id.as_str() == "self" || n.id.as_str() == "cls");

                    if first_matches && second_is_self_or_cls {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ067",
                                "Consider using 'super()' without arguments (Python 3 style).",
                                self.filename,
                            )
                            .with_range(text_range(call.range())),
                        );
                    }
                }
            }
        }
        walk_expr(self, expr);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::common::test_helpers::run_check;
    use super::*;

    // ── DJ053: PointlessStatement ─────────────────────────────────────────

    #[test]
    fn dj053_flags_bare_name_statement() {
        let src = r#"
x = 1
x
"#;
        let codes = run_check(&PointlessStatement, src);
        assert!(
            codes.contains(&"DJ053".to_string()),
            "expected DJ053 for bare name statement, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj053_flags_attribute_access_statement() {
        let src = r#"
obj.value
"#;
        let codes = run_check(&PointlessStatement, src);
        assert!(
            codes.contains(&"DJ053".to_string()),
            "expected DJ053 for bare attribute access, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj053_flags_comparison_statement() {
        let src = r#"
x = 1
x == 1
"#;
        let codes = run_check(&PointlessStatement, src);
        assert!(
            codes.contains(&"DJ053".to_string()),
            "expected DJ053 for comparison as statement, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj053_no_flag_for_call_statement() {
        let src = r#"
print("hello")
"#;
        let codes = run_check(&PointlessStatement, src);
        assert!(
            !codes.contains(&"DJ053".to_string()),
            "unexpected DJ053 for function call statement, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj053_no_flag_for_module_docstring() {
        let src = r#"
"""This is the module docstring."""

x = 1
"#;
        let codes = run_check(&PointlessStatement, src);
        assert!(
            !codes.contains(&"DJ053".to_string()),
            "unexpected DJ053 for module docstring, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj053_no_flag_for_function_docstring() {
        let src = r#"
def foo():
    """Function docstring."""
    return 1
"#;
        let codes = run_check(&PointlessStatement, src);
        assert!(
            !codes.contains(&"DJ053".to_string()),
            "unexpected DJ053 for function docstring, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj053_no_flag_for_class_docstring() {
        let src = r#"
class Foo:
    """Class docstring."""
    pass
"#;
        let codes = run_check(&PointlessStatement, src);
        assert!(
            !codes.contains(&"DJ053".to_string()),
            "unexpected DJ053 for class docstring, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj053_flags_string_literal_that_is_not_docstring() {
        let src = r#"
x = 1
"this is not a docstring"
"#;
        let codes = run_check(&PointlessStatement, src);
        assert!(
            codes.contains(&"DJ053".to_string()),
            "expected DJ053 for non-docstring string literal statement, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj053_no_flag_for_yield_statement() {
        let src = r#"
def gen():
    yield 1
"#;
        let codes = run_check(&PointlessStatement, src);
        assert!(
            !codes.contains(&"DJ053".to_string()),
            "unexpected DJ053 for yield statement, got: {:?}",
            codes
        );
    }

    // ── DJ059: RedefinedBuiltin ───────────────────────────────────────────

    #[test]
    fn dj059_flags_assignment_to_builtin_name() {
        let src = r#"
list = [1, 2, 3]
"#;
        let codes = run_check(&RedefinedBuiltin, src);
        assert!(
            codes.contains(&"DJ059".to_string()),
            "expected DJ059 for 'list = ...', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj059_flags_function_named_as_builtin() {
        let src = r#"
def str(x):
    return repr(x)
"#;
        let codes = run_check(&RedefinedBuiltin, src);
        assert!(
            codes.contains(&"DJ059".to_string()),
            "expected DJ059 for 'def str(...)', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj059_flags_class_named_as_builtin() {
        let src = r#"
class type:
    pass
"#;
        let codes = run_check(&RedefinedBuiltin, src);
        assert!(
            codes.contains(&"DJ059".to_string()),
            "expected DJ059 for 'class type:', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj059_no_flag_for_underscore_prefix() {
        let src = r#"
_list = [1, 2, 3]
"#;
        let codes = run_check(&RedefinedBuiltin, src);
        assert!(
            !codes.contains(&"DJ059".to_string()),
            "unexpected DJ059 for _-prefixed name, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj059_no_flag_inside_init_method() {
        let src = r#"
class Foo:
    def __init__(self, id, type):
        id = id
        self.type = type
"#;
        let codes = run_check(&RedefinedBuiltin, src);
        assert!(
            !codes.contains(&"DJ059".to_string()),
            "unexpected DJ059 inside __init__, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj059_flags_builtin_redefined_in_regular_method() {
        let src = r#"
class Foo:
    def bar(self):
        list = []
"#;
        let codes = run_check(&RedefinedBuiltin, src);
        assert!(
            codes.contains(&"DJ059".to_string()),
            "expected DJ059 for builtin redefined in method, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj059_no_flag_for_ordinary_name() {
        let src = r#"
my_list = [1, 2, 3]
"#;
        let codes = run_check(&RedefinedBuiltin, src);
        assert!(
            !codes.contains(&"DJ059".to_string()),
            "unexpected DJ059 for ordinary name, got: {:?}",
            codes
        );
    }

    /// `cls` is a method-convention name, not a builtin — must not be flagged.
    #[test]
    fn dj059_no_flag_for_cls_parameter() {
        let src = r#"
class Foo:
    @classmethod
    def create(cls):
        return cls()
"#;
        let codes = run_check(&RedefinedBuiltin, src);
        assert!(
            !codes.contains(&"DJ059".to_string()),
            "unexpected DJ059 for cls parameter, got: {:?}",
            codes
        );
    }

    // ── DJ061: BroadExceptionCaught ───────────────────────────────────────

    #[test]
    fn dj061_flags_bare_exception_catch() {
        let src = r#"
try:
    pass
except Exception:
    pass
"#;
        let codes = run_check(&BroadExceptionCaught, src);
        assert!(
            codes.contains(&"DJ061".to_string()),
            "expected DJ061 for 'except Exception', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj061_flags_exception_with_alias() {
        let src = r#"
try:
    pass
except Exception as e:
    pass
"#;
        let codes = run_check(&BroadExceptionCaught, src);
        assert!(
            codes.contains(&"DJ061".to_string()),
            "expected DJ061 for 'except Exception as e', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj061_no_flag_for_specific_exception() {
        let src = r#"
try:
    pass
except ValueError:
    pass
"#;
        let codes = run_check(&BroadExceptionCaught, src);
        assert!(
            !codes.contains(&"DJ061".to_string()),
            "unexpected DJ061 for specific exception, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj061_no_flag_for_bare_except() {
        let src = r#"
try:
    pass
except:
    pass
"#;
        let codes = run_check(&BroadExceptionCaught, src);
        assert!(
            !codes.contains(&"DJ061".to_string()),
            "unexpected DJ061 for bare except, got: {:?}",
            codes
        );
    }

    // ── DJ063: ForgottenDebugStatement ───────────────────────────────────

    #[test]
    fn dj063_flags_builtin_breakpoint() {
        let src = r#"
breakpoint()
"#;
        let codes = run_check(&ForgottenDebugStatement, src);
        assert!(
            codes.contains(&"DJ063".to_string()),
            "expected DJ063 for breakpoint(), got: {:?}",
            codes
        );
    }

    #[test]
    fn dj063_flags_set_trace() {
        let src = r#"
import pdb
pdb.set_trace()
"#;
        let codes = run_check(&ForgottenDebugStatement, src);
        assert!(
            codes.contains(&"DJ063".to_string()),
            "expected DJ063 for pdb.set_trace(), got: {:?}",
            codes
        );
    }

    #[test]
    fn dj063_flags_embed() {
        let src = r#"
import IPython
IPython.embed()
"#;
        let codes = run_check(&ForgottenDebugStatement, src);
        assert!(
            codes.contains(&"DJ063".to_string()),
            "expected DJ063 for IPython.embed(), got: {:?}",
            codes
        );
    }

    #[test]
    fn dj063_flags_interact() {
        let src = r#"
import code
code.interact()
"#;
        let codes = run_check(&ForgottenDebugStatement, src);
        assert!(
            codes.contains(&"DJ063".to_string()),
            "expected DJ063 for code.interact(), got: {:?}",
            codes
        );
    }

    #[test]
    fn dj063_no_flag_for_ordinary_call() {
        let src = r#"
result = my_function()
"#;
        let codes = run_check(&ForgottenDebugStatement, src);
        assert!(
            !codes.contains(&"DJ063".to_string()),
            "unexpected DJ063 for ordinary call, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj063_no_flag_for_ordinary_call_no_dj053() {
        let src = r#"
my_function()
"#;
        let codes = run_check(&ForgottenDebugStatement, src);
        assert!(
            !codes.contains(&"DJ063".to_string()),
            "unexpected DJ063 for ordinary call, got: {:?}",
            codes
        );
    }

    // ── DJ065: SingletonComparison ────────────────────────────────────────

    #[test]
    fn dj065_flags_eq_none() {
        let src = r#"
if x == None:
    pass
"#;
        let codes = run_check(&SingletonComparison, src);
        assert!(
            codes.contains(&"DJ065".to_string()),
            "expected DJ065 for '== None', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj065_flags_neq_none() {
        let src = r#"
if x != None:
    pass
"#;
        let codes = run_check(&SingletonComparison, src);
        assert!(
            codes.contains(&"DJ065".to_string()),
            "expected DJ065 for '!= None', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj065_flags_eq_true() {
        let src = r#"
if x == True:
    pass
"#;
        let codes = run_check(&SingletonComparison, src);
        assert!(
            codes.contains(&"DJ065".to_string()),
            "expected DJ065 for '== True', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj065_flags_eq_false() {
        let src = r#"
if x == False:
    pass
"#;
        let codes = run_check(&SingletonComparison, src);
        assert!(
            codes.contains(&"DJ065".to_string()),
            "expected DJ065 for '== False', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj065_no_flag_for_is_none() {
        let src = r#"
if x is None:
    pass
"#;
        let codes = run_check(&SingletonComparison, src);
        assert!(
            !codes.contains(&"DJ065".to_string()),
            "unexpected DJ065 for 'is None', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj065_no_flag_for_is_not_none() {
        let src = r#"
if x is not None:
    pass
"#;
        let codes = run_check(&SingletonComparison, src);
        assert!(
            !codes.contains(&"DJ065".to_string()),
            "unexpected DJ065 for 'is not None', got: {:?}",
            codes
        );
    }

    #[test]
    fn dj065_no_flag_for_ordinary_equality() {
        let src = r#"
if x == 42:
    pass
"#;
        let codes = run_check(&SingletonComparison, src);
        assert!(
            !codes.contains(&"DJ065".to_string()),
            "unexpected DJ065 for ordinary integer equality, got: {:?}",
            codes
        );
    }

    // ── DJ067: SuperWithArguments ─────────────────────────────────────────

    #[test]
    fn dj067_flags_super_with_class_and_self() {
        let src = r#"
class MyView:
    def dispatch(self, request):
        return super(MyView, self).dispatch(request)
"#;
        let codes = run_check(&SuperWithArguments, src);
        assert!(
            codes.contains(&"DJ067".to_string()),
            "expected DJ067 for super(MyView, self), got: {:?}",
            codes
        );
    }

    #[test]
    fn dj067_flags_super_with_class_and_cls() {
        let src = r#"
class MyMeta:
    @classmethod
    def create(cls):
        return super(MyMeta, cls).create()
"#;
        let codes = run_check(&SuperWithArguments, src);
        assert!(
            codes.contains(&"DJ067".to_string()),
            "expected DJ067 for super(MyMeta, cls), got: {:?}",
            codes
        );
    }

    #[test]
    fn dj067_no_flag_for_super_without_args() {
        let src = r#"
class MyView:
    def dispatch(self, request):
        return super().dispatch(request)
"#;
        let codes = run_check(&SuperWithArguments, src);
        assert!(
            !codes.contains(&"DJ067".to_string()),
            "unexpected DJ067 for super(), got: {:?}",
            codes
        );
    }

    #[test]
    fn dj067_no_flag_for_super_with_wrong_class() {
        let src = r#"
class MyView:
    def dispatch(self, request):
        return super(OtherClass, self).dispatch(request)
"#;
        let codes = run_check(&SuperWithArguments, src);
        assert!(
            !codes.contains(&"DJ067".to_string()),
            "unexpected DJ067 for super(OtherClass, self) inside MyView, got: {:?}",
            codes
        );
    }

    #[test]
    fn dj067_no_flag_for_super_outside_class() {
        let src = r#"
result = super(Foo, self)
"#;
        let codes = run_check(&SuperWithArguments, src);
        assert!(
            !codes.contains(&"DJ067".to_string()),
            "unexpected DJ067 for super outside class, got: {:?}",
            codes
        );
    }
}
