// ── DJ055-DJ066: Best practice checks ────────────────────────────────────
//
// Checks for best practices: unreachable code, unused imports, unused
// variables, redefined outer names, and comparison with callables.

use thorn_api::visitor::{Visitor, walk_expr, walk_stmt};
use thorn_api::ast::*;
use std::collections::{HashMap, HashSet};
use thorn_api::{AstCheck, CheckContext, Diagnostic, Level};

use super::common::{is_type_checking_if, offset_to_line, text_range};

// ── DJ064: UnreachableCode ────────────────────────────────────────────────

pub struct UnreachableCode;

impl AstCheck for UnreachableCode {
    fn code(&self) -> &'static str {
        "DJ064"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = UnreachableCodeVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct UnreachableCodeVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

fn is_terminator(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Return(_) | Stmt::Raise(_) | Stmt::Break(_) | Stmt::Continue(_)
    )
}

impl<'a> Visitor for UnreachableCodeVisitor<'a> {
    fn visit_body(&mut self, body: &[Stmt]) {
        for i in 0..body.len() {
            if is_terminator(&body[i]) {
                if let Some(next) = body.get(i + 1) {
                    self.diags.push(
                        Diagnostic::new("DJ064", "Unreachable code.", self.filename)
                            .with_range(text_range(next.range())),
                    );
                }
                break;
            }
        }

        for stmt in body {
            match stmt {
                Stmt::If(s) if is_type_checking_if(s) => {}

                Stmt::Try(s) => {
                    self.visit_body(&s.body);
                    for h in &s.handlers {
                        self.visit_body(&h.body);
                    }
                    self.visit_body(&s.orelse);
                    for fs in &s.finalbody {
                        walk_stmt(self, fs);
                    }
                }

                other => walk_stmt(self, other),
            }
        }
    }
}

// ── DJ055: UnusedImport ───────────────────────────────────────────────────
//
// Fix: respect `# noqa` comments on import lines. If the source line for
// an import contains a `noqa` annotation, the import is silently skipped.

pub struct UnusedImport;

impl AstCheck for UnusedImport {
    fn code(&self) -> &'static str {
        "DJ055"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Skip __init__.py — imports there are often re-exports.
        if ctx.filename.ends_with("__init__.py") || ctx.filename.ends_with("__init__") {
            return vec![];
        }

        // ── Pass 1: collect imported names ────────────────────────────────
        // Each entry: (local_name, range, source_line_has_noqa)
        let mut imported: Vec<(String, thorn_api::ByteRange, bool)> = vec![];
        let mut all_names: HashSet<String> = HashSet::new();

        collect_imports_at_module_level(
            &ctx.module.body,
            ctx.source,
            &mut imported,
            &mut all_names,
            false,
        );

        if imported.is_empty() {
            return vec![];
        }

        // ── Pass 2: collect all Name references ───────────────────────────
        let mut ref_collector = NameRefCollector {
            refs: HashSet::new(),
        };
        ref_collector.visit_body(&ctx.module.body);
        let refs = ref_collector.refs;

        // ── Pass 3: diff ──────────────────────────────────────────────────
        let mut diags = vec![];
        for (name, range, has_noqa) in imported {
            if name.starts_with('_') {
                continue;
            }
            if all_names.contains(&name) {
                continue;
            }
            if has_noqa {
                continue;
            }
            if !refs.contains(&name) {
                diags.push(
                    Diagnostic::new(
                        "DJ055",
                        format!("Unused import '{name}'."),
                        ctx.filename,
                    )
                    .with_range(text_range(range)),
                );
            }
        }
        diags
    }
}

/// Return `true` if the line at `offset` in `source` contains a `# noqa`
/// annotation (case-insensitive).
fn line_has_noqa(source: &str, offset: u32) -> bool {
    let byte_offset = offset as usize;
    let start = source[..byte_offset].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let end = source[byte_offset..]
        .find('\n')
        .map(|p| byte_offset + p)
        .unwrap_or(source.len());
    let line = &source[start..end];
    // A noqa comment is `# noqa` optionally followed by `:CODE`, ` CODE`, etc.
    // We match the substring case-insensitively.
    line.to_ascii_lowercase().contains("# noqa")
}

/// Recursively collect module-level imports, honouring TYPE_CHECKING blocks
/// (those are skipped) and __future__ imports (also skipped).
fn collect_imports_at_module_level(
    body: &[Stmt],
    source: &str,
    out: &mut Vec<(String, thorn_api::ByteRange, bool)>,
    all_names: &mut HashSet<String>,
    inside_type_checking: bool,
) {
    for stmt in body {
        match stmt {
            Stmt::Import(s) if !inside_type_checking => {
                let noqa = line_has_noqa(source, s.range().start);
                for alias in &s.names {
                    let local = alias
                        .asname
                        .as_ref()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| {
                            alias.name.split('.').next().unwrap_or("").to_string()
                        });
                    out.push((local, s.range(), noqa));
                }
            }
            Stmt::ImportFrom(s) if !inside_type_checking => {
                if s.module.as_deref() == Some("__future__") {
                    continue;
                }
                let noqa = line_has_noqa(source, s.range().start);
                for alias in &s.names {
                    if alias.name.as_str() == "*" {
                        continue;
                    }
                    let local = alias
                        .asname
                        .as_ref()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| alias.name.to_string());
                    out.push((local, s.range(), noqa));
                }
            }
            Stmt::Assign(s) => {
                for target in &s.targets {
                    if let Expr::Name(n) = target {
                        if n.id.as_str() == "__all__" {
                            collect_string_literals_from_expr(&s.value, all_names);
                        }
                    }
                }
            }
            Stmt::If(s) => {
                let in_tc = is_type_checking_if(s);
                collect_imports_at_module_level(&s.body, source, out, all_names, in_tc);
                for clause in &s.elif_else_clauses {
                    collect_imports_at_module_level(
                        &clause.body,
                        source,
                        out,
                        all_names,
                        false,
                    );
                }
            }
            _ => {}
        }
    }
}

fn collect_string_literals_from_expr(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::List(l) => {
            for elt in &l.elts {
                collect_string_literals_from_expr(elt, out);
            }
        }
        Expr::Tuple(t) => {
            for elt in &t.elts {
                collect_string_literals_from_expr(elt, out);
            }
        }
        Expr::StringLiteral(s) => {
            out.insert(s.value.clone());
        }
        _ => {}
    }
}

/// Visitor that collects every `Expr::Name` identifier used anywhere in the tree.
struct NameRefCollector {
    refs: HashSet<String>,
}

impl Visitor for NameRefCollector {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Name(n) = expr {
            self.refs.insert(n.id.to_string());
        }
        walk_expr(self, expr);
    }
}

/// Visitor that collects name *reads* only — skips pure assignment targets.
struct NameReadCollector {
    refs: HashSet<String>,
}

impl Visitor for NameReadCollector {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assign(s) => {
                // Visit targets to catch reads inside subscripts, attributes,
                // etc. (e.g. `d[key] = val` — `key` is a read).
                // But top-level Name targets are writes, handled by
                // visit_assign_target_for_reads.
                for target in &s.targets {
                    self.visit_assign_target_for_reads(target);
                }
                self.visit_expr(&s.value);
            }
            Stmt::AnnAssign(s) => {
                self.visit_assign_target_for_reads(&s.target);
                if let Some(v) = &s.value {
                    self.visit_expr(v);
                }
            }
            Stmt::AugAssign(s) => {
                self.visit_expr(&s.target);
                self.visit_expr(&s.value);
            }
            Stmt::For(s) => {
                self.visit_expr(&s.iter);
                self.visit_body(&s.body);
                self.visit_body(&s.orelse);
            }
            other => walk_stmt(self, other),
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Name(n) = expr {
            self.refs.insert(n.id.to_string());
        }
        walk_expr(self, expr);
    }
}

impl NameReadCollector {
    /// Visit an assignment target, collecting reads from subscripts and
    /// attribute accesses but NOT the top-level Name (which is a write).
    /// E.g. in `d[key] = val`, `d` and `key` are reads, the subscript
    /// itself is the write target.
    fn visit_assign_target_for_reads(&mut self, target: &Expr) {
        match target {
            // Bare name target — this is a write, skip it.
            Expr::Name(_) => {}
            // Subscript target like `d[key]` — `d` and `key` are reads.
            Expr::Subscript(s) => {
                self.visit_expr(&s.value);
                self.visit_expr(&s.slice);
            }
            // Attribute target like `obj.attr` — `obj` is a read.
            Expr::Attribute(a) => {
                self.visit_expr(&a.value);
            }
            // Starred target like `*rest` — recurse.
            Expr::Starred(s) => {
                self.visit_assign_target_for_reads(&s.value);
            }
            // Tuple/list unpacking — recurse into each element.
            Expr::Tuple(t) => {
                for elt in &t.elts {
                    self.visit_assign_target_for_reads(elt);
                }
            }
            Expr::List(l) => {
                for elt in &l.elts {
                    self.visit_assign_target_for_reads(elt);
                }
            }
            // Anything else — treat as a read.
            other => self.visit_expr(other),
        }
    }
}

// ── DJ056: UnusedVariable ─────────────────────────────────────────────────

pub struct UnusedVariable;

impl AstCheck for UnusedVariable {
    fn code(&self) -> &'static str {
        "DJ056"
    }

    fn level(&self) -> Level {
        Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut diags = vec![];
        visit_functions_for_unused_vars(&ctx.module.body, ctx.filename, &mut diags);
        diags
    }
}

fn visit_functions_for_unused_vars(body: &[Stmt], filename: &str, diags: &mut Vec<Diagnostic>) {
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(f) => {
                analyze_unused_vars_in_function(f, filename, diags);
                visit_functions_for_unused_vars(&f.body, filename, diags);
            }
            Stmt::ClassDef(c) => {
                visit_functions_for_unused_vars(&c.body, filename, diags);
            }
            Stmt::If(s) => {
                visit_functions_for_unused_vars(&s.body, filename, diags);
                for clause in &s.elif_else_clauses {
                    visit_functions_for_unused_vars(&clause.body, filename, diags);
                }
            }
            Stmt::For(s) => {
                visit_functions_for_unused_vars(&s.body, filename, diags);
                visit_functions_for_unused_vars(&s.orelse, filename, diags);
            }
            Stmt::While(s) => {
                visit_functions_for_unused_vars(&s.body, filename, diags);
                visit_functions_for_unused_vars(&s.orelse, filename, diags);
            }
            Stmt::With(s) => {
                visit_functions_for_unused_vars(&s.body, filename, diags);
            }
            Stmt::Try(s) => {
                visit_functions_for_unused_vars(&s.body, filename, diags);
                for h in &s.handlers {
                    visit_functions_for_unused_vars(&h.body, filename, diags);
                }
                visit_functions_for_unused_vars(&s.orelse, filename, diags);
                visit_functions_for_unused_vars(&s.finalbody, filename, diags);
            }
            _ => {}
        }
    }
}

fn analyze_unused_vars_in_function(
    func: &StmtFunctionDef,
    filename: &str,
    diags: &mut Vec<Diagnostic>,
) {
    let mut nonlocal_global: HashSet<String> = HashSet::new();
    collect_nonlocal_global(&func.body, &mut nonlocal_global);

    let mut params: HashSet<String> = HashSet::new();
    collect_param_names(&func.parameters, &mut params);

    let mut assigned: HashMap<String, thorn_api::ByteRange> = HashMap::new();
    collect_assignments_in_body(&func.body, &mut assigned, &nonlocal_global, &params);

    if assigned.is_empty() {
        return;
    }

    let mut reader = NameReadCollector {
        refs: HashSet::new(),
    };
    reader.visit_body(&func.body);
    let refs = reader.refs;

    // Build a set of names assigned from Call expressions — these are
    // likely invoked for side effects and the variable is incidental
    // (e.g. `response = client.get(...)` in tests).
    let mut call_assigned: HashSet<String> = HashSet::new();
    collect_call_assigned_names(&func.body, &mut call_assigned);

    for (name, range) in &assigned {
        if !refs.contains(name) {
            // If the value comes from a function/method call, the call was
            // likely the purpose — don't flag the unused binding.
            if call_assigned.contains(name) {
                continue;
            }
            diags.push(
                Diagnostic::new(
                    "DJ056",
                    format!("Unused variable '{name}'."),
                    filename,
                )
                .with_range(text_range(*range))
                .with_level(Level::All),
            );
        }
    }
}

/// Collect names that are assigned from a Call expression (function/method call).
fn collect_call_assigned_names(body: &[Stmt], out: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Assign(s) => {
                if matches!(s.value.as_ref(), Expr::Call(_)) {
                    for target in &s.targets {
                        if let Expr::Name(n) = target {
                            out.insert(n.id.as_str().to_string());
                        }
                    }
                }
            }
            Stmt::For(s) => {
                collect_call_assigned_names(&s.body, out);
                collect_call_assigned_names(&s.orelse, out);
            }
            Stmt::If(s) => {
                collect_call_assigned_names(&s.body, out);
                for c in &s.elif_else_clauses {
                    collect_call_assigned_names(&c.body, out);
                }
            }
            Stmt::While(s) => {
                collect_call_assigned_names(&s.body, out);
            }
            Stmt::With(s) => {
                collect_call_assigned_names(&s.body, out);
            }
            Stmt::Try(s) => {
                collect_call_assigned_names(&s.body, out);
                for h in &s.handlers {
                    collect_call_assigned_names(&h.body, out);
                }
                collect_call_assigned_names(&s.orelse, out);
                collect_call_assigned_names(&s.finalbody, out);
            }
            _ => {}
        }
    }
}

fn collect_nonlocal_global(body: &[Stmt], out: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Global(g) => {
                for name in &g.names {
                    out.insert(name.to_string());
                }
            }
            Stmt::Nonlocal(n) => {
                for name in &n.names {
                    out.insert(name.to_string());
                }
            }
            _ => {}
        }
    }
}

fn collect_param_names(params: &Parameters, out: &mut HashSet<String>) {
    for p in &params.posonlyargs {
        out.insert(p.name.as_str().to_string());
    }
    for p in &params.args {
        out.insert(p.name.as_str().to_string());
    }
    for p in &params.kwonlyargs {
        out.insert(p.name.as_str().to_string());
    }
    if let Some(v) = &params.vararg {
        out.insert(v.name.as_str().to_string());
    }
    if let Some(k) = &params.kwarg {
        out.insert(k.name.as_str().to_string());
    }
}

fn collect_assignments_in_body(
    body: &[Stmt],
    out: &mut HashMap<String, thorn_api::ByteRange>,
    skip: &HashSet<String>,
    params: &HashSet<String>,
) {
    for stmt in body {
        match stmt {
            Stmt::Assign(s) => {
                for target in &s.targets {
                    collect_assign_target(target, s.range(), out, skip, params);
                }
            }
            Stmt::AnnAssign(s) => {
                if s.value.is_some() {
                    collect_assign_target(&s.target, s.range(), out, skip, params);
                }
            }
            Stmt::For(s) => {
                collect_assign_target(&s.target, s.range(), out, skip, params);
                collect_assignments_in_body(&s.body, out, skip, params);
                collect_assignments_in_body(&s.orelse, out, skip, params);
            }
            Stmt::If(s) => {
                collect_assignments_in_body(&s.body, out, skip, params);
                for clause in &s.elif_else_clauses {
                    collect_assignments_in_body(&clause.body, out, skip, params);
                }
            }
            Stmt::While(s) => {
                collect_assignments_in_body(&s.body, out, skip, params);
                collect_assignments_in_body(&s.orelse, out, skip, params);
            }
            Stmt::With(s) => {
                collect_assignments_in_body(&s.body, out, skip, params);
            }
            Stmt::Try(s) => {
                collect_assignments_in_body(&s.body, out, skip, params);
                for h in &s.handlers {
                    collect_assignments_in_body(&h.body, out, skip, params);
                }
                collect_assignments_in_body(&s.orelse, out, skip, params);
                collect_assignments_in_body(&s.finalbody, out, skip, params);
            }
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            Stmt::AugAssign(_) => {}
            _ => {}
        }
    }
}

fn collect_assign_target(
    target: &Expr,
    range: thorn_api::ByteRange,
    out: &mut HashMap<String, thorn_api::ByteRange>,
    skip: &HashSet<String>,
    params: &HashSet<String>,
) {
    match target {
        Expr::Name(n) => {
            let name = n.id.as_str();
            if name.starts_with('_') {
                return;
            }
            if skip.contains(name) || params.contains(name) {
                return;
            }
            out.entry(name.to_string()).or_insert(range);
        }
        Expr::Tuple(t) => {
            for elt in &t.elts {
                collect_assign_target(elt, range, out, skip, params);
            }
        }
        Expr::List(l) => {
            for elt in &l.elts {
                collect_assign_target(elt, range, out, skip, params);
            }
        }
        _ => {}
    }
}

// ── DJ058: RedefinedOuterName ─────────────────────────────────────────────

pub struct RedefinedOuterName;

impl AstCheck for RedefinedOuterName {
    fn code(&self) -> &'static str {
        "DJ058"
    }

    fn level(&self) -> Level {
        Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let module_names = collect_module_scope_names(&ctx.module.body);

        let mut diags = vec![];
        for stmt in &ctx.module.body {
            check_function_for_redefined(stmt, &module_names, ctx.filename, ctx.source, &mut diags);
        }
        diags
    }
}

/// Info about a module-level name: its range and whether it comes from a
/// decorated function definition (pytest fixtures, DI providers, etc.).
struct ModuleName {
    range: thorn_api::ByteRange,
    is_decorated_function: bool,
}

fn collect_module_scope_names(
    body: &[Stmt],
) -> HashMap<String, ModuleName> {
    let mut names: HashMap<String, ModuleName> = HashMap::new();
    for stmt in body {
        match stmt {
            Stmt::Import(s) => {
                for alias in &s.names {
                    let name = alias
                        .asname
                        .as_ref()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| {
                            alias.name.split('.').next().unwrap_or("").to_string()
                        });
                    names.entry(name).or_insert(ModuleName {
                        range: s.range(),
                        is_decorated_function: false,
                    });
                }
            }
            Stmt::ImportFrom(s) => {
                for alias in &s.names {
                    if alias.name.as_str() == "*" {
                        continue;
                    }
                    let name = alias
                        .asname
                        .as_ref()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| alias.name.to_string());
                    names.entry(name).or_insert(ModuleName {
                        range: s.range(),
                        is_decorated_function: false,
                    });
                }
            }
            Stmt::Assign(s) => {
                for target in &s.targets {
                    if let Expr::Name(n) = target {
                        names.entry(n.id.to_string()).or_insert(ModuleName {
                            range: s.range(),
                            is_decorated_function: false,
                        });
                    }
                }
            }
            Stmt::AnnAssign(s) => {
                if let Expr::Name(n) = s.target.as_ref() {
                    names.entry(n.id.to_string()).or_insert(ModuleName {
                        range: s.range(),
                        is_decorated_function: false,
                    });
                }
            }
            Stmt::FunctionDef(f) => {
                let has_decorators = !f.decorator_list.is_empty();
                names.entry(f.name.to_string()).or_insert(ModuleName {
                    range: f.range(),
                    is_decorated_function: has_decorators,
                });
            }
            Stmt::ClassDef(c) => {
                names.entry(c.name.to_string()).or_insert(ModuleName {
                    range: c.range(),
                    is_decorated_function: false,
                });
            }
            _ => {}
        }
    }
    names
}

fn check_function_for_redefined(
    stmt: &Stmt,
    module_names: &HashMap<String, ModuleName>,
    filename: &str,
    source: &str,
    diags: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::FunctionDef(f) => {
            let is_init = f.name.as_str() == "__init__";

            if !is_init {
                let param_names = collect_all_param_names_with_range(&f.parameters);
                for (name, range) in &param_names {
                    if should_skip_redefined_name(name) {
                        continue;
                    }
                    if let Some(outer) = module_names.get(name) {
                        // If the outer name is a decorated function, a parameter
                        // with the same name is almost certainly dependency
                        // injection (pytest fixtures, FastAPI deps, etc.) — not
                        // accidental shadowing.
                        if outer.is_decorated_function {
                            continue;
                        }
                        let outer_line =
                            offset_to_line(source, outer.range.start as usize);
                        diags.push(
                            Diagnostic::new(
                                "DJ058",
                                format!("Redefining name '{name}' from outer scope (line {outer_line})."),
                                filename,
                            )
                            .with_range(text_range(*range))
                            .with_level(Level::All),
                        );
                    }
                }
            }

            let mut local_assigns: HashMap<String, thorn_api::ByteRange> = HashMap::new();
            let mut params: HashSet<String> = HashSet::new();
            collect_param_names(&f.parameters, &mut params);
            let skip_empty = HashSet::new();
            collect_assignments_in_body(&f.body, &mut local_assigns, &skip_empty, &params);

            for (name, range) in &local_assigns {
                if should_skip_redefined_name(name) {
                    continue;
                }
                if let Some(outer) = module_names.get(name) {
                    let outer_line =
                        offset_to_line(source, outer.range.start as usize);
                    diags.push(
                        Diagnostic::new(
                            "DJ058",
                            format!("Redefining name '{name}' from outer scope (line {outer_line})."),
                            filename,
                        )
                        .with_range(text_range(*range))
                        .with_level(Level::All),
                    );
                }
            }

            for s in &f.body {
                check_function_for_redefined(s, module_names, filename, source, diags);
            }
        }
        Stmt::ClassDef(c) => {
            for s in &c.body {
                check_function_for_redefined(s, module_names, filename, source, diags);
            }
        }
        Stmt::If(s) => {
            for s in &s.body {
                check_function_for_redefined(s, module_names, filename, source, diags);
            }
            for clause in &s.elif_else_clauses {
                for s in &clause.body {
                    check_function_for_redefined(s, module_names, filename, source, diags);
                }
            }
        }
        _ => {}
    }
}

fn should_skip_redefined_name(name: &str) -> bool {
    name.starts_with('_') || name == "self" || name == "cls"
}

fn collect_all_param_names_with_range(
    params: &Parameters,
) -> Vec<(String, thorn_api::ByteRange)> {
    let mut out = vec![];
    for p in &params.posonlyargs {
        out.push((
            p.name.as_str().to_string(),
            p.range(),
        ));
    }
    for p in &params.args {
        out.push((
            p.name.as_str().to_string(),
            p.range(),
        ));
    }
    for p in &params.kwonlyargs {
        out.push((
            p.name.as_str().to_string(),
            p.range(),
        ));
    }
    if let Some(v) = &params.vararg {
        out.push((v.name.as_str().to_string(), v.range()));
    }
    if let Some(k) = &params.kwarg {
        out.push((k.name.as_str().to_string(), k.range()));
    }
    out
}

// ── DJ066: ComparisonWithCallable ─────────────────────────────────────────

pub struct ComparisonWithCallable;

impl AstCheck for ComparisonWithCallable {
    fn code(&self) -> &'static str {
        "DJ066"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut func_collector = FunctionNameCollector {
            func_names: HashSet::new(),
        };
        func_collector.visit_body(&ctx.module.body);
        let func_names = func_collector.func_names;

        let mut v = ComparisonWithCallableVisitor {
            diags: vec![],
            filename: ctx.filename,
            func_names: &func_names,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct FunctionNameCollector {
    func_names: HashSet<String>,
}

impl Visitor for FunctionNameCollector {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(f) = stmt {
            self.func_names.insert(f.name.to_string());
        } else {
            walk_stmt(self, stmt);
        }
    }
}

struct ComparisonWithCallableVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    func_names: &'a HashSet<String>,
}

impl<'a> Visitor for ComparisonWithCallableVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Compare(cmp) = expr {
            for (op, comparator) in cmp.ops.iter().zip(cmp.comparators.iter()) {
                if matches!(op, CmpOp::Eq | CmpOp::NotEq) {
                    if let Some(name) = callable_name_in_expr(&cmp.left, self.func_names) {
                        if !is_callable_or_none(comparator, self.func_names) {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ066",
                                    format!("Comparing against callable '{name}'. Did you omit the parentheses?"),
                                    self.filename,
                                )
                                .with_range(text_range(cmp.range())),
                            );
                            break;
                        }
                    }
                    if let Some(name) = callable_name_in_expr(comparator, self.func_names) {
                        if !is_callable_or_none(&cmp.left, self.func_names) {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ066",
                                    format!("Comparing against callable '{name}'. Did you omit the parentheses?"),
                                    self.filename,
                                )
                                .with_range(text_range(cmp.range())),
                            );
                            break;
                        }
                    }
                }
            }
        }
        walk_expr(self, expr);
    }
}

fn callable_name_in_expr<'a>(
    expr: &Expr,
    func_names: &'a HashSet<String>,
) -> Option<&'a str> {
    if let Expr::Name(n) = expr {
        let name = n.id.as_str();
        if func_names.contains(name) {
            return Some(func_names.get(name).map(|s| s.as_str()).unwrap());
        }
    }
    None
}

fn is_callable_or_none(expr: &Expr, func_names: &HashSet<String>) -> bool {
    match expr {
        Expr::Name(n) => {
            let s = n.id.as_str();
            s == "None" || func_names.contains(s)
        }
        Expr::NoneLiteral(_) => true,
        _ => false,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::common::test_helpers::{run_check, run_check_filename};

    // ── DJ064: UnreachableCode ────────────────────────────────────────────

    #[test]
    fn dj064_triggers_return_then_stmt() {
        let src = r#"
def foo():
    return 1
    x = 2
"#;
        let codes = run_check(&UnreachableCode, src);
        assert!(
            codes.contains(&"DJ064".to_string()),
            "expected DJ064, got {:?}",
            codes
        );
    }

    #[test]
    fn dj064_triggers_raise_then_stmt() {
        let src = r#"
def foo():
    raise ValueError("oops")
    do_something()
"#;
        let codes = run_check(&UnreachableCode, src);
        assert!(
            codes.contains(&"DJ064".to_string()),
            "expected DJ064, got {:?}",
            codes
        );
    }

    #[test]
    fn dj064_triggers_break_then_stmt() {
        let src = r#"
def foo():
    for i in range(10):
        break
        x = i
"#;
        let codes = run_check(&UnreachableCode, src);
        assert!(
            codes.contains(&"DJ064".to_string()),
            "expected DJ064, got {:?}",
            codes
        );
    }

    #[test]
    fn dj064_triggers_continue_then_stmt() {
        let src = r#"
def foo():
    for i in range(10):
        continue
        x = i
"#;
        let codes = run_check(&UnreachableCode, src);
        assert!(
            codes.contains(&"DJ064".to_string()),
            "expected DJ064, got {:?}",
            codes
        );
    }

    #[test]
    fn dj064_no_trigger_return_at_end() {
        let src = r#"
def foo():
    x = 1
    return x
"#;
        let codes = run_check(&UnreachableCode, src);
        assert!(
            !codes.contains(&"DJ064".to_string()),
            "unexpected DJ064, got {:?}",
            codes
        );
    }

    #[test]
    fn dj064_no_trigger_in_finally() {
        let src = r#"
def foo():
    try:
        pass
    finally:
        return 1
        x = 2
"#;
        let codes = run_check(&UnreachableCode, src);
        assert!(
            !codes.contains(&"DJ064".to_string()),
            "unexpected DJ064 in finally, got {:?}",
            codes
        );
    }

    #[test]
    fn dj064_no_trigger_type_checking_block() {
        let src = r#"
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    return
    x = 1
"#;
        let codes = run_check(&UnreachableCode, src);
        assert!(
            !codes.contains(&"DJ064".to_string()),
            "unexpected DJ064 in TYPE_CHECKING block, got {:?}",
            codes
        );
    }

    // ── DJ055: UnusedImport ───────────────────────────────────────────────

    #[test]
    fn dj055_triggers_unused_import() {
        let src = r#"
import os
x = 1
"#;
        let codes = run_check(&UnusedImport, src);
        assert!(
            codes.contains(&"DJ055".to_string()),
            "expected DJ055, got {:?}",
            codes
        );
    }

    #[test]
    fn dj055_triggers_unused_from_import() {
        let src = r#"
from os import path
x = 1
"#;
        let codes = run_check(&UnusedImport, src);
        assert!(
            codes.contains(&"DJ055".to_string()),
            "expected DJ055, got {:?}",
            codes
        );
    }

    #[test]
    fn dj055_no_trigger_used_import() {
        let src = r#"
import os
print(os.getcwd())
"#;
        let codes = run_check(&UnusedImport, src);
        assert!(
            !codes.contains(&"DJ055".to_string()),
            "unexpected DJ055, got {:?}",
            codes
        );
    }

    #[test]
    fn dj055_no_trigger_in_all() {
        let src = r#"
from os import path
__all__ = ["path"]
"#;
        let codes = run_check(&UnusedImport, src);
        assert!(
            !codes.contains(&"DJ055".to_string()),
            "unexpected DJ055 for name in __all__, got {:?}",
            codes
        );
    }

    #[test]
    fn dj055_no_trigger_type_checking() {
        let src = r#"
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    import os
"#;
        let codes = run_check(&UnusedImport, src);
        assert!(
            !codes.contains(&"DJ055".to_string()),
            "unexpected DJ055 for TYPE_CHECKING import, got {:?}",
            codes
        );
    }

    #[test]
    fn dj055_no_trigger_future_import() {
        let src = r#"
from __future__ import annotations
"#;
        let codes = run_check(&UnusedImport, src);
        assert!(
            !codes.contains(&"DJ055".to_string()),
            "unexpected DJ055 for __future__ import, got {:?}",
            codes
        );
    }

    #[test]
    fn dj055_no_trigger_init_file() {
        let src = r#"
import os
"#;
        let codes = run_check_filename(&UnusedImport, src, "__init__.py");
        assert!(
            !codes.contains(&"DJ055".to_string()),
            "unexpected DJ055 in __init__.py, got {:?}",
            codes
        );
    }

    #[test]
    fn dj055_no_trigger_underscore_alias() {
        let src = r#"
from django.utils.translation import gettext_lazy as _
"#;
        let codes = run_check(&UnusedImport, src);
        assert!(
            !codes.contains(&"DJ055".to_string()),
            "unexpected DJ055 for _ alias, got {:?}",
            codes
        );
    }

    /// A `# noqa` comment on an import line suppresses DJ055.
    #[test]
    fn dj055_no_trigger_noqa_comment() {
        let src = "import os  # noqa\nx = 1\n";
        let codes = run_check(&UnusedImport, src);
        assert!(
            !codes.contains(&"DJ055".to_string()),
            "unexpected DJ055 for noqa-annotated import, got {:?}",
            codes
        );
    }

    /// A `# noqa: DJ055` comment (specific code) also suppresses DJ055.
    #[test]
    fn dj055_no_trigger_noqa_specific_code() {
        let src = "from os import path  # noqa: DJ055\nx = 1\n";
        let codes = run_check(&UnusedImport, src);
        assert!(
            !codes.contains(&"DJ055".to_string()),
            "unexpected DJ055 for noqa:DJ055-annotated import, got {:?}",
            codes
        );
    }

    // ── DJ056: UnusedVariable ─────────────────────────────────────────────

    #[test]
    fn dj056_triggers_unused_local() {
        let src = r#"
def foo():
    x = 1
    return 42
"#;
        let codes = run_check(&UnusedVariable, src);
        assert!(
            codes.contains(&"DJ056".to_string()),
            "expected DJ056, got {:?}",
            codes
        );
    }

    #[test]
    fn dj056_no_trigger_used_local() {
        let src = r#"
def foo():
    x = 1
    return x
"#;
        let codes = run_check(&UnusedVariable, src);
        assert!(
            !codes.contains(&"DJ056".to_string()),
            "unexpected DJ056, got {:?}",
            codes
        );
    }

    #[test]
    fn dj056_no_trigger_underscore_prefix() {
        let src = r#"
def foo():
    _ignored = compute()
    return 42
"#;
        let codes = run_check(&UnusedVariable, src);
        assert!(
            !codes.contains(&"DJ056".to_string()),
            "unexpected DJ056 for _-prefixed name, got {:?}",
            codes
        );
    }

    #[test]
    fn dj056_no_trigger_global_decl() {
        let src = r#"
counter = 0

def foo():
    global counter
    counter = 1
"#;
        let codes = run_check(&UnusedVariable, src);
        assert!(
            !codes.contains(&"DJ056".to_string()),
            "unexpected DJ056 for global, got {:?}",
            codes
        );
    }

    #[test]
    fn dj056_no_trigger_params() {
        let src = r#"
def foo(x, y):
    return x
"#;
        let codes = run_check(&UnusedVariable, src);
        assert!(
            !codes.contains(&"DJ056".to_string()),
            "unexpected DJ056 for parameter, got {:?}",
            codes
        );
    }

    #[test]
    fn dj056_no_trigger_augmented_assign() {
        let src = r#"
def foo():
    x = 0
    x += 1
    return x
"#;
        let codes = run_check(&UnusedVariable, src);
        assert!(
            !codes.contains(&"DJ056".to_string()),
            "unexpected DJ056 for augmented assign, got {:?}",
            codes
        );
    }

    // ── DJ058: RedefinedOuterName ─────────────────────────────────────────

    #[test]
    fn dj058_triggers_param_shadows_module_name() {
        let src = r#"
import os

def foo(os):
    return os
"#;
        let codes = run_check(&RedefinedOuterName, src);
        assert!(
            codes.contains(&"DJ058".to_string()),
            "expected DJ058, got {:?}",
            codes
        );
    }

    #[test]
    fn dj058_triggers_local_shadows_module_name() {
        let src = r#"
MY_CONST = 42

def foo():
    MY_CONST = 99
    return MY_CONST
"#;
        let codes = run_check(&RedefinedOuterName, src);
        assert!(
            codes.contains(&"DJ058".to_string()),
            "expected DJ058, got {:?}",
            codes
        );
    }

    #[test]
    fn dj058_no_trigger_unique_name() {
        let src = r#"
import os

def foo(path):
    return path
"#;
        let codes = run_check(&RedefinedOuterName, src);
        assert!(
            !codes.contains(&"DJ058".to_string()),
            "unexpected DJ058, got {:?}",
            codes
        );
    }

    #[test]
    fn dj058_no_trigger_underscore_prefix() {
        let src = r#"
_helper = 1

def foo(_helper):
    return _helper
"#;
        let codes = run_check(&RedefinedOuterName, src);
        assert!(
            !codes.contains(&"DJ058".to_string()),
            "unexpected DJ058 for _ prefix, got {:?}",
            codes
        );
    }

    #[test]
    fn dj058_no_trigger_init_params() {
        let src = r#"
value = 42

class Foo:
    def __init__(self, value):
        self.value = value
"#;
        let codes = run_check(&RedefinedOuterName, src);
        assert!(
            !codes.contains(&"DJ058".to_string()),
            "unexpected DJ058 for __init__ param, got {:?}",
            codes
        );
    }

    #[test]
    fn dj058_no_trigger_self_cls() {
        let src = r#"
self = object()

class Foo:
    def bar(self):
        return self
"#;
        let codes = run_check(&RedefinedOuterName, src);
        assert!(
            !codes.contains(&"DJ058".to_string()),
            "unexpected DJ058 for self/cls, got {:?}",
            codes
        );
    }

    // ── DJ066: ComparisonWithCallable ─────────────────────────────────────

    #[test]
    fn dj066_triggers_eq_with_function_name() {
        let src = r#"
def get_value():
    return 42

if result == get_value:
    pass
"#;
        let codes = run_check(&ComparisonWithCallable, src);
        assert!(
            codes.contains(&"DJ066".to_string()),
            "expected DJ066, got {:?}",
            codes
        );
    }

    #[test]
    fn dj066_triggers_neq_with_function_name() {
        let src = r#"
def compute():
    return 1

x = compute != result
"#;
        let codes = run_check(&ComparisonWithCallable, src);
        assert!(
            codes.contains(&"DJ066".to_string()),
            "expected DJ066, got {:?}",
            codes
        );
    }

    #[test]
    fn dj066_no_trigger_called_function() {
        let src = r#"
def get_value():
    return 42

if result == get_value():
    pass
"#;
        let codes = run_check(&ComparisonWithCallable, src);
        assert!(
            !codes.contains(&"DJ066".to_string()),
            "unexpected DJ066 for called function, got {:?}",
            codes
        );
    }

    #[test]
    fn dj066_no_trigger_non_function_name() {
        let src = r#"
STATUS_OK = 200

if response_code == STATUS_OK:
    pass
"#;
        let codes = run_check(&ComparisonWithCallable, src);
        assert!(
            !codes.contains(&"DJ066".to_string()),
            "unexpected DJ066 for non-function name, got {:?}",
            codes
        );
    }

    #[test]
    fn dj066_no_trigger_compare_none() {
        let src = r#"
def get_value():
    return None

if get_value == None:
    pass
"#;
        let codes = run_check(&ComparisonWithCallable, src);
        assert!(
            !codes.contains(&"DJ066".to_string()),
            "unexpected DJ066 for None comparison, got {:?}",
            codes
        );
    }

    #[test]
    fn dj066_no_trigger_is_operator() {
        let src = r#"
def get_value():
    return 42

if result is get_value:
    pass
"#;
        let codes = run_check(&ComparisonWithCallable, src);
        assert!(
            !codes.contains(&"DJ066".to_string()),
            "unexpected DJ066 for 'is' operator, got {:?}",
            codes
        );
    }
}
