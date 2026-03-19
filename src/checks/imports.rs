//! Cross-file import graph analysis: circular imports (DJ042) and unused
//! Django-specific imports (DJ043).

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::*;
use ruff_text_size::Ranged;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thorn_api::{Diagnostic, Level};

// ── Data structures ───────────────────────────────────────────────────────

/// A single name (or alias) from a `from X import Y [as Z]` statement.
#[derive(Debug, Clone)]
struct ImportedName {
    /// The original imported symbol, e.g. `gettext_lazy`.
    name: String,
    /// The local alias, e.g. `_` from `import gettext_lazy as _`.
    /// `None` when there is no `as` clause.
    alias: Option<String>,
}

impl ImportedName {
    /// The identifier that is actually visible in the file's scope.
    /// If an alias was given that is what other code will reference.
    fn local_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone)]
struct ImportInfo {
    /// Resolved absolute module path, e.g. `pescheck_core.models.user`
    module: String,
    /// Specific names imported via `from X import A, B` (empty for `import X`)
    names: Vec<ImportedName>,
    /// 1-based source line of the import statement
    line: u32,
    /// True when the import is inside `if TYPE_CHECKING:` – skip for cycles
    is_type_checking: bool,
}

#[derive(Debug)]
struct ModuleInfo {
    file_path: String,
    imports: Vec<ImportInfo>,
}

// ── Directory walker ──────────────────────────────────────────────────────

/// Directories whose contents should never be analysed.
const SKIP_DIRS: &[&str] = &[
    ".venv",
    "venv",
    "node_modules",
    "__pycache__",
    ".git",
    "site-packages",
    "migrations",
];

fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

fn collect_python_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !should_skip_dir(dir_name) {
                collect_python_files(&path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("py") {
            out.push(path);
        }
    }
}

// ── Module-path helpers ───────────────────────────────────────────────────

/// Convert an absolute file path to a dotted Python module string relative to
/// `project_dir`.
///
/// `project_dir/pescheck_core/models/user.py` → `pescheck_core.models.user`
/// `project_dir/pescheck_core/models/__init__.py` → `pescheck_core.models`
fn path_to_module(file: &Path, project_dir: &Path) -> Option<String> {
    let rel = file.strip_prefix(project_dir).ok()?;
    let mut parts: Vec<&str> = rel
        .components()
        .filter_map(|c| {
            if let std::path::Component::Normal(s) = c {
                s.to_str()
            } else {
                None
            }
        })
        .collect();

    // Strip .py extension from the last component
    if let Some(last) = parts.last_mut() {
        if let Some(stem) = last.strip_suffix(".py") {
            *last = stem;
        }
    }

    // `__init__` represents the package itself
    if parts.last() == Some(&"__init__") {
        parts.pop();
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("."))
    }
}

/// Resolve a relative import level + module name to an absolute module path.
///
/// `level=1` means `.` (same package), `level=2` means `..` (parent), etc.
fn resolve_relative_import(
    current_file: &Path,
    level: u32,
    module_name: &str,
    project_dir: &Path,
) -> Option<String> {
    // Start from the directory that contains current_file.
    // For each additional dot beyond the first, go up one more level.
    let mut base = current_file.parent()?.to_path_buf();
    for _ in 1..level {
        base = base.parent()?.to_path_buf();
    }

    let base_module = path_to_module(&base.join("__init__.py"), project_dir)
        .or_else(|| path_to_module(&base, project_dir));

    let base_module = base_module.unwrap_or_default();

    if module_name.is_empty() {
        Some(base_module)
    } else if base_module.is_empty() {
        Some(module_name.to_string())
    } else {
        Some(format!("{base_module}.{module_name}"))
    }
}

// ── AST visitors ─────────────────────────────────────────────────────────

/// Collects all import statements in a single file, aware of `TYPE_CHECKING`
/// guards.
struct ImportCollector<'a> {
    current_file: &'a Path,
    project_dir: &'a Path,
    source: &'a str,
    imports: Vec<ImportInfo>,
    in_type_checking: bool,
}

impl<'a> ImportCollector<'a> {
    fn line_of(&self, range: ruff_text_size::TextRange) -> u32 {
        let offset = u32::from(range.start()) as usize;
        let before = &self.source[..offset.min(self.source.len())];
        before.chars().filter(|c| *c == '\n').count() as u32 + 1
    }

    fn add_import_from(&mut self, import: &StmtImportFrom, is_type_checking: bool) {
        let level = import.level;
        let raw_module = import.module.as_ref().map(|m| m.as_str()).unwrap_or("");

        let module = if level > 0 {
            // Relative import
            match resolve_relative_import(self.current_file, level, raw_module, self.project_dir) {
                Some(m) => m,
                None => return,
            }
        } else {
            raw_module.to_string()
        };

        let names: Vec<ImportedName> = import
            .names
            .iter()
            .map(|alias| ImportedName {
                name: alias.name.as_str().to_string(),
                alias: alias.asname.as_ref().map(|a| a.as_str().to_string()),
            })
            .collect();

        let line = self.line_of(import.range());

        self.imports.push(ImportInfo {
            module,
            names,
            line,
            is_type_checking,
        });
    }

    fn add_import(&mut self, import: &StmtImport, is_type_checking: bool) {
        let line = self.line_of(import.range());
        for alias in &import.names {
            self.imports.push(ImportInfo {
                module: alias.name.as_str().to_string(),
                names: vec![],
                line,
                is_type_checking,
            });
        }
    }
}

impl<'a> Visitor<'_> for ImportCollector<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Import(import) => {
                self.add_import(import, self.in_type_checking);
            }
            Stmt::ImportFrom(import) => {
                self.add_import_from(import, self.in_type_checking);
            }
            Stmt::If(if_stmt) => {
                // Detect `if TYPE_CHECKING:` guards
                let guard = is_type_checking_guard(&if_stmt.test);
                let prev = self.in_type_checking;
                if guard {
                    self.in_type_checking = true;
                }
                // Visit the body of the if
                self.visit_body(&if_stmt.body);
                self.in_type_checking = prev;
                // Visit elif/else branches without the TYPE_CHECKING flag
                for clause in &if_stmt.elif_else_clauses {
                    self.visit_body(&clause.body);
                }
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }
}

/// Returns true when `expr` represents `TYPE_CHECKING` or
/// `typing.TYPE_CHECKING`.
fn is_type_checking_guard(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "TYPE_CHECKING",
        Expr::Attribute(a) => {
            a.attr.as_str() == "TYPE_CHECKING"
                && matches!(a.value.as_ref(), Expr::Name(n) if n.id.as_str() == "typing")
        }
        _ => false,
    }
}

// ── Name-usage visitor ────────────────────────────────────────────────────

/// Collects every `Name` identifier referenced in a file's body (excluding
/// import statements themselves, which define names but don't constitute usage).
struct NameUsageCollector {
    used: HashSet<String>,
}

impl Visitor<'_> for NameUsageCollector {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        // Skip import statements – they define, not use
        match stmt {
            Stmt::Import(_) | Stmt::ImportFrom(_) => {}
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Name(n) = expr {
            self.used.insert(n.id.as_str().to_string());
        }
        visitor::walk_expr(self, expr);
    }
}

// ── Build import graph ────────────────────────────────────────────────────

fn build_import_graph(project_dir: &Path) -> HashMap<String, ModuleInfo> {
    let mut py_files = Vec::new();
    collect_python_files(project_dir, &mut py_files);

    let mut graph: HashMap<String, ModuleInfo> = HashMap::new();

    for file_path in &py_files {
        let module_key = match path_to_module(file_path, project_dir) {
            Some(m) => m,
            None => continue,
        };

        let source = match std::fs::read_to_string(file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let parsed =
            match ruff_python_parser::parse(&source, ruff_python_parser::Mode::Module.into()) {
                Ok(p) => p,
                Err(_) => continue,
            };

        let module_ast = match parsed.into_syntax().module() {
            Some(m) => m.clone(),
            None => continue,
        };

        let mut collector = ImportCollector {
            current_file: file_path,
            project_dir,
            source: &source,
            imports: Vec::new(),
            in_type_checking: false,
        };
        collector.visit_body(&module_ast.body);

        graph.insert(
            module_key,
            ModuleInfo {
                file_path: file_path.to_string_lossy().to_string(),
                imports: collector.imports,
            },
        );
    }

    graph
}

// ── DJ042: Circular import detection ─────────────────────────────────────

/// DFS-based cycle detection over the import graph.
///
/// Only edges between modules that exist in the project are followed; stdlib
/// and third-party imports are ignored. TYPE_CHECKING-guarded imports are
/// also excluded because they are never executed at runtime.
fn detect_cycles(graph: &HashMap<String, ModuleInfo>) -> Vec<Diagnostic> {
    // Build an adjacency map restricted to project-internal modules.
    let project_modules: HashSet<&str> = graph.keys().map(|s| s.as_str()).collect();

    let edges: HashMap<&str, Vec<&str>> = graph
        .iter()
        .map(|(module, info)| {
            let deps: Vec<&str> = info
                .imports
                .iter()
                .filter(|imp| !imp.is_type_checking)
                .filter_map(|imp| {
                    // Look for exact match first, then prefix match for sub-modules.
                    if project_modules.contains(imp.module.as_str()) {
                        Some(imp.module.as_str())
                    } else {
                        // `from pescheck_core.models import Foo` has module
                        // `pescheck_core.models` – check if that's in the graph.
                        None
                    }
                })
                .collect();
            (module.as_str(), deps)
        })
        .collect();

    let mut visited: HashSet<&str> = HashSet::new();
    let mut rec_stack: Vec<&str> = Vec::new(); // current DFS path
    let mut in_stack: HashSet<&str> = HashSet::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    // Track which cycles we have already reported (by sorted set of nodes)
    let mut reported: HashSet<Vec<String>> = HashSet::new();

    // Collect all modules sorted for deterministic output.
    let mut all_modules: Vec<&str> = graph.keys().map(|s| s.as_str()).collect();
    all_modules.sort_unstable();

    for start in &all_modules {
        if !visited.contains(start) {
            dfs(
                start,
                &edges,
                &mut visited,
                &mut rec_stack,
                &mut in_stack,
                &mut diagnostics,
                &mut reported,
                graph,
            );
        }
    }

    diagnostics
}

#[allow(clippy::too_many_arguments)]
fn dfs<'a>(
    node: &'a str,
    edges: &HashMap<&'a str, Vec<&'a str>>,
    visited: &mut HashSet<&'a str>,
    rec_stack: &mut Vec<&'a str>,
    in_stack: &mut HashSet<&'a str>,
    diagnostics: &mut Vec<Diagnostic>,
    reported: &mut HashSet<Vec<String>>,
    graph: &HashMap<String, ModuleInfo>,
) {
    visited.insert(node);
    rec_stack.push(node);
    in_stack.insert(node);

    if let Some(neighbors) = edges.get(node) {
        for &neighbor in neighbors {
            if !visited.contains(neighbor) {
                dfs(
                    neighbor,
                    edges,
                    visited,
                    rec_stack,
                    in_stack,
                    diagnostics,
                    reported,
                    graph,
                );
            } else if in_stack.contains(neighbor) {
                // Back edge found – extract the cycle path
                let cycle_start = rec_stack.iter().position(|&n| n == neighbor).unwrap_or(0);
                let cycle: Vec<&str> = rec_stack[cycle_start..].to_vec();

                // Build a canonical key to avoid duplicate reports
                let mut key: Vec<String> = cycle.iter().map(|s| s.to_string()).collect();
                key.sort_unstable();

                // Suppress any cycle that passes through an __init__.py module.
                // Such cycles almost always represent Django's re-export /
                // autodiscovery pattern (e.g. models/__init__.py re-exporting
                // submodules) rather than a real circular dependency.
                // Self-cycles (len == 1) are naturally covered because the
                // single node would itself be __init__.py.
                let involves_init = cycle.iter().any(|m| {
                    graph
                        .get(*m)
                        .is_some_and(|info| info.file_path.ends_with("__init__.py"))
                });

                if !involves_init && !reported.contains(&key) {
                    reported.insert(key);

                    // Build human-readable cycle string: A → B → C → A
                    let mut path_parts: Vec<&str> = cycle.to_vec();
                    path_parts.push(cycle[0]); // close the loop
                    let cycle_str = path_parts.join(" → ");

                    // Anchor the diagnostic on the first file in the cycle
                    let first_module = cycle[0];
                    let file_path = graph
                        .get(first_module)
                        .map(|m| m.file_path.as_str())
                        .unwrap_or(first_module);

                    // Find the line of the import that creates the back-edge
                    let line = graph.get(first_module).and_then(|m| {
                        m.imports
                            .iter()
                            .find(|imp| {
                                imp.module == neighbor
                                    || imp.module.starts_with(&format!("{neighbor}."))
                            })
                            .map(|imp| imp.line)
                    });

                    let mut d = Diagnostic::new(
                        "DJ042",
                        format!("Circular import: {cycle_str}"),
                        file_path,
                    )
                    .with_level(Level::Fix);
                    d.line = line;
                    diagnostics.push(d);
                }
            }
        }
    }

    rec_stack.pop();
    in_stack.remove(node);
}

// ── DJ043: Unused Django model/view/serializer imports ────────────────────

/// Module suffixes that indicate Django-specific symbols (models, views, etc.)
const DJANGO_MODULE_SUFFIXES: &[&str] = &[
    ".models",
    ".views",
    ".serializers",
    ".forms",
    ".admin",
    ".signals",
    ".managers",
    ".querysets",
    ".permissions",
    ".validators",
    ".filters",
];

fn is_django_module(module: &str) -> bool {
    // The module itself might be `django.db.models`, `rest_framework.serializers`, etc.
    // or a project-level `app.models`, `app.views`, ...
    DJANGO_MODULE_SUFFIXES
        .iter()
        .any(|suffix| module.ends_with(suffix))
        || module.starts_with("django.")
        || module.starts_with("rest_framework.")
}

fn detect_unused_imports(graph: &HashMap<String, ModuleInfo>) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for (module_path, info) in graph {
        let source = match std::fs::read_to_string(&info.file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let parsed =
            match ruff_python_parser::parse(&source, ruff_python_parser::Mode::Module.into()) {
                Ok(p) => p,
                Err(_) => continue,
            };

        let module_ast = match parsed.into_syntax().module() {
            Some(m) => m.clone(),
            None => continue,
        };

        // Collect all name usages in the file
        let mut usage_collector = NameUsageCollector {
            used: HashSet::new(),
        };
        usage_collector.visit_body(&module_ast.body);
        let used_names = &usage_collector.used;

        // __init__.py files are re-export hubs by convention; skip them entirely
        // to avoid flagging intentional re-exports as unused.
        if info.file_path.ends_with("__init__.py") {
            continue;
        }

        for import in &info.imports {
            // Only check Django-specific `from X import Y` statements
            if import.names.is_empty() {
                continue; // bare `import X` – skip
            }
            if !is_django_module(&import.module) {
                continue; // not a Django module – ruff handles generic unused imports
            }

            for imported in &import.names {
                // Wildcard imports are always "used" by convention
                if imported.name == "*" {
                    continue;
                }

                // Check the local name (alias if present, otherwise the
                // original name).  This handles the ubiquitous pattern
                //   from django.utils.translation import gettext_lazy as _
                // where `_` is used throughout but `gettext_lazy` is not.
                let local = imported.local_name();
                if !used_names.contains(local) {
                    let display_name = if imported.alias.is_some() {
                        format!("{} as {}", imported.name, local)
                    } else {
                        imported.name.clone()
                    };
                    let mut d = Diagnostic::new(
                        "DJ043",
                        format!(
                            "Imported '{}' from '{}' is unused.",
                            display_name, import.module
                        ),
                        &info.file_path,
                    )
                    .with_level(Level::All);
                    d.line = Some(import.line);
                    diagnostics.push(d);
                }
            }
        }

        // Suppress unused warning – module_path is used as the map key only.
        let _ = module_path;
    }

    diagnostics
}

// ── Public entry point ────────────────────────────────────────────────────

/// Run cross-file import graph analysis on all Python files under
/// `project_dir` and return a list of diagnostics.
///
/// - **DJ042** (HIGH) – Circular import detected.
/// - **DJ043** (LOW)  – Unused Django model/view/serializer import.
pub fn check_imports(project_dir: &Path) -> Vec<Diagnostic> {
    let graph = build_import_graph(project_dir);

    if graph.is_empty() {
        return vec![];
    }

    let mut diagnostics = Vec::new();
    diagnostics.extend(detect_cycles(&graph));
    diagnostics.extend(detect_unused_imports(&graph));
    diagnostics
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_project() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    // ── path_to_module ────────────────────────────────────────────────────

    #[test]
    fn test_path_to_module_regular_file() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(root, "myapp/models/user.py", "");
        let file = root.join("myapp/models/user.py");
        assert_eq!(
            path_to_module(&file, root),
            Some("myapp.models.user".into())
        );
    }

    #[test]
    fn test_path_to_module_init() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(root, "myapp/models/__init__.py", "");
        let file = root.join("myapp/models/__init__.py");
        assert_eq!(path_to_module(&file, root), Some("myapp.models".into()));
    }

    #[test]
    fn test_path_to_module_top_level() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(root, "settings.py", "");
        let file = root.join("settings.py");
        assert_eq!(path_to_module(&file, root), Some("settings".into()));
    }

    // ── resolve_relative_import ───────────────────────────────────────────

    #[test]
    fn test_resolve_relative_single_dot() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(root, "app/views.py", "");
        let file = root.join("app/views.py");
        // `from . import models` in app/views.py  → app.models
        let resolved = resolve_relative_import(&file, 1, "models", root);
        assert_eq!(resolved, Some("app.models".into()));
    }

    #[test]
    fn test_resolve_relative_double_dot() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(root, "app/sub/views.py", "");
        let file = root.join("app/sub/views.py");
        // `from ..models import Foo` → app.models
        let resolved = resolve_relative_import(&file, 2, "models", root);
        assert_eq!(resolved, Some("app.models".into()));
    }

    #[test]
    fn test_resolve_relative_empty_module_name() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(root, "app/views.py", "");
        let file = root.join("app/views.py");
        // `from . import something` with level=1 and empty name → app
        let resolved = resolve_relative_import(&file, 1, "", root);
        assert_eq!(resolved, Some("app".into()));
    }

    // ── build_import_graph ────────────────────────────────────────────────

    #[test]
    fn test_build_graph_skips_migrations() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            "app/migrations/0001_initial.py",
            "from django.db import models",
        );
        write_file(root, "app/models.py", "");
        let graph = build_import_graph(root);
        // migrations should be skipped
        assert!(!graph.keys().any(|k| k.contains("migrations")));
        assert!(graph.contains_key("app.models"));
    }

    #[test]
    fn test_build_graph_skips_venv() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            ".venv/lib/python3/site-packages/django/db/models.py",
            "",
        );
        write_file(root, "app/models.py", "");
        let graph = build_import_graph(root);
        assert!(!graph.keys().any(|k| k.contains("django")));
    }

    #[test]
    fn test_build_graph_collects_imports() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(root, "app/models.py", "from django.db import models\n");
        write_file(root, "app/views.py", "from app.models import MyModel\n");
        let graph = build_import_graph(root);
        let views = graph.get("app.views").expect("app.views");
        assert!(views.imports.iter().any(|i| i.module == "app.models"));
    }

    // ── TYPE_CHECKING guard ────────────────────────────────────────────────

    #[test]
    fn test_type_checking_imports_marked() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            "app/views.py",
            "from __future__ import annotations\nfrom typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from app.models import MyModel\n",
        );
        let graph = build_import_graph(root);
        let views = graph.get("app.views").expect("app.views");
        let imp = views
            .imports
            .iter()
            .find(|i| i.module == "app.models")
            .expect("import");
        assert!(
            imp.is_type_checking,
            "should be marked as type-checking only"
        );
    }

    // ── Circular import detection ─────────────────────────────────────────

    #[test]
    fn test_no_false_positive_no_cycle() {
        let tmp = make_project();
        let root = tmp.path();
        // a → b, no cycle
        write_file(root, "a.py", "from b import something\n");
        write_file(root, "b.py", "x = 1\n");
        let graph = build_import_graph(root);
        let diags = detect_cycles(&graph);
        assert!(diags.is_empty(), "expected no cycle, got: {:?}", diags);
    }

    #[test]
    fn test_detects_simple_cycle() {
        let tmp = make_project();
        let root = tmp.path();
        // a → b → a
        write_file(root, "a.py", "from b import x\n");
        write_file(root, "b.py", "from a import y\n");
        let graph = build_import_graph(root);
        let diags = detect_cycles(&graph);
        assert_eq!(diags.len(), 1, "expected one cycle, got: {:?}", diags);
        assert_eq!(diags[0].code, "DJ042");
        assert!(diags[0].message.contains("→"));
    }

    #[test]
    fn test_detects_three_node_cycle() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(root, "a.py", "from b import x\n");
        write_file(root, "b.py", "from c import y\n");
        write_file(root, "c.py", "from a import z\n");
        let graph = build_import_graph(root);
        let diags = detect_cycles(&graph);
        assert!(!diags.is_empty(), "expected a 3-node cycle");
        assert_eq!(diags[0].code, "DJ042");
    }

    #[test]
    fn test_type_checking_cycle_excluded() {
        let tmp = make_project();
        let root = tmp.path();
        // a → b is a TYPE_CHECKING import; b → a is real. No runtime cycle.
        write_file(
            root,
            "a.py",
            "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from b import BClass\n",
        );
        write_file(root, "b.py", "from a import AClass\n");
        let graph = build_import_graph(root);
        let diags = detect_cycles(&graph);
        assert!(
            diags.is_empty(),
            "TYPE_CHECKING import should not trigger cycle"
        );
    }

    // ── Unused import detection ───────────────────────────────────────────

    #[test]
    fn test_used_import_no_diagnostic() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            "app/views.py",
            "from app.models import MyModel\ndef my_view():\n    return MyModel.objects.all()\n",
        );
        write_file(root, "app/models.py", "");
        let graph = build_import_graph(root);
        let diags = detect_unused_imports(&graph);
        assert!(diags.is_empty(), "used import should not be flagged");
    }

    #[test]
    fn test_unused_model_import_flagged() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            "app/views.py",
            "from app.models import MyModel\ndef my_view():\n    return []\n",
        );
        write_file(root, "app/models.py", "");
        let graph = build_import_graph(root);
        let diags = detect_unused_imports(&graph);
        assert_eq!(diags.len(), 1, "unused model import should be flagged");
        assert_eq!(diags[0].code, "DJ043");
        assert!(diags[0].message.contains("MyModel"));
    }

    #[test]
    fn test_non_django_unused_import_not_flagged() {
        let tmp = make_project();
        let root = tmp.path();
        // Generic stdlib import – should not be flagged by DJ043
        write_file(
            root,
            "app/views.py",
            "from os.path import join\ndef f():\n    return []\n",
        );
        write_file(root, "app/models.py", "");
        let graph = build_import_graph(root);
        let diags = detect_unused_imports(&graph);
        let dj043: Vec<_> = diags.iter().filter(|d| d.code == "DJ043").collect();
        assert!(
            dj043.is_empty(),
            "non-Django import should not be flagged by DJ043"
        );
    }

    #[test]
    fn test_wildcard_import_not_flagged() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(root, "app/views.py", "from app.models import *\n");
        write_file(root, "app/models.py", "");
        let graph = build_import_graph(root);
        let diags = detect_unused_imports(&graph);
        assert!(diags.is_empty(), "wildcard import should not be flagged");
    }

    #[test]
    fn test_unused_serializer_import_flagged() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            "app/views.py",
            "from rest_framework.serializers import ModelSerializer\nclass Foo:\n    pass\n",
        );
        let graph = build_import_graph(root);
        let diags = detect_unused_imports(&graph);
        assert!(
            !diags.is_empty(),
            "unused serializer import should be flagged"
        );
        assert_eq!(diags[0].code, "DJ043");
    }

    #[test]
    fn test_is_django_module() {
        assert!(is_django_module("django.db.models"));
        assert!(is_django_module("rest_framework.serializers"));
        assert!(is_django_module("myapp.models"));
        assert!(is_django_module("myapp.views"));
        assert!(!is_django_module("os.path"));
        assert!(!is_django_module("collections"));
        assert!(!is_django_module("myapp.utils"));
    }

    // ── DJ043: alias tracking ─────────────────────────────────────────────

    /// `from django.utils.translation import gettext_lazy as _` — `_` is used
    /// in the file, so this must NOT be reported as unused.
    #[test]
    fn test_aliased_import_used_via_alias_not_flagged() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            "app/models.py",
            "from django.utils.translation import gettext_lazy as _\n\
             name = _(\"Hello\")\n",
        );
        let graph = build_import_graph(root);
        let diags = detect_unused_imports(&graph);
        assert!(
            diags.is_empty(),
            "aliased import used via alias should not be flagged, got: {:?}",
            diags
        );
    }

    /// `from app.models import MyModel as M` — `MyModel` is unused, but `M`
    /// (the alias) IS used.  Must NOT be reported.
    #[test]
    fn test_aliased_model_import_used_via_alias_not_flagged() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            "app/views.py",
            "from app.models import MyModel as M\ndef view():\n    return M.objects.all()\n",
        );
        write_file(root, "app/models.py", "");
        let graph = build_import_graph(root);
        let diags = detect_unused_imports(&graph);
        assert!(
            diags.is_empty(),
            "import used via alias should not be flagged, got: {:?}",
            diags
        );
    }

    /// `from app.models import MyModel as M` — neither `MyModel` nor `M` is
    /// used.  Must still be reported (with "MyModel as M" in the message).
    #[test]
    fn test_aliased_model_import_unused_flagged_with_alias_in_message() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            "app/views.py",
            "from app.models import MyModel as M\ndef view():\n    return []\n",
        );
        write_file(root, "app/models.py", "");
        let graph = build_import_graph(root);
        let diags = detect_unused_imports(&graph);
        assert_eq!(diags.len(), 1, "unused aliased import should be flagged");
        assert_eq!(diags[0].code, "DJ043");
        assert!(
            diags[0].message.contains("MyModel as M"),
            "message should mention both name and alias, got: {}",
            diags[0].message
        );
    }

    // ── DJ043: __init__.py skipping ────────────────────────────────────────

    /// An `__init__.py` that re-exports names from submodules must not trigger
    /// DJ043 even when the imported symbols are not referenced in the same file.
    #[test]
    fn test_init_py_unused_imports_not_flagged() {
        let tmp = make_project();
        let root = tmp.path();
        // Package __init__.py re-exports Foo for callers of the package
        write_file(root, "app/__init__.py", "from app.models import Foo\n");
        write_file(root, "app/models.py", "");
        let graph = build_import_graph(root);
        let diags = detect_unused_imports(&graph);
        assert!(
            diags.is_empty(),
            "__init__.py re-exports should not be flagged, got: {:?}",
            diags
        );
    }

    // ── DJ042: __init__.py cycle suppression ──────────────────────────────

    /// A cycle that passes through an __init__.py module must be suppressed.
    /// Pattern: screening → models (__init__.py) → package → screening
    #[test]
    fn test_cycle_through_init_not_flagged() {
        let tmp = make_project();
        let root = tmp.path();
        // models/__init__.py re-exports from submodules
        write_file(
            root,
            "app/models/__init__.py",
            "from app.models.package import PackageModel\n",
        );
        write_file(
            root,
            "app/models/package.py",
            "from app.models.screening import ScreeningModel\n",
        );
        write_file(
            root,
            "app/models/screening.py",
            "from app.models import PackageModel\n",
        );
        let graph = build_import_graph(root);
        let diags = detect_cycles(&graph);
        let dj042: Vec<_> = diags.iter().filter(|d| d.code == "DJ042").collect();
        assert!(
            dj042.is_empty(),
            "cycle through __init__.py should be suppressed, got: {:?}",
            dj042
        );
    }

    /// A direct two-module cycle (no __init__.py involved) must still be flagged.
    #[test]
    fn test_direct_cycle_without_init_still_flagged() {
        let tmp = make_project();
        let root = tmp.path();
        write_file(
            root,
            "app/check.py",
            "from app.screening import ScreeningModel\n",
        );
        write_file(
            root,
            "app/screening.py",
            "from app.check import CheckModel\n",
        );
        let graph = build_import_graph(root);
        let diags = detect_cycles(&graph);
        let dj042: Vec<_> = diags.iter().filter(|d| d.code == "DJ042").collect();
        assert_eq!(
            dj042.len(),
            1,
            "direct two-module cycle should still be flagged, got: {:?}",
            dj042
        );
    }
}
