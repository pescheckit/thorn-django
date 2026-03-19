//! Django settings checks with import resolution.

use ruff_python_ast::*;
use ruff_python_parser::parse;
use ruff_text_size::Ranged;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thorn_api::Diagnostic;

#[derive(Debug)]
struct ResolvedSetting {
    value: SettingValue,
    file: String,
    line: u32,
}

#[derive(Debug)]
enum SettingValue {
    Bool(bool),
    HardcodedString,
    Other,
}

pub fn check_settings(project_dir: &Path, settings_module: &str) -> Vec<Diagnostic> {
    let rel_path = settings_module.replace('.', "/") + ".py";
    let settings_file = project_dir.join(&rel_path);

    if !settings_file.exists() {
        return vec![];
    }

    let settings = resolve_settings(&settings_file, project_dir);
    let mut diagnostics = Vec::new();

    // DJ012: DEBUG = True
    if let Some(s) = settings.get("DEBUG") {
        if matches!(s.value, SettingValue::Bool(true)) {
            diagnostics.push(make_diag(
                "DJ012",
                "DEBUG = True in final settings. Use an environment variable.",
                &s.file,
                s.line,
            ));
        }
    }

    // DJ016: SECRET_KEY hardcoded
    if let Some(s) = settings.get("SECRET_KEY") {
        if matches!(s.value, SettingValue::HardcodedString) {
            diagnostics.push(make_diag(
                "DJ016",
                "SECRET_KEY is hardcoded. Use os.environ['SECRET_KEY'].",
                &s.file,
                s.line,
            ));
        }
    }

    // DJ013: Missing security settings
    let security_settings = [
        ("SECURE_SSL_REDIRECT", "enables HTTPS redirect"),
        (
            "SESSION_COOKIE_SECURE",
            "prevents session hijacking over HTTP",
        ),
        ("CSRF_COOKIE_SECURE", "prevents CSRF token theft over HTTP"),
        (
            "SECURE_HSTS_SECONDS",
            "enables HTTP Strict Transport Security",
        ),
    ];

    let settings_last_line = std::fs::read_to_string(&settings_file)
        .map(|s| s.lines().count() as u32)
        .unwrap_or(1);

    for (name, purpose) in security_settings {
        if !settings.contains_key(name) {
            diagnostics.push(make_diag(
                "DJ013",
                &format!("{name} is not set — {purpose}. See Django deployment checklist."),
                &rel_path,
                settings_last_line,
            ));
        }
    }

    diagnostics
}

fn resolve_settings(file: &Path, project_dir: &Path) -> HashMap<String, ResolvedSetting> {
    let mut settings = HashMap::new();
    let mut visited = Vec::new();
    collect_settings(file, project_dir, &mut settings, &mut visited);
    settings
}

fn collect_settings(
    file: &Path,
    project_dir: &Path,
    settings: &mut HashMap<String, ResolvedSetting>,
    visited: &mut Vec<PathBuf>,
) {
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if visited.contains(&canonical) {
        return;
    }
    visited.push(canonical);

    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(_) => return,
    };

    let parsed = match parse(&source, ruff_python_parser::Mode::Module.into()) {
        Ok(p) => p,
        Err(_) => return,
    };

    let module = match parsed.into_syntax().module() {
        Some(m) => m.clone(),
        None => return,
    };

    let filename = file.to_string_lossy().to_string();

    for stmt in &module.body {
        match stmt {
            Stmt::ImportFrom(import) => {
                if let Some(module_name) = &import.module {
                    let imports_star = import.names.iter().any(|a| a.name.as_str() == "*");
                    if imports_star {
                        if let Some(imported_file) = resolve_import(
                            file,
                            module_name.as_str(),
                            Some(import.level),
                            project_dir,
                        ) {
                            collect_settings(&imported_file, project_dir, settings, visited);
                        }
                    }
                }
            }
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        let name = n.id.as_str();
                        if name.chars().all(|c| c.is_uppercase() || c == '_') && name.len() > 1 {
                            let line = source[..assign.range().start().to_usize()]
                                .chars()
                                .filter(|c| *c == '\n')
                                .count() as u32
                                + 1;
                            let value = extract_value(&assign.value);
                            settings.insert(
                                name.to_string(),
                                ResolvedSetting {
                                    value,
                                    file: filename.clone(),
                                    line,
                                },
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn resolve_import(
    current_file: &Path,
    module_name: &str,
    level: Option<u32>,
    project_dir: &Path,
) -> Option<PathBuf> {
    let level = level.unwrap_or(0);
    if level > 0 {
        let mut base = current_file.parent()?.to_path_buf();
        for _ in 1..level {
            base = base.parent()?.to_path_buf();
        }
        let rel = module_name.replace('.', "/");
        let candidate = base.join(format!("{rel}.py"));
        if candidate.exists() {
            return Some(candidate);
        }
        let candidate = base.join(&rel).join("__init__.py");
        if candidate.exists() {
            return Some(candidate);
        }
    } else {
        let rel = module_name.replace('.', "/");
        let candidate = project_dir.join(format!("{rel}.py"));
        if candidate.exists() {
            return Some(candidate);
        }
        let candidate = project_dir.join(&rel).join("__init__.py");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn extract_value(expr: &Expr) -> SettingValue {
    match expr {
        Expr::BooleanLiteral(b) => SettingValue::Bool(b.value),
        Expr::StringLiteral(_) => SettingValue::HardcodedString,
        _ => SettingValue::Other,
    }
}

fn make_diag(code: &str, message: &str, file: &str, line: u32) -> Diagnostic {
    let mut d = Diagnostic::new(code, message, file);
    if line > 0 {
        d.line = Some(line);
    }
    d
}
