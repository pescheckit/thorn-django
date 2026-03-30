// ── DJ051-DJ062: Python correctness checks ────────────────────────────────
//
// These checks detect common Python mistakes that are language-level bugs:
// dangerous defaults, redefined functions, duplicate dict keys, assert on
// tuple, bare except, and raised NotImplemented.

use thorn_api::ast::*;
use thorn_api::visitor::{walk_expr, walk_stmt, Visitor};
use thorn_api::{AstCheck, CheckContext, Diagnostic, Level};

use super::common::{is_mutable_default, line_of_offset, text_range};

// ── DJ051: DangerousDefaultValue ─────────────────────────────────────────

pub struct DangerousDefaultValue;

impl AstCheck for DangerousDefaultValue {
    fn code(&self) -> &'static str {
        "DJ051"
    }

    fn level(&self) -> Level {
        Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = DangerousDefaultVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct DangerousDefaultVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor for DangerousDefaultVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(func) = stmt {
            for param in func
                .parameters
                .posonlyargs
                .iter()
                .chain(func.parameters.args.iter())
                .chain(func.parameters.kwonlyargs.iter())
            {
                if let Some(default) = &param.default {
                    if is_mutable_default(default) {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ051",
                                "Dangerous default value (mutable). Mutable defaults are shared between calls — use None and assign inside the function.",
                                self.filename,
                            )
                            .with_range(text_range(default.range())),
                        );
                    }
                }
            }
        }
        walk_stmt(self, stmt);
    }
}

// ── DJ052: FunctionRedefined ──────────────────────────────────────────────

pub struct FunctionRedefined;

impl AstCheck for FunctionRedefined {
    fn code(&self) -> &'static str {
        "DJ052"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        check_scope_for_redefined(&ctx.module.body, ctx.source, ctx.filename, &mut diags);
        diags
    }
}

/// Walk a scope body (module or class) and flag any function/class whose name
/// was already defined earlier in the same scope.
fn check_scope_for_redefined(
    body: &[Stmt],
    source: &str,
    filename: &str,
    diags: &mut Vec<Diagnostic>,
) {
    let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for stmt in body {
        match stmt {
            Stmt::FunctionDef(func) => {
                let name = &func.name;
                if has_overload_decorator(&func.decorator_list) {
                    check_scope_for_redefined(&func.body, source, filename, diags);
                    continue;
                }
                let line = line_of_offset(source, func.range().start);
                if let Some(first_line) = seen.get(name) {
                    diags.push(
                        Diagnostic::new(
                            "DJ052",
                            format!("'{name}' already defined at line {first_line}."),
                            filename,
                        )
                        .with_range(text_range(func.range())),
                    );
                } else {
                    seen.insert(name.to_string(), line);
                }
                check_scope_for_redefined(&func.body, source, filename, diags);
            }
            Stmt::ClassDef(cls) => {
                let name = &cls.name;
                let line = line_of_offset(source, cls.range().start);
                if let Some(first_line) = seen.get(name) {
                    diags.push(
                        Diagnostic::new(
                            "DJ052",
                            format!("'{name}' already defined at line {first_line}."),
                            filename,
                        )
                        .with_range(text_range(cls.range())),
                    );
                } else {
                    seen.insert(name.to_string(), line);
                }
                check_scope_for_redefined(&cls.body, source, filename, diags);
            }
            _ => {}
        }
    }
}

/// Return true if the decorator list contains @overload, @typing.overload,
/// @singledispatch, or @singledispatchmethod.
fn has_overload_decorator(decorator_list: &[Expr]) -> bool {
    decorator_list.iter().any(|dec| match dec {
        Expr::Name(n) => {
            matches!(
                n.id.as_str(),
                "overload" | "singledispatch" | "singledispatchmethod"
            )
        }
        Expr::Attribute(a) => {
            matches!(
                a.attr.as_str(),
                "overload" | "singledispatch" | "singledispatchmethod"
            )
        }
        _ => false,
    })
}

// ── DJ054: DuplicateDictKey ───────────────────────────────────────────────

pub struct DuplicateDictKey;

impl AstCheck for DuplicateDictKey {
    fn code(&self) -> &'static str {
        "DJ054"
    }

    fn level(&self) -> Level {
        Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = DuplicateDictKeyVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct DuplicateDictKeyVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

/// A normalised string representation of a constant key used for duplicate
/// detection. Only constants (str, int, float, bool, None) are compared.
fn const_key_repr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::StringLiteral(s) => Some(format!("str:{}", s.value)),
        Expr::NumberLiteral(n) => Some(format!("num:{:?}", n.value)),
        Expr::BooleanLiteral(b) => Some(format!("bool:{}", b.value)),
        Expr::NoneLiteral(_) => Some("none:None".to_string()),
        _ => None,
    }
}

impl<'a> Visitor for DuplicateDictKeyVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Dict(dict) = expr {
            let mut seen: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
            for item in &dict.items {
                if let Some(key_expr) = &item.key {
                    if let Some(key_repr) = const_key_repr(key_expr) {
                        let key_display = match key_expr {
                            Expr::StringLiteral(s) => format!("'{}'", s.value),
                            Expr::NumberLiteral(n) => format!("{:?}", n.value),
                            Expr::BooleanLiteral(b) => {
                                if b.value {
                                    "True".to_string()
                                } else {
                                    "False".to_string()
                                }
                            }
                            Expr::NoneLiteral(_) => "None".to_string(),
                            _ => key_repr.clone(),
                        };
                        if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(key_repr) {
                            e.insert(());
                        } else {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ054",
                                    format!("Duplicate key {key_display} in dictionary."),
                                    self.filename,
                                )
                                .with_range(text_range(key_expr.range())),
                            );
                        }
                    }
                }
            }
        }
        walk_expr(self, expr);
    }
}

// ── DJ057: AssertOnTuple ──────────────────────────────────────────────────

pub struct AssertOnTuple;

impl AstCheck for AssertOnTuple {
    fn code(&self) -> &'static str {
        "DJ057"
    }

    fn level(&self) -> Level {
        Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = AssertOnTupleVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct AssertOnTupleVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor for AssertOnTupleVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::Assert(assert_stmt) = stmt {
            if let Expr::Tuple(tup) = assert_stmt.test.as_ref() {
                if !tup.elts.is_empty() {
                    self.diags.push(
                        Diagnostic::new(
                            "DJ057",
                            "Assert on non-empty tuple is always True. Did you mean 'assert condition, message'?",
                            self.filename,
                        )
                        .with_range(text_range(assert_stmt.range())),
                    );
                }
            }
        }
        walk_stmt(self, stmt);
    }
}

// ── DJ060: BareExcept ─────────────────────────────────────────────────────

pub struct BareExcept;

impl AstCheck for BareExcept {
    fn code(&self) -> &'static str {
        "DJ060"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = BareExceptVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct BareExceptVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor for BareExceptVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::Try(try_stmt) = stmt {
            for handler in &try_stmt.handlers {
                if handler.type_.is_none() {
                    self.diags.push(
                        Diagnostic::new(
                            "DJ060",
                            "Bare 'except:' catches BaseException including SystemExit and KeyboardInterrupt. Use 'except Exception:' instead.",
                            self.filename,
                        )
                        .with_range(text_range(handler.range())),
                    );
                }
            }
        }
        walk_stmt(self, stmt);
    }
}

// ── DJ062: NotImplementedRaised ───────────────────────────────────────────

pub struct NotImplementedRaised;

impl AstCheck for NotImplementedRaised {
    fn code(&self) -> &'static str {
        "DJ062"
    }

    fn level(&self) -> Level {
        Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = NotImplementedRaisedVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct NotImplementedRaisedVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor for NotImplementedRaisedVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::Raise(raise_stmt) = stmt {
            if let Some(exc) = &raise_stmt.exc {
                if let Expr::Name(name) = exc.as_ref() {
                    if name.id.as_str() == "NotImplemented" {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ062",
                                "Raise NotImplementedError, not NotImplemented. NotImplemented is for binary operator fallback, not exceptions.",
                                self.filename,
                            )
                            .with_range(text_range(raise_stmt.range())),
                        );
                    }
                }
            }
        }
        walk_stmt(self, stmt);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::common::test_helpers::run_check;
    use super::*;

    // ── DJ051: DangerousDefaultValue ──────────────────────────────────────

    #[test]
    fn dj051_triggers_on_list_default() {
        let src = "def foo(x=[]):\n    pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            codes.contains(&"DJ051".to_string()),
            "expected DJ051, got {codes:?}"
        );
    }

    #[test]
    fn dj051_triggers_on_dict_default() {
        let src = "def foo(x={}):\n    pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            codes.contains(&"DJ051".to_string()),
            "expected DJ051, got {codes:?}"
        );
    }

    #[test]
    fn dj051_triggers_on_set_default() {
        let src = "def foo(x=set()):\n    pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            codes.contains(&"DJ051".to_string()),
            "expected DJ051, got {codes:?}"
        );
    }

    #[test]
    fn dj051_triggers_on_list_call() {
        let src = "def foo(x=list()):\n    pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            codes.contains(&"DJ051".to_string()),
            "expected DJ051, got {codes:?}"
        );
    }

    #[test]
    fn dj051_triggers_on_dict_call() {
        let src = "def foo(x=dict()):\n    pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            codes.contains(&"DJ051".to_string()),
            "expected DJ051, got {codes:?}"
        );
    }

    #[test]
    fn dj051_triggers_on_defaultdict() {
        let src = "from collections import defaultdict\ndef foo(x=defaultdict(list)):\n    pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            codes.contains(&"DJ051".to_string()),
            "expected DJ051, got {codes:?}"
        );
    }

    #[test]
    fn dj051_no_trigger_on_none_default() {
        let src = "def foo(x=None):\n    pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            !codes.contains(&"DJ051".to_string()),
            "unexpected DJ051, got {codes:?}"
        );
    }

    #[test]
    fn dj051_no_trigger_on_immutable_default() {
        let src = "def foo(x=42, y='hello', z=()):\n    pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            !codes.contains(&"DJ051".to_string()),
            "unexpected DJ051, got {codes:?}"
        );
    }

    #[test]
    fn dj051_triggers_on_kwonly_list() {
        let src = "def foo(*, opts=[]):\n    pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            codes.contains(&"DJ051".to_string()),
            "expected DJ051, got {codes:?}"
        );
    }

    #[test]
    fn dj051_triggers_inside_method() {
        let src = "class Foo:\n    def bar(self, x={}):\n        pass\n";
        let codes = run_check(&DangerousDefaultValue, src);
        assert!(
            codes.contains(&"DJ051".to_string()),
            "expected DJ051, got {codes:?}"
        );
    }

    // ── DJ052: FunctionRedefined ───────────────────────────────────────────

    #[test]
    fn dj052_triggers_on_redefined_function() {
        let src = "def foo():\n    pass\n\ndef foo():\n    pass\n";
        let codes = run_check(&FunctionRedefined, src);
        assert!(
            codes.contains(&"DJ052".to_string()),
            "expected DJ052, got {codes:?}"
        );
    }

    #[test]
    fn dj052_no_trigger_on_unique_functions() {
        let src = "def foo():\n    pass\n\ndef bar():\n    pass\n";
        let codes = run_check(&FunctionRedefined, src);
        assert!(
            !codes.contains(&"DJ052".to_string()),
            "unexpected DJ052, got {codes:?}"
        );
    }

    #[test]
    fn dj052_no_trigger_on_overload() {
        let src = r#"
from typing import overload

@overload
def process(x: int) -> int: ...

@overload
def process(x: str) -> str: ...

def process(x):
    return x
"#;
        let codes = run_check(&FunctionRedefined, src);
        assert!(
            !codes.contains(&"DJ052".to_string()),
            "unexpected DJ052, got {codes:?}"
        );
    }

    #[test]
    fn dj052_no_trigger_on_typing_overload() {
        let src = r#"
import typing

@typing.overload
def process(x: int) -> int: ...

@typing.overload
def process(x: str) -> str: ...

def process(x):
    return x
"#;
        let codes = run_check(&FunctionRedefined, src);
        assert!(
            !codes.contains(&"DJ052".to_string()),
            "unexpected DJ052, got {codes:?}"
        );
    }

    #[test]
    fn dj052_triggers_class_redefined() {
        let src = "class Foo:\n    pass\n\nclass Foo:\n    pass\n";
        let codes = run_check(&FunctionRedefined, src);
        assert!(
            codes.contains(&"DJ052".to_string()),
            "expected DJ052, got {codes:?}"
        );
    }

    #[test]
    fn dj052_no_trigger_same_name_different_scopes() {
        let src = "def helper():\n    pass\n\nclass Foo:\n    def helper(self):\n        pass\n";
        let codes = run_check(&FunctionRedefined, src);
        assert!(
            !codes.contains(&"DJ052".to_string()),
            "unexpected DJ052, got {codes:?}"
        );
    }

    // ── DJ054: DuplicateDictKey ────────────────────────────────────────────

    #[test]
    fn dj054_triggers_on_duplicate_string_key() {
        let src = "d = {'a': 1, 'b': 2, 'a': 3}\n";
        let codes = run_check(&DuplicateDictKey, src);
        assert!(
            codes.contains(&"DJ054".to_string()),
            "expected DJ054, got {codes:?}"
        );
    }

    #[test]
    fn dj054_triggers_on_duplicate_int_key() {
        let src = "d = {1: 'a', 2: 'b', 1: 'c'}\n";
        let codes = run_check(&DuplicateDictKey, src);
        assert!(
            codes.contains(&"DJ054".to_string()),
            "expected DJ054, got {codes:?}"
        );
    }

    #[test]
    fn dj054_no_trigger_on_unique_keys() {
        let src = "d = {'a': 1, 'b': 2, 'c': 3}\n";
        let codes = run_check(&DuplicateDictKey, src);
        assert!(
            !codes.contains(&"DJ054".to_string()),
            "unexpected DJ054, got {codes:?}"
        );
    }

    #[test]
    fn dj054_no_trigger_on_variable_keys() {
        let src = "d = {k1: 1, k2: 2}\n";
        let codes = run_check(&DuplicateDictKey, src);
        assert!(
            !codes.contains(&"DJ054".to_string()),
            "unexpected DJ054, got {codes:?}"
        );
    }

    #[test]
    fn dj054_triggers_on_duplicate_none_key() {
        let src = "d = {None: 1, None: 2}\n";
        let codes = run_check(&DuplicateDictKey, src);
        assert!(
            codes.contains(&"DJ054".to_string()),
            "expected DJ054, got {codes:?}"
        );
    }

    // ── DJ057: AssertOnTuple ───────────────────────────────────────────────

    #[test]
    fn dj057_triggers_on_non_empty_tuple() {
        let src = "assert (x == 1, 'message')\n";
        let codes = run_check(&AssertOnTuple, src);
        assert!(
            codes.contains(&"DJ057".to_string()),
            "expected DJ057, got {codes:?}"
        );
    }

    #[test]
    fn dj057_triggers_on_single_element_tuple() {
        let src = "assert (False,)\n";
        let codes = run_check(&AssertOnTuple, src);
        assert!(
            codes.contains(&"DJ057".to_string()),
            "expected DJ057, got {codes:?}"
        );
    }

    #[test]
    fn dj057_no_trigger_on_plain_assert() {
        let src = "assert x == 1, 'message'\n";
        let codes = run_check(&AssertOnTuple, src);
        assert!(
            !codes.contains(&"DJ057".to_string()),
            "unexpected DJ057, got {codes:?}"
        );
    }

    #[test]
    fn dj057_no_trigger_on_empty_tuple() {
        let src = "assert ()\n";
        let codes = run_check(&AssertOnTuple, src);
        assert!(
            !codes.contains(&"DJ057".to_string()),
            "unexpected DJ057, got {codes:?}"
        );
    }

    // ── DJ060: BareExcept ─────────────────────────────────────────────────

    #[test]
    fn dj060_triggers_on_bare_except() {
        let src = "try:\n    pass\nexcept:\n    pass\n";
        let codes = run_check(&BareExcept, src);
        assert!(
            codes.contains(&"DJ060".to_string()),
            "expected DJ060, got {codes:?}"
        );
    }

    #[test]
    fn dj060_no_trigger_on_typed_except() {
        let src = "try:\n    pass\nexcept Exception:\n    pass\n";
        let codes = run_check(&BareExcept, src);
        assert!(
            !codes.contains(&"DJ060".to_string()),
            "unexpected DJ060, got {codes:?}"
        );
    }

    #[test]
    fn dj060_no_trigger_on_specific_except() {
        let src = "try:\n    pass\nexcept ValueError:\n    pass\n";
        let codes = run_check(&BareExcept, src);
        assert!(
            !codes.contains(&"DJ060".to_string()),
            "unexpected DJ060, got {codes:?}"
        );
    }

    #[test]
    fn dj060_triggers_bare_among_typed() {
        let src = "try:\n    pass\nexcept ValueError:\n    pass\nexcept:\n    pass\n";
        let codes = run_check(&BareExcept, src);
        assert!(
            codes.contains(&"DJ060".to_string()),
            "expected DJ060, got {codes:?}"
        );
    }

    // ── DJ062: NotImplementedRaised ───────────────────────────────────────

    #[test]
    fn dj062_triggers_on_raise_not_implemented() {
        let src = "def foo():\n    raise NotImplemented\n";
        let codes = run_check(&NotImplementedRaised, src);
        assert!(
            codes.contains(&"DJ062".to_string()),
            "expected DJ062, got {codes:?}"
        );
    }

    #[test]
    fn dj062_no_trigger_on_raise_not_implemented_error() {
        let src = "def foo():\n    raise NotImplementedError\n";
        let codes = run_check(&NotImplementedRaised, src);
        assert!(
            !codes.contains(&"DJ062".to_string()),
            "unexpected DJ062, got {codes:?}"
        );
    }

    #[test]
    fn dj062_no_trigger_on_raise_value_error() {
        let src = "def foo():\n    raise ValueError('bad')\n";
        let codes = run_check(&NotImplementedRaised, src);
        assert!(
            !codes.contains(&"DJ062".to_string()),
            "unexpected DJ062, got {codes:?}"
        );
    }

    #[test]
    fn dj062_triggers_inside_method() {
        let src = "class Foo:\n    def bar(self):\n        raise NotImplemented\n";
        let codes = run_check(&NotImplementedRaised, src);
        assert!(
            codes.contains(&"DJ062".to_string()),
            "expected DJ062, got {codes:?}"
        );
    }
}
