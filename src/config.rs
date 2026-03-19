//! Django-specific config reading from pyproject.toml.
//!
//! Priority for settings module:
//! 1. THORN_DJANGO_SETTINGS env var
//! 2. [tool.thorn-django] settings in pyproject.toml
//! 3. [tool.pylint.DJANGO] django-settings-module in pyproject.toml
//! 4. DJANGO_SETTINGS_MODULE env var

/// Django plugin configuration, read from `[tool.thorn-django]` in pyproject.toml.
///
/// All thresholds have sensible defaults that match the original hardcoded values so
/// projects without a `[tool.thorn-django]` section get exactly the same behaviour as
/// before this change was introduced.
#[derive(Debug, Clone)]
pub struct DjangoConfig {
    /// Maximum nesting depth before DJ041 fires (default: 4).
    pub max_nesting_depth: u32,
    /// Maximum number of function arguments before DJ034 fires (default: 6).
    pub max_function_args: u32,
    /// Maximum number of return statements before DJ035 fires (default: 6).
    pub max_return_statements: u32,
    /// Maximum number of branches before DJ036 fires (default: 12).
    pub max_branches: u32,
    /// Maximum number of local variables before DJ037 fires (default: 15).
    pub max_local_variables: u32,
    /// Maximum number of statements before DJ038 fires (default: 50).
    pub max_statements: u32,
    /// Maximum number of model fields before DJ039 fires (default: 20).
    pub max_model_fields: u32,
    /// Maximum number of class methods before DJ040 fires (default: 20).
    pub max_class_methods: u32,
}

impl Default for DjangoConfig {
    fn default() -> Self {
        Self {
            max_nesting_depth: 4,
            max_function_args: 6,
            max_return_statements: 6,
            max_branches: 12,
            max_local_variables: 15,
            max_statements: 50,
            max_model_fields: 20,
            max_class_methods: 20,
        }
    }
}

/// Parse `[tool.thorn-django]` from the given pyproject.toml content string.
///
/// Unknown keys and parse errors are silently ignored; missing keys fall back to
/// the defaults defined in [`DjangoConfig::default`].
pub fn read_django_config(toml_content: &str) -> DjangoConfig {
    let mut cfg = DjangoConfig::default();

    let doc: toml::Value = match toml_content.parse() {
        Ok(v) => v,
        Err(_) => return cfg,
    };

    let section = match doc.get("tool").and_then(|t| t.get("thorn-django")) {
        Some(s) => s,
        None => return cfg,
    };

    if let Some(v) = section
        .get("max-nesting-depth")
        .and_then(|v| v.as_integer())
    {
        cfg.max_nesting_depth = v.max(1) as u32;
    }
    if let Some(v) = section
        .get("max-function-args")
        .and_then(|v| v.as_integer())
    {
        cfg.max_function_args = v.max(1) as u32;
    }
    if let Some(v) = section
        .get("max-return-statements")
        .and_then(|v| v.as_integer())
    {
        cfg.max_return_statements = v.max(1) as u32;
    }
    if let Some(v) = section.get("max-branches").and_then(|v| v.as_integer()) {
        cfg.max_branches = v.max(1) as u32;
    }
    if let Some(v) = section
        .get("max-local-variables")
        .and_then(|v| v.as_integer())
    {
        cfg.max_local_variables = v.max(1) as u32;
    }
    if let Some(v) = section.get("max-statements").and_then(|v| v.as_integer()) {
        cfg.max_statements = v.max(1) as u32;
    }
    if let Some(v) = section.get("max-model-fields").and_then(|v| v.as_integer()) {
        cfg.max_model_fields = v.max(1) as u32;
    }
    if let Some(v) = section
        .get("max-class-methods")
        .and_then(|v| v.as_integer())
    {
        cfg.max_class_methods = v.max(1) as u32;
    }

    cfg
}

/// Locate pyproject.toml by walking up from the current working directory and
/// parse the `[tool.thorn-django]` section.  Falls back to [`DjangoConfig::default`]
/// when no file is found or when the TOML cannot be parsed.
pub fn read_django_config_from_cwd() -> DjangoConfig {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return DjangoConfig::default(),
    };

    let mut current = cwd;
    loop {
        let candidate = current.join("pyproject.toml");
        if candidate.exists() {
            let content = match std::fs::read_to_string(&candidate) {
                Ok(s) => s,
                Err(_) => return DjangoConfig::default(),
            };
            return read_django_config(&content);
        }
        if !current.pop() {
            break;
        }
    }

    DjangoConfig::default()
}

/// Read exclude patterns from `[tool.pylint]` in pyproject.toml.
pub fn read_django_excludes(toml_content: &str) -> Vec<String> {
    let doc: toml::Value = match toml_content.parse() {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mut excludes = Vec::new();

    if let Some(pylint) = doc.get("tool").and_then(|t| t.get("pylint")) {
        let main = pylint.get("main").unwrap_or(pylint);

        if let Some(ignore_paths) = main.get("ignore-paths").and_then(|v| v.as_array()) {
            for path in ignore_paths {
                if let Some(s) = path.as_str() {
                    let s = s.trim_start_matches('^').trim_end_matches('$');
                    let s = s.replace(".*", "*");
                    excludes.push(format!("*/{s}"));
                }
            }
        }

        if let Some(ignore) = main.get("ignore").and_then(|v| v.as_array()) {
            for name in ignore {
                if let Some(s) = name.as_str() {
                    excludes.push(format!("*/{s}/*"));
                }
            }
        }
    }

    excludes
}

/// Read the `graph_file` path from `[tool.thorn-django]` in pyproject.toml, if set.
pub fn read_graph_file_path(toml_content: &str) -> Option<String> {
    let doc: toml::Value = toml_content.parse().ok()?;
    doc.get("tool")
        .and_then(|t| t.get("thorn-django"))
        .and_then(|td| td.get("graph_file"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Read Django settings module. Checks in order:
/// 1. THORN_DJANGO_SETTINGS env var
/// 2. [tool.thorn-django] settings
/// 3. [tool.pylint.DJANGO] django-settings-module
/// 4. DJANGO_SETTINGS_MODULE env var
pub fn read_django_settings(toml_content: &str) -> Option<String> {
    if let Ok(s) = std::env::var("THORN_DJANGO_SETTINGS") {
        if !s.is_empty() {
            return Some(s);
        }
    }

    if let Ok(doc) = toml_content.parse::<toml::Value>() {
        if let Some(s) = doc
            .get("tool")
            .and_then(|t| t.get("thorn-django"))
            .and_then(|td| td.get("settings"))
            .and_then(|v| v.as_str())
        {
            return Some(s.to_string());
        }

        if let Some(s) = doc
            .get("tool")
            .and_then(|t| t.get("pylint"))
            .and_then(|p| p.get("DJANGO"))
            .and_then(|d| d.get("django-settings-module"))
            .and_then(|v| v.as_str())
        {
            return Some(s.to_string());
        }
    }

    if let Ok(s) = std::env::var("DJANGO_SETTINGS_MODULE") {
        if !s.is_empty() {
            return Some(s);
        }
    }

    None
}
