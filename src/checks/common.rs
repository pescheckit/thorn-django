// ── Shared helpers, constants, and test utilities ─────────────────────────
//
// All shared code lives here: the canonical PYTHON_BUILTINS list, the
// `is_builtin` helper, Django-specific AST predicates, and the `run_check`
// test helper.

use thorn_api::ast::*;

// ── Python built-in detection ────────────────────────────────────────────
//
// Instead of maintaining an exhaustive static list, we detect builtins by
// combining a small set of core names with pattern-based recognition for
// exception/warning classes (which follow strict PascalCase naming).

/// Core Python built-in functions and constants. These are the names
/// available in every Python scope without any import.
const BUILTIN_FUNCTIONS_AND_CONSTANTS: &[&str] = &[
    // constants
    "True", "False", "None", "NotImplemented", "Ellipsis",
    // type constructors
    "bool", "int", "float", "complex", "str", "bytes", "bytearray",
    "list", "tuple", "set", "frozenset", "dict", "type", "object",
    "memoryview", "slice", "property", "classmethod", "staticmethod",
    // functions
    "print", "len", "range", "enumerate", "zip", "map", "filter",
    "sorted", "reversed", "iter", "next", "super", "isinstance",
    "issubclass", "hasattr", "getattr", "setattr", "delattr",
    "callable", "open", "input", "id", "hash", "repr", "abs",
    "round", "min", "max", "sum", "all", "any", "pow", "divmod",
    "hex", "oct", "bin", "ord", "chr", "vars", "dir", "locals",
    "globals", "exec", "eval", "compile", "breakpoint", "format",
];

/// Return `true` when `name` looks like a Python built-in.
///
/// This combines:
/// - Exact match for core functions/constants
/// - `self`/`cls` (method parameter conventions)
/// - Dunder names (`__name__`, `__all__`, etc.)
/// - PascalCase names ending in `Error`, `Warning`, or `Exception`
///   (covers all stdlib exception/warning classes generically)
pub(crate) fn is_builtin(name: &str) -> bool {
    // Method parameter conventions — never flag these
    if name == "self" || name == "cls" {
        return true;
    }
    // Dunder names are always built-in or special
    if name.starts_with("__") && name.ends_with("__") {
        return true;
    }
    // Exception & warning class naming patterns (PascalCase + suffix)
    if is_exception_or_warning_name(name) {
        return true;
    }
    BUILTIN_FUNCTIONS_AND_CONSTANTS.contains(&name)
}

/// Return `true` when `name` follows the Python exception/warning naming
/// pattern: PascalCase ending in `Error`, `Warning`, `Exception`, `Exit`,
/// or `Interrupt`. This generically covers all stdlib and most third-party
/// exception classes without needing an exhaustive list.
fn is_exception_or_warning_name(name: &str) -> bool {
    if name.is_empty() || !name.as_bytes()[0].is_ascii_uppercase() {
        return false;
    }
    name.ends_with("Error")
        || name.ends_with("Warning")
        || name.ends_with("Exception")
        || name == "SystemExit"
        || name == "KeyboardInterrupt"
        || name == "GeneratorExit"
        || name == "StopIteration"
        || name == "StopAsyncIteration"
}

/// Convert a thorn `ByteRange` to itself (identity function kept for
/// source compatibility with call sites that used the old ruff adapter).
#[inline]
pub(crate) fn text_range(r: thorn_api::ByteRange) -> thorn_api::ByteRange {
    r
}

// ── Shared AST predicates ─────────────────────────────────────────────────

/// Return `true` if `expr` is a mutable default value:
///   - a list/dict/set literal, or
///   - a no-arg call to a mutable-container constructor.
///
/// Detection is generic: any call to `list()`, `dict()`, `set()`,
/// `bytearray()` (built-in mutable types) or to names containing "dict",
/// "list", "set", "deque", "counter", "array" (catches `defaultdict()`,
/// `OrderedDict()`, `Counter()`, `deque()`, etc.) without needing an
/// exhaustive list.
pub(crate) fn is_mutable_default(expr: &Expr) -> bool {
    if matches!(expr, Expr::List(_) | Expr::Dict(_) | Expr::Set(_)) {
        return true;
    }
    if let Expr::Call(call) = expr {
        let name = match call.func.as_ref() {
            Expr::Name(n) => n.id.as_str(),
            Expr::Attribute(a) => a.attr.as_str(),
            _ => return false,
        };
        let lower = name.to_ascii_lowercase();
        // Match built-in mutable types and common container names generically
        if lower == "list" || lower == "dict" || lower == "set" || lower == "bytearray"
            || lower.contains("dict") || lower.contains("deque")
            || lower.contains("counter") || lower.contains("array")
        {
            return true;
        }
    }
    false
}

/// Return `true` when the `if` statement's test is a bare `TYPE_CHECKING`
/// reference or `typing.TYPE_CHECKING`.
pub(crate) fn is_type_checking_if(if_stmt: &StmtIf) -> bool {
    match &if_stmt.test.as_ref() {
        Expr::Name(n) => n.id.as_str() == "TYPE_CHECKING",
        Expr::Attribute(a) => a.attr.as_str() == "TYPE_CHECKING",
        _ => false,
    }
}

/// Compute the 1-based line number of a byte offset inside `source`.
pub(crate) fn offset_to_line(source: &str, offset: usize) -> u32 {
    let capped = offset.min(source.len());
    source[..capped].chars().filter(|c| *c == '\n').count() as u32 + 1
}

/// Compute a 1-based line number for the given byte offset in `source`.
pub(crate) fn line_of_offset(source: &str, offset: u32) -> u32 {
    offset_to_line(source, offset as usize)
}

// ── Django-specific helpers (used by ast.rs) ──────────────────────────────

pub(crate) fn is_django_model(class: &StmtClassDef) -> bool {
    class.arguments.as_ref().is_some_and(|args| {
        args.args.iter().any(|base| match base {
            Expr::Attribute(a) => a.attr.as_str() == "Model",
            Expr::Name(n) => n.id.as_str() == "Model",
            _ => false,
        })
    })
}

pub(crate) fn is_model_form(class: &StmtClassDef) -> bool {
    class.arguments.as_ref().is_some_and(|args| {
        args.args.iter().any(|base| match base {
            Expr::Name(n) => n.id.as_str() == "ModelForm",
            Expr::Attribute(a) => a.attr.as_str() == "ModelForm",
            _ => false,
        })
    })
}

pub(crate) fn has_null_true(arguments: &Arguments) -> bool {
    arguments.keywords.iter().any(|kw| {
        kw.arg.as_ref().is_some_and(|a| a.as_str() == "null")
            && matches!(&kw.value, Expr::BooleanLiteral(b) if b.value)
    })
}

pub(crate) fn has_unique_true(arguments: &Arguments) -> bool {
    arguments.keywords.iter().any(|kw| {
        kw.arg.as_ref().is_some_and(|a| a.as_str() == "unique")
            && matches!(&kw.value, Expr::BooleanLiteral(b) if b.value)
    })
}

// ── Test helpers ──────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_helpers {
    use thorn_api::{AppGraph, AstCheck, CheckContext};

    /// Parse `source` as a Python module, run `check` against it with
    /// filename `"test.py"`, and return the list of diagnostic codes produced.
    pub fn run_check(check: &dyn AstCheck, source: &str) -> Vec<String> {
        run_check_filename(check, source, "test.py")
    }

    /// Same as `run_check` but with a caller-supplied `filename`, enabling
    /// tests of filename-based skip rules.
    pub fn run_check_filename(check: &dyn AstCheck, source: &str, filename: &str) -> Vec<String> {
        let module = thorn_core::parser::parse_python(source).unwrap();
        let graph = AppGraph::default();
        let ctx = CheckContext::new(&module, source, filename, &graph);
        check.check(&ctx).into_iter().map(|d| d.code).collect()
    }
}
